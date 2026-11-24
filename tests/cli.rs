#![cfg(feature = "cli")]

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn cli(home: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-rust"))
        .arg("--home")
        .arg(home)
        .args(arguments)
        .output()
        .expect("run CLI")
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "CLI failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("one JSON result");
    assert_eq!(value["type"], "result");
    value["data"].clone()
}

fn failure(output: Output, code: &str) {
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("one JSON error");
    assert_eq!(value["type"], "error");
    assert_eq!(value["error"]["code"], code, "{value}");
}

#[test]
fn home_identity_and_explicit_import_search_source_survive_new_processes() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let source = directory.path().join("architecture.md");
    std::fs::write(
        &source,
        "Durable orchid execution records survive process restarts.",
    )
    .unwrap();
    let first = success(cli(&home, &["init"]));
    let again = success(cli(&home, &["init"]));
    assert_eq!(first["agent_id"], again["agent_id"]);
    let imported = success(cli(
        &home,
        &[
            "import",
            "--file",
            source.to_str().unwrap(),
            "--operation-id",
            "fixture-import",
        ],
    ));
    let retry = success(cli(
        &home,
        &[
            "import",
            "--file",
            source.to_str().unwrap(),
            "--operation-id",
            "fixture-import",
        ],
    ));
    assert_eq!(imported, retry);
    let hits = success(cli(&home, &["search", "orchid"]));
    assert_eq!(hits["hits"].as_array().unwrap().len(), 1);
    let document_id = imported["document_id"].as_str().unwrap();
    let retrieved = success(cli(&home, &["source", document_id, "--raw"]));
    assert!(
        retrieved["document"]["source_uri"]
            .as_str()
            .unwrap()
            .starts_with("file:///")
    );
    assert_eq!(
        retrieved["document"]["text"],
        "Durable orchid execution records survive process restarts."
    );
    assert_eq!(
        retrieved["raw_bytes"],
        "Durable orchid execution records survive process restarts.".len()
    );
    assert!(retrieved["raw_base64"].is_string());
}

#[test]
fn binary_import_requires_explicit_extracted_text() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let source = directory.path().join("paper.bin");
    let text = directory.path().join("extracted.txt");
    std::fs::write(&source, b"binary\xff\x00").unwrap();
    std::fs::write(&text, "Explicitly extracted cedar knowledge").unwrap();
    failure(
        cli(&home, &["import", "--file", source.to_str().unwrap()]),
        "extracted_text_required",
    );
    let imported = success(cli(
        &home,
        &[
            "import",
            "--file",
            source.to_str().unwrap(),
            "--text",
            text.to_str().unwrap(),
            "--source-uri",
            "https://example.test/paper",
        ],
    ));
    let retrieved = success(cli(
        &home,
        &["source", imported["document_id"].as_str().unwrap()],
    ));
    assert_eq!(
        retrieved["document"]["source_uri"],
        "https://example.test/paper"
    );
    assert_eq!(
        retrieved["document"]["text"],
        "Explicitly extracted cedar knowledge"
    );
    assert!(retrieved.get("raw_base64").is_none());
}

