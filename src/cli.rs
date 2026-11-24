//! Terminal adapter. All run advancement is delegated to the shared durable host.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use theorem_agent_core::{
    Binding, Decision, DecisionRequest, Effect, Observation, ProviderError, validate_id,
};
use theorem_agent_runtime::{
    DecisionProvider, DurableHost, EffectExecutor, LangGraphWorker, RunMode, RuntimeError,
};
use theorem_agent_store::{DocumentInput, EmbeddedAgentStore, StoreOptions};

use crate::{
    AgentRecords, AssessmentTask, HostConnection, HostContext, LocalEnvironment,
    RustAssessmentPolicy, RustToolExecutor, ToolId, Workspace, invalid, json_error, store_error,
};

#[derive(Parser)]
#[command(
    name = "agent-rust",
    version,
    about = "A persistent Rust specialist with Theorem policy contracts and real LangGraph recovery"
)]
pub struct Cli {
    /// Dedicated absolute directory for this agent's RustyRed identity/context/receipts.
    #[arg(long, global = true)]
    pub home: Option<PathBuf>,
    /// Installed theorem-agent-worker entrypoint, or an explicit Python interpreter.
    #[arg(long, global = true, default_value = "theorem-agent-worker")]
    worker: PathBuf,
    /// Literal interpreter arguments, e.g. --worker-arg=-m --worker-arg=theorem_agent_langgraph.
    #[arg(long, global = true, allow_hyphen_values = true)]
    worker_arg: Vec<String>,
    /// Deadline for one worker invocation; unfinished work remains durable.
    #[arg(long, global = true, default_value_t = 3600, value_parser = clap::value_parser!(u64).range(1..=86400))]
    worker_timeout_seconds: u64,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create or reopen an agent home and report its persistent identity.
    Init {
        #[arg(long)]
        name: Option<String>,
    },
    /// Read or update display metadata without changing identity, context, or runs.
    Profile {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        role: Option<String>,
    },
    /// Import an explicit source and supplied UTF-8 extracted text into RustyRed.
    Import(ImportArgs),
    /// Search only context explicitly imported into this agent home.
    Search {
        query: String,
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
    },
    /// Retrieve imported text/provenance and optionally the exact original bytes.
    Source {
        document_id: String,
        #[arg(long)]
        raw: bool,
    },
    /// Report detected Cargo commands and source-integration recipes; installs nothing.
    Doctor {
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
    /// Start a new explicit assessment. Default: cargo check --locked --all-targets.
    Run(RunArgs),
    /// Resume an existing run using its original provider/tool/workspace binding.
    Resume(ResumeArgs),
    /// Read checkpointed run status without consulting a model or executing a tool.
    Status { run_id: String },
}

#[derive(Args)]
struct ImportArgs {
    #[arg(long)]
    file: PathBuf,
    /// Extracted UTF-8 text file; required when the original source is binary.
    #[arg(long)]
    text: Option<PathBuf>,
    /// Logical source URI. Default is the actual canonical source file URI.
    #[arg(long)]
    source_uri: Option<String>,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    media_type: Option<String>,
    #[arg(long)]
    document_id: Option<String>,
    /// Stable caller operation ID; by default derived from the complete import request.
    #[arg(long)]
    operation_id: Option<String>,
}

#[derive(Args)]
struct WorkspaceArgs {
    /// Existing crate/workspace directory. Persistent identity never uses this pathname.
    #[arg(long)]
    workspace: PathBuf,
    /// Stable caller identity; required outside a Git repository with a hosted origin.
    #[arg(long)]
    workspace_id: Option<String>,
}

#[derive(Args)]
struct ProviderArgs {
    /// Explicit native host adapter executable; no MCP hop or automatic fallback.
    #[arg(long)]
    host_command: Option<PathBuf>,
    /// Literal host arguments; do not put secrets here or in provider identities.
    #[arg(long, requires = "host_command", allow_hyphen_values = true)]
    host_arg: Vec<String>,
}

#[derive(Args)]
struct RunArgs {
    #[command(flatten)]
    workspace: WorkspaceArgs,
    #[command(flatten)]
    provider: ProviderArgs,
    #[arg(long)]
    run_id: Option<String>,
    /// Explicitly admitted tools; repeat to select several. No source repair is inferred.
    #[arg(long, value_enum, default_value = "check", value_delimiter = ',')]
    tool: Vec<ToolId>,
    /// Optional task context forwarded to a native host; standalone follows --tool exactly.
    #[arg(long)]
    goal: Option<String>,
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=86400))]
    timeout_seconds: u64,
    /// LangGraph supersteps for this invocation; reaching the limit returns status running.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=1_000_000))]
    max_steps: u32,
}

