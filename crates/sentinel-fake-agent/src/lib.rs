use sentinel_agent_api::AgentEvent;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeAgentScenario {
    Success,
    Failure,
    FailureExitZero,
    Delayed,
    Malformed,
    Partial,
    CancellationChild,
    StderrSecrets,
    OversizedEvent,
    Burst,
    LingeringStdoutChild,
    LingeringStderrChild,
}

impl FakeAgentScenario {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::FailureExitZero => "failure-exit-zero",
            Self::Delayed => "delayed",
            Self::Malformed => "malformed",
            Self::Partial => "partial",
            Self::CancellationChild => "cancellation-child",
            Self::StderrSecrets => "stderr-secrets",
            Self::OversizedEvent => "oversized-event",
            Self::Burst => "burst",
            Self::LingeringStdoutChild => "lingering-stdout-child",
            Self::LingeringStderrChild => "lingering-stderr-child",
        }
    }
}

impl FromStr for FakeAgentScenario {
    type Err = ();
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "success" => Ok(Self::Success),
            "failure" => Ok(Self::Failure),
            "failure-exit-zero" => Ok(Self::FailureExitZero),
            "delayed" => Ok(Self::Delayed),
            "malformed" => Ok(Self::Malformed),
            "partial" => Ok(Self::Partial),
            "cancellation-child" => Ok(Self::CancellationChild),
            "stderr-secrets" => Ok(Self::StderrSecrets),
            "oversized-event" => Ok(Self::OversizedEvent),
            "burst" => Ok(Self::Burst),
            "lingering-stdout-child" => Ok(Self::LingeringStdoutChild),
            "lingering-stderr-child" => Ok(Self::LingeringStderrChild),
            _ => Err(()),
        }
    }
}

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
