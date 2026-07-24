use anyhow::{bail, Result};
use sentinel_agent_api::{detect_installation, parse_json_line, AgentKind, InstallationStatus};
use sentinel_process::{ProcessEvent, SupervisedProcess};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command as StdCommand,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CONFIRMATION_FLAG: &str = "--confirm-real-agent";
const CLEANUP_FLAG: &str = "--cleanup";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Codex,
    Claude,
}

impl Agent {
    fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
    fn kind(self) -> AgentKind {
        match self {
            Self::Codex => AgentKind::Codex,
            Self::Claude => AgentKind::ClaudeCode,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeTest {
    Normal,
    Cancellation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeCommand {
    Environment,
    Agent {
        agent: Agent,
        test: ProbeTest,
        confirmed: bool,
        cleanup: bool,
    },
    All {
        confirmed: bool,
        cleanup: bool,
    },
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    ValidationFailed = 1,
    AgentUnavailable = 2,
    ConfirmationMissing = 3,
    UnsafeEnvironment = 4,
}

#[derive(Default, Debug, Serialize)]
pub struct EventSummary {
    pub event_types: BTreeSet<String>,
    pub session_id: Option<String>,
    pub raw_event_count: usize,
    pub tool_or_command_events: usize,
}

#[derive(Debug, Serialize)]
struct ProbeSummary<'a> {
    run_id: &'a str,
    agent: &'a str,
    test: &'a str,
    fixture_path: String,
    artifact_root: String,
    stdout_log: String,
    stderr_log: String,
    summary_path: String,
    event_summary: &'a EventSummary,
    validation_failures: &'a [String],
    cancellation_latency_ms: Option<u128>,
    cancellation_validation: Option<&'a CancellationValidation>,
}

#[derive(Debug, Serialize)]
struct CancellationValidation {
    cancellation_requested: bool,
    agent_process_terminated: bool,
    process_group_terminated: bool,
    completion_file_absent: bool,
}

struct SummaryDetails<'a> {
    failures: &'a [String],
    cancellation_latency_ms: Option<u128>,
    cancellation_validation: Option<&'a CancellationValidation>,
}

pub fn parse_args(args: &[String]) -> Result<ProbeCommand> {
    let command = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("expected environment, codex, claude, or all"))?;
    match command {
        "environment" if args.len() == 1 => Ok(ProbeCommand::Environment),
        "codex" | "claude" => {
            let agent = if command == "codex" {
                Agent::Codex
            } else {
                Agent::Claude
            };
            let (test, confirmed, cleanup) = parse_agent_flags(&args[1..])?;
            Ok(ProbeCommand::Agent {
                agent,
                test,
                confirmed,
                cleanup,
            })
        }
        "all" => {
            let (confirmed, cleanup) = parse_common_flags(&args[1..])?;
            Ok(ProbeCommand::All { confirmed, cleanup })
        }
        _ => bail!("unsupported command: {command}"),
    }
}

fn parse_agent_flags(args: &[String]) -> Result<(ProbeTest, bool, bool)> {
    let mut test = ProbeTest::Normal;
    let mut confirmed = false;
    let mut cleanup = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            CONFIRMATION_FLAG => confirmed = true,
            CLEANUP_FLAG => cleanup = true,
            "--test" => {
                index += 1;
                test = match args.get(index).map(String::as_str) {
                    Some("cancellation") => ProbeTest::Cancellation,
                    Some(value) => bail!("unsupported test: {value}"),
                    None => bail!("--test requires a test name"),
                };
            }
            value => bail!("unsupported argument: {value}"),
        }
        index += 1;
    }
    Ok((test, confirmed, cleanup))
}

fn parse_common_flags(args: &[String]) -> Result<(bool, bool)> {
    let mut confirmed = false;
    let mut cleanup = false;
    for argument in args {
        match argument.as_str() {
            CONFIRMATION_FLAG => confirmed = true,
            CLEANUP_FLAG => cleanup = true,
            value => bail!("unsupported argument: {value}"),
        }
    }
    Ok((confirmed, cleanup))
}

