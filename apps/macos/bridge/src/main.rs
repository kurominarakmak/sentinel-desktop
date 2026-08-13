//! Narrow, stdio-only service boundary for the native macOS shell.
//!
//! This process owns no workflow decisions. It maps durable V3 state to JSON
//! snapshots and forwards the already-existing final-approval supervisor call.

use sentinel_codex::{CodexProgram, CodexTaskStarter, StartedCodexTask};
use sentinel_core::{
    v3::{ApprovalId, TaskId},
    CoreError, RunRepository,
};
use sentinel_git::inspect_repository;
use sentinel_review::FinalApprovalSupervisor;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Request {
    Capabilities,
    ActiveTask,
    TaskDetail {
        task_id: String,
    },
    Subscribe,
    StartTask {
        request_id: String,
        provider: String,
        summary: String,
        prompt: String,
    },
    Supervisor {
        command: String,
        approval_id: Option<String>,
        approve: Option<bool>,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDto {
    id: String,
    label: String,
    available: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct TaskDto {
    id: String,
    summary: String,
    lifecycle: String,
    recovery_required: bool,
    recovery_reason: Option<String>,
    version: u64,
    updated_at_ms: i64,
}

fn task_dto(task: sentinel_core::v3::Task) -> TaskDto {
    let recovery_required = task.recovery_condition != sentinel_core::v3::RecoveryCondition::None;
    TaskDto {
        id: task.id.to_string(),
        summary: task.summary,
        lifecycle: format!("{:?}", task.lifecycle).to_lowercase(),
        recovery_reason: recovery_required
            .then(|| format!("{:?}", task.recovery_condition).to_lowercase()),
        recovery_required,
        version: task.version,
        updated_at_ms: task.updated_at_ms,
    }
}

async fn active_task(repository: &RunRepository) -> Result<Option<TaskDto>, CoreError> {
    let mut tasks = repository.v3().list_tasks().await?;
    tasks.sort_by_key(|task| (task.updated_at_ms, task.version));
    Ok(tasks.pop().map(task_dto))
}

async fn task_detail(repository: &RunRepository, task_id: String) -> Result<Value, CoreError> {
    let task_id = sentinel_core::v3::TaskId(task_id);
    let task = repository.v3().get_task(&task_id).await?;
    let events = repository.v3().list_events(&task_id).await?;
    let worktree = repository.v3().get_task_worktree(&task_id).await.ok();
    let validations = repository.v3().list_validation_results(&task_id).await?;
    let findings = repository.v3().list_review_findings(&task_id).await?;
    let repair_rounds = repository.v3().list_repair_rounds(&task_id).await?;
    Ok(json!({
        "task": task_dto(task),
        "activity": events.into_iter().map(|event| json!({"kind":format!("{:?}", event.kind).to_lowercase(),"provider":event.provider,"occurredAtMs":event.occurred_at_ms,"payload":event.payload})).collect::<Vec<_>>(),
        "worktree": worktree.map(|value| json!({"repositoryRoot":value.repository_root,"path":value.worktree_path,"branch":value.branch,"baseCommit":value.base_commit,"state":value.state})),
        "validations": validations.into_iter().map(|value| json!({"id":value.id.to_string(),"profile":value.profile_id,"check":value.check_name,"required":value.required,"state":format!("{:?}",value.lifecycle).to_lowercase(),"summary":value.summary,"updatedAtMs":value.updated_at_ms})).collect::<Vec<_>>(),
        "findings": findings.into_iter().map(|value| json!({"id":value.id.to_string(),"severity":value.severity,"disposition":format!("{:?}",value.disposition).to_lowercase(),"summary":value.summary,"evidence":value.evidence})).collect::<Vec<_>>(),
        "repairRounds": repair_rounds.into_iter().map(|value| json!({"id":value.id.to_string(),"round":value.round_number,"state":format!("{:?}",value.lifecycle).to_lowercase(),"updatedAtMs":value.updated_at_ms})).collect::<Vec<_>>()
    }))
}

fn codex_program() -> Option<CodexProgram> {
    let configured = std::env::var_os("AGENT_SENTINEL_CODEX_EXECUTABLE").map(PathBuf::from);
    let path = configured.or_else(|| {
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join("codex"))
                .find(|candidate| candidate.is_file())
        })
    });
    path.and_then(|path| CodexProgram::from_executable(path).ok())
}

fn repository_root() -> Option<PathBuf> {
    std::env::var_os("SENTINEL_REPOSITORY_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .filter(|path| path.is_dir())
}

async fn start_task(
    repository: &RunRepository,
    program: Option<CodexProgram>,
    root: &Path,
    provider: String,
    summary: String,
    prompt: String,
    owned_tasks: &mut HashMap<TaskId, StartedCodexTask>,
) -> Result<TaskDto, &'static str> {
    if provider != "codex" || prompt.trim().is_empty() || summary.trim().is_empty() {
        return Err("task submission was rejected");
    }
    if inspect_repository(root).await.is_err() {
        return Err("repository is unavailable or invalid");
    }
    let starter = CodexTaskStarter::new(program, repository.clone());
    let mut started = starter
        .start_task(summary, root)
        .await
        .map_err(|_| "Codex is unavailable")?;
    started
        .start_turn(&prompt)
        .await
        .map_err(|_| "Codex rejected the task prompt")?;
    let task = task_dto(started.task().clone());
    owned_tasks.insert(TaskId(task.id.clone()), started);
    Ok(task)
}

