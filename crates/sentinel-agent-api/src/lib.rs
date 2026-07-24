use serde::{Deserialize, Serialize};
use std::{path::PathBuf, process::Command};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    SessionStarted { session_id: String },
    PhaseChanged { phase: String },
    Message { text: String },
    CommandStarted { command: String },
    CommandCompleted { command: String, exit_code: i32 },
    FileChanged { path: String },
    WaitingForInput,
    Completed,
    Failed { error: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Fake,
    Codex,
    ClaudeCode,
}

impl AgentKind {
    pub fn executable(&self) -> &'static str {
        match self {
            Self::Fake => "",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InstallationStatus {
    Available {
        executable: PathBuf,
        version: String,
    },
    NotInstalled,
    Unusable {
        detail: String,
    },
}

pub fn detect_installation(agent: AgentKind) -> InstallationStatus {
    if agent == AgentKind::Fake {
        return InstallationStatus::Available {
            executable: PathBuf::from("built-in"),
            version: "spike".into(),
        };
    }
    let executable = agent.executable();
    match Command::new(executable).arg("--version").output() {
        Ok(output) if output.status.success() => InstallationStatus::Available {
            executable: PathBuf::from(executable),
            version: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        },
        Ok(output) => InstallationStatus::Unusable {
            detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            InstallationStatus::NotInstalled
        }
        Err(error) => InstallationStatus::Unusable {
            detail: error.to_string(),
        },
    }
}

pub fn parse_json_line(line: &str) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(line)
}