pub async fn run(command: ProbeCommand) -> ExitCode {
    match command {
        ProbeCommand::Environment => {
            print_environment();
            ExitCode::Success
        }
        ProbeCommand::Agent {
            agent,
            test,
            confirmed,
            cleanup,
        } => run_agent_checked(agent, test, confirmed, cleanup).await,
        ProbeCommand::All { confirmed, cleanup } => {
            let results = [
                run_agent_checked(Agent::Codex, ProbeTest::Normal, confirmed, cleanup).await,
                run_agent_checked(Agent::Codex, ProbeTest::Cancellation, confirmed, cleanup).await,
                run_agent_checked(Agent::Claude, ProbeTest::Normal, confirmed, cleanup).await,
                run_agent_checked(Agent::Claude, ProbeTest::Cancellation, confirmed, cleanup).await,
            ];
            if results.iter().all(|result| *result == ExitCode::Success) {
                ExitCode::Success
            } else if results.contains(&ExitCode::ConfirmationMissing) {
                ExitCode::ConfirmationMissing
            } else if results.contains(&ExitCode::AgentUnavailable) {
                ExitCode::AgentUnavailable
            } else if results.contains(&ExitCode::UnsafeEnvironment) {
                ExitCode::UnsafeEnvironment
            } else {
                ExitCode::ValidationFailed
            }
        }
    }
}

pub fn exit_code_for(
    confirmation_missing: bool,
    agent_available: bool,
    unsafe_setup: bool,
    validation_passed: bool,
) -> ExitCode {
    if confirmation_missing {
        ExitCode::ConfirmationMissing
    } else if !agent_available {
        ExitCode::AgentUnavailable
    } else if unsafe_setup {
        ExitCode::UnsafeEnvironment
    } else if validation_passed {
        ExitCode::Success
    } else {
        ExitCode::ValidationFailed
    }
}

async fn run_agent_checked(
    agent: Agent,
    test: ProbeTest,
    confirmed: bool,
    cleanup: bool,
) -> ExitCode {
    if !confirmed {
        eprintln!(
            "sentinel-probe: {} makes a real agent request; rerun with {CONFIRMATION_FLAG}",
            agent.name()
        );
        return exit_code_for(true, true, false, false);
    }
    if !matches!(
        detect_installation(agent.kind()),
        InstallationStatus::Available { .. }
    ) || !appears_authenticated(agent)
    {
        eprintln!("sentinel-probe: {} is unavailable or unauthenticated; run `sentinel-probe environment`", agent.name());
        return exit_code_for(false, false, false, false);
    }
    match run_agent_probe(agent, test, cleanup).await {
        Ok(()) => exit_code_for(false, true, false, true),
        Err(ProbeError::UnsafeSetup(error)) => {
            eprintln!(
                "sentinel-probe: {} probe could not create a safe fixture: {error:#}",
                agent.name()
            );
            exit_code_for(false, true, true, false)
        }
        Err(ProbeError::Validation(error)) => {
            eprintln!("sentinel-probe: {} probe failed: {error:#}", agent.name());
            exit_code_for(false, true, false, false)
        }
    }
}

