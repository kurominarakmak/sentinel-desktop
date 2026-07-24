use sentinel_agent_api::AgentEvent;

pub fn deterministic_events() -> Vec<AgentEvent> {
    vec![
        AgentEvent::SessionStarted {
            session_id: "fake-session-001".into(),
        },
        AgentEvent::PhaseChanged {
            phase: "Planning".into(),
        },
        AgentEvent::Message {
            text: "Reading repository".into(),
        },
        AgentEvent::PhaseChanged {
            phase: "Editing".into(),
        },
        AgentEvent::FileChanged {
            path: "src/example.rs".into(),
        },
        AgentEvent::CommandStarted {
            command: "fake-test".into(),
        },
        AgentEvent::CommandCompleted {
            command: "fake-test".into(),
            exit_code: 0,
        },
        AgentEvent::Completed,
    ]
}

pub fn emit<F: FnMut(AgentEvent)>(mut sink: F) {
    for event in deterministic_events() {
        sink(event);
    }
}
