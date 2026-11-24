use serde::{Deserialize, Serialize};
use serde_json::json;
use theorem_agent_core::{
    Decision, DecisionRequest, Effect, Observation, ObservationStatus, ProviderError,
};
use theorem_agent_runtime::DecisionProvider;

use crate::{ToolId, invalid};

/// An explicit assessment plan. It carries no executable strings or model settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssessmentTask {
    pub schema_version: u32,
    pub tools: Vec<ToolId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
}

impl AssessmentTask {
    pub fn new(tools: Vec<ToolId>) -> Self {
        Self {
            schema_version: 1,
            tools,
            goal: None,
        }
    }

    pub fn validate(&self) -> Result<(), ProviderError> {
        let unique: std::collections::BTreeSet<_> = self.tools.iter().copied().collect();
        if self.schema_version != 1
            || self.tools.is_empty()
            || self.tools.len() > 16
            || unique.len() != self.tools.len()
        {
            return Err(invalid(
                "invalid_task",
                "assessment needs 1 to 16 distinct admitted tools and schema_version 1",
            ));
        }
        if self
            .goal
            .as_ref()
            .is_some_and(|goal| goal.len() > 16 * 1024)
        {
            return Err(invalid("invalid_task", "assessment goal exceeds 16 KiB"));
        }
        Ok(())
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PolicyState {
    next: usize,
    checks: Vec<CheckResult>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckResult {
    tool: ToolId,
    observation: Observation,
}

/// Deterministic standalone policy: assess exactly the tools requested by the caller.
/// A native host can replace this through `DecisionProvider`; no model is hidden here.
pub struct RustAssessmentPolicy;

impl DecisionProvider for RustAssessmentPolicy {
    fn decide(&mut self, request: &DecisionRequest) -> Result<Decision, ProviderError> {
        let task: AssessmentTask = serde_json::from_value(request.task.clone())
            .map_err(|_| invalid("invalid_task", "task is not a versioned Rust assessment"))?;
        task.validate()?;
        let sequence: usize = request.sequence.try_into().map_err(|_| invalid_state())?;
        let mut state = if request.sequence == 0 {
            if !request.policy_state.is_null() || request.observation.is_some() {
                return Err(invalid_state());
            }
            PolicyState::default()
        } else {
            serde_json::from_value::<PolicyState>(request.policy_state.clone())
                .map_err(|_| invalid_state())?
        };
        if state.next != sequence || sequence > task.tools.len() {
            return Err(invalid_state());
        }
        if sequence > 0 {
            let observation = request.observation.as_ref().ok_or_else(invalid_state)?;
            if observation.status == ObservationStatus::Unknown
                || state.checks.len() + 1 != sequence
            {
                return Err(invalid_state());
            }
            state.checks.push(CheckResult {
                tool: task.tools[sequence - 1],
                observation: observation.clone(),
            });
        }
        if sequence == task.tools.len() {
            let passed = state
                .checks
                .iter()
                .all(|check| check.observation.status == ObservationStatus::Completed);
            let result = json!({"policy":"deterministic-rust-assessment-v1", "outcome":if passed {"passed"} else {"findings"}, "checks":state.checks});
            // Completed evidence lives in the result. Duplicating the complete
            // report in terminal policy state wastes checkpoint/frame capacity.
            state.checks.clear();
            return Ok(Decision::Complete {
                result,
                policy_state: serde_json::to_value(&state).map_err(crate::json_error)?,
            });
        }
        state.next += 1;
        Ok(Decision::Effect {
            effect: Effect {
                capability: "rust.tool".into(),
                arguments: json!({"tool":task.tools[sequence]}),
            },
            policy_state: serde_json::to_value(&state).map_err(crate::json_error)?,
        })
    }
}

fn invalid_state() -> ProviderError {
    invalid(
        "invalid_policy_state",
        "the next policy decision requires the matching persisted terminal observation",
    )
}
