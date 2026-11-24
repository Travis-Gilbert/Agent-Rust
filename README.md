# Agent-Rust

A portable Rust specialist with its own embedded RustyRed home. Theorem's shared Rust contracts own policy and effect admission, and the **actual LangGraph Python worker** owns checkpointing and recovery.

The standalone policy performs an explicit assessment using a fixed Cargo tool catalogue. It has no hidden model client. A native host can supply the next-action policy and capability execution through the documented native command adapter. Both modes use the same `DurableHost` and `LangGraphWorker`; this repository implements no second run loop.

## Build and install the worker

`Cargo.toml` and `Cargo.lock` pin all three framework crates to [Theorem revision `04ad34cdb0df7cb6e30504c0ec638d7b7d975071`](https://github.com/Theoremrs/Theorem/commit/04ad34cdb0df7cb6e30504c0ec638d7b7d975071). An independent build needs Git read access to that repository. Install the worker from a Theorem checkout at the same revision so its protocol matches the Rust host.

```sh
cargo build --locked
uv sync --locked --python 3.12 --project /path/to/Theorem/packages/theorem-agent-langgraph
export THEOREM_AGENT_WORKER=/path/to/Theorem/packages/theorem-agent-langgraph/.venv/bin/theorem-agent-worker
```

The recovery worker is packaged separately because this implementation uses upstream LangGraph’s Python runtime. It needs no LangSmith account or hosted agent service. Installing this package does not install cargo-mutants, Kani, Miri, or other optional developer tools.

The default Cargo feature `cli` enables the executable, clap, its import utilities, and `embedded-store`. A native consumer can use `default-features = false` to reuse the specialist policy/executor/host adapters **without linking `theorem-agent-store` or RustyRed**. Add the `embedded-store` feature explicitly when the native consumer also wants the provided `AgentRecords` adapter and database backend. Its own host can otherwise supply the framework's `RecordStore` implementation.

## Persistent home and explicit context

Always supply an absolute, dedicated `--home`. There is no implicit shared global agent identity. A new home creates a persistent UUID, `agent.rr`, and, when a run starts, LangGraph's `checkpoints.sqlite3`. RustyRed stores imported documents and host decision/effect receipts; SQLite stores execution checkpoints. Close the home before moving or copying it. Its identity does not depend on its pathname.

```sh
agent-rust --home /absolute/agent-home init --name Ferris
agent-rust --home /absolute/agent-home profile
agent-rust --home /absolute/agent-home profile --name "Rust reviewer" --role "Rust verification"
agent-rust --home /absolute/agent-home import --file /absolute/architecture.md
agent-rust --home /absolute/agent-home search "durable execution"
agent-rust --home /absolute/agent-home source sha256:document-digest
```

The name and role are bounded metadata in `agent.profile`. Renaming the profile or choosing a different model does not create a new UUID or discard imported context. These fields are not automatically injected as model instructions. Reopening `init` preserves the existing profile unless `--name` is explicitly supplied.

An import accepts UTF-8 text directly. For a binary original, supply `--text /absolute/extracted.txt`; the caller supplies extraction rather than the CLI guessing a parser. `--source-uri` can specify the logical original source. Without it, provenance is the actual canonical file URI. The metadata also records the actual supplied file URI. There is no recursive crawl, implicit network fetch, or automatic prompt injection from imported content.

`--document-id` selects an immutable document identity. Otherwise the content and provenance determine it. `--operation-id` permits an explicit stable retry identity; the default derives one from the full request and raw-source digest. Reusing an identity with changed content fails. Search returns ranked excerpts and provenance. `source --raw` additionally returns the exact original bytes as base64.

## Run an assessment

```sh
agent-rust --home /absolute/agent-home --worker "$THEOREM_AGENT_WORKER" run \
  --workspace /absolute/rust-project --workspace-id rust-project \
  --run-id assessment-1 --tool check --tool test

agent-rust --home /absolute/agent-home --worker "$THEOREM_AGENT_WORKER" status assessment-1

agent-rust --home /absolute/agent-home --worker "$THEOREM_AGENT_WORKER" resume assessment-1 \
  --workspace /new/location/rust-project --workspace-id rust-project
```

`--workspace` is explicit on both run and resume. If the checkout has a hosted Git `origin`, `--workspace-id` is optional: identity hashes the normalized remote host/repository and the workspace's relative location inside that repository. The absolute checkout root and remote credentials are excluded. A non-Git workspace, missing origin, local-file remote, or unsupported remote needs an explicit stable ID. The caller is responsible for attaching the intended checkout when reusing an explicit ID.

The default selection is `check`. Repeat `--tool` or give a comma-separated list to admit more operations. The standalone policy runs exactly that list and reports each terminal observation. `--goal` is descriptive context forwarded to a native host; it does not turn the standalone policy into a model agent.

The default is an assessment without a source repair operation. Cargo builds can write artifacts, resolve/download workspace dependencies, and run build scripts. `test`, `mutants`, Kani and Miri execute project-defined code or harnesses. Run them in an environment appropriate for that project. This adapter is a process runner, not an isolation sandbox.

`--max-steps` limits LangGraph supersteps per invocation, not policy decisions or Cargo commands. Exhaustion returns `running`, then `resume` continues. `--timeout-seconds` binds each local command/native call; `--worker-timeout-seconds` controls the entire invocation. A completed run remains readable and resumable without repeating its checks.

## Tool catalogue

`doctor --workspace /absolute/project` probes Cargo/rustc and `cargo --list`, then returns JSON describing command detection and integration requirements. A detected command is not proof that a project's toolchain and harnesses are ready. Admission records versions of the selected local tools; resume refuses a changed provider binding.

| Tool ID | Fixed command | Scope |
| --- | --- | --- |
| `check` | `cargo check --locked --all-targets` | Default compile assessment; requires a resolved lockfile |
| `test` | `cargo test --locked --all-targets` | Explicit project tests |
| `clippy` | `cargo clippy --locked --all-targets -- -D warnings` | Installed Clippy; warnings produce a failed observation |
| `shear` | `cargo shear` | Unused-dependency report; no `--fix` |
| `outdated` | `cargo outdated --format json` | Dependency-version report |
| `mutants` | `cargo mutants` | Explicit, potentially expensive mutation testing |
| `kani` | `cargo kani` | Existing proof harnesses and installed Kani |
| `miri` | `cargo miri test --locked` | Miri in the selected compatible toolchain |
| `expand` | `cargo expand` | Diagnostic macro expansion |
| `upgrade-preview` | `cargo upgrade --dry-run` | cargo-edit preview; no manifest upgrade applied |

[Loom](https://github.com/tokio-rs/loom), [Criterion](https://github.com/bheisler/criterion.rs), [hotpath](https://github.com/hotpath-rs/hotpath), and [rust-sitter](https://crates.io/crates/rust-sitter) are source integrations. They need suitable tests, benchmarks, instrumentation, or grammars; the CLI does not infer a blind command for them. [BeautifulRust](https://github.com/morganlinton/BeautifulRust) is reference material. `doctor` includes the upstream links for every entry. There is no dependency installation or source-edit command in this version.

Tool output is retained as evidence, not parsed into an invented proof of correctness. Exit success means that selected command completed successfully. It does not establish test sufficiency, proof-harness coverage, or benchmark quality. Each stdout/stderr stream is bounded at 64 KiB of captured and JSON-encoded bytes and explicitly marked if truncated. Output collection shares the command deadline even if a detached descendant retains an inherited pipe. Linux retains the unreaped leader for safe process-group teardown. On macOS and other fallback targets, a leader that exits early is reaped; the runner then bounds output collection without signaling a potentially reused group ID, so descendants can remain.

## Recovery and uncertain effects

The graph checkpoints a Decision before an effect and its Observation before asking for another Decision. The Rust host persists stable decision/effect identities and receipts in RustyRed. Replayed callbacks return those receipts instead of blindly repeating work.

An execution timeout, lost native response, or process death after effect intent can leave an unknown outcome. That pauses the graph. A standalone Cargo process has no durable external job service to inspect after a crash, so it does not claim successful reconciliation and does not automatically rerun the command. A native provider can implement `effect.reconcile` against its own job/receipt ledger. Repeated unresolved resumes remain paused. Operator resolution/migration of a permanently uncertain standalone effect is not implemented in this version.

There is no background daemon. Checkpoints survive process exit, but continuous work needs a supervising host. Moving a closed home preserves context and identity; it does not move the attached source checkout or restore its working tree. Admission records Git revision/dirty status as evidence, not a filesystem snapshot.

## Native Theorem and terminal hosts

Within a native application, consume the library with the framework's `DecisionProvider`/`EffectExecutor` seam and reuse admitted providers. Across a process boundary, `--host-command /absolute/native-adapter` uses [the native protocol](NATIVE_PROTOCOL.md). Provider identity and versioned capabilities are checked before a run and again on resume. Provider failure never activates a local fallback mid-run.

This is a **new adapter contract**. The repository does not claim that an existing `theorem` CLI subcommand already implements it. A Theorem adapter must return one next-action Decision and separately apply/reconcile effects; it must not wrap an entire effectful model/tool turn inside `policy.decide`.

Claude Code and Codex can invoke the standalone CLI from their terminal tools, with a native skill/subagent describing its use. They retain their own model and permission controls. An MCP wrapper could expose this same core later, but it is not required to use the terminal CLI and is not inserted in the native Theorem path.

## JSON and validation

Normal commands print one JSON envelope with `protocol_version: 1`, `type: "result"`, and `data`. Failures print one `type: "error"` envelope with a stable code/message. Help and version retain standard clap output. Argument errors exit 2, runtime/validation errors exit 1, and a valid `completed`, `paused`, or `running` run result exits 0. Inspect the returned assessment outcome for tool findings.

Run unit, adapter and real embedded-store CLI tests with `cargo test --locked`. The process-level worker test is explicitly marked because it requires Python dependencies:

```sh
THEOREM_AGENT_WORKER=/absolute/theorem-agent-worker \
  cargo test --locked --test cli -- --include-ignored
```

Those tests use the real RustyRed store and real LangGraph/SQLite worker across new OS processes. One runs `check` and `test` against a tiny throwaway Rust crate, resumes after a checkpoint budget, and checks that completed status/resume do not repeat work. Another uses a native fixture provider that completes an effect but loses its response; resume inspects its existing child receipt and completes without local fallback or duplicate execution. No model or paid API is required. Framework tests separately cover additional crash windows around durable receipts.
