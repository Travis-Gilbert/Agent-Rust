use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use theorem_agent_core::{ProviderError, digest, validate_id};

use crate::{invalid, process::run_command};

/// Local attachment plus a stable repository identity that survives moving a checkout.
#[derive(Clone, Debug)]
pub struct Workspace {
    pub directory: PathBuf,
    pub identity: String,
    pub evidence: Value,
}

impl Workspace {
    pub fn resolve(path: &Path, explicit_id: Option<&str>) -> Result<Self, ProviderError> {
        let directory = path.canonicalize().map_err(|_| {
            invalid(
                "invalid_workspace",
                "workspace must be an existing directory",
            )
        })?;
        if !directory.is_dir() {
            return Err(invalid(
                "invalid_workspace",
                "workspace must be a directory",
            ));
        }
        let revision = git(&directory, &["rev-parse", "HEAD"]).ok();
        let dirty = git(
            &directory,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )
        .ok()
        .map(|status| !status.is_empty());
        if let Some(id) = explicit_id {
            validate_id(id).map_err(|_| {
                invalid(
                    "invalid_workspace_id",
                    "workspace-id needs 1 to 128 ASCII letters, digits, underscores or hyphens",
                )
            })?;
            return Ok(Self {
                directory,
                identity: format!("workspace:{id}"),
                evidence: json!({"identity_source":"explicit", "revision":revision, "dirty":dirty}),
            });
        }
        let root = git(&directory, &["rev-parse", "--show-toplevel"]).map_err(|_| {
            invalid(
                "workspace_id_required",
                "a non-Git workspace requires --workspace-id",
            )
        })?;
        let root = PathBuf::from(root).canonicalize().map_err(|_| {
            invalid(
                "workspace_id_required",
                "cannot resolve the Git repository root; supply --workspace-id",
            )
        })?;
        let relative = directory.strip_prefix(&root).map_err(|_| {
            invalid(
                "workspace_id_required",
                "workspace is outside its Git root; supply --workspace-id",
            )
        })?;
        let relative = relative
            .to_str()
            .ok_or_else(|| {
                invalid(
                    "workspace_id_required",
                    "repository path is not UTF-8; supply --workspace-id",
                )
            })?
            .replace('\\', "/");
        let remote = git(&root, &["remote", "get-url", "origin"]).map_err(|_| {
            invalid(
                "workspace_id_required",
                "Git origin is missing; supply --workspace-id",
            )
        })?;
        let remote = normalize_remote(&remote)?;
        let repository_digest = digest(&json!({"repository":remote,"relative_workspace":relative}))
            .map_err(|_| invalid("invalid_workspace", "cannot encode workspace identity"))?;
        Ok(Self {
            directory,
            identity: format!("git:{repository_digest}"),
            evidence: json!({"identity_source":"git-origin", "relative_workspace":relative, "revision":revision, "dirty":dirty}),
        })
    }
}

/// Normalize hosted Git origin without retaining authentication or local pathnames.
pub fn normalize_remote(remote: &str) -> Result<String, ProviderError> {
    let remote = remote.trim();
    let url = if remote.contains("://") {
        url::Url::parse(remote).ok()
    } else if let Some((host, path)) = remote.split_once(':') {
        if host.contains('/') || host.contains('\\') || host.len() < 2 {
            None
        } else {
            url::Url::parse(&format!("ssh://{host}/{path}")).ok()
        }
    } else {
        None
    };
    let url = url.ok_or_else(remote_error)?;
    if !matches!(url.scheme(), "https" | "http" | "ssh" | "git") {
        return Err(remote_error());
    }
    let host = url.host_str().ok_or_else(remote_error)?;
    let path = url.path().trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty() || host.is_empty() || url.query().is_some() || url.fragment().is_some() {
        return Err(remote_error());
    }
    let port = url
        .port()
        .filter(|port| !(url.scheme() == "ssh" && *port == 22));
    Ok(match port {
        Some(port) => format!("{}:{port}/{path}", host.to_ascii_lowercase()),
        None => format!("{}/{path}", host.to_ascii_lowercase()),
    })
}

fn remote_error() -> ProviderError {
    invalid(
        "workspace_id_required",
        "origin must identify a hosted repository without query parameters; otherwise supply --workspace-id",
    )
}

fn git(directory: &Path, args: &[&str]) -> Result<String, ProviderError> {
    let output = run_command(Path::new("git"), args, directory, Duration::from_secs(10))?;
    if !output.success || output.stdout_truncated {
        return Err(invalid(
            "git_unavailable",
            "Git metadata is unavailable or too large",
        ));
    }
    Ok(output.stdout.trim_end().to_owned())
}
