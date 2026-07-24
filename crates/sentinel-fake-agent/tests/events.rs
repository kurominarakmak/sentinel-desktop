use sentinel_agent_api::AgentEvent;

#[test]
fn fake_agent_has_deterministic_terminal_order() {
    let events = sentinel_fake_agent::deterministic_events();
    assert!(matches!(
        events.first(),
        Some(AgentEvent::SessionStarted { .. })
    ));
    assert!(matches!(events.last(), Some(AgentEvent::Completed)));
    let command = events
        .iter()
        .position(|event| matches!(event, AgentEvent::CommandStarted { .. }))
        .unwrap();
    let complete = events
        .iter()
        .position(|event| matches!(event, AgentEvent::CommandCompleted { .. }))
        .unwrap();
    assert!(command < complete);
}
