use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use theorem_agent_core::{
    Binding, Capability, CapabilityCatalog, Decision, DecisionRequest, Effect, Observation,
    PROTOCOL_VERSION, ProviderError, digest,
};
use theorem_agent_runtime::{DecisionProvider, EffectExecutor, JsonCommand};

use crate::{ToolId, invalid};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Handshake {
    protocol_version: u32,
    provider_id: String,
    capabilities: Vec<Capability>,
}

/// One explicitly configured native provider command. A handshake is a compatibility
/// assertion by that trusted provider; process presence never grants authority.
#[derive(Clone)]
pub struct HostConnection {
    command: JsonCommand,
    handshake: Handshake,
}

/// Context supplied on every host call. The local attachment is transport context;
/// stable provider/workspace identities remain in the persisted binding.
#[derive(Clone, Debug, Serialize)]
pub struct HostContext {
    pub run_id: String,
    pub agent_id: String,
    pub workspace: PathBuf,
    pub binding: Binding,
    pub admitted_tools: Vec<ToolId>,
}

impl HostConnection {
    pub fn connect(
        program: &Path,
        args: Vec<String>,
        timeout: Duration,
    ) -> Result<Self, ProviderError> {
        if !program.is_absolute() || !program.is_file() {
            return Err(invalid(
                "invalid_host_command",
                "host-command must be an existing absolute executable path",
            ));
        }
        let command = JsonCommand::new(program).args(args).timeout(timeout);
        let response =
            command.call(&json!({"protocol_version":PROTOCOL_VERSION,"method":"capabilities"}))?;
        let mut handshake: Handshake = serde_json::from_value(response).map_err(|_| {
            invalid(
                "invalid_host_handshake",
                "host must return protocol_version, provider_id and versioned capabilities",
            )
        })?;
        if handshake.protocol_version != PROTOCOL_VERSION
            || handshake.provider_id.trim().is_empty()
            || handshake.provider_id.len() > 256
            || handshake.provider_id.chars().any(char::is_control)
        {
            return Err(invalid(
                "invalid_host_handshake",
                "host protocol or stable provider identity is invalid",
            ));
        }
        let mut catalog = CapabilityCatalog::default();
        for capability in &handshake.capabilities {
            catalog.register(capability.clone()).map_err(|_| {
                invalid(
                    "invalid_host_handshake",
                    "host capability catalogue is malformed or duplicates a capability",
                )
            })?;
            if capability.version != 1 {
                return Err(invalid(
                    "incompatible_host",
                    "Agent-Rust currently supports capability contract version 1",
                ));
            }
        }
        if !handshake
            .capabilities
            .iter()
            .any(|capability| capability.name == "policy.decide")
            || !handshake
                .capabilities
                .iter()
                .any(|capability| capability.name != "policy.decide")
        {
            return Err(invalid(
                "incompatible_host",
                "host must publish policy.decide and at least one execution capability",
            ));
        }
        handshake.capabilities.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Self { command, handshake })
    }

    pub fn binding(&self, workspace: String) -> Result<Binding, ProviderError> {
        let identity = digest(&self.handshake)
            .map_err(|_| invalid("invalid_host_handshake", "cannot encode host capabilities"))?;
        Ok(Binding {
            policy: format!("native-policy-v1:{identity}"),
            execution: format!("native-execution-v1:{identity}"),
            workspace,
        })
    }

    pub fn provider_id(&self) -> &str {
        &self.handshake.provider_id
    }

    pub fn adapters(&self, context: HostContext) -> (HostPolicy, HostExecutor) {
        (
            HostPolicy {
                connection: self.clone(),
                context: context.clone(),
            },
            HostExecutor {
                connection: self.clone(),
                context,
            },
        )
    }

    fn call(
        &self,
        context: &HostContext,
        method: &str,
        request_id: &str,
        params: Value,
    ) -> Result<Value, ProviderError> {
        let response = self.command.call(&json!({
            "protocol_version":PROTOCOL_VERSION,"method":method,"request_id":request_id,
            "provider_id":self.handshake.provider_id,"context":context,"params":params
        }))?;
        if response.get("protocol_version").and_then(Value::as_u64)
            != Some(u64::from(PROTOCOL_VERSION))
            || response.get("request_id").and_then(Value::as_str) != Some(request_id)
        {
            return Err(invalid(
                "invalid_host_response",
                "host response version or request identity changed",
            ));
        }
        match (response.get("result"), response.get("error")) {
            (Some(result), None) => Ok(result.clone()),
            (None, Some(error)) => Err(serde_json::from_value(error.clone()).map_err(|_| {
                invalid(
                    "invalid_host_response",
                    "host error needs a code and message",
                )
            })?),
            _ => Err(invalid(
                "invalid_host_response",
                "host response must contain exactly one result or error",
            )),
        }
    }

    fn admits(&self, effect: &Effect) -> bool {
        effect.capability != "policy.decide"
            && self
                .handshake
                .capabilities
                .iter()
                .any(|capability| capability.name == effect.capability)
    }
}