fn appears_authenticated(agent: Agent) -> bool {
    let arguments = match agent {
        Agent::Codex => ["login", "status"].as_slice(),
        Agent::Claude => ["auth", "status"].as_slice(),
    };
    StdCommand::new(agent.name())
        .args(arguments)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn print_environment() {
    println!("operating_system: {}", std::env::consts::OS);
    print_tool("git", &["--version"], None);
    print_tool("codex", &["--version"], Some(&["login", "status"]));
    print_tool("claude", &["--version"], Some(&["auth", "status"]));
    print_tool("jq", &["--version"], None);
}

fn print_run_paths(fixture: &Path, artifacts: &ArtifactPaths) {
    println!("fixture path: {}", fixture.display());
    println!("artifact directory: {}", artifacts.root.display());
    println!("stdout log path: {}", artifacts.stdout.display());
    println!("stderr log path: {}", artifacts.stderr.display());
    println!("summary path: {}", artifacts.summary.display());
}

fn print_tool(name: &str, version_args: &[&str], auth_args: Option<&[&str]>) {
    let path = executable_path(name)
        .map(|value| value.display().to_string())
        .unwrap_or_else(|| "not_installed".into());
    let version = command_text(name, version_args).unwrap_or_else(|_| "unavailable".into());
    println!("{name}: path={path}; version={}", redact(&version));
    if let Some(args) = auth_args {
        let status = command_text(name, args)
            .map(|_| "appears_available")
            .unwrap_or("unknown_or_unavailable");
        println!("{name}_authentication: {status}");
    }
}

async fn run_agent_probe(
    agent: Agent,
    test: ProbeTest,
    cleanup: bool,
) -> std::result::Result<(), ProbeError> {
    let artifacts = ArtifactPaths::create(agent, test).map_err(ProbeError::UnsafeSetup)?;
    let fixture = create_fixture(test).map_err(ProbeError::UnsafeSetup)?;
    print_run_paths(&fixture, &artifacts);
    fs::write(
        artifacts.root.join("fixture-path.txt"),
        fixture.display().to_string(),
    )
    .map_err(|error| ProbeError::UnsafeSetup(error.into()))?;
    fs::write(
        artifacts.root.join("detected-versions.txt"),
        format!(
            "{}\n",
            command_text(agent.name(), &["--version"]).unwrap_or_default()
        ),
    )
    .map_err(|error| ProbeError::UnsafeSetup(error.into()))?;
    let result = match test {
        ProbeTest::Normal => run_normal_probe(agent, &fixture, &artifacts).await,
        ProbeTest::Cancellation => run_cancellation_probe(agent, &fixture, &artifacts).await,
    };
    let success = result.is_ok();
    if success || cleanup {
        if let Err(error) = fs::remove_dir_all(&fixture) {
            return Err(ProbeError::Validation(anyhow::anyhow!(
                "could not remove fixture {}: {error}",
                fixture.display()
            )));
        }
    } else {
        eprintln!(
            "sentinel-probe: preserving failed fixture at {}",
            fixture.display()
        );
    }
    if success {
        println!(
            "sentinel-probe: {} {} PASS (run_id={})",
            agent.name(),
            if test == ProbeTest::Normal {
                "normal/resume"
            } else {
                "cancellation"
            },
            artifacts.run_id
        );
    }
    result.map_err(ProbeError::Validation)
}

async fn run_normal_probe(agent: Agent, fixture: &Path, artifacts: &ArtifactPaths) -> Result<()> {
    let first = execute_agent(agent, fixture, first_prompt(agent), artifacts).await?;
    let mut failures = validate_first(agent, fixture, &first.summary);
    let session = first
        .summary
        .session_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no session identifier captured"))?;
    if failures.is_empty() {
        let second =
            execute_resume(agent, fixture, &session, resume_prompt(agent), artifacts).await?;
        failures.extend(validate_resume(agent, fixture, &second.summary));
        if let Some(resumed_session) = second.summary.session_id {
            if resumed_session != session {
                failures.push("resume reported a different session identifier".into());
            }
        }
    }
    write_summary(
        agent,
        ProbeTest::Normal,
        fixture,
        artifacts,
        &first.summary,
        SummaryDetails {
            failures: &failures,
            cancellation_latency_ms: None,
            cancellation_validation: None,
        },
    )?;
    println!(
        "normalized event summary: {} event type(s), {} raw JSON event(s), session={}",
        first.summary.event_types.len(),
        first.summary.raw_event_count,
        first
            .summary
            .session_id
            .as_deref()
            .unwrap_or("not_observed")
    );
    if failures.is_empty() {
        Ok(())
    } else {
        bail!(failures.join("; "))
    }
}

async fn run_cancellation_probe(
    agent: Agent,
    fixture: &Path,
    artifacts: &ArtifactPaths,
) -> Result<()> {
    let args = initial_agent_args(agent, fixture, cancellation_prompt(agent), true);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (mut process, mut output) =
        SupervisedProcess::start_in(agent.name(), &refs, Some(fixture)).await?;
    let started = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut active_observed = false;
    let mut summary = EventSummary::default();
    while Instant::now() < deadline {
        if let Some(exit) = process.try_wait()? {
            bail!("agent exited before cancellation could be tested: {exit:?}");
        }
        if let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_millis(250), output.recv()).await
        {
            record_process_event(&event, artifacts, &mut summary)?;
            if event_text(&event).contains("wait.sh") {
                active_observed = true;
                break;
            }
        }
    }
    if !active_observed {
        let _ = process.cancel(Duration::from_secs(2)).await;
        bail!("agent did not expose the required active wait.sh state before cancellation");
    }
    let exit = process.cancel(Duration::from_secs(3)).await?;
    let latency = started.elapsed().as_millis();
    let mut failures = Vec::new();
    #[cfg(unix)]
    let process_group_terminated = !process.process_group_is_alive();
    #[cfg(not(unix))]
    let process_group_terminated = true;
    if !process_group_terminated {
        failures.push("probe process group remained alive after cancellation".into());
    }
    let completion_file_absent = !fixture.join("completion.txt").exists();
    if !completion_file_absent {
        failures.push("completion.txt was created before cancellation".into());
    }
    let cancellation_validation = CancellationValidation {
        cancellation_requested: true,
        agent_process_terminated: exit.is_some(),
        process_group_terminated,
        completion_file_absent,
    };
    if !cancellation_validation.agent_process_terminated {
        failures.push("agent process did not report termination after cancellation".into());
    }
    fs::write(
        artifacts.root.join("cancellation-result.txt"),
        format!(
            "exit_code={exit:?}\nlatency_ms={latency}\ncancellation_requested={}\nagent_process_terminated={}\nprocess_group_terminated={}\ncompletion_file_absent={}\n",
            cancellation_validation.cancellation_requested,
            cancellation_validation.agent_process_terminated,
            cancellation_validation.process_group_terminated,
            cancellation_validation.completion_file_absent,
        ),
    )?;
    write_summary(
        agent,
        ProbeTest::Cancellation,
        fixture,
        artifacts,
        &summary,
        SummaryDetails {
            failures: &failures,
            cancellation_latency_ms: Some(latency),
            cancellation_validation: Some(&cancellation_validation),
        },
    )?;
    if failures.is_empty() {
        Ok(())
    } else {
        bail!(failures.join("; "))
    }
}