fn response(value: Value) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{}", value);
    let _ = stdout.flush();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_path = std::env::args()
        .nth(1)
        .ok_or("database path argument is required")?;
    let database_url = format!("sqlite://{}", PathBuf::from(database_path).display());
    let runtime = tokio::runtime::Runtime::new()?;
    let repository = runtime.block_on(RunRepository::open(&database_url))?;
    let root = repository_root();
    let program = codex_program();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if let Ok(request) = serde_json::from_str::<Request>(&line) {
                let _ = sender.send(request);
            } else {
                response(json!({"kind":"error","message":"invalid native bridge request"}));
            }
        }
    });

    let mut subscribed = false;
    let mut last_snapshot = None;
    let mut owned_tasks = HashMap::new();
    let mut accepted_submissions = HashMap::new();
    loop {
        match receiver.recv_timeout(Duration::from_millis(400)) {
            Ok(Request::Capabilities) => response(json!({
                "kind":"capabilities",
                "repository":root.as_ref().map(|path| path.display().to_string()),
                "providers":[
                    ProviderDto { id:"codex".into(), label:"Codex".into(), available:program.is_some() },
                    ProviderDto { id:"claude_code".into(), label:"Claude Code".into(), available:false }
                ]
            })),
            Ok(Request::ActiveTask) => match runtime.block_on(active_task(&repository)) {
                Ok(task) => response(json!({"kind":"active_task","task":task})),
                Err(_) => response(json!({"kind":"error","message":"could not read active task"})),
            },
            Ok(Request::TaskDetail { task_id }) => match runtime
                .block_on(task_detail(&repository, task_id))
            {
                Ok(detail) => response(json!({"kind":"task_detail","detail":detail})),
                Err(_) => response(json!({"kind":"error","message":"could not read task detail"})),
            },
            Ok(Request::Subscribe) => {
                subscribed = true;
                last_snapshot = None;
                response(json!({"kind":"subscribed"}));
            }
            Ok(Request::StartTask {
                request_id,
                provider,
                summary,
                prompt,
            }) => {
                if let Some(task) = accepted_submissions.get(&request_id) {
                    response(
                        json!({"kind":"task_start_result","requestId":request_id,"accepted":true,"task":task}),
                    );
                    continue;
                }
                let result = match root.as_deref() {
                    Some(root) => runtime.block_on(start_task(
                        &repository,
                        program.clone(),
                        root,
                        provider,
                        summary,
                        prompt,
                        &mut owned_tasks,
                    )),
                    None => Err("repository is unavailable or invalid"),
                };
                match result {
                    Ok(task) => {
                        accepted_submissions.insert(request_id.clone(), task);
                        response(
                            json!({"kind":"task_start_result","requestId":request_id,"accepted":true,"task":accepted_submissions.get(&request_id)}),
                        );
                    }
                    Err(message) => response(
                        json!({"kind":"task_start_result","requestId":request_id,"accepted":false,"message":message}),
                    ),
                }
            }
            Ok(Request::Supervisor {
                command,
                approval_id,
                approve,
            }) => {
                let result = match (command.as_str(), approval_id, approve) {
                    ("decide_final_approval", Some(approval_id), Some(approve)) => runtime
                        .block_on(FinalApprovalSupervisor::require_human_approval(
                            repository.clone(),
                            &ApprovalId(approval_id),
                            approve,
                        ))
                        .map_err(|_| ()),
                    _ => Err(()),
                };
                response(if result.is_ok() {
                    json!({"kind":"supervisor_result","ok":true})
                } else {
                    json!({"kind":"supervisor_result","ok":false,"message":"command was rejected by the Rust supervisor"})
                });
            }
            Err(mpsc::RecvTimeoutError::Timeout) if subscribed => {
                match runtime.block_on(active_task(&repository)) {
                    Ok(task) => {
                        let snapshot = serde_json::to_string(&task).unwrap_or_default();
                        if last_snapshot.as_ref() != Some(&snapshot) {
                            last_snapshot = Some(snapshot);
                            response(json!({"kind":"task_update","task":task}));
                        }
                    }
                    Err(_) => {
                        response(json!({"kind":"error","message":"could not refresh task state"}))
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_requests_are_narrow_and_typed() {
        assert!(matches!(
            serde_json::from_str::<Request>(r#"{"kind":"active_task"}"#),
            Ok(Request::ActiveTask)
        ));
        assert!(serde_json::from_str::<Request>(r#"{"kind":"direct_sql"}"#).is_err());
    }

    #[test]
    fn task_start_request_requires_provider_and_intent_fields() {
        assert!(matches!(
            serde_json::from_str::<Request>(
                r#"{"kind":"start_task","request_id":"request-1","provider":"codex","summary":"Task","prompt":"Do it"}"#
            ),
            Ok(Request::StartTask { .. })
        ));
        assert!(
            serde_json::from_str::<Request>(r#"{"kind":"start_task","provider":"codex"}"#).is_err()
        );
    }
}
