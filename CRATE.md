# agent-rust

## Responsibility

This crate is the first external consumer of Theorem's portable-agent contracts. It owns a Rust assessment policy, a fixed Cargo command registry, native host adapters, an embedded-store record adapter, and a clap CLI. It does not implement a scheduler or recovery graph.

`theorem-agent-core` defines Decision, Effect, Observation, binding and capability contracts. `theorem-agent-store` owns the RustyRed home, identity and explicit context import/search. `theorem-agent-runtime` owns durable decision/effect receipts and supervised worker/native-command transport. The separately installed `theorem-agent-langgraph` package uses upstream LangGraph with SQLite checkpoints.

The store dependency and `AgentRecords` adapter are behind `embedded-store`. The default `cli` feature enables it; a `default-features = false` native library consumer can reuse the policy and execution adapters without bringing another database backend into its dependency graph. UUID/base64/direct SHA-256 import utilities are also enabled only by `cli`.

## Modules

- `policy`: deterministic standalone next-action policy; it requires a persisted terminal observation to advance.
- `tools`: admitted argv catalogue, bounded execution and environment/version evidence.
- `workspace`: explicit or normalized Git identity independent of local checkout pathname.
- `host`: versioned native capability handshake, Decision delegation and effect reconciliation.
- `cli`: JSON commands and immutable run admission metadata.
- `process`: bounded child output and finite deadlines; owned Unix process groups support teardown while the leader remains unreaped.

## State and authority

Agent identity belongs to the selected home, independently of a run or model. Name/role metadata belongs to `agent.profile` and can change without changing that UUID. A run fixes provider identity, execution configuration and workspace identity. The CLI opens the RustyRed home for exclusive writing; the shared worker runtime additionally owns its home lock. `status` supplies providers that reject all callbacks.

Imported text is data with provenance. No imported content is automatically executed or used as model instructions. Host capability publication describes compatibility and explicit admission, not authentication. Host commands must be explicitly configured absolute paths, and the provider remains responsible for authorization on every request. Credentials stay outside saved bindings and command arguments.

## Current limits

The standalone policy is a deterministic assessment, not a conversational/autonomous model agent. No source repair, automatic crate installation, MCP server, Theorem UI, scheduler, generic shell, multimodal extraction, or library-instrumentation wizard is present. Ordinary tool output is bounded; full external job logs need a future artifact service.

Unknown effect outcomes are never treated as successful or blindly retried. Standalone Cargo has no post-crash inspection service and can remain paused. Native providers should use stable effect IDs to inspect their own durable job receipts. There is no checkpoint-schema migration or manual receipt override command.

The process-tree termination implementation is Unix-oriented. Linux uses `waitid` with `WNOWAIT` to retain the leader until group teardown completes. macOS and other targets use `try_wait`; once that reaps an exited leader, the runner does not signal its potentially reused process-group ID. Output collection still stops at its deadline, but descendants may remain after an early leader exit. Windows child-tree control has not been validated. This runner does not sandbox project code or prevent a deliberately detached descendant.

## Validation boundaries

Unit tests cover policy sequencing, unknown observation rejection and remote normalization. Adapter tests cover moved checkouts, denied tool arguments, native policy authority and lost-response uncertainty. CLI tests exercise actual RustyRed persistence across processes. The explicit worker integration test uses actual LangGraph and a tiny Rust crate. Framework tests own detailed receipt/checkpoint crash windows.

### Verified on 2026-11-24

The framework dependencies are pinned to Theorem commit `04ad34cdb0df7cb6e30504c0ec638d7b7d975071`, tree `320f02be9d7f6629a9f9c2468501b066d107c431`. Cargo generated the committed Git-source lockfile during an actual GitHub HTTPS fetch using the validation device's configured Git authentication. The three framework package sources changed from the integration paths to that revision; all other package names, versions, sources, and checksums stayed unchanged.

| Environment | Gate | Result |
| --- | --- | --- |
| Linux, Rust 1.99.1 | Native dependency tree and library check; default Clippy with warnings denied | Passed; native graph contains neither `theorem-agent-store` nor RustyRed |
| Linux, Rust 1.99.1 | Full consumer suite with the two real LangGraph worker tests enabled | 17 passed, 0 failed, 0 ignored |
| macOS, Rust 1.96.1 | Git-source metadata, native library check/tree, and default CLI binary build | Passed; exact published commit/tree and all 39 framework file blobs verified |
| macOS, Rust 1.96.1 | `cargo test --locked --no-default-features --lib process::tests` | 3 passed, covering output bounds and deadlines on the macOS fallback |
| macOS, Rust 1.96.1 and Python 3.12 | Real pinned worker installed with `uv sync --locked`; actual Cargo assessment across process restarts | Checkpoint budget returned `running`, resume completed it, and identity plus completed status/resume remained stable |

The Linux behavioral suite used the paired framework integration checkout. The subsequent target-configuration correction routes macOS to the existing bounded `try_wait` implementation and leaves the Linux `WNOWAIT` execution path unchanged. The macOS immutable-source gate used both Rust crates and Python worker code from Cargo's verified checkout of the published commit. These gates establish the stated build, persistence, and recovery behaviors; project correctness still depends on the selected Cargo tools and their coverage.