pub struct HostPolicy {
    connection: HostConnection,
    context: HostContext,
}

impl DecisionProvider for HostPolicy {
    fn decide(&mut self, request: &DecisionRequest) -> Result<Decision, ProviderError> {
        if request.agent_id != self.context.agent_id || request.binding != self.context.binding {
            return Err(invalid(
                "binding_mismatch",
                "policy request does not match the bound native host",
            ));
        }
        let result = self.connection.call(
            &self.context,
            "policy.decide",
            &request.decision_id,
            serde_json::to_value(request).map_err(crate::json_error)?,
        )?;
        serde_json::from_value(result).map_err(|_| {
            invalid(
                "invalid_host_response",
                "host did not return a valid next-action Decision",
            )
        })
    }
}

pub struct HostExecutor {
    connection: HostConnection,
    context: HostContext,
}

impl HostExecutor {
    fn validate(&self, effect: &Effect) -> Result<(), ProviderError> {
        if !self.connection.admits(effect) {
            return Err(invalid(
                "capability_denied",
                "effect capability was not published in the bound host catalogue",
            ));
        }
        if effect.capability == "rust.tool" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Arguments {
                tool: ToolId,
            }
            let args: Arguments =
                serde_json::from_value(effect.arguments.clone()).map_err(|_| {
                    invalid(
                        "invalid_effect",
                        "rust.tool needs an admitted tool identifier",
                    )
                })?;
            if !self.context.admitted_tools.contains(&args.tool) {
                return Err(invalid(
                    "capability_denied",
                    "native host selected a Rust tool outside this run's admission",
                ));
            }
        }
        Ok(())
    }
}

impl EffectExecutor for HostExecutor {
    fn execute(&mut self, effect_id: &str, effect: &Effect) -> Result<Observation, ProviderError> {
        self.validate(effect)?;
        let result = self.connection.call(
            &self.context,
            "effect.execute",
            &format!("{effect_id}:execute"),
            json!({"effect_id":effect_id,"effect":effect}),
        );
        match result {
            Ok(value) => serde_json::from_value(value).map_err(|_| {
                invalid(
                    "invalid_host_response",
                    "host did not return an Observation",
                )
            }),
            Err(error) => Ok(Observation::unknown(
                json!({"effect_id":effect_id,"provider_id":self.connection.provider_id(),"error":error}),
            )),
        }
    }

    fn reconcile(
        &mut self,
        effect_id: &str,
        effect: &Effect,
    ) -> Result<Option<Observation>, ProviderError> {
        self.validate(effect)?;
        let result = self.connection.call(
            &self.context,
            "effect.reconcile",
            &format!("{effect_id}:reconcile"),
            json!({"effect_id":effect_id,"effect":effect}),
        )?;
        serde_json::from_value(result).map_err(|_| {
            invalid(
                "invalid_host_response",
                "reconciliation must return an Observation or null",
            )
        })
    }
}