async fn execute_agent(
    agent: Agent,
    fixture: &Path,
    prompt: String,
    artifacts: &ArtifactPaths,
) -> Result<Capture> {
    execute(
        agent,
        fixture,
        initial_agent_args(agent, fixture, prompt, false),
        artifacts,
    )
    .await
}

async fn execute_resume(
    agent: Agent,
    fixture: &Path,
    session: &str,
    prompt: String,
    artifacts: &ArtifactPaths,
) -> Result<Capture> {
    execute(
        agent,
        fixture,
        resume_agent_args(agent, session, prompt),
        artifacts,
    )
    .await
}

async fn execute(
    agent: Agent,
    fixture: &Path,
    args: Vec<String>,
    artifacts: &ArtifactPaths,
) -> Result<Capture> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let (mut process, mut output) =
        SupervisedProcess::start_in(agent.name(), &refs, Some(fixture)).await?;
    let mut summary = EventSummary::default();
    while let Some(event) = output.recv().await {
        record_process_event(&event, artifacts, &mut summary)?;
    }
    let exit = process.wait().await?;
    if exit != Some(0) {
        bail!("agent process exited with {exit:?}");
    }
    Ok(Capture { summary })
}

fn initial_agent_args(
    agent: Agent,
    fixture: &Path,
    prompt: String,
    cancellation: bool,
) -> Vec<String> {
    let directory = fixture.display().to_string();
    match agent {
        Agent::Codex => vec![
            "exec",
            "--json",
            "--sandbox",
            "workspace-write",
            "-C",
            &directory,
            &prompt,
        ],
        Agent::Claude => vec![
            "--print",
            "--output-format",
            "stream-json",
            "--permission-mode",
            "acceptEdits",
            "--allowedTools",
            if cancellation {
                "Read,Write,Bash(sh wait.sh)"
            } else {
                "Read,Write"
            },
            &prompt,
        ],
    }
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn resume_agent_args(agent: Agent, session: &str, prompt: String) -> Vec<String> {
    match agent {
        Agent::Codex => vec!["exec", "resume", "--json", session, &prompt],
        Agent::Claude => vec![
            "--resume",
            session,
            "--print",
            "--output-format",
            "stream-json",
            "--permission-mode",
            "acceptEdits",
            "--allowedTools",
            "Read,Write",
            &prompt,
        ],
    }
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn first_prompt(agent: Agent) -> String {
    match agent {
        Agent::Codex => "Read README.md. In this disposable fixture repository only, create result.txt by executing exactly: `printf '%s\\n' 'Agent Sentinel Codex probe passed' > result.txt`. The logical content must be exactly `Agent Sentinel Codex probe passed`: no period, no quotes, no markdown, no leading or trailing spaces, no blank lines, and no additional text. A single final newline is allowed. Do not change any other file. Do not use network, dependencies, credentials, git push, merge, or files outside this repository.".into(),
        Agent::Claude => "Read README.md. Create result.txt containing exactly one line: Agent Sentinel Claude probe passed. Do not change any other file. Do not use network, dependencies, credentials, git push, merge, or files outside this repository.".into(),
    }
}
fn resume_prompt(agent: Agent) -> String {
    match agent {
        Agent::Codex => "In this disposable fixture repository only, append the second logical line to result.txt by executing exactly: `printf '%s\\n' 'Codex resume passed' >> result.txt`. The final logical content must be exactly two lines: `Agent Sentinel Codex probe passed` followed by `Codex resume passed`. Do not add a period, quotes, markdown, leading or trailing spaces, blank lines, or any additional text. A single final newline is allowed. Do not change any other file.".into(),
        Agent::Claude => "Append exactly this second line to result.txt: Claude resume passed. Do not change any other file.".into(),
    }
}
fn cancellation_prompt(agent: Agent) -> String {
    format!("Run `sh wait.sh` before doing anything else. It is intentionally slow. After it exits, create completion.txt with {} cancellation probe passed. Do not change any other file.", agent.name())
}

fn validate_first(agent: Agent, fixture: &Path, summary: &EventSummary) -> Vec<String> {
    let mut failures = Vec::new();
    if agent == Agent::Codex && !summary.event_types.contains("thread.started") {
        failures.push("thread.started was not observed".into());
    }
    if agent == Agent::Codex && !summary.event_types.contains("turn.completed") {
        failures.push("turn.completed was not observed".into());
    }
    if agent == Agent::Claude
        && !summary
            .event_types
            .iter()
            .any(|kind| kind.contains("assistant"))
    {
        failures.push("Claude assistant event was not observed".into());
    }
    if agent == Agent::Claude
        && !summary
            .event_types
            .iter()
            .any(|kind| kind.contains("result"))
    {
        failures.push("Claude final result event was not observed".into());
    }
    if summary.session_id.is_none() {
        failures.push("session identifier was not observed".into());
    }
    if agent == Agent::Claude && summary.raw_event_count == 0 {
        failures.push("Claude initial structured event was not observed".into());
    }
    if agent == Agent::Claude && summary.tool_or_command_events == 0 {
        failures.push("Claude tool or command event was not observed".into());
    }
    let expected = format!(
        "Agent Sentinel {} probe passed",
        if agent == Agent::Codex {
            "Codex"
        } else {
            "Claude"
        }
    );
    failures.extend(validate_fixture(fixture, &expected, &["result.txt"]));
    failures
}

fn validate_resume(agent: Agent, fixture: &Path, summary: &EventSummary) -> Vec<String> {
    let expected = format!(
        "Agent Sentinel {} probe passed\n{} resume passed",
        if agent == Agent::Codex {
            "Codex"
        } else {
            "Claude"
        },
        if agent == Agent::Codex {
            "Codex"
        } else {
            "Claude"
        }
    );
    let mut failures = validate_fixture(fixture, &expected, &["result.txt"]);
    if agent == Agent::Codex && !summary.event_types.contains("turn.completed") {
        failures.push("resume turn.completed was not observed".into());
    }
    failures
}

pub fn validate_fixture(
    fixture: &Path,
    expected_result: &str,
    allowed_changes: &[&str],
) -> Vec<String> {
    let mut failures = Vec::new();
    match fs::read_to_string(fixture.join("result.txt")) {
        Ok(actual) if matches_logical_content(&actual, expected_result) => {}
        Ok(_) => failures.push("result.txt content did not match exactly".into()),
        Err(_) => failures.push("result.txt was not created".into()),
    }
    match git_output(fixture, &["status", "--porcelain"]) {
        Ok(status) => {
            for line in status.lines() {
                if let Some(path) = line.get(3..) {
                    if !allowed_changes.contains(&path) {
                        failures.push(format!("unexpected modified file: {path}"));
                    }
                }
            }
        }
        Err(error) => failures.push(format!("could not inspect fixture changes: {error}")),
    }
    failures
}

fn matches_logical_content(actual: &str, expected: &str) -> bool {
    actual == expected || actual.strip_suffix('\n') == Some(expected)
}

fn create_fixture(test: ProbeTest) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("agent-sentinel-probe-{}", unique_suffix()));
    fs::create_dir_all(&path)?;
    fs::write(
        path.join("README.md"),
        "Agent Sentinel disposable probe fixture\n",
    )?;
    if test == ProbeTest::Cancellation {
        fs::write(path.join("wait.sh"), "#!/bin/sh\nsleep 20\n")?;
    }
    git_output(&path, &["init"])?;
    git_output(&path, &["add", "."])?;
    git_output(
        &path,
        &[
            "-c",
            "user.email=probe@example.test",
            "-c",
            "user.name=Probe",
            "commit",
            "-m",
            "fixture",
        ],
    )?;
    Ok(path)
}

