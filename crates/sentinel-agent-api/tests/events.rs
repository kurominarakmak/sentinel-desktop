use sentinel_agent_api::{detect_installation, AgentEvent, AgentKind, InstallationStatus};

#[test]
fn events_round_trip_as_tagged_json() {
    let event = AgentEvent::CommandCompleted {
        command: "cargo test".into(),
        exit_code: 0,
    };
    let encoded = serde_json::to_string(&event).unwrap();
    assert!(encoded.contains("command_completed"));
    assert_eq!(serde_json::from_str::<AgentEvent>(&encoded).unwrap(), event);
}

#[test]
fn missing_agent_is_reported_without_panic() {
    let result = detect_installation(AgentKind::Codex);
    assert!(matches!(
        result,
        InstallationStatus::Available { .. }
            | InstallationStatus::NotInstalled
            | InstallationStatus::Unusable { .. }
    ));
}