#[test]
fn a_named_profile_persists_without_changing_agent_identity_or_context() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let initial = success(cli(&home, &["init", "--name", "Ferris"]));
    let document = directory.path().join("context.txt");
    std::fs::write(&document, "Persistent orchid knowledge").unwrap();
    success(cli(
        &home,
        &["import", "--file", document.to_str().unwrap()],
    ));
    let profile = success(cli(&home, &["profile"]));
    assert_eq!(profile["name"], "Ferris");
    assert_eq!(profile["role"], "Rust engineering");
    let changed = success(cli(
        &home,
        &[
            "profile",
            "--name",
            "Rust reviewer",
            "--role",
            "Rust verification",
        ],
    ));
    assert_eq!(changed["agent_id"], initial["agent_id"]);
    assert_eq!(changed["name"], "Rust reviewer");
    assert_eq!(
        success(cli(&home, &["init"]))["agent_id"],
        initial["agent_id"]
    );
    assert_eq!(success(cli(&home, &["profile"])), changed);
    assert_eq!(
        success(cli(&home, &["search", "orchid"]))["hits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    failure(cli(&home, &["profile", "--name", ""]), "invalid_profile");
    assert_eq!(
        success(cli(&home, &["profile"])),
        changed,
        "rejected metadata leaves the profile intact"
    );
}

#[test]
fn doctor_distinguishes_source_integrations_from_executable_tools() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let report = success(cli(
        &home,
        &["doctor", "--workspace", directory.path().to_str().unwrap()],
    ));
    let integrations = report["integrations"].as_array().unwrap();
    for id in ["loom", "criterion", "hotpath", "rust-sitter"] {
        let recipe = integrations
            .iter()
            .find(|recipe| recipe["id"] == id)
            .unwrap();
        assert_eq!(recipe["kind"], "source-library");
        assert!(recipe.get("argv").is_none());
    }
    assert!(
        !home.exists(),
        "doctor does not initialize a persistent home"
    );
}

#[test]
fn unsafe_home_invalid_cli_and_non_git_workspace_are_explicit_errors() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    failure(cli(Path::new("relative-home"), &["init"]), "invalid_home");
    failure(cli(Path::new("/"), &["init"]), "invalid_home");
    failure(
        cli(
            &home,
            &[
                "run",
                "--workspace",
                directory.path().to_str().unwrap(),
                "--run-id",
                "fixture",
            ],
        ),
        "workspace_id_required",
    );
    failure(
        cli(
            &home,
            &[
                "run",
                "--workspace",
                directory.path().to_str().unwrap(),
                "--workspace-id",
                "fixture",
                "--tool",
                "shell",
            ],
        ),
        "invalid_arguments",
    );
    failure(cli(&home, &["status", "missing"]), "run_not_found");
}

fn worker() -> String {
    std::env::var("AGENT_RUST_TEST_WORKER").or_else(|_| std::env::var("THEOREM_AGENT_WORKER"))
        .expect("set AGENT_RUST_TEST_WORKER or THEOREM_AGENT_WORKER to the installed real theorem-agent-worker executable")
}

#[test]
#[ignore = "requires the real LangGraph worker; set AGENT_RUST_TEST_WORKER and run --include-ignored"]
fn real_rustyred_and_langgraph_run_resume_and_status_a_tiny_rust_crate() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let workspace = directory.path().join("tiny");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname = \"agent-rust-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        workspace.join("Cargo.lock"),
        "version = 4\n[[package]]\nname = \"agent-rust-fixture\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        workspace.join("src/lib.rs"),
        "pub fn add(a:u32,b:u32)->u32{a+b}\n#[test] fn it_adds(){assert_eq!(add(2,3),5);}\n",
    )
    .unwrap();
    let worker = worker();
    let first = success(cli(
        &home,
        &[
            "--worker",
            &worker,
            "run",
            "--workspace",
            workspace.to_str().unwrap(),
            "--workspace-id",
            "fixture",
            "--run-id",
            "assessment",
            "--tool",
            "check",
            "--tool",
            "test",
            "--max-steps",
            "1",
        ],
    ));
    assert_eq!(first["status"], "running");
    let resumed = success(cli(
        &home,
        &[
            "--worker",
            &worker,
            "resume",
            "assessment",
            "--workspace",
            workspace.to_str().unwrap(),
            "--workspace-id",
            "fixture",
        ],
    ));
    assert_eq!(resumed["status"], "completed");
    assert_eq!(resumed["state"]["result"]["outcome"], "passed");
    let checks = resumed["state"]["result"]["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0]["tool"], "check");
    assert_eq!(checks[1]["tool"], "test");
    let status = success(cli(&home, &["--worker", &worker, "status", "assessment"]));
    assert_eq!(
        status, resumed,
        "read-only status recovers the saved run without executing cargo"
    );
    let completed_again = success(cli(
        &home,
        &[
            "--worker",
            &worker,
            "resume",
            "assessment",
            "--workspace",
            workspace.to_str().unwrap(),
            "--workspace-id",
            "fixture",
        ],
    ));
    assert_eq!(completed_again, resumed);
    failure(
        cli(
            &home,
            &[
                "--worker",
                &worker,
                "run",
                "--workspace",
                workspace.to_str().unwrap(),
                "--workspace-id",
                "fixture",
                "--run-id",
                "assessment",
            ],
        ),
        "run_exists",
    );
    failure(
        cli(
            &home,
            &[
                "--worker",
                &worker,
                "resume",
                "assessment",
                "--workspace",
                workspace.to_str().unwrap(),
                "--workspace-id",
                "another",
            ],
        ),
        "binding_mismatch",
    );
}

