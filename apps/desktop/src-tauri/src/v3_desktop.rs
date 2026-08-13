use super::*;
use sentinel_core::v3::{ApprovalId, TaskId};
use sentinel_review::FinalApprovalSupervisor;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct V3TaskDto {
    pub id: String,
    pub summary: String,
    pub lifecycle: String,
    pub recovery_required: bool,
    pub recovery_reason: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct V3TaskDetailDto {
    pub task: V3TaskDto,
    pub activity: Vec<serde_json::Value>,
    pub worktree: Option<serde_json::Value>,
    pub diff: Option<serde_json::Value>,
    pub validations: Vec<serde_json::Value>,
    pub findings: Vec<serde_json::Value>,
    pub repair_rounds: Vec<serde_json::Value>,
    pub final_approval_packet: Option<serde_json::Value>,
    pub final_approval_id: Option<String>,
    pub agent: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V3TaskRequest {
    pub task_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V3ApprovalDecisionRequest {
    pub approval_id: String,
    pub approve: bool,
}

fn task_dto(task: sentinel_core::v3::Task) -> V3TaskDto {
    V3TaskDto {
        id: task.id.to_string(),
        summary: task.summary,
        lifecycle: format!("{:?}", task.lifecycle).to_lowercase(),
        recovery_required: task.recovery_condition != sentinel_core::v3::RecoveryCondition::None,
        recovery_reason: (task.recovery_condition != sentinel_core::v3::RecoveryCondition::None)
            .then(|| format!("{:?}", task.recovery_condition).to_lowercase()),
    }
}
#[tauri::command]
pub async fn list_v3_tasks(state: State<'_, DesktopState>) -> Result<Vec<V3TaskDto>, SafeError> {
    state
        .repository
        .v3()
        .list_tasks()
        .await
        .map_err(safe_error)
        .map(|tasks| tasks.into_iter().map(task_dto).collect())
}
#[tauri::command]
pub async fn get_v3_task_detail(
    state: State<'_, DesktopState>,
    request: V3TaskRequest,
) -> Result<V3TaskDetailDto, SafeError> {
    if request.task_id.trim().is_empty() {
        return Err(input_error());
    }
    let id = TaskId(request.task_id);
    let task = state
        .repository
        .v3()
        .get_task(&id)
        .await
        .map_err(safe_error)?;
    let events = state
        .repository
        .v3()
        .list_events(&id)
        .await
        .map_err(safe_error)?;
    let worktree = state.repository.v3().get_task_worktree(&id).await.ok();
    let merge = state
        .repository
        .v3()
        .get_task_worktree_merge_preparation(&id)
        .await
        .ok();
    let validations = state
        .repository
        .v3()
        .list_validation_results(&id)
        .await
        .map_err(safe_error)?;
    let findings = state
        .repository
        .v3()
        .list_review_findings(&id)
        .await
        .map_err(safe_error)?;
    let rounds = state
        .repository
        .v3()
        .list_repair_rounds(&id)
        .await
        .map_err(safe_error)?;
    let artifacts = state
        .repository
        .v3()
        .list_artifacts(&id)
        .await
        .map_err(safe_error)?;
    let final_packet = artifacts
        .iter()
        .rev()
        .find(|artifact| artifact.kind == "final_approval_packet")
        .map(|artifact| artifact.metadata.clone());
    let pending = state
        .repository
        .v3()
        .list_approvals_for_task(&id)
        .await
        .map_err(safe_error)?;
    let agent = state
        .repository
        .v3()
        .list_sessions_for_task(&id)
        .await
        .map_err(safe_error)?
        .into_iter()
        .last()
        .map(|session| format!("{} · {:?}", session.provider, session.lifecycle).to_lowercase());
    Ok(V3TaskDetailDto {
        task: task_dto(task),
        activity: events.into_iter().map(|event| serde_json::json!({"kind":format!("{:?}", event.kind).to_lowercase(),"provider":event.provider,"occurredAtMs":event.occurred_at_ms,"payload":event.payload})).collect(),
        worktree: worktree.map(|value| serde_json::json!({"repositoryRoot":value.repository_root,"path":value.worktree_path,"branch":value.branch,"baseCommit":value.base_commit,"state":value.state})),
        diff: merge.and_then(|value| serde_json::from_str(&value.diff_json).ok()),
        validations: validations.into_iter().map(|value| serde_json::json!({"id":value.id.to_string(),"profile":value.profile_id,"check":value.check_name,"required":value.required,"state":format!("{:?}", value.lifecycle).to_lowercase(),"summary":value.summary,"createdAtMs":value.created_at_ms,"updatedAtMs":value.updated_at_ms})).collect(),
        findings: findings.into_iter().map(|value| serde_json::json!({"id":value.id.to_string(),"severity":value.severity,"disposition":format!("{:?}", value.disposition).to_lowercase(),"summary":value.summary,"evidence":value.evidence})).collect(),
        repair_rounds: rounds.into_iter().map(|value| serde_json::json!({"id":value.id.to_string(),"round":value.round_number,"state":format!("{:?}", value.lifecycle).to_lowercase(),"createdAtMs":value.created_at_ms,"updatedAtMs":value.updated_at_ms})).collect(),
        final_approval_packet: final_packet,
        final_approval_id: pending.into_iter().rev().find(|value| value.action_kind == "v3_final_git_action" && value.lifecycle == sentinel_core::v3::ApprovalLifecycle::Pending).map(|value| value.id.to_string()),
        agent,
    })
}
#[tauri::command]
pub async fn decide_v3_final_approval(
    state: State<'_, DesktopState>,
    app: AppHandle,
    request: V3ApprovalDecisionRequest,
) -> Result<(), SafeError> {
    if request.approval_id.trim().is_empty() {
        return Err(input_error());
    }
    let approval_id = ApprovalId(request.approval_id);
    FinalApprovalSupervisor::require_human_approval(
        state.repository.clone(),
        &approval_id,
        request.approve,
    )
    .await
    .map_err(safe_error)?;
    let _ = app.emit("v3-state-changed", approval_id.to_string());
    Ok(())
}