fn record_process_event(
    event: &ProcessEvent,
    artifacts: &ArtifactPaths,
    summary: &mut EventSummary,
) -> Result<()> {
    match event {
        ProcessEvent::Stdout(line) => {
            append(&artifacts.stdout, &redact(line))?;
            append(&artifacts.raw, &redact(line))?;
            if let Ok(value) = parse_json_line(line) {
                absorb_event(&value, summary);
                append(
                    &artifacts.normalized,
                    &serde_json::to_string(
                        &json!({"type": event_type(&value), "session_id": find_string(&value, &["thread_id", "session_id"])}),
                    )?,
                )?;
            }
        }
        ProcessEvent::Stderr(line) => append(&artifacts.stderr, &redact(line))?,
        ProcessEvent::OutputError => append(&artifacts.stderr, "process output stream error")?,
        ProcessEvent::Exited(_) => {}
    }
    Ok(())
}

pub fn redact(input: &str) -> String {
    let mut redact_next = false;
    input
        .split_whitespace()
        .map(|word| {
            let lower = word.to_ascii_lowercase();
            let should_redact = redact_next
                || lower.contains("token")
                || lower.contains("secret")
                || lower.contains("password")
                || lower.contains("api_key")
                || lower.contains("apikey")
                || word.starts_with("sk-")
                || lower.starts_with("bearer");
            redact_next = lower == "bearer" || lower.ends_with("token:");
            if should_redact {
                "[REDACTED]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn absorb_event(value: &Value, summary: &mut EventSummary) {
    summary.raw_event_count += 1;
    let kind = event_type(value);
    if !kind.is_empty() {
        if kind.contains("command") || kind.contains("tool") || kind.contains("item") {
            summary.tool_or_command_events += 1;
        }
        summary.event_types.insert(kind);
    }
    if summary.session_id.is_none() {
        summary.session_id = find_string(value, &["thread_id", "session_id"]);
    }
}

pub fn event_type(value: &Value) -> String {
    ["type", "event_type", "subtype"]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned()
}
fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(value) = map.get(*key).and_then(Value::as_str) {
                    return Some(value.to_owned());
                }
            }
            map.values().find_map(|value| find_string(value, keys))
        }
        Value::Array(values) => values.iter().find_map(|value| find_string(value, keys)),
        _ => None,
    }
}
fn event_text(event: &ProcessEvent) -> &str {
    match event {
        ProcessEvent::Stdout(line) | ProcessEvent::Stderr(line) => line,
        ProcessEvent::OutputError | ProcessEvent::Exited(_) => "",
    }
}

