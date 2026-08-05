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

/// App Server is intentionally advertised as capability metadata only in
/// Phase 4. No App Server process is launched or trusted by this phase.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexCapabilities {
    pub exec_json: bool,
    pub app_server_experimental: bool,
}

pub fn codex_capabilities(installation: &InstallationStatus) -> CodexCapabilities {
    CodexCapabilities {
        exec_json: matches!(installation, InstallationStatus::Available { .. }),
        app_server_experimental: false,
    }
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

/// Maximum provider JSON-line transport size. The caller must apply this
/// before UTF-8 decoding so a provider cannot cause unbounded allocation.
pub const MAX_CODEX_JSON_LINE_BYTES: usize = 16 * 1024;
pub const MAX_CLAUDE_JSON_LINE_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodexProtocolError {
    Oversized,
    InvalidUtf8,
    Malformed,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeProtocolError {
    Oversized,
    InvalidUtf8,
    Malformed,
    Unsupported,
}

/// Converts the small documented `claude --output-format stream-json` subset
/// needed by the normalized contract. Provider content is never forwarded
/// unless its exact event shape and bounded string field are allowlisted.
pub fn parse_claude_stream_json_line(bytes: &[u8]) -> Result<AgentEvent, ClaudeProtocolError> {
    if bytes.is_empty() || bytes.len() > MAX_CLAUDE_JSON_LINE_BYTES {
        return Err(ClaudeProtocolError::Oversized);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ClaudeProtocolError::InvalidUtf8)?;
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| ClaudeProtocolError::Malformed)?;
    let kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or(ClaudeProtocolError::Malformed)?;
    let bounded = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty() && v.len() <= 4096)
            .map(str::to_owned)
            .ok_or(ClaudeProtocolError::Malformed)
    };
    match kind {
        "system" if value.get("subtype").and_then(serde_json::Value::as_str) == Some("init") => {
            Ok(AgentEvent::SessionStarted {
                session_id: bounded("session_id")?,
            })
        }
        "assistant" => Ok(AgentEvent::Message {
            text: bounded("text")?,
        }),
        "result" if value.get("subtype").and_then(serde_json::Value::as_str) == Some("success") => {
            Ok(AgentEvent::Completed)
        }
        "result" => Ok(AgentEvent::Failed {
            error: "Claude reported a failed turn.".into(),
        }),
        "permission_request" => Ok(AgentEvent::WaitingForInput),
        _ => Err(ClaudeProtocolError::Unsupported),
    }
}

/// Fixed noninteractive stream-json command construction. The caller cannot
/// inject flags, session IDs, or executable paths.
pub fn claude_stream_argv(prompt: &str) -> Result<Vec<String>, ClaudeProtocolError> {
    if prompt.trim().is_empty() || prompt.len() > 8_000 || prompt.contains('\0') {
        return Err(ClaudeProtocolError::Malformed);
    }
    Ok(vec![
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--print".into(),
        prompt.into(),
    ])
}

pub fn claude_resume_argv(session: &str, prompt: &str) -> Result<Vec<String>, ClaudeProtocolError> {
    if session.is_empty()
        || session.len() > 128
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(ClaudeProtocolError::Malformed);
    }
    let mut argv = vec!["--resume".into(), session.into()];
    argv.extend(claude_stream_argv(prompt)?);
    Ok(argv)
}

/// Converts the conservative documented subset of `codex exec --json` event
/// shapes into the adapter contract. Provider fields not explicitly allowed
/// here are never forwarded to the product surface.
pub fn parse_codex_exec_json_line(bytes: &[u8]) -> Result<AgentEvent, CodexProtocolError> {
    if bytes.is_empty() || bytes.len() > MAX_CODEX_JSON_LINE_BYTES {
        return Err(CodexProtocolError::Oversized);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| CodexProtocolError::InvalidUtf8)?;
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| CodexProtocolError::Malformed)?;
    let kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or(CodexProtocolError::Malformed)?;
    let bounded = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty() && text.len() <= 4 * 1024)
            .map(str::to_owned)
            .ok_or(CodexProtocolError::Malformed)
    };
    match kind {
        "thread.started" => Ok(AgentEvent::SessionStarted {
            session_id: bounded("thread_id")?,
        }),
        "item.completed" => Ok(AgentEvent::Message {
            text: bounded("text")?,
        }),
        "turn.completed" => Ok(AgentEvent::Completed),
        "turn.failed" => Ok(AgentEvent::Failed {
            error: "Codex reported a failed turn.".into(),
        }),
        _ => Err(CodexProtocolError::Unsupported),
    }
}

/// Fixed argv construction for one non-interactive, structured Codex turn.
/// It accepts no caller-supplied flags and never invokes a shell.
pub fn codex_exec_argv(task: &str) -> Result<Vec<String>, CodexProtocolError> {
    if task.trim().is_empty() || task.len() > 16 * 1024 || task.contains('\0') {
        return Err(CodexProtocolError::Malformed);
    }
    Ok(vec![
        "exec".into(),
        "--json".into(),
        "--".into(),
        task.into(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_exec_parser_is_bounded_and_allowlisted() {
        assert_eq!(
            parse_codex_exec_json_line(br#"{"type":"thread.started","thread_id":"t-1"}"#),
            Ok(AgentEvent::SessionStarted {
                session_id: "t-1".into()
            })
        );
        assert_eq!(
            parse_codex_exec_json_line(br#"{"type":"turn.completed"}"#),
            Ok(AgentEvent::Completed)
        );
        assert_eq!(
            parse_codex_exec_json_line(br#"{"type":"command.executed","command":"secret"}"#),
            Err(CodexProtocolError::Unsupported)
        );
        assert_eq!(
            codex_exec_argv("fix it").unwrap(),
            vec!["exec", "--json", "--", "fix it"]
        );
    }
    #[test]
    fn claude_stream_parser_and_fixed_resume_are_bounded() {
        assert_eq!(
            parse_claude_stream_json_line(
                br#"{"type":"system","subtype":"init","session_id":"s_1"}"#
            ),
            Ok(AgentEvent::SessionStarted {
                session_id: "s_1".into()
            })
        );
        assert_eq!(
            parse_claude_stream_json_line(br#"{"type":"permission_request"}"#),
            Ok(AgentEvent::WaitingForInput)
        );
        assert!(
            parse_claude_stream_json_line(br#"{"type":"tool_use","command":"secret"}"#).is_err()
        );
        assert_eq!(
            claude_resume_argv("s_1", "continue").unwrap(),
            vec![
                "--resume",
                "s_1",
                "--output-format",
                "stream-json",
                "--verbose",
                "--print",
                "continue"
            ]
        );
    }
}