#[derive(Args)]
struct ResumeArgs {
    run_id: String,
    #[command(flatten)]
    workspace: WorkspaceArgs,
    #[command(flatten)]
    provider: ProviderArgs,
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=1_000_000))]
    max_steps: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProviderMode {
    Standalone,
    NativeHost,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunManifest {
    schema_version: u32,
    agent_id: String,
    run_id: String,
    binding: Binding,
    mode: ProviderMode,
    task: AssessmentTask,
    timeout_seconds: u64,
    workspace_evidence: Value,
    local_environment: Option<LocalEnvironment>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfile {
    schema_version: u32,
    agent_id: String,
    name: String,
    role: String,
}

impl Cli {
    pub fn execute(self) -> Result<Value, ProviderError> {
        let home = self.home.ok_or_else(|| {
            invalid(
                "invalid_arguments",
                "an explicit --home absolute directory is required",
            )
        })?;
        validate_home(&home)?;
        let worker = LangGraphWorker::new(self.worker)
            .args(self.worker_arg)
            .timeout(Duration::from_secs(self.worker_timeout_seconds));
        match self.command {
            Commands::Init { name } => {
                let mut store = open_store(&home)?;
                let profile = profile(&mut store, name, None)?;
                Ok(
                    json!({"agent_id":store.identity().id,"schema_version":store.identity().schema_version,"home":store.home(),"store":"rustyred","profile":profile}),
                )
            }
            Commands::Profile { name, role } => profile(&mut open_store(&home)?, name, role),
            Commands::Import(args) => import(&home, args),
            Commands::Search { query, limit } => {
                let store = open_store(&home)?;
                Ok(
                    json!({"agent_id":store.identity().id,"hits":store.search(&query, limit as usize).map_err(store_error)?}),
                )
            }
            Commands::Source { document_id, raw } => {
                let store = open_store(&home)?;
                let source = store
                    .read_source(&document_id)
                    .map_err(store_error)?
                    .ok_or_else(|| {
                        invalid("source_not_found", "document is not in this agent home")
                    })?;
                let mut result = json!({"document":source.document,"raw_bytes":source.raw_bytes.as_ref().map(Vec::len)});
                if raw {
                    result["raw_base64"] = source
                        .raw_bytes
                        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
                        .into();
                }
                Ok(result)
            }
            Commands::Doctor { workspace } => {
                let workspace = workspace.unwrap_or(std::env::current_dir().map_err(|_| {
                    invalid("invalid_workspace", "cannot resolve current directory")
                })?);
                if !workspace.is_dir() {
                    return Err(invalid("invalid_workspace", "doctor workspace must exist"));
                }
                Ok(crate::doctor(&workspace))
            }
            Commands::Run(args) => start(&home, &worker, args),
            Commands::Resume(args) => resume(&home, &worker, args),
            Commands::Status { run_id } => status(&home, &worker, &run_id),
        }
    }
}

fn validate_home(path: &Path) -> Result<(), ProviderError> {
    if !path.is_absolute()
        || path.parent().is_none()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(invalid(
            "invalid_home",
            "--home must name a dedicated absolute directory, not the filesystem root or a parent-relative path",
        ));
    }
    if let Ok(canonical) = path.canonicalize()
        && (!canonical.is_dir() || canonical.parent().is_none())
    {
        return Err(invalid(
            "invalid_home",
            "agent home must be a dedicated directory",
        ));
    }
    Ok(())
}

fn open_store(home: &Path) -> Result<EmbeddedAgentStore, ProviderError> {
    EmbeddedAgentStore::open(
        home,
        StoreOptions {
            device_name: "agent-rust".into(),
            ..StoreOptions::default()
        },
    )
    .map_err(store_error)
}

fn open_existing(home: &Path) -> Result<EmbeddedAgentStore, ProviderError> {
    if !home.join("agent.rr").is_file() {
        return Err(invalid("run_not_found", "agent home has no saved runs"));
    }
    open_store(home)
}

fn profile(
    store: &mut EmbeddedAgentStore,
    name: Option<String>,
    role: Option<String>,
) -> Result<Value, ProviderError> {
    let stored = store.get_record("agent.profile").map_err(store_error)?;
    let changed = stored.is_none() || name.is_some() || role.is_some();
    let mut profile: AgentProfile = match stored {
        Some(value) => serde_json::from_value(value)
            .map_err(|_| invalid("invalid_profile", "stored profile has an unsupported shape"))?,
        None => AgentProfile {
            schema_version: 1,
            agent_id: store.identity().id.clone(),
            name: "Rust specialist".into(),
            role: "Rust engineering".into(),
        },
    };
    if profile.schema_version != 1 || profile.agent_id != store.identity().id {
        return Err(invalid(
            "invalid_profile",
            "profile schema or ownership does not match this agent home",
        ));
    }
    if let Some(name) = name {
        profile.name = name.trim().to_owned();
    }
    if let Some(role) = role {
        profile.role = role.trim().to_owned();
    }
    for value in [&profile.name, &profile.role] {
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(invalid(
                "invalid_profile",
                "name and role must contain 1 to 256 UTF-8 bytes without control characters",
            ));
        }
    }
    let value = serde_json::to_value(profile).map_err(json_error)?;
    if changed {
        store
            .put_record("agent.profile", &value)
            .map_err(store_error)?;
    }
    Ok(value)
}

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, ProviderError> {
    let file = File::open(path).map_err(|error| {
        invalid(
            "source_io",
            format!("cannot open explicitly supplied source: {error}"),
        )
    })?;
    if !file
        .metadata()
        .map_err(|_| invalid("source_io", "cannot inspect source"))?
        .is_file()
    {
        return Err(invalid("invalid_source", "source must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("source_io", "cannot read source"))?;
    if bytes.len() > maximum {
        return Err(invalid(
            "input_too_large",
            "source exceeds the configured import byte limit",
        ));
    }
    Ok(bytes)
}

fn import(home: &Path, args: ImportArgs) -> Result<Value, ProviderError> {
    let limits = StoreOptions::default();
    let source = args
        .file
        .canonicalize()
        .map_err(|_| invalid("source_io", "source file does not exist"))?;
    let bytes = read_bounded(&source, limits.max_source_bytes)?;
    let binary = args.text.is_some();
    let text = match args.text {
        Some(path) => {
            String::from_utf8(read_bounded(&path, limits.max_document_bytes)?).map_err(|_| {
                invalid(
                    "invalid_extracted_text",
                    "--text must point to UTF-8 extracted text",
                )
            })?
        }
        None => String::from_utf8(bytes.clone()).map_err(|_| {
            invalid(
                "extracted_text_required",
                "binary sources require an explicit --text file with extracted UTF-8 text",
            )
        })?,
    };
    if text.len() > limits.max_document_bytes {
        return Err(invalid(
            "input_too_large",
            "extracted text exceeds the document byte limit",
        ));
    }
    let actual_uri = url::Url::from_file_path(&source)
        .map_err(|_| {
            invalid(
                "invalid_source",
                "source path cannot be represented as a file URI",
            )
        })?
        .to_string();
    let document = DocumentInput {
        id: args.document_id,
        title: args.title.unwrap_or_else(|| {
            source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        }),
        source_uri: args.source_uri.unwrap_or(actual_uri.clone()),
        media_type: args.media_type.unwrap_or_else(|| {
            if binary {
                "application/octet-stream"
            } else {
                "text/plain"
            }
            .into()
        }),
        text,
        metadata: json!({"import_source_uri":actual_uri,"extraction":"caller-supplied"}),
    };
    let operation_id = match args.operation_id {
        Some(id) => id,
        None => format!(
            "import-{}",
            theorem_agent_core::digest(
                &json!({"document":document,"raw_sha256":format!("{:x}", Sha256::digest(&bytes))})
            )
            .map_err(|_| invalid("invalid_source", "cannot encode import identity"))?
        ),
    };
    let mut store = open_store(home)?;
    serde_json::to_value(
        store
            .ingest_document(&operation_id, document, Some(&bytes))
            .map_err(store_error)?,
    )
    .map_err(json_error)
}

type Providers = (
    Binding,
    Box<dyn DecisionProvider>,
    Box<dyn EffectExecutor>,
    Option<LocalEnvironment>,
);

fn providers(
    agent_id: &str,
    run_id: &str,
    workspace: &Workspace,
    args: ProviderArgs,
    task: &AssessmentTask,
    timeout_seconds: u64,
) -> Result<Providers, ProviderError> {
    let timeout = Duration::from_secs(timeout_seconds);
    if let Some(program) = args.host_command {
        let host = HostConnection::connect(&program, args.host_arg, timeout)?;
        let binding = host.binding(workspace.identity.clone())?;
        let context = HostContext {
            run_id: run_id.into(),
            agent_id: agent_id.into(),
            workspace: workspace.directory.clone(),
            binding: binding.clone(),
            admitted_tools: task.tools.clone(),
        };
        let (policy, executor) = host.adapters(context);
        Ok((binding, Box::new(policy), Box::new(executor), None))
    } else {
        let environment = LocalEnvironment::inspect(&workspace.directory, &task.tools)?;
        let binding = Binding {
            policy: "rust-assessment-v1".into(),
            execution: environment.binding(&task.tools, timeout_seconds)?,
            workspace: workspace.identity.clone(),
        };
        Ok((
            binding,
            Box::new(RustAssessmentPolicy),
            Box::new(RustToolExecutor::new(
                workspace.directory.clone(),
                &task.tools,
                timeout,
            )),
            Some(environment),
        ))
    }
}

fn start(home: &Path, worker: &LangGraphWorker, args: RunArgs) -> Result<Value, ProviderError> {
    let run_id = args
        .run_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    validate_id(&run_id).map_err(|_| {
        invalid(
            "invalid_run_id",
            "run-id needs 1 to 128 ASCII letters, digits, underscores or hyphens",
        )
    })?;
    let workspace = Workspace::resolve(
        &args.workspace.workspace,
        args.workspace.workspace_id.as_deref(),
    )?;
    if !workspace.directory.join("Cargo.toml").is_file() {
        return Err(invalid("invalid_workspace", "workspace needs a Cargo.toml"));
    }
    let mut task = AssessmentTask::new(args.tool);
    task.goal = args.goal;
    task.validate()?;
    let mode = if args.provider.host_command.is_some() {
        ProviderMode::NativeHost
    } else {
        ProviderMode::Standalone
    };
    let mut store = open_store(home)?;
    if store
        .get_record(&run_key(&run_id))
        .map_err(store_error)?
        .is_some()
    {
        return Err(invalid(
            "run_exists",
            "run-id already belongs to an admitted run; use resume or a new run-id",
        ));
    }
    let agent_id = store.identity().id.clone();
    let (binding, mut policy, mut executor, environment) = providers(
        &agent_id,
        &run_id,
        &workspace,
        args.provider,
        &task,
        args.timeout_seconds,
    )?;
    let manifest = RunManifest {
        schema_version: 1,
        agent_id,
        run_id,
        binding,
        mode,
        task,
        timeout_seconds: args.timeout_seconds,
        workspace_evidence: workspace.evidence,
        local_environment: environment,
    };
    store
        .put_record(
            &run_key(&manifest.run_id),
            &serde_json::to_value(&manifest).map_err(json_error)?,
        )
        .map_err(store_error)?;
    let task = serde_json::to_value(&manifest.task).map_err(json_error)?;
    invoke(
        home,
        worker,
        &mut store,
        &manifest,
        policy.as_mut(),
        executor.as_mut(),
        RunMode::Start { task },
        args.max_steps,
    )
}

fn resume(home: &Path, worker: &LangGraphWorker, args: ResumeArgs) -> Result<Value, ProviderError> {
    let mut store = open_existing(home)?;
    let manifest = load_manifest(&store, &args.run_id)?;
    let workspace = Workspace::resolve(
        &args.workspace.workspace,
        args.workspace.workspace_id.as_deref(),
    )?;
    if workspace.identity != manifest.binding.workspace {
        return Err(invalid(
            "binding_mismatch",
            "workspace identity differs from the original run",
        ));
    }
    let requested_mode = if args.provider.host_command.is_some() {
        ProviderMode::NativeHost
    } else {
        ProviderMode::Standalone
    };
    if requested_mode != manifest.mode {
        return Err(invalid(
            "binding_mismatch",
            "resume requires the original provider mode; supply the same --host-command for a native run",
        ));
    }
    let (binding, mut policy, mut executor, _) = providers(
        &manifest.agent_id,
        &manifest.run_id,
        &workspace,
        args.provider,
        &manifest.task,
        manifest.timeout_seconds,
    )?;
    if binding != manifest.binding {
        return Err(invalid(
            "binding_mismatch",
            "provider identities, selected tool versions, or material configuration changed since admission",
        ));
    }
    invoke(
        home,
        worker,
        &mut store,
        &manifest,
        policy.as_mut(),
        executor.as_mut(),
        RunMode::Resume,
        args.max_steps,
    )
}

fn status(home: &Path, worker: &LangGraphWorker, run_id: &str) -> Result<Value, ProviderError> {
    let mut store = open_existing(home)?;
    let manifest = load_manifest(&store, run_id)?;
    invoke(
        home,
        worker,
        &mut store,
        &manifest,
        &mut NoPolicy,
        &mut NoExecutor,
        RunMode::Status,
        100,
    )
}

fn load_manifest(store: &EmbeddedAgentStore, run_id: &str) -> Result<RunManifest, ProviderError> {
    validate_id(run_id).map_err(|_| invalid("invalid_run_id", "invalid run-id"))?;
    let value = store
        .get_record(&run_key(run_id))
        .map_err(store_error)?
        .ok_or_else(|| invalid("run_not_found", "run-id is not in this agent home"))?;
    let manifest: RunManifest = serde_json::from_value(value).map_err(|_| {
        invalid(
            "invalid_run_manifest",
            "saved run manifest has an unsupported shape",
        )
    })?;
    if manifest.schema_version != 1
        || manifest.agent_id != store.identity().id
        || manifest.run_id != run_id
        || !(1..=86400).contains(&manifest.timeout_seconds)
    {
        return Err(invalid(
            "invalid_run_manifest",
            "saved run manifest schema or ownership is invalid",
        ));
    }
    manifest.task.validate()?;
    manifest
        .binding
        .validate()
        .map_err(|_| invalid("invalid_run_manifest", "saved provider binding is invalid"))?;
    Ok(manifest)
}

#[allow(clippy::too_many_arguments)]
fn invoke(
    home: &Path,
    worker: &LangGraphWorker,
    store: &mut EmbeddedAgentStore,
    manifest: &RunManifest,
    policy: &mut dyn DecisionProvider,
    executor: &mut dyn EffectExecutor,
    mode: RunMode,
    max_steps: u32,
) -> Result<Value, ProviderError> {
    let mut records = AgentRecords(store);
    let mut host = match &mode {
        RunMode::Start { .. } => DurableHost::new(
            &manifest.agent_id,
            &manifest.run_id,
            manifest.binding.clone(),
            &mut records,
            policy,
            executor,
        ),
        RunMode::Resume | RunMode::Status => DurableHost::attach(
            &manifest.agent_id,
            &manifest.run_id,
            manifest.binding.clone(),
            &mut records,
            policy,
            executor,
        ),
    }
    .map_err(runtime_error)?;
    let result = worker
        .invoke(home, &mut host, mode, max_steps)
        .map_err(runtime_error)?;
    serde_json::to_value(result).map_err(json_error)
}

fn runtime_error(error: RuntimeError) -> ProviderError {
    invalid(error.code(), error.to_string())
}
fn run_key(run_id: &str) -> String {
    format!("agent-rust:run:{run_id}")
}

struct NoPolicy;
impl DecisionProvider for NoPolicy {
    fn decide(&mut self, _: &DecisionRequest) -> Result<Decision, ProviderError> {
        Err(invalid("status_callback", "status cannot decide work"))
    }
}
struct NoExecutor;
impl EffectExecutor for NoExecutor {
    fn execute(&mut self, _: &str, _: &Effect) -> Result<Observation, ProviderError> {
        Err(invalid("status_callback", "status cannot execute work"))
    }
}