fn write_summary(
    agent: Agent,
    test: ProbeTest,
    fixture: &Path,
    artifacts: &ArtifactPaths,
    summary: &EventSummary,
    details: SummaryDetails<'_>,
) -> Result<()> {
    let value = ProbeSummary {
        run_id: &artifacts.run_id,
        agent: agent.name(),
        test: if test == ProbeTest::Normal {
            "normal"
        } else {
            "cancellation"
        },
        fixture_path: fixture.display().to_string(),
        artifact_root: artifacts.root.display().to_string(),
        stdout_log: artifacts.stdout.display().to_string(),
        stderr_log: artifacts.stderr.display().to_string(),
        summary_path: artifacts.summary.display().to_string(),
        event_summary: summary,
        validation_failures: details.failures,
        cancellation_latency_ms: details.cancellation_latency_ms,
        cancellation_validation: details.cancellation_validation,
    };
    fs::write(&artifacts.summary, serde_json::to_vec_pretty(&value)?)?;
    fs::write(
        artifacts.root.join("validation-failures.txt"),
        if details.failures.is_empty() {
            "none\n".to_owned()
        } else {
            format!("{}\n", details.failures.join("\n"))
        },
    )?;
    Ok(())
}
fn git_output(directory: &Path, args: &[&str]) -> Result<String> {
    let output = StdCommand::new("git")
        .args(args)
        .current_dir(directory)
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        bail!(
            "git {:?}: {}",
            args,
            redact(&String::from_utf8_lossy(&output.stderr))
        )
    }
}
fn command_text(program: &str, args: &[&str]) -> Result<String> {
    let output = StdCommand::new(program).args(args).output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        bail!("command failed")
    }
}
fn executable_path(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(program))
        .find(|path| path.is_file())
}
fn append(path: &Path, line: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}
fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

