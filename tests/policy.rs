use agent_rust::{AssessmentTask, RustAssessmentPolicy, ToolId, normalize_remote};
use serde_json::json;
use theorem_agent_core::{Binding, Decision, DecisionRequest, Observation};
use theorem_agent_runtime::DecisionProvider;

fn request() -> DecisionRequest {
    DecisionRequest {
        decision_id: "run:decision:0".into(),
        sequence: 0,
        task: serde_json::to_value(AssessmentTask::new(vec![ToolId::Check, ToolId::Test])).unwrap(),
        policy_state: serde_json::Value::Null,
        observation: None,
        agent_id: "agent".into(),
        binding: Binding {
            policy: "policy-v1".into(),
            execution: "tools-v1".into(),
            workspace: "fixture".into(),
        },
    }
}

#[test]
fn policy_requires_a_persisted_observation_before_advancing_the_plan() {
    let mut policy = RustAssessmentPolicy;
    let mut request = request();
    let first = policy.decide(&request).unwrap();
    let Decision::Effect {
        effect,
        policy_state,
    } = first
    else {
        panic!("expected check")
    };
    assert_eq!(effect.capability, "rust.tool");
    assert_eq!(effect.arguments, json!({"tool":"check"}));
    request.sequence = 1;
    request.decision_id = "run:decision:1".into();
    request.policy_state = policy_state;
    assert_eq!(
        policy.decide(&request).unwrap_err().code,
        "invalid_policy_state"
    );
    request.observation = Some(Observation::completed(json!({"exit_code":0})));
    let Decision::Effect {
        effect,
        policy_state,
    } = policy.decide(&request).unwrap()
    else {
        panic!("expected test")
    };
    assert_eq!(effect.arguments, json!({"tool":"test"}));
    request.sequence = 2;
    request.decision_id = "run:decision:2".into();
    request.policy_state = policy_state;
    request.observation = Some(Observation::failed(json!({"exit_code":1})));
    let Decision::Complete { result, .. } = policy.decide(&request).unwrap() else {
        panic!("expected report")
    };
    assert_eq!(result["outcome"], "findings");
    assert_eq!(result["checks"].as_array().unwrap().len(), 2);
}

#[test]
fn unknown_outcomes_are_never_consumed_as_terminal_observations() {
    let mut policy = RustAssessmentPolicy;
    let mut request = request();
    let Decision::Effect { policy_state, .. } = policy.decide(&request).unwrap() else {
        unreachable!()
    };
    request.sequence = 1;
    request.policy_state = policy_state;
    request.observation = Some(Observation::unknown(json!({"reason":"lost receipt"})));
    assert_eq!(
        policy.decide(&request).unwrap_err().code,
        "invalid_policy_state"
    );
}

#[test]
fn remote_identity_ignores_transport_credentials_but_preserves_repository() {
    let expected = "github.com/owner/repository";
    assert_eq!(
        normalize_remote("git@github.com:owner/repository.git").unwrap(),
        expected
    );
    assert_eq!(
        normalize_remote("https://user:private-token@github.com/owner/repository.git").unwrap(),
        expected
    );
    assert_eq!(
        normalize_remote("ssh://git@github.com/owner/repository").unwrap(),
        expected
    );
    assert!(normalize_remote("/absolute/local/clone").is_err());
    assert!(normalize_remote("file:///absolute/local/clone").is_err());
    assert_eq!(
        normalize_remote("git@github.com:owner/repository.git.git").unwrap(),
        "github.com/owner/repository.git"
    );
}
