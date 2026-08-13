//! Narrow, stdio-only service boundary for the native macOS shell.
//!
//! This process owns no workflow decisions. It maps durable V3 state to JSON
//! snapshots and forwards the already-existing final-approval supervisor call.

use sentinel_core::{v3::ApprovalId, CoreError, RunRepository};
use sentinel_review::FinalApprovalSupervisor;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::mpsc,
    time::Duration,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Request {
    ActiveTask,
    TaskDetail {
        task_id: String,
    },
    Subscribe,
    Supervisor {
        command: String,
        approval_id: Option<String>,
        approve: Option<bool>,
    },
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
    loop {
        match receiver.recv_timeout(Duration::from_millis(400)) {
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
}