struct Capture {
    summary: EventSummary,
}

enum ProbeError {
    UnsafeSetup(anyhow::Error),
    Validation(anyhow::Error),
}
struct ArtifactPaths {
    run_id: String,
    root: PathBuf,
    raw: PathBuf,
    normalized: PathBuf,
    stdout: PathBuf,
    stderr: PathBuf,
    summary: PathBuf,
}
impl ArtifactPaths {
    fn create(agent: Agent, test: ProbeTest) -> Result<Self> {
        Self::create_in(
            &std::env::current_dir()?.join("target/agent-sentinel-probes"),
            format!("run-{}", unique_suffix()),
            agent,
            test,
        )
    }

    fn create_in(base: &Path, run_id: String, agent: Agent, test: ProbeTest) -> Result<Self> {
        let root = base.join(format!(
            "{}-{}-{}",
            run_id,
            agent.name(),
            if test == ProbeTest::Normal {
                "normal"
            } else {
                "cancellation"
            }
        ));
        fs::create_dir_all(&root)?;
        fs::File::create(root.join("stdout.log"))?;
        fs::File::create(root.join("stderr.log"))?;
        Ok(Self {
            run_id,
            raw: root.join("raw-agent-events.jsonl"),
            normalized: root.join("normalized-events.jsonl"),
            stdout: root.join("stdout.log"),
            stderr: root.join("stderr.log"),
            summary: root.join("probe-summary.json"),
            root,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_confirmation_and_cancellation() {
        let args = vec!["codex", "--test", "cancellation", CONFIRMATION_FLAG]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(
            parse_args(&args).unwrap(),
            ProbeCommand::Agent {
                agent: Agent::Codex,
                test: ProbeTest::Cancellation,
                confirmed: true,
                cleanup: false
            }
        );
    }
    #[test]
    fn rejects_unknown_cli_arguments() {
        let args = vec!["codex", "--unsafe"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(parse_args(&args).is_err());
    }
    #[test]
    fn redacts_obvious_secret_values() {
        let redacted = redact("Authorization: Bearer top-secret token_value sk-abc");
        assert!(!redacted.contains("top-secret"));
        assert!(!redacted.contains("sk-abc"));
        assert!(redacted.contains("[REDACTED]"));
    }
    #[test]
    fn summarizes_nested_session_events() {
        let mut summary = EventSummary::default();
        absorb_event(
            &json!({"type":"thread.started","data":{"thread_id":"thread-1"}}),
            &mut summary,
        );
        assert_eq!(summary.session_id.as_deref(), Some("thread-1"));
        assert!(summary.event_types.contains("thread.started"));
    }
    #[test]
    fn validates_expected_and_unexpected_changes() {
        let fixture = tempdir().unwrap();
        git_output(fixture.path(), &["init"]).unwrap();
        git_output(
            fixture.path(),
            &[
                "-c",
                "user.email=a@b.test",
                "-c",
                "user.name=test",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        fs::write(fixture.path().join("result.txt"), "ok\n").unwrap();
        fs::write(fixture.path().join("extra.txt"), "no\n").unwrap();
        let failures = validate_fixture(fixture.path(), "ok\n", &["result.txt"]);
        assert!(failures.iter().any(|failure| failure.contains("extra.txt")));
    }
    #[test]
    fn accepts_exact_content_with_one_final_newline() {
        let expected = "Agent Sentinel Codex probe passed";
        assert!(matches_logical_content(expected, expected));
        assert!(matches_logical_content(
            "Agent Sentinel Codex probe passed\n",
            expected
        ));
        assert!(!matches_logical_content(
            " Agent Sentinel Codex probe passed",
            expected
        ));
        assert!(!matches_logical_content(
            "Agent Sentinel Codex probe passed ",
            expected
        ));
    }
    #[test]
    fn rejects_an_added_period_or_other_punctuation() {
        let expected = "Agent Sentinel Codex probe passed";
        assert!(!matches_logical_content(
            "Agent Sentinel Codex probe passed.\n",
            expected
        ));
        assert!(!matches_logical_content(
            "Agent Sentinel Codex probe passed!\n",
            expected
        ));
    }
    #[test]
    fn rejects_extra_lines() {
        let expected = "Agent Sentinel Codex probe passed";
        assert!(!matches_logical_content(
            "Agent Sentinel Codex probe passed\nextra\n",
            expected
        ));
    }
    #[test]
    fn accepts_exact_two_line_resume_output() {
        let expected = "Agent Sentinel Codex probe passed\nCodex resume passed";
        assert!(matches_logical_content(
            "Agent Sentinel Codex probe passed\nCodex resume passed\n",
            expected
        ));
    }
    #[test]
    fn codex_initial_command_uses_the_configured_fixture_sandbox() {
        let fixture = Path::new("/tmp/disposable-fixture");
        let args = initial_agent_args(Agent::Codex, fixture, "first prompt".into(), false);
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--sandbox", "workspace-write"]));
        assert_eq!(args.last(), Some(&"first prompt".to_owned()));
    }
    #[test]
    fn codex_resume_command_has_verified_argument_order() {
        let prompt = "follow up with spaces, apostrophe: it's safe; printf '%s\\n'\nand newline";
        let args = resume_agent_args(Agent::Codex, "thread-123", prompt.into());
        assert_eq!(&args[..3], ["exec", "resume", "--json"]);
        assert!(!args.iter().any(|argument| argument == "--sandbox"));
        assert_eq!(args[3], "thread-123");
        assert_eq!(args[4], prompt);
        assert_eq!(args.len(), 5);
    }
    #[test]
    fn artifact_paths_are_scoped_to_one_run_identifier() {
        let base = tempdir().unwrap();
        let initial = ArtifactPaths::create_in(
            base.path(),
            "run-initial".into(),
            Agent::Codex,
            ProbeTest::Normal,
        )
        .unwrap();
        let resume = ArtifactPaths::create_in(
            base.path(),
            "run-resume".into(),
            Agent::Codex,
            ProbeTest::Normal,
        )
        .unwrap();
        assert_ne!(initial.root, resume.root);
        assert!(initial.summary.starts_with(&initial.root));
        assert!(resume.summary.starts_with(&resume.root));
        assert!(!initial.summary.starts_with(&resume.root));
        assert!(initial.stdout.is_file());
        assert!(initial.stderr.is_file());
        assert_eq!(fs::read_to_string(&initial.stdout).unwrap(), "");
        assert_eq!(fs::read_to_string(&initial.stderr).unwrap(), "");
    }
    #[test]
    fn captures_cancellation_events_in_the_summary() {
        let base = tempdir().unwrap();
        let artifacts = ArtifactPaths::create_in(
            base.path(),
            "run-events".into(),
            Agent::Codex,
            ProbeTest::Cancellation,
        )
        .unwrap();
        let mut summary = EventSummary::default();
        record_process_event(
            &ProcessEvent::Stdout(
                r#"{"type":"thread.started","thread_id":"thread-cancellation"}"#.into(),
            ),
            &artifacts,
            &mut summary,
        )
        .unwrap();
        assert_eq!(summary.raw_event_count, 1);
        assert!(summary.event_types.contains("thread.started"));
        assert_eq!(summary.session_id.as_deref(), Some("thread-cancellation"));
    }
    #[test]
    fn creates_a_disposable_git_fixture() {
        let fixture = create_fixture(ProbeTest::Normal).unwrap();
        assert!(fixture.join("README.md").is_file());
        assert_eq!(
            git_output(&fixture, &["status", "--porcelain"]).unwrap(),
            ""
        );
        fs::remove_dir_all(fixture).unwrap();
    }
    #[test]
    fn maps_exit_codes() {
        assert_eq!(
            exit_code_for(true, true, false, true),
            ExitCode::ConfirmationMissing
        );
        assert_eq!(
            exit_code_for(false, false, false, true),
            ExitCode::AgentUnavailable
        );
        assert_eq!(
            exit_code_for(false, true, true, true),
            ExitCode::UnsafeEnvironment
        );
        assert_eq!(
            exit_code_for(false, true, false, false),
            ExitCode::ValidationFailed
        );
    }
    #[tokio::test]
    async fn cancellation_uses_a_fake_process_without_agents() {
        let (mut process, _) = SupervisedProcess::start("sh", &["-c", "sleep 10"])
            .await
            .unwrap();
        let _exit = process.cancel(Duration::from_millis(50)).await.unwrap();
        assert!(process.try_wait().unwrap().is_some());
    }
}
