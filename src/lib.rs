//! Rust assessment policy and tool adapters for the shared Theorem durable host.
//!
//! This crate owns specialist behavior, explicit tool admission and terminal UX.
//! `theorem-agent-runtime` and the real LangGraph worker own run execution/recovery.

mod host;
mod policy;
mod process;
mod tools;
mod workspace;

#[cfg(feature = "cli")]
pub mod cli;

pub use host::{HostConnection, HostContext, HostExecutor, HostPolicy};
pub use policy::{AssessmentTask, RustAssessmentPolicy};
pub use tools::{LocalEnvironment, RustToolExecutor, ToolId, doctor};
pub use workspace::{Workspace, normalize_remote};

#[cfg(feature = "embedded-store")]
use serde_json::Value;
use theorem_agent_core::ProviderError;
#[cfg(feature = "embedded-store")]
use theorem_agent_runtime::RecordStore;
#[cfg(feature = "embedded-store")]
use theorem_agent_store::{EmbeddedAgentStore, StoreError};

/// Small adapter; receipts share this agent's embedded RustyRed home with context.
#[cfg(feature = "embedded-store")]
pub struct AgentRecords<'a>(pub &'a mut EmbeddedAgentStore);

#[cfg(feature = "embedded-store")]
impl RecordStore for AgentRecords<'_> {
    fn load(&self, key: &str) -> Result<Option<Value>, ProviderError> {
        self.0.get_record(key).map_err(store_error)
    }

    fn save(&mut self, key: &str, value: &Value) -> Result<(), ProviderError> {
        self.0.put_record(key, value).map_err(store_error)
    }
}

#[cfg(feature = "embedded-store")]
pub(crate) fn store_error(error: StoreError) -> ProviderError {
    ProviderError::new(error.code(), error.to_string())
}

pub(crate) fn invalid(code: &str, message: impl Into<String>) -> ProviderError {
    ProviderError::new(code, message)
}

pub(crate) fn json_error(error: serde_json::Error) -> ProviderError {
    ProviderError::new("invalid_json", error.to_string())
}
