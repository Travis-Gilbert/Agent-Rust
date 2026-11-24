use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use theorem_agent_core::{Effect, Observation, ProviderError, digest};
use theorem_agent_runtime::EffectExecutor;

use crate::{invalid, process::run_command};

/// Explicit tools in registry revision 1. Source-integrated libraries are not commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum ToolId {
    Check,
    Test,
    Clippy,
    Shear,
    Outdated,
    Mutants,
    Kani,
    Miri,
    Expand,
    UpgradePreview,
}

impl ToolId {
    pub const ALL: [Self; 10] = [
        Self::Check,
        Self::Test,
        Self::Clippy,
        Self::Shear,
        Self::Outdated,
        Self::Mutants,
        Self::Kani,
        Self::Miri,
        Self::Expand,
        Self::UpgradePreview,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Test => "test",
            Self::Clippy => "clippy",
            Self::Shear => "shear",
            Self::Outdated => "outdated",
            Self::Mutants => "mutants",
            Self::Kani => "kani",
            Self::Miri => "miri",
            Self::Expand => "expand",
            Self::UpgradePreview => "upgrade-preview",
        }
    }

    /// Exact argv; no free-form arguments and no automatic repair/install command.
    pub fn argv(self) -> &'static [&'static str] {
        match self {
            Self::Check => &["check", "--locked", "--all-targets"],
            Self::Test => &["test", "--locked", "--all-targets"],
            Self::Clippy => &[
                "clippy",
                "--locked",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            Self::Shear => &["shear"],
            Self::Outdated => &["outdated", "--format", "json"],
            Self::Mutants => &["mutants"],
            Self::Kani => &["kani"],
            Self::Miri => &["miri", "test", "--locked"],
            Self::Expand => &["expand"],
            Self::UpgradePreview => &["upgrade", "--dry-run"],
        }
    }

    fn source(self) -> &'static str {
        match self {
            Self::Check => "https://doc.rust-lang.org/cargo/commands/cargo-check.html",
            Self::Test => "https://doc.rust-lang.org/cargo/commands/cargo-test.html",
            Self::Clippy => "https://github.com/rust-lang/rust-clippy",
            Self::Shear => "https://github.com/Boshen/cargo-shear",
            Self::Outdated => "https://github.com/kbknapp/cargo-outdated",
            Self::Mutants => "https://github.com/sourcefrog/cargo-mutants",
            Self::Kani => "https://github.com/model-checking/kani",
            Self::Miri => "https://github.com/rust-lang/miri",
            Self::Expand => "https://github.com/dtolnay/cargo-expand",
            Self::UpgradePreview => "https://github.com/killercup/cargo-edit",
        }
    }

    fn note(self) -> &'static str {
        match self {
            Self::Check => {
                "Compiles all targets; Cargo.lock must already resolve. Build scripts run and build artifacts may be written."
            }
            Self::Test => {
                "Compiles and executes project tests and build scripts; their side effects are project-defined."
            }
            Self::Clippy => {
                "Requires Clippy in the selected toolchain; warnings are reported as a failing command."
            }
            Self::Shear => {
                "Reports possible unused dependencies without --fix. Findings require review, especially macro-generated references."
            }
            Self::Outdated => {
                "Queries dependency versions and may access registries; JSON output is retained as tool evidence."
            }
            Self::Mutants => {
                "Opt-in mutation testing; may be expensive and writes reports/build artifacts."
            }
            Self::Kani => {
                "Requires installed Kani and suitable proof harnesses; harness coverage is not inferred."
            }
            Self::Miri => {
                "Requires Miri in the already selected compatible toolchain; no component or toolchain is installed here."
            }
            Self::Expand => {
                "Requires cargo-expand and its compatible compiler; expansion is diagnostic evidence, not a source rewrite."
            }
            Self::UpgradePreview => {
                "cargo-edit preview only. --dry-run does not apply manifest upgrades."
            }
        }
    }
}

/// Captured versions of the explicitly selected local providers; included in run binding.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalEnvironment {
    pub cargo_version: String,
    pub rustc_version: String,
    pub tool_versions: BTreeMap<String, String>,
}

impl LocalEnvironment {
    pub fn inspect(workspace: &Path, admitted: &[ToolId]) -> Result<Self, ProviderError> {
        let cargo_version = version(Path::new("cargo"), &["--version"], workspace)?;
        let rustc_version = version(Path::new("rustc"), &["--version"], workspace)?;
        let mut tool_versions = BTreeMap::new();
        for tool in admitted {
            let tool_version = match tool {
                ToolId::Check | ToolId::Test => cargo_version.clone(),
                _ => version(
                    Path::new("cargo"),
                    &[tool.argv()[0], "--version"],
                    workspace,
                )?,
            };
            tool_versions.insert(tool.name().to_owned(), tool_version);
        }
        Ok(Self {
            cargo_version,
            rustc_version,
            tool_versions,
        })
    }

    pub fn binding(&self, tools: &[ToolId], timeout_seconds: u64) -> Result<String, ProviderError> {
        let hash = digest(
            &json!({"registry":1,"tools":tools,"versions":self,"timeout_seconds":timeout_seconds}),
        )
        .map_err(|_| invalid("invalid_binding", "cannot encode local provider binding"))?;
        Ok(format!("rust-tools-v1:{hash}"))
    }
}

