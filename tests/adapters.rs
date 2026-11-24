use std::path::Path;
use std::process::Command;
use std::time::Duration;

use agent_rust::{HostConnection, HostContext, RustToolExecutor, ToolId, Workspace};
use serde_json::json;
use theorem_agent_core::{Decision, DecisionRequest, Effect, ObservationStatus};
use theorem_agent_runtime::{DecisionProvider, EffectExecutor};

#[test]
fn git_workspace_identity_survives_a_checkout_move() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let moved = root.path().join("moved");
    std::fs::create_dir_all(first.join("crates/member")).unwrap();
    git(&first, &["init", "--quiet"]);
    git(
        &first,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:owner/repository.git",
        ],
    );
    let before = Workspace::resolve(&first.join("crates/member"), None).unwrap();
    std::fs::rename(&first, &moved).unwrap();
    let after = Workspace::resolve(&moved.join("crates/member"), None).unwrap();
    assert_eq!(before.identity, after.identity);
    assert_ne!(before.directory, after.directory);
    assert_ne!(
        after.identity,
        Workspace::resolve(&moved, None).unwrap().identity,
        "separate member roots stay distinguishable"
    );
    assert!(!after.identity.contains(root.path().to_str().unwrap()));
}

fn git(path: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn executor_refuses_unadmitted_tools_and_extra_shell_arguments() {
    let directory = tempfile::tempdir().unwrap();
    let mut executor = RustToolExecutor::new(
        directory.path().into(),
        &[ToolId::Check],
        Duration::from_secs(1),
    );
    let effect = Effect {
        capability: "shell".into(),
        arguments: json!({"command":"echo wrong"}),
    };
    assert_eq!(
        executor.execute("effect", &effect).unwrap_err().code,
        "capability_denied"
    );
    let effect = Effect {
        capability: "rust.tool".into(),
        arguments: json!({"tool":"test"}),
    };
    assert_eq!(
        executor.execute("effect", &effect).unwrap_err().code,
        "capability_denied"
    );
    let effect = Effect {
        capability: "rust.tool".into(),
        arguments: json!({"tool":"check","argv":["--fix"]}),
    };
    assert_eq!(
        executor.execute("effect", &effect).unwrap_err().code,
        "invalid_effect"
    );
}

#[cfg(unix)]
fn native_fixture(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, r#"#!/usr/bin/env python3
import json, os, sys
r = json.load(sys.stdin)
if r['method'] == 'capabilities':
    print(json.dumps({'protocol_version': 1, 'provider_id': 'fixture-policy-config-v1', 'capabilities': [
        {'name': 'policy.decide', 'provider': 'fixture-policy-config-v1', 'version': 1},
        {'name': 'rust.tool', 'provider': 'fixture-policy-config-v1', 'version': 1}]}))
elif r['method'] == 'policy.decide':
    assert r['params']['binding'] == r['context']['binding']
    print(json.dumps({'protocol_version': 1, 'request_id': r['request_id'], 'result': {'kind':'complete','result':{'source':'native-host'},'policy_state':None}}))
elif r['method'] == 'effect.execute':
    os._exit(9)
elif r['method'] == 'effect.reconcile':
    print(json.dumps({'protocol_version':1,'request_id':r['request_id'],'result':None}))
else:
    raise RuntimeError('unexpected method')
"#).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
#[cfg(unix)]
fn native_adapter_uses_host_policy_and_never_falls_back_after_a_lost_effect_response() {
    let directory = tempfile::tempdir().unwrap();
    let program = directory.path().join("native-fixture");
    native_fixture(&program);
    let connection = HostConnection::connect(&program, vec![], Duration::from_secs(10)).unwrap();
    let binding = connection.binding("workspace:fixture".into()).unwrap();
    let (mut policy, mut executor) = connection.adapters(HostContext {
        run_id: "run".into(),
        agent_id: "agent".into(),
        workspace: directory.path().into(),
        binding: binding.clone(),
        admitted_tools: vec![ToolId::Check],
    });
    let request = DecisionRequest {
        decision_id: "run:decision:0".into(),
        sequence: 0,
        task: json!({"goal":"host chooses"}),
        policy_state: serde_json::Value::Null,
        observation: None,
        agent_id: "agent".into(),
        binding,
    };
    let Decision::Complete { result, .. } = policy.decide(&request).unwrap() else {
        panic!("native host is the policy")
    };
    assert_eq!(result, json!({"source":"native-host"}));
    let effect = Effect {
        capability: "rust.tool".into(),
        arguments: json!({"tool":"check"}),
    };
    let observation = executor.execute("run:effect:0", &effect).unwrap();
    assert_eq!(observation.status, ObservationStatus::Unknown);
    assert!(
        executor
            .reconcile("run:effect:0", &effect)
            .unwrap()
            .is_none()
    );
    let unadmitted = Effect {
        capability: "rust.tool".into(),
        arguments: json!({"tool":"test"}),
    };
    assert_eq!(
        executor
            .execute("run:effect:1", &unadmitted)
            .unwrap_err()
            .code,
        "capability_denied"
    );
}

#[test]
fn host_command_must_be_an_explicit_absolute_file() {
    assert!(HostConnection::connect(Path::new("theorem"), vec![], Duration::from_secs(1)).is_err());
}