#[test]
#[cfg(unix)]
#[ignore = "requires the real LangGraph worker; set AGENT_RUST_TEST_WORKER and run --include-ignored"]
fn real_worker_native_host_recovers_a_lost_child_receipt_without_local_fallback() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname=\"native-fixture\"\nversion=\"0.0.0\"\n",
    )
    .unwrap();
    let native = directory.path().join("native-provider");
    std::fs::write(&native, r#"#!/usr/bin/env python3
import json, os, pathlib, sys
r = json.load(sys.stdin)
if r['method'] == 'capabilities':
    print(json.dumps({'protocol_version':1,'provider_id':'fixture-v1','capabilities':[
        {'name':'policy.decide','provider':'fixture-v1','version':1},
        {'name':'fixture.effect','provider':'fixture-v1','version':1}]}))
    sys.exit(0)
receipt = pathlib.Path(r['context']['workspace']) / 'effect-receipt.txt'
if r['method'] == 'policy.decide':
    if r['params']['sequence'] == 0:
        result = {'kind':'effect','effect':{'capability':'fixture.effect','arguments':{}},'policy_state':{'waiting':True}}
    else:
        assert r['params']['observation']['status'] == 'completed'
        result = {'kind':'complete','result':{'source':'native','receipt':receipt.read_text()},'policy_state':None}
elif r['method'] == 'effect.execute':
    assert not receipt.exists(), 'must not execute twice'
    receipt.write_text(r['params']['effect_id'])
    os._exit(9)
elif r['method'] == 'effect.reconcile':
    result = {'status':'completed','output':{'receipt':receipt.read_text()}} if receipt.exists() else None
else:
    raise AssertionError('unsupported native method')
print(json.dumps({'protocol_version':1,'request_id':r['request_id'],'result':result}))
"#).unwrap();
    std::fs::set_permissions(&native, std::fs::Permissions::from_mode(0o700)).unwrap();
    let worker = worker();
    let start = success(cli(
        &home,
        &[
            "--worker",
            &worker,
            "run",
            "--workspace",
            workspace.to_str().unwrap(),
            "--workspace-id",
            "native-fixture",
            "--run-id",
            "native-run",
            "--host-command",
            native.to_str().unwrap(),
        ],
    ));
    assert_eq!(start["status"], "paused");
    failure(
        cli(
            &home,
            &[
                "--worker",
                &worker,
                "resume",
                "native-run",
                "--workspace",
                workspace.to_str().unwrap(),
                "--workspace-id",
                "native-fixture",
            ],
        ),
        "binding_mismatch",
    );
    let resumed = success(cli(
        &home,
        &[
            "--worker",
            &worker,
            "resume",
            "native-run",
            "--workspace",
            workspace.to_str().unwrap(),
            "--workspace-id",
            "native-fixture",
            "--host-command",
            native.to_str().unwrap(),
        ],
    ));
    assert_eq!(resumed["status"], "completed");
    assert_eq!(resumed["state"]["result"]["source"], "native");
    assert_eq!(resumed["state"]["result"]["receipt"], "native-run:effect:0");
    assert_eq!(
        success(cli(&home, &["--worker", &worker, "status", "native-run"])),
        resumed
    );
}