fn version(program: &Path, args: &[&str], workspace: &Path) -> Result<String, ProviderError> {
    let output = run_command(program, args, workspace, Duration::from_secs(20))?;
    if !output.success || output.stdout_truncated || output.stderr_truncated {
        return Err(invalid(
            "tool_unavailable",
            format!(
                "{} {} is unavailable in this workspace; run doctor and install/configure it explicitly",
                program.display(),
                args.join(" ")
            ),
        ));
    }
    let value = if output.stdout.trim().is_empty() {
        output.stderr.trim()
    } else {
        output.stdout.trim()
    };
    if value.is_empty() {
        return Err(invalid(
            "tool_unavailable",
            "tool did not provide an implementation version",
        ));
    }
    Ok(value.to_owned())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolArguments {
    tool: ToolId,
}

/// An explicitly admitted executor bound to one workspace. Recovery never re-runs an
/// uncertain command: without a completed host receipt, reconciliation remains unknown.
pub struct RustToolExecutor {
    workspace: PathBuf,
    admitted: BTreeSet<ToolId>,
    timeout: Duration,
}

impl RustToolExecutor {
    pub fn new(workspace: PathBuf, admitted: &[ToolId], timeout: Duration) -> Self {
        Self {
            workspace,
            admitted: admitted.iter().copied().collect(),
            timeout,
        }
    }
}

impl EffectExecutor for RustToolExecutor {
    fn execute(&mut self, effect_id: &str, effect: &Effect) -> Result<Observation, ProviderError> {
        if effect.capability != "rust.tool" {
            return Err(invalid(
                "capability_denied",
                "standalone executor only admits rust.tool",
            ));
        }
        let arguments: ToolArguments =
            serde_json::from_value(effect.arguments.clone()).map_err(|_| {
                invalid(
                    "invalid_effect",
                    "rust.tool requires only a known tool identifier",
                )
            })?;
        if !self.admitted.contains(&arguments.tool) {
            return Err(invalid(
                "capability_denied",
                "tool was not admitted when the run started",
            ));
        }
        let output = match run_command(
            Path::new("cargo"),
            arguments.tool.argv(),
            &self.workspace,
            self.timeout,
        ) {
            Ok(output) => output,
            Err(error) if error.code == "command_unavailable" => {
                return Ok(Observation::failed(
                    json!({"effect_id":effect_id,"tool":arguments.tool,"error":error}),
                ));
            }
            Err(error) => {
                return Ok(Observation::unknown(
                    json!({"effect_id":effect_id,"tool":arguments.tool,"error":error}),
                ));
            }
        };
        let result = json!({"effect_id":effect_id,"tool":arguments.tool,"program":"cargo","argv":arguments.tool.argv(),"process":output});
        if output.timed_out {
            Ok(Observation::unknown(result))
        } else if output.success {
            Ok(Observation::completed(result))
        } else {
            Ok(Observation::failed(result))
        }
    }
}

/// Describe detected commands and integration recipes. Detection does not prove a
/// project's harnesses or toolchain are ready, and this function installs nothing.
pub fn doctor(workspace: &Path) -> Value {
    let cargo = run_command(
        Path::new("cargo"),
        &["--version"],
        workspace,
        Duration::from_secs(10),
    );
    let rustc = run_command(
        Path::new("rustc"),
        &["--version"],
        workspace,
        Duration::from_secs(10),
    );
    let list = run_command(
        Path::new("cargo"),
        &["--list"],
        workspace,
        Duration::from_secs(10),
    );
    let available: BTreeSet<String> = list
        .as_ref()
        .ok()
        .filter(|output| output.success)
        .map(|output| {
            output
                .stdout
                .lines()
                .filter_map(|line| line.split_whitespace().next())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let tools: Vec<Value> = ToolId::ALL.into_iter().map(|tool| json!({
        "id":tool,"kind":"cargo-command","detected":available.contains(tool.argv()[0]),"program":"cargo", "argv":tool.argv(),"default":tool == ToolId::Check,"note":tool.note(),"source":tool.source()
    })).collect();
    json!({
        "registry_version":1,
        "cargo":probe_json(cargo),"rustc":probe_json(rustc),"cargo_list":probe_json(list),"tools":tools,
        "integrations":[
            {"id":"loom","kind":"source-library","source":"https://github.com/tokio-rs/loom","note":"Write tests with Loom synchronization primitives and bounded model exploration before selecting appropriate tests."},
            {"id":"criterion","kind":"source-library","source":"https://github.com/bheisler/criterion.rs","note":"Add benchmark targets, measured functions and a baseline. No arbitrary cargo bench is inferred."},
            {"id":"hotpath","kind":"source-library","source":"https://github.com/hotpath-rs/hotpath","note":"Instrument selected code and configure collection before profiling; it is not an ambient diagnostic command."},
            {"id":"rust-sitter","kind":"source-library","source":"https://crates.io/crates/rust-sitter","note":"Grammar/parser construction library, not a general Rust source analyzer or compiler replacement."},
            {"id":"beautiful-rust","kind":"reference","source":"https://github.com/morganlinton/BeautifulRust","note":"Reference material for human or model review; no executable is inferred."}
        ]
    })
}

fn probe_json(output: Result<crate::process::CommandOutput, ProviderError>) -> Value {
    match output {
        Ok(output) => json!({"result":output}),
        Err(error) => json!({"error":error}),
    }
}
