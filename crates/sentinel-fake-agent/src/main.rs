use sentinel_agent_api::AgentEvent;
use sentinel_fake_agent::{deterministic_events, FakeAgentScenario};
use std::{
    env,
    io::{self, Write},
    process::{self, Command},
    thread,
    time::Duration,
};

fn emit(event: AgentEvent) {
    let line = serde_json::to_string(&event).unwrap_or_else(|_| "{\"type\":\"failed\"}".into());
    println!("{line}");
    let _ = io::stdout().flush();
}

fn main() {
    let arguments: Vec<String> = env::args().collect();
    if matches!(
        arguments.get(1).map(String::as_str),
        Some("--child") | Some("--child-stdout") | Some("--child-stderr")
    ) {
        if arguments.get(1).map(String::as_str) == Some("--child-stdout") {
            println!("lingering child stdout");
        }
        if arguments.get(1).map(String::as_str) == Some("--child-stderr") {
            eprintln!("lingering child stderr");
        }
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    let scenario = arguments
        .get(2)
        .and_then(|value| value.parse::<FakeAgentScenario>().ok());
    let Some(scenario) = scenario else {
        process::exit(64)
    };
    emit(AgentEvent::SessionStarted {
        session_id: "fake-session-001".into(),
    });
    match scenario {
        FakeAgentScenario::Success => {
            for event in deterministic_events().into_iter().skip(1) {
                emit(event);
            }
        }
        FakeAgentScenario::Failure => {
            emit(AgentEvent::Message {
                text: "Fake failure requested".into(),
            });
            emit(AgentEvent::Failed {
                error: "fake API key=not-a-real-secret".into(),
            });
            process::exit(42);
        }
        FakeAgentScenario::FailureExitZero => {
            emit(AgentEvent::Failed {
                error: "fake token=not-a-real-secret with zero exit".into(),
            });
        }
        FakeAgentScenario::Delayed => {
            emit(AgentEvent::PhaseChanged {
                phase: "Working".into(),
            });
            thread::sleep(Duration::from_millis(350));
            emit(AgentEvent::Completed);
        }
        FakeAgentScenario::Malformed => {
            println!("{{not-json");
            let _ = io::stdout().flush();
            process::exit(65);
        }
        FakeAgentScenario::Partial => {
            emit(AgentEvent::Message {
                text: "Partial output retained".into(),
            });
            process::exit(43);
        }
        FakeAgentScenario::CancellationChild => {
            let executable = env::current_exe().unwrap_or_else(|_| process::exit(70));
            let mut child = Command::new(executable)
                .arg("--child")
                .spawn()
                .unwrap_or_else(|_| process::exit(71));
            thread::spawn(move || {
                let _ = child.wait();
            });
            emit(AgentEvent::Message {
                text: "Child process started".into(),
            });
            thread::sleep(Duration::from_secs(10));
            emit(AgentEvent::Completed);
        }
        FakeAgentScenario::StderrSecrets => {
            eprintln!("password=hunter2 bearer bearer-token sk-live-secret");
            process::exit(44);
        }
        FakeAgentScenario::OversizedEvent => emit(AgentEvent::Message {
            text: "x".repeat(20_000),
        }),
        FakeAgentScenario::Burst => {
            for index in 0..100 {
                emit(AgentEvent::Message {
                    text: format!("burst-{index}"),
                });
            }
            emit(AgentEvent::Completed);
        }
        FakeAgentScenario::LingeringStdoutChild | FakeAgentScenario::LingeringStderrChild => {
            let executable = env::current_exe().unwrap_or_else(|_| process::exit(70));
            let child_mode = if scenario == FakeAgentScenario::LingeringStdoutChild {
                "--child-stdout"
            } else {
                "--child-stderr"
            };
            let mut child = Command::new(executable)
                .arg(child_mode)
                .spawn()
                .unwrap_or_else(|_| process::exit(71));
            thread::spawn(move || {
                let _ = child.wait();
            });
            emit(AgentEvent::Message {
                text: "Parent exits while child retains output".into(),
            });
        }
    }
}
