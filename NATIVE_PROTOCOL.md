# Native host protocol, version 1

`--host-command` names an explicitly authorized absolute executable. Repeated `--host-arg` values are literal argv entries. The shared `JsonCommand` transport starts one process per request, supplies one JSON line on stdin, closes stdin, and requires one JSON response line followed by successful exit. No shell or MCP transport is involved.

Credentials and live provider handles are supplied by the native adapter's own environment/session. Do not put them in provider IDs, bindings, or command arguments. The native adapter can attach to an already running Theorem owner or borrow in-process providers in its own host implementation.

## Handshake

The CLI first sends:

```json
{"protocol_version":1,"method":"capabilities"}
```

A compatible adapter returns exactly the handshake fields:

```json
{
  "protocol_version": 1,
  "provider_id": "theorem-policy-config-v1",
  "capabilities": [
    {"name":"policy.decide","provider":"theorem-policy-config-v1","version":1},
    {"name":"rust.tool","provider":"theorem-tools-v1","version":1},
    {"name":"compute.offload","provider":"theorem-offload-v1","version":1}
  ]
}
```

`provider_id` must be stable and change when material policy/implementation/configuration changes. Capability versions describe the operation contract. This first version accepts contract version 1, requires `policy.decide` and at least one execution capability, and refuses duplicates. The sorted full handshake is hashed into policy/execution binding identities. Resume requires the same binding. A mismatch or unavailable host is an error; it does not select a local provider.

## Requests after admission

Each request contains `protocol_version`, `method`, `request_id`, the expected `provider_id`, `context`, and `params`. Context contains:

- `run_id` and persistent `agent_id`;
- `workspace`, the absolute local attachment for this invocation;
- `binding`, with nonsecret stable policy/execution/workspace identities;
- `admitted_tools`, the caller's explicit Rust tool selection.

The absolute local attachment is passed to the host as invocation context. It is not used as persistent workspace identity. The provider must validate the expected identity/binding and apply its own authorization before accessing the workspace or other capabilities.

Every response after handshake echoes the request ID and contains exactly one result or error:

```json
{"protocol_version":1,"request_id":"run:decision:0","result":{"kind":"complete","result":{"message":"assessment complete"},"policy_state":null}}
```

```json
{"protocol_version":1,"request_id":"run:decision:0","error":{"code":"unavailable","message":"bound provider is unavailable"}}
```

The methods are:

| Method | Request ID | Params | Result |
| --- | --- | --- | --- |
| `policy.decide` | Stable `decision_id` | A framework `DecisionRequest` | One `Decision` |
| `effect.execute` | `effect_id` + `:execute` | `effect_id` and `effect` | One `Observation` |
| `effect.reconcile` | `effect_id` + `:reconcile` | The same `effect_id` and `effect` | Observation or `null` while unresolved |

The framework creates `run-id:decision:N` and `run-id:effect:N`. Request transport IDs are not substitutes for effect IDs in the host's durable ledger.

`DecisionRequest` includes `decision_id`, `sequence`, original `task`, opaque `policy_state`, prior `observation` or null, `agent_id`, and the fixed `binding`. Return one of:

```json
{"kind":"effect","effect":{"capability":"rust.tool","arguments":{"tool":"check"}},"policy_state":{"phase":"compile"}}
```

```json
{"kind":"complete","result":{"summary":"No further work selected"},"policy_state":{"phase":"done"}}
```

The `policy.decide` operation must select the next action without executing that action. It must not call a complete effectful agent turn and then return a fabricated Decision. Theorem may consult its router/model/offload admission logic here while LangGraph remains policy-free.

`Observation` has `status: "completed" | "failed" | "unknown"` and an arbitrary JSON `output`. A native `rust.tool` effect is restricted to a selected catalogue tool with no extra argv. Other effects must belong to the handshake's published capability set; their provider owns argument-schema validation and authorization.

## Recovery

The outer Rust durable host writes an intent before `effect.execute`. The native provider must also keep durable child-job/receipt identity if work crosses another process or service boundary. Namespace receipts by **`agent_id` plus the stable effect ID**: caller-selected run IDs can repeat across independent agent homes. Verify the effect and fixed binding before returning an existing receipt. Use the same agent scope for cached policy decisions. The local workspace attachment can move; its pathname is not the stable binding. Agent IDs identify scope and are not authentication credentials. When an execution response is lost, the CLI reports an unknown observation and LangGraph pauses.

`effect.reconcile` must inspect the existing job or receipt. It must not rerun an operation merely because a process is absent. `null` or an unknown observation leaves the run paused; a known terminal observation is persisted before the next policy callback.

Capability presence is an explicit compatible publication, not automatic negotiation with an arbitrary environment. A native in-process host can use the same framework traits directly without this command wrapper.
