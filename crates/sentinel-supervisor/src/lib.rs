//! Product-level V3 workflow authority shared by desktop presentation shells.
//!
//! Presentation bridges send typed intent to this service. The service creates
//! and reconciles the task worktree before any provider process is allowed to
//! start, owns live provider processes, and derives UI actions from durable
//! state. Provider completion is retained as evidence and never finalizes a
//! Sentinel task by itself.

mod api_tools;

use sentinel_claude::{ClaudeProcess, ClaudeProgram, ClaudeSession, PROVIDER as CLAUDE_PROVIDER};
use sentinel_codex::{
    CodexAppServer, CodexProgram, CodexTaskReconciliation, CodexTaskStarter, StartedCodexTask,
    PROVIDER as CODEX_PROVIDER,
};
use sentinel_core::{
    redact,
    v3::{
        ApprovalId, ApprovalLifecycle, CreateArtifact, CreateRepairRound, CreateSession,
        CreateTask, EventId, EventKind, FindingDisposition, NormalizedEventEnvelope,
        RecoveryCondition, RepairRoundLifecycle, SessionLifecycle, Task, TaskId, TaskLifecycle,
    },
    RunRepository,
};
use sentinel_git::{inspect_repository, RepositoryState};
use sentinel_provider_api::{
    agent::{run_agent_loop, AgentLoopRequest, AgentLoopResult},
    CancellationHandle, MemoryCredentialStore, MessageRole, ProviderError, ProviderId,
    ProviderMessage, ProviderRegistry, ProviderRequest, ProviderRole, ProviderSelection,
    ResponseFormat, WorkflowProviderConfiguration, CLAUDE_PROVIDER_ID, CODEX_PROVIDER_ID,
};
use sentinel_review::{
    FinalApprovalError, FinalApprovalSupervisor, FindingSeverity, RepairError, RepairEvidence,
    RepairFinding, RepairImplementer, RepairPacket, ReviewCandidate, ReviewError, ReviewPacket,
    ReviewSupervisor, ReviewValidation, Reviewer,
};
use sentinel_validation::{load_repository_profiles, ValidationProfile, ValidationSupervisor};
use sentinel_worktree::{TransactionError, WorktreeTransaction};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use tokio::time;

const PROVIDER_STAGE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_REPAIR_ROUNDS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Codex,
    ClaudeCode,
}

impl ProviderKind {
    pub fn parse(value: &str) -> Result<Self, SupervisorError> {
        match value {
            "codex" => Ok(Self::Codex),
            "claude_code" => Ok(Self::ClaudeCode),
            _ => Err(SupervisorError::ProviderUnavailable),
        }
    }

    pub fn durable_id(self) -> &'static str {
        match self {
            Self::Codex => CODEX_PROVIDER,
            Self::ClaudeCode => CLAUDE_PROVIDER,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SupervisorPrograms {
    pub codex: Option<CodexProgram>,
    pub claude: Option<ClaudeProgram>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartTaskIntent {
    pub provider: ProviderKind,
    pub summary: String,
    pub prompt: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableActions {
    pub stop: bool,
    pub approve: bool,
    pub reject: bool,
    pub approval_id: Option<ApprovalId>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SupervisorError {
    #[error("repository or worktree ownership could not be proven")]
    Ownership,
    #[error("provider is unavailable")]
    ProviderUnavailable,
    #[error("task intent is invalid")]
    InvalidIntent,
    #[error("task is recovery-required")]
    RecoveryRequired,
    #[error("task or action is not available")]
    NotAvailable,
    #[error("provider operation failed")]
    Provider,
    #[error("durable state operation failed")]
    Storage,
}

enum OwnedProvider {
    Codex(StartedCodexTask),
    Claude {
        process: ClaudeProcess,
        session: ClaudeSession,
    },
    Api {
        session_id: sentinel_core::v3::SessionId,
        provider_id: ProviderId,
        cancellation: CancellationHandle,
        completion: tokio::task::JoinHandle<Result<AgentLoopResult, ProviderError>>,
    },
}

/// The one process-local owner of active provider children for a desktop host.
pub struct SentinelSupervisor {
    repository: RunRepository,
    primary_root: PathBuf,
    worktree_root: PathBuf,
    programs: SupervisorPrograms,
    provider_registry: Arc<ProviderRegistry>,
    provider_workflow: WorkflowProviderConfiguration,
    owned: HashMap<TaskId, OwnedProvider>,
    reconciled_startup: bool,
}

impl SentinelSupervisor {
    pub fn new(
        repository: RunRepository,
        primary_root: impl AsRef<Path>,
        worktree_root: impl AsRef<Path>,
        programs: SupervisorPrograms,
    ) -> Result<Self, SupervisorError> {
        let provider_registry = Arc::new(ProviderRegistry::with_managed_cli_providers(
            Arc::new(MemoryCredentialStore::default()),
            programs.codex.is_some(),
            programs.claude.is_some(),
        ));
        let provider_workflow = WorkflowProviderConfiguration {
            implementer: managed_selection(CODEX_PROVIDER_ID),
            reviewer: managed_selection(if programs.claude.is_some() {
                CLAUDE_PROVIDER_ID
            } else {
                CODEX_PROVIDER_ID
            }),
            repair: None,
        };
        Self::with_provider_runtime(
            repository,
            primary_root,
            worktree_root,
            programs,
            provider_registry,
            provider_workflow,
        )
    }

    pub fn with_provider_runtime(
        repository: RunRepository,
        primary_root: impl AsRef<Path>,
        worktree_root: impl AsRef<Path>,
        programs: SupervisorPrograms,
        provider_registry: Arc<ProviderRegistry>,
        provider_workflow: WorkflowProviderConfiguration,
    ) -> Result<Self, SupervisorError> {
        let primary_root = primary_root
            .as_ref()
            .canonicalize()
            .map_err(|_| SupervisorError::Ownership)?;
        std::fs::create_dir_all(worktree_root.as_ref()).map_err(|_| SupervisorError::Ownership)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                worktree_root.as_ref(),
                std::fs::Permissions::from_mode(0o700),
            )
            .map_err(|_| SupervisorError::Ownership)?;
        }
        let worktree_root = worktree_root
            .as_ref()
            .canonicalize()
            .map_err(|_| SupervisorError::Ownership)?;
        if primary_root == worktree_root || worktree_root.starts_with(&primary_root) {
            return Err(SupervisorError::Ownership);
        }
        Ok(Self {
            repository,
            primary_root,
            worktree_root,
            programs,
            provider_registry,
            provider_workflow,
            owned: HashMap::new(),
            reconciled_startup: false,
        })
    }

    pub fn update_provider_runtime(
        &mut self,
        provider_registry: Arc<ProviderRegistry>,
        provider_workflow: WorkflowProviderConfiguration,
    ) -> Result<(), SupervisorError> {
        provider_registry
            .validate_workflow_configuration(&provider_workflow)
            .map_err(|_| SupervisorError::ProviderUnavailable)?;
        self.provider_registry = provider_registry;
        self.provider_workflow = provider_workflow;
        Ok(())
    }

    pub fn provider_available(&self, provider: ProviderKind) -> bool {
        match provider {
            ProviderKind::Codex => self.programs.codex.is_some(),
            ProviderKind::ClaudeCode => self.programs.claude.is_some(),
        }
    }

    pub fn repository(&self) -> &RunRepository {
        &self.repository
    }

    pub fn primary_root(&self) -> &Path {
        &self.primary_root
    }

    /// Returns the most recently updated coding workflow task. Utility/draft
    /// records can never displace a task, while the latest terminal task remains
    /// visible long enough for native failed/cancelled state presentation.
    pub async fn active_task(&self) -> Result<Option<Task>, SupervisorError> {
        let tasks = self
            .repository
            .v3()
            .list_tasks()
            .await
            .map_err(|_| SupervisorError::Storage)?;
        let mut candidates = tasks
            .into_iter()
            .filter(|task| {
                task.lifecycle != TaskLifecycle::Draft
                    && (task.workflow_id.starts_with("v3:")
                        || matches!(task.workflow_id.as_str(), "codex-v3" | "claude-v3"))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|task| (task.updated_at_ms, task.version));
        for task in candidates.into_iter().rev() {
            if self.owned.contains_key(&task.id) {
                return Ok(Some(task));
            }
            if self
                .repository
                .v3()
                .get_task_worktree(&task.id)
                .await
                .is_ok_and(|worktree| Path::new(&worktree.repository_root) == self.primary_root)
            {
                return Ok(Some(task));
            }
        }
        Ok(None)
    }

    /// Creates the durable task and owned worktree before starting the selected
    /// provider. Failure retains evidence/worktree and never falls back to the
    /// primary checkout.
    pub async fn start_task(&mut self, intent: StartTaskIntent) -> Result<Task, SupervisorError> {
        let selection = managed_selection(match intent.provider {
            ProviderKind::Codex => CODEX_PROVIDER_ID,
            ProviderKind::ClaudeCode => CLAUDE_PROVIDER_ID,
        });
        self.start_with_selection(selection, intent.summary, intent.prompt)
            .await
    }

    /// Starts a task with the configured implementer, or with an explicit
    /// provider override selected by the user. Reviewer and repair selections
    /// are pinned separately with the task before any provider is launched.
    pub async fn start_configured_task(
        &mut self,
        provider_override: Option<&str>,
        summary: String,
        prompt: String,
    ) -> Result<Task, SupervisorError> {
        let selection = match provider_override {
            Some(provider) if !provider.is_empty() && provider != "configured" => {
                self.selection_for_provider(provider, ProviderRole::Implementer)?
            }
            _ => self.provider_workflow.implementer.clone(),
        };
        self.start_with_selection(selection, summary, prompt).await
    }

    fn selection_for_provider(
        &self,
        provider: &str,
        role: ProviderRole,
    ) -> Result<ProviderSelection, SupervisorError> {
        let provider = ProviderId::new(provider_alias(provider))
            .map_err(|_| SupervisorError::ProviderUnavailable)?;
        let config = self
            .provider_registry
            .provider(&provider)
            .ok_or(SupervisorError::ProviderUnavailable)?;
        let selection = ProviderSelection {
            provider_id: provider,
            model_id: config.model_id.clone(),
        };
        self.provider_registry
            .validate_selection(&selection, role)
            .map_err(|_| SupervisorError::ProviderUnavailable)?;
        Ok(selection)
    }

    async fn start_with_selection(
        &mut self,
        selection: ProviderSelection,
        summary: String,
        prompt: String,
    ) -> Result<Task, SupervisorError> {
        self.provider_registry
            .validate_selection(&selection, ProviderRole::Implementer)
            .map_err(|_| SupervisorError::ProviderUnavailable)?;
        let pinned_roles = WorkflowProviderConfiguration {
            implementer: selection.clone(),
            reviewer: self.provider_workflow.reviewer.clone(),
            repair: self.provider_workflow.repair.clone(),
        };
        self.provider_registry
            .validate_workflow_configuration(&pinned_roles)
            .map_err(|_| SupervisorError::ProviderUnavailable)?;
        if summary.trim().is_empty()
            || prompt.trim().is_empty()
            || prompt.len() > 8_000
            || prompt.contains('\0')
        {
            return Err(SupervisorError::InvalidIntent);
        }
        if self
            .active_task()
            .await?
            .is_some_and(|task| task.recovery_condition != RecoveryCondition::None)
        {
            return Err(SupervisorError::RecoveryRequired);
        }
        let task = self
            .repository
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: format!("v3:{}", selection.provider_id),
                    summary,
                },
                now_ms(),
            )
            .await
            .map_err(|_| SupervisorError::Storage)?;
        let prepared = self
            .repository
            .v3()
            .transition_task(&task, TaskLifecycle::Preparing, now_ms())
            .await
            .map_err(|_| SupervisorError::Storage)?;
        let worktree = match WorktreeTransaction::create(
            self.repository.clone(),
            prepared.id.clone(),
            &self.primary_root,
            &self.worktree_root,
        )
        .await
        {
            Ok(worktree) => worktree,
            Err(error) => {
                let _ = self
                    .repository
                    .v3()
                    .transition_task(&prepared, TaskLifecycle::Failed, now_ms())
                    .await;
                return Err(map_worktree(error));
            }
        };
        let cwd = PathBuf::from(&worktree.worktree_path);
        self.repository
            .v3()
            .create_artifact(
                CreateArtifact {
                    task_id: prepared.id.clone(),
                    kind: "provider_role_configuration".into(),
                    display_name: "task-provider-roles".into(),
                    content_hash: None,
                    metadata: serde_json::to_value(&pinned_roles)
                        .map_err(|_| SupervisorError::Storage)?,
                },
                now_ms(),
            )
            .await
            .map_err(|_| SupervisorError::Storage)?;
        let started = match selection.provider_id.as_str() {
            CODEX_PROVIDER_ID => {
                let starter =
                    CodexTaskStarter::new(self.programs.codex.clone(), self.repository.clone());
                let mut started = match starter.start_prepared_task(prepared.clone(), &cwd).await {
                    Ok(started) => started,
                    Err(_) => {
                        fail_task(&self.repository, &prepared).await;
                        return Err(SupervisorError::Provider);
                    }
                };
                if let Err(error) = started.start_turn(&prompt).await {
                    fail_task(&self.repository, &prepared).await;
                    return Err(match error {
                        sentinel_codex::CodexError::InvalidInput => SupervisorError::InvalidIntent,
                        _ => SupervisorError::Provider,
                    });
                }
                OwnedProvider::Codex(started)
            }
            CLAUDE_PROVIDER_ID => {
                let program = self
                    .programs
                    .claude
                    .clone()
                    .ok_or(SupervisorError::ProviderUnavailable)?;
                let (process, session) = match ClaudeProcess::start(
                    program,
                    self.repository.clone(),
                    prepared.id.clone(),
                    &cwd,
                    &prompt,
                )
                .await
                {
                    Ok(started) => started,
                    Err(_) => {
                        fail_task(&self.repository, &prepared).await;
                        return Err(SupervisorError::Provider);
                    }
                };
                activate_session(&self.repository, &session.session_id).await?;
                self.repository
                    .v3()
                    .transition_task(&prepared, TaskLifecycle::Implementing, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                OwnedProvider::Claude { process, session }
            }
            _ => {
                let session = self
                    .repository
                    .v3()
                    .create_session(
                        CreateSession {
                            task_id: prepared.id.clone(),
                            provider: selection.provider_id.to_string(),
                            provider_session_ref: format!(
                                "sentinel-api:{}:{}",
                                selection.provider_id, prepared.id
                            ),
                        },
                        now_ms(),
                    )
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                activate_session(&self.repository, &session.id).await?;
                self.repository
                    .v3()
                    .transition_task(&prepared, TaskLifecycle::Implementing, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                let tools = Arc::new(
                    api_tools::OwnedWorktreeTools::new(&cwd, &self.primary_root)
                        .map_err(|_| SupervisorError::Ownership)?,
                );
                let registry = self.provider_registry.clone();
                let cancellation = CancellationHandle::default();
                let run_cancellation = cancellation.clone();
                let request_id = format!("task-{}", prepared.id);
                let provider_id = selection.provider_id.clone();
                let durable_provider_id = provider_id.clone();
                let durable_repository = self.repository.clone();
                let durable_task_id = prepared.id.clone();
                let durable_session_id = session.id.clone();
                let completion = tokio::spawn(async move {
                    let outcome = run_agent_loop(
                        &registry,
                        AgentLoopRequest {
                            request_id,
                            selection: selection.clone(),
                            role: ProviderRole::Implementer,
                            system_prompt: implementation_system_prompt(),
                            user_prompt: prompt,
                            response_format: ResponseFormat::Text,
                            max_output_tokens: None,
                            timeout_ms: PROVIDER_STAGE_TIMEOUT.as_millis() as u64,
                            max_rounds: 12,
                        },
                        tools,
                        Arc::new(|_| {}),
                        run_cancellation,
                    )
                    .await;
                    persist_api_run_outcome(
                        &durable_repository,
                        &durable_task_id,
                        &durable_session_id,
                        &durable_provider_id,
                        &outcome,
                    )
                    .await
                    .map_err(|_| {
                        ProviderError::Unavailable(
                            "durable API provider result could not be persisted".into(),
                        )
                    })?;
                    outcome
                });
                OwnedProvider::Api {
                    session_id: session.id,
                    provider_id,
                    cancellation,
                    completion,
                }
            }
        };
        self.owned.insert(prepared.id.clone(), started);
        self.repository
            .v3()
            .get_task(&prepared.id)
            .await
            .map_err(|_| SupervisorError::Storage)
    }

    pub async fn available_actions(
        &self,
        task: &Task,
    ) -> Result<AvailableActions, SupervisorError> {
        if task.recovery_condition != RecoveryCondition::None {
            return Ok(AvailableActions {
                stop: false,
                approve: false,
                reject: false,
                approval_id: None,
            });
        }
        let approval_id = self
            .repository
            .v3()
            .list_approvals_for_task(&task.id)
            .await
            .map_err(|_| SupervisorError::Storage)?
            .into_iter()
            .rev()
            .find(|approval| {
                approval.action_kind == "v3_final_git_action"
                    && approval.lifecycle == ApprovalLifecycle::Pending
            })
            .map(|approval| approval.id);
        Ok(AvailableActions {
            stop: matches!(
                task.lifecycle,
                TaskLifecycle::Preparing
                    | TaskLifecycle::Implementing
                    | TaskLifecycle::AwaitingApproval
                    | TaskLifecycle::Repairing
            ) && self.owned.contains_key(&task.id),
            approve: approval_id.is_some(),
            reject: approval_id.is_some(),
            approval_id,
        })
    }

    pub async fn stop(&mut self, task_id: &TaskId) -> Result<(), SupervisorError> {
        let task = self
            .repository
            .v3()
            .get_task(task_id)
            .await
            .map_err(|_| SupervisorError::NotAvailable)?;
        if task.recovery_condition != RecoveryCondition::None {
            return Err(SupervisorError::RecoveryRequired);
        }
        if matches!(self.owned.get(task_id), Some(OwnedProvider::Api { .. })) {
            let Some(OwnedProvider::Api {
                session_id,
                cancellation,
                mut completion,
                ..
            }) = self.owned.remove(task_id)
            else {
                return Err(SupervisorError::NotAvailable);
            };
            let durable = self
                .repository
                .v3()
                .get_session(&session_id)
                .await
                .map_err(|_| SupervisorError::Storage)?;
            self.repository
                .v3()
                .transition_session(&durable, SessionLifecycle::Cancelling, now_ms())
                .await
                .map_err(|_| SupervisorError::Storage)?;
            cancellation.cancel();
            if time::timeout(Duration::from_secs(5), &mut completion)
                .await
                .is_err()
            {
                completion.abort();
            }
            let latest_session = self
                .repository
                .v3()
                .get_session(&session_id)
                .await
                .map_err(|_| SupervisorError::Storage)?;
            if latest_session.lifecycle == SessionLifecycle::Cancelling {
                self.repository
                    .v3()
                    .transition_session(&latest_session, SessionLifecycle::Cancelled, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
            }
            let current = self
                .repository
                .v3()
                .get_task(task_id)
                .await
                .map_err(|_| SupervisorError::Storage)?;
            if !current.lifecycle.terminal() {
                self.repository
                    .v3()
                    .transition_task(&current, TaskLifecycle::Cancelled, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
            }
            return Ok(());
        }
        match self.owned.get_mut(task_id) {
            Some(OwnedProvider::Codex(started)) => started
                .cancel()
                .await
                .map_err(|_| SupervisorError::Provider),
            Some(OwnedProvider::Claude { process, session }) => {
                let durable = self
                    .repository
                    .v3()
                    .get_session(&session.session_id)
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                let cancelling = self
                    .repository
                    .v3()
                    .transition_session(&durable, SessionLifecycle::Cancelling, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                process
                    .cancel()
                    .await
                    .map_err(|_| SupervisorError::Provider)?;
                self.repository
                    .v3()
                    .transition_session(&cancelling, SessionLifecycle::Cancelled, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                let current = self
                    .repository
                    .v3()
                    .get_task(task_id)
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                if !current.lifecycle.terminal() {
                    self.repository
                        .v3()
                        .transition_task(&current, TaskLifecycle::Cancelled, now_ms())
                        .await
                        .map_err(|_| SupervisorError::Storage)?;
                }
                Ok(())
            }
            Some(OwnedProvider::Api { .. }) | None => Err(SupervisorError::NotAvailable),
        }
    }

    pub async fn decide_final_approval(
        &self,
        task_id: &TaskId,
        approval_id: &ApprovalId,
        approve: bool,
    ) -> Result<(), SupervisorError> {
        let task = self
            .repository
            .v3()
            .get_task(task_id)
            .await
            .map_err(|_| SupervisorError::NotAvailable)?;
        if task.recovery_condition != RecoveryCondition::None {
            return Err(SupervisorError::RecoveryRequired);
        }
        let approval = self
            .repository
            .v3()
            .get_approval(approval_id)
            .await
            .map_err(|_| SupervisorError::NotAvailable)?;
        if approval.task_id != *task_id
            || approval.action_kind != "v3_final_git_action"
            || approval.lifecycle != ApprovalLifecycle::Pending
        {
            return Err(SupervisorError::NotAvailable);
        }
        match FinalApprovalSupervisor::require_human_approval(
            self.repository.clone(),
            approval_id,
            approve,
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(FinalApprovalError::ApprovalRejected) if !approve => {
                let current = self
                    .repository
                    .v3()
                    .get_task(task_id)
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                self.repository
                    .v3()
                    .transition_task(&current, TaskLifecycle::Blocked, now_ms())
                    .await
                    .map(|_| ())
                    .map_err(|_| SupervisorError::Storage)
            }
            Err(_) => Err(SupervisorError::NotAvailable),
        }
    }

    /// Advances only transitions justified by newly persisted provider or
    /// deterministic evidence. Calling it repeatedly is safe: durable state
    /// gates every stage and provider text never grants approval or completion.
    pub async fn reconcile_progress(&mut self) -> Result<(), SupervisorError> {
        self.reconcile_finished_api_runs().await?;
        let Some(task) = self.active_task().await? else {
            return Ok(());
        };
        if task.recovery_condition != RecoveryCondition::None || task.lifecycle.terminal() {
            return Ok(());
        }
        let result = match task.lifecycle {
            TaskLifecycle::Implementing if self.implementer_turn_completed(&task.id).await? => self
                .repository
                .v3()
                .transition_task(&task, TaskLifecycle::Validating, now_ms())
                .await
                .map(|_| ())
                .map_err(|_| SupervisorError::Storage),
            TaskLifecycle::Validating => self.run_validation_stage(task.clone()).await,
            TaskLifecycle::Reviewing => self.run_review_stage(task.clone()).await,
            _ => Ok(()),
        };
        if let Err(error) = result {
            let _ = self
                .repository
                .v3()
                .create_artifact(
                    CreateArtifact {
                        task_id: task.id.clone(),
                        kind: "workflow_stage_failure".into(),
                        display_name: format!("{:?}", task.lifecycle).to_lowercase(),
                        content_hash: None,
                        metadata: serde_json::json!({"error":redact(&error.to_string())}),
                    },
                    now_ms(),
                )
                .await;
            if let Ok(current) = self.repository.v3().get_task(&task.id).await {
                if !current.lifecycle.terminal()
                    && current.recovery_condition == RecoveryCondition::None
                {
                    let _ = self
                        .repository
                        .v3()
                        .transition_task(&current, TaskLifecycle::Blocked, now_ms())
                        .await;
                }
            }
            return Err(error);
        }
        Ok(())
    }

    async fn reconcile_finished_api_runs(&mut self) -> Result<(), SupervisorError> {
        let finished = self
            .owned
            .iter()
            .filter_map(|(task_id, provider)| match provider {
                OwnedProvider::Api { completion, .. } if completion.is_finished() => {
                    Some(task_id.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for task_id in finished {
            let Some(OwnedProvider::Api {
                session_id,
                provider_id,
                completion,
                ..
            }) = self.owned.remove(&task_id)
            else {
                continue;
            };
            let _ = (session_id, provider_id);
            completion
                .await
                .map_err(|_| SupervisorError::Provider)?
                .map_err(|_| SupervisorError::Provider)?;
        }
        Ok(())
    }

    async fn implementer_turn_completed(&self, task_id: &TaskId) -> Result<bool, SupervisorError> {
        self.repository
            .v3()
            .list_events(task_id)
            .await
            .map_err(|_| SupervisorError::Storage)
            .map(|events| events.iter().any(provider_turn_completed))
    }

    fn selected_profile(&self) -> Result<ValidationProfile, SupervisorError> {
        let profiles = load_repository_profiles(&self.primary_root)
            .map_err(|_| SupervisorError::NotAvailable)?;
        profiles
            .profiles
            .iter()
            .find(|profile| profile.id == "default")
            .or_else(|| profiles.profiles.first())
            .cloned()
            .ok_or(SupervisorError::NotAvailable)
    }

    async fn run_validation_stage(&mut self, task: Task) -> Result<(), SupervisorError> {
        let profile = match self.selected_profile() {
            Ok(profile) => profile,
            Err(_) => {
                self.repository
                    .v3()
                    .transition_task(&task, TaskLifecycle::Blocked, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                return Ok(());
            }
        };
        let (_sender, mut cancellation) = tokio::sync::watch::channel(false);
        let passed = ValidationSupervisor::run_selected(
            self.repository.clone(),
            task.id.clone(),
            &self.primary_root,
            &profile.id,
            &mut cancellation,
        )
        .await
        .map_err(|_| SupervisorError::NotAvailable)?
        .results
        .iter()
        .all(|result| result.outcome == "passed");
        let current = self
            .repository
            .v3()
            .get_task(&task.id)
            .await
            .map_err(|_| SupervisorError::Storage)?;
        self.repository
            .v3()
            .transition_task(
                &current,
                if passed {
                    TaskLifecycle::Reviewing
                } else {
                    TaskLifecycle::Blocked
                },
                now_ms(),
            )
            .await
            .map_err(|_| SupervisorError::Storage)?;
        Ok(())
    }

    async fn run_review_stage(&mut self, task: Task) -> Result<(), SupervisorError> {
        let inspection = inspect_repository(&self.primary_root)
            .await
            .map_err(|_| SupervisorError::Ownership)?;
        let target_branch = inspection
            .branch
            .filter(|_| inspection.is_primary && inspection.state == RepositoryState::Valid)
            .ok_or(SupervisorError::Ownership)?;
        WorktreeTransaction::prepare_merge(
            self.repository.clone(),
            task.id.clone(),
            &self.primary_root,
            &target_branch,
        )
        .await
        .map_err(map_worktree)?;
        let packet = ReviewSupervisor::build_packet(
            self.repository.clone(),
            task.id.clone(),
            &self.primary_root,
        )
        .await
        .map_err(|_| SupervisorError::NotAvailable)?;
        let candidates = match self.cross_model_review(&task, &packet).await {
            Ok(candidates) => candidates,
            Err(error) => {
                self.persist_reconciliation(&task.id, "reviewer", "unavailable")
                    .await?;
                return Err(error);
            }
        };
        self.reconcile_repaired_findings(&task.id, &candidates)
            .await?;
        ReviewSupervisor::persist_candidates(
            self.repository.clone(),
            task.id.clone(),
            &packet,
            candidates.clone(),
        )
        .await
        .map_err(|_| SupervisorError::NotAvailable)?;
        self.repository
            .v3()
            .create_artifact(
                CreateArtifact {
                    task_id: task.id.clone(),
                    kind: "cross_model_review_completed".into(),
                    display_name: "read-only-review".into(),
                    content_hash: None,
                    metadata: serde_json::json!({"finding_count":candidates.len()}),
                },
                now_ms(),
            )
            .await
            .map_err(|_| SupervisorError::Storage)?;
        let changed: std::collections::HashSet<_> = packet
            .changed_files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        let findings = self
            .repository
            .v3()
            .list_review_findings(&task.id)
            .await
            .map_err(|_| SupervisorError::Storage)?;
        for finding in findings.iter().filter(|finding| {
            finding.disposition == FindingDisposition::Reported
                && matches!(finding.severity.as_str(), "blocker" | "high")
                && finding
                    .evidence
                    .get("file")
                    .and_then(|value| value.as_str())
                    .is_some_and(|file| changed.contains(file))
        }) {
            self.repository
                .v3()
                .transition_review_finding(finding, FindingDisposition::ConfirmedBlocking, now_ms())
                .await
                .map_err(|_| SupervisorError::Storage)?;
        }
        let confirmed = self
            .repository
            .v3()
            .list_review_findings(&task.id)
            .await
            .map_err(|_| SupervisorError::Storage)?
            .iter()
            .any(|finding| finding.disposition == FindingDisposition::ConfirmedBlocking);
        if confirmed {
            // The durable blocker and review packet remain available. Repair
            // is entered only through the bounded repair coordinator.
            self.repository
                .v3()
                .transition_task(&task, TaskLifecycle::Repairing, now_ms())
                .await
                .map_err(|_| SupervisorError::Storage)?;
            return self.run_repair_stage(task.id).await;
        }
        let profile = self.selected_profile()?;
        let fixed = FixedReviewer(candidates);
        let no_repair = NoRepair;
        FinalApprovalSupervisor::prepare(
            self.repository.clone(),
            task.id.clone(),
            &self.primary_root,
            profile,
            &fixed,
            &no_repair,
            3,
        )
        .await
        .map_err(|_| SupervisorError::NotAvailable)?;
        let current = self
            .repository
            .v3()
            .get_task(&task.id)
            .await
            .map_err(|_| SupervisorError::Storage)?;
        self.repository
            .v3()
            .transition_task(&current, TaskLifecycle::ReadyForHuman, now_ms())
            .await
            .map_err(|_| SupervisorError::Storage)?;
        Ok(())
    }

    async fn reconcile_repaired_findings(
        &self,
        task_id: &TaskId,
        candidates: &[ReviewCandidate],
    ) -> Result<(), SupervisorError> {
        let findings = self
            .repository
            .v3()
            .list_review_findings(task_id)
            .await
            .map_err(|_| SupervisorError::Storage)?;
        for finding in findings.iter().filter(|finding| {
            finding.disposition == FindingDisposition::ConfirmedBlocking
                && finding.repair_round_id.is_some()
        }) {
            let file = finding
                .evidence
                .get("file")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let remains = candidates.iter().any(|candidate| {
                matches!(candidate.severity, FindingSeverity::Blocker)
                    && candidate.summary == finding.summary
                    && candidate.file == file
            });
            self.repository
                .v3()
                .transition_review_finding(
                    finding,
                    if remains {
                        FindingDisposition::Dismissed
                    } else {
                        FindingDisposition::Repaired
                    },
                    now_ms(),
                )
                .await
                .map_err(|_| SupervisorError::Storage)?;
            if let Some(round_id) = &finding.repair_round_id {
                let round = self
                    .repository
                    .v3()
                    .get_repair_round(round_id)
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                if round.lifecycle == RepairRoundLifecycle::AwaitingValidation {
                    self.repository
                        .v3()
                        .transition_repair_round(
                            &round,
                            if remains {
                                RepairRoundLifecycle::Exhausted
                            } else {
                                RepairRoundLifecycle::Resolved
                            },
                            now_ms(),
                        )
                        .await
                        .map_err(|_| SupervisorError::Storage)?;
                }
            }
        }
        Ok(())
    }

    async fn cross_model_review(
        &mut self,
        task: &Task,
        packet: &ReviewPacket,
    ) -> Result<Vec<ReviewCandidate>, SupervisorError> {
        let roles = self.roles_for_task(&task.id).await?;
        let reviewer = roles.reviewer;
        self.provider_registry
            .validate_selection(&reviewer, ProviderRole::Reviewer)
            .map_err(|_| SupervisorError::ProviderUnavailable)?;
        let review_root = self.worktree_root.join(format!("review-{}", task.id));
        std::fs::create_dir_all(&review_root).map_err(|_| SupervisorError::Ownership)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&review_root, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| SupervisorError::Ownership)?;
        }
        let review_root = review_root
            .canonicalize()
            .map_err(|_| SupervisorError::Ownership)?;
        if review_root.starts_with(&self.primary_root) {
            return Err(SupervisorError::Ownership);
        }
        let prompt = review_prompt(packet)?;
        let before = last_event_sequence(&self.repository, &task.id).await?;
        let output = if reviewer.provider_id.as_str() == CLAUDE_PROVIDER_ID {
            let program = self
                .programs
                .claude
                .clone()
                .ok_or(SupervisorError::ProviderUnavailable)?;
            let (mut process, session) = ClaudeProcess::start_read_only(
                program,
                self.repository.clone(),
                task.id.clone(),
                &review_root,
                &prompt,
            )
            .await
            .map_err(|_| SupervisorError::Provider)?;
            activate_session_if_needed(&self.repository, &session.session_id).await?;
            time::timeout(PROVIDER_STAGE_TIMEOUT, process.wait_for_exit())
                .await
                .map_err(|_| SupervisorError::Provider)?
                .map_err(|_| SupervisorError::Provider)?;
            complete_session_if_active(&self.repository, &session.session_id).await?;
            latest_claude_result(&self.repository, &task.id, before).await?
        } else if reviewer.provider_id.as_str() == CODEX_PROVIDER_ID {
            let program = self
                .programs
                .codex
                .clone()
                .ok_or(SupervisorError::ProviderUnavailable)?;
            let mut server = CodexAppServer::start(
                program,
                self.repository.clone(),
                task.id.clone(),
                &review_root,
            )
            .await
            .map_err(|_| SupervisorError::Provider)?;
            let session = server
                .start_thread(&review_root)
                .await
                .map_err(|_| SupervisorError::Provider)?;
            activate_session_if_needed(&self.repository, &session.session_id).await?;
            server
                .start_read_only_turn(&session, &prompt, &review_root)
                .await
                .map_err(|_| SupervisorError::Provider)?;
            wait_for_provider_completion(&self.repository, &task.id, CODEX_PROVIDER, before)
                .await?;
            let output = latest_codex_message(&self.repository, &task.id, before).await?;
            complete_session_if_active(&self.repository, &session.session_id).await?;
            server
                .shutdown()
                .await
                .map_err(|_| SupervisorError::Provider)?;
            output
        } else {
            let session = self
                .repository
                .v3()
                .create_session(
                    CreateSession {
                        task_id: task.id.clone(),
                        provider: reviewer.provider_id.to_string(),
                        provider_session_ref: format!(
                            "sentinel-api-review:{}:{}",
                            reviewer.provider_id, task.id
                        ),
                    },
                    now_ms(),
                )
                .await
                .map_err(|_| SupervisorError::Storage)?;
            activate_session(&self.repository, &session.id).await?;
            let request = ProviderRequest {
                request_id: format!("review-{}", task.id),
                model_id: reviewer.model_id.clone(),
                messages: vec![
                    ProviderMessage {
                        role: MessageRole::System,
                        content: review_system_prompt(),
                        tool_call_id: None,
                        name: None,
                        tool_calls: Vec::new(),
                    },
                    ProviderMessage {
                        role: MessageRole::User,
                        content: prompt,
                        tool_call_id: None,
                        name: None,
                        tool_calls: Vec::new(),
                    },
                ],
                // JSON is required by the review contract, but using text mode
                // keeps providers without native structured-output support
                // eligible; Sentinel still strictly parses the response.
                tools: Vec::new(),
                response_format: ResponseFormat::Text,
                max_output_tokens: None,
                timeout_ms: PROVIDER_STAGE_TIMEOUT.as_millis() as u64,
            };
            let completion = async {
                let run = self
                    .provider_registry
                    .start(&reviewer.provider_id, request, Arc::new(|_| {}))
                    .await
                    .map_err(|_| SupervisorError::Provider)?;
                time::timeout(PROVIDER_STAGE_TIMEOUT, run.completion())
                    .await
                    .map_err(|_| SupervisorError::Provider)?
                    .map_err(|_| SupervisorError::Provider)
            }
            .await;
            match completion {
                Ok(completion) => {
                    complete_session_if_active(&self.repository, &session.id).await?;
                    completion.text
                }
                Err(error) => {
                    let durable = self
                        .repository
                        .v3()
                        .get_session(&session.id)
                        .await
                        .map_err(|_| SupervisorError::Storage)?;
                    if durable.lifecycle == SessionLifecycle::Active {
                        self.repository
                            .v3()
                            .transition_session(&durable, SessionLifecycle::Failed, now_ms())
                            .await
                            .map_err(|_| SupervisorError::Storage)?;
                    }
                    return Err(error);
                }
            }
        };
        parse_review_output(&output)
    }

    async fn roles_for_task(
        &self,
        task_id: &TaskId,
    ) -> Result<WorkflowProviderConfiguration, SupervisorError> {
        let artifacts = self
            .repository
            .v3()
            .list_artifacts(task_id)
            .await
            .map_err(|_| SupervisorError::Storage)?;
        artifacts
            .into_iter()
            .rev()
            .find(|artifact| artifact.kind == "provider_role_configuration")
            .map(|artifact| {
                serde_json::from_value(artifact.metadata).map_err(|_| SupervisorError::Storage)
            })
            .unwrap_or_else(|| Ok(self.provider_workflow.clone()))
    }

    async fn run_repair_stage(&mut self, task_id: TaskId) -> Result<(), SupervisorError> {
        WorktreeTransaction::reopen(self.repository.clone(), task_id.clone(), &self.primary_root)
            .await
            .map_err(map_worktree)?;
        let rounds = self
            .repository
            .v3()
            .list_repair_rounds(&task_id)
            .await
            .map_err(|_| SupervisorError::Storage)?;
        if rounds.len() >= MAX_REPAIR_ROUNDS {
            let task = self
                .repository
                .v3()
                .get_task(&task_id)
                .await
                .map_err(|_| SupervisorError::Storage)?;
            self.repository
                .v3()
                .transition_task(&task, TaskLifecycle::Blocked, now_ms())
                .await
                .map_err(|_| SupervisorError::Storage)?;
            return Ok(());
        }
        let finding = self
            .repository
            .v3()
            .list_review_findings(&task_id)
            .await
            .map_err(|_| SupervisorError::Storage)?
            .into_iter()
            .find(|finding| {
                finding.disposition == FindingDisposition::ConfirmedBlocking
                    && finding.repair_round_id.is_none()
                    && matches!(finding.severity.as_str(), "blocker" | "high")
            })
            .ok_or(SupervisorError::NotAvailable)?;
        let repair_finding = repair_finding_from_durable(&finding)?;
        let round = self
            .repository
            .v3()
            .create_repair_round(
                CreateRepairRound {
                    task_id: task_id.clone(),
                    round_number: rounds.len() as u32 + 1,
                },
                now_ms(),
            )
            .await
            .map_err(|_| SupervisorError::Storage)?;
        let active = self
            .repository
            .v3()
            .transition_repair_round(&round, RepairRoundLifecycle::Active, now_ms())
            .await
            .map_err(|_| SupervisorError::Storage)?;
        self.repository
            .v3()
            .assign_review_finding_to_repair_round(&finding, &active, now_ms())
            .await
            .map_err(|_| SupervisorError::Storage)?;
        let packet = RepairPacket {
            repair_round_id: active.id.to_string(),
            task_id: task_id.to_string(),
            findings: vec![repair_finding],
            validations: validation_facts(&self.repository, &task_id).await?,
            implementer_session_id: original_implementer_session(&self.repository, &task_id)
                .await?
                .map(|session| session.id.to_string()),
        };
        self.repository
            .v3()
            .create_artifact(
                CreateArtifact {
                    task_id: task_id.clone(),
                    kind: "repair_handoff".into(),
                    display_name: format!("repair-round-{}", active.round_number),
                    content_hash: None,
                    metadata: serde_json::to_value(&packet)
                        .map_err(|_| SupervisorError::Storage)?,
                },
                now_ms(),
            )
            .await
            .map_err(|_| SupervisorError::Storage)?;
        self.send_repair(&task_id, &packet).await?;
        self.repository
            .v3()
            .transition_repair_round(&active, RepairRoundLifecycle::AwaitingValidation, now_ms())
            .await
            .map_err(|_| SupervisorError::Storage)?;
        let task = self
            .repository
            .v3()
            .get_task(&task_id)
            .await
            .map_err(|_| SupervisorError::Storage)?;
        self.repository
            .v3()
            .transition_task(&task, TaskLifecycle::Validating, now_ms())
            .await
            .map_err(|_| SupervisorError::Storage)?;
        Ok(())
    }

    async fn send_repair(
        &mut self,
        task_id: &TaskId,
        packet: &RepairPacket,
    ) -> Result<(), SupervisorError> {
        let prompt = repair_prompt(packet)?;
        let roles = self.roles_for_task(task_id).await?;
        let selection = roles.repair_selection().clone();
        self.provider_registry
            .validate_selection(&selection, ProviderRole::Repair)
            .map_err(|_| SupervisorError::ProviderUnavailable)?;
        let before = last_event_sequence(&self.repository, task_id).await?;
        if selection.provider_id.as_str() == CODEX_PROVIDER_ID {
            if let Some(OwnedProvider::Codex(started)) = self.owned.get_mut(task_id) {
                started
                    .start_turn(&prompt)
                    .await
                    .map_err(|_| SupervisorError::Provider)?;
                return wait_for_provider_completion(
                    &self.repository,
                    task_id,
                    CODEX_PROVIDER,
                    before,
                )
                .await;
            }
            let program = self
                .programs
                .codex
                .clone()
                .ok_or(SupervisorError::ProviderUnavailable)?;
            let worktree = self
                .repository
                .v3()
                .get_task_worktree(task_id)
                .await
                .map_err(|_| SupervisorError::Storage)?;
            let cwd = Path::new(&worktree.worktree_path);
            let mut server =
                CodexAppServer::start(program, self.repository.clone(), task_id.clone(), cwd)
                    .await
                    .map_err(|_| SupervisorError::Provider)?;
            let session = server
                .start_thread(cwd)
                .await
                .map_err(|_| SupervisorError::Provider)?;
            activate_session_if_needed(&self.repository, &session.session_id).await?;
            server
                .start_turn(&session, &prompt)
                .await
                .map_err(|_| SupervisorError::Provider)?;
            wait_for_provider_completion(&self.repository, task_id, CODEX_PROVIDER, before).await?;
            complete_session_if_active(&self.repository, &session.session_id).await?;
            return server
                .shutdown()
                .await
                .map_err(|_| SupervisorError::Provider);
        }
        if selection.provider_id.as_str() == CLAUDE_PROVIDER_ID {
            if let Some(OwnedProvider::Claude { process, session }) = self.owned.get_mut(task_id) {
                let _ = time::timeout(PROVIDER_STAGE_TIMEOUT, process.wait_for_exit()).await;
                let program = self
                    .programs
                    .claude
                    .clone()
                    .ok_or(SupervisorError::ProviderUnavailable)?;
                let worktree = self
                    .repository
                    .v3()
                    .get_task_worktree(task_id)
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                let (mut resumed, durable_session) = ClaudeProcess::resume(
                    program,
                    self.repository.clone(),
                    task_id.clone(),
                    Path::new(&worktree.worktree_path),
                    &session.provider_session_id,
                    &prompt,
                )
                .await
                .map_err(|_| SupervisorError::Provider)?;
                time::timeout(PROVIDER_STAGE_TIMEOUT, resumed.wait_for_exit())
                    .await
                    .map_err(|_| SupervisorError::Provider)?
                    .map_err(|_| SupervisorError::Provider)?;
                *process = resumed;
                *session = durable_session;
                return Ok(());
            }
            let program = self
                .programs
                .claude
                .clone()
                .ok_or(SupervisorError::ProviderUnavailable)?;
            let worktree = self
                .repository
                .v3()
                .get_task_worktree(task_id)
                .await
                .map_err(|_| SupervisorError::Storage)?;
            let (mut process, session) = ClaudeProcess::start(
                program,
                self.repository.clone(),
                task_id.clone(),
                Path::new(&worktree.worktree_path),
                &prompt,
            )
            .await
            .map_err(|_| SupervisorError::Provider)?;
            activate_session_if_needed(&self.repository, &session.session_id).await?;
            time::timeout(PROVIDER_STAGE_TIMEOUT, process.wait_for_exit())
                .await
                .map_err(|_| SupervisorError::Provider)?
                .map_err(|_| SupervisorError::Provider)?;
            complete_session_if_active(&self.repository, &session.session_id).await?;
            return Ok(());
        }

        let worktree = self
            .repository
            .v3()
            .get_task_worktree(task_id)
            .await
            .map_err(|_| SupervisorError::Storage)?;
        let session = self
            .repository
            .v3()
            .create_session(
                CreateSession {
                    task_id: task_id.clone(),
                    provider: selection.provider_id.to_string(),
                    provider_session_ref: format!(
                        "sentinel-api-repair:{}:{}",
                        selection.provider_id, task_id
                    ),
                },
                now_ms(),
            )
            .await
            .map_err(|_| SupervisorError::Storage)?;
        activate_session(&self.repository, &session.id).await?;
        let tools = Arc::new(
            api_tools::OwnedWorktreeTools::new(
                Path::new(&worktree.worktree_path),
                &self.primary_root,
            )
            .map_err(|_| SupervisorError::Ownership)?,
        );
        let result = time::timeout(
            PROVIDER_STAGE_TIMEOUT,
            run_agent_loop(
                &self.provider_registry,
                AgentLoopRequest {
                    request_id: format!("repair-{}", task_id),
                    selection,
                    role: ProviderRole::Repair,
                    system_prompt: implementation_system_prompt(),
                    user_prompt: prompt,
                    response_format: ResponseFormat::Text,
                    max_output_tokens: None,
                    timeout_ms: PROVIDER_STAGE_TIMEOUT.as_millis() as u64,
                    max_rounds: 8,
                },
                tools,
                Arc::new(|_| {}),
                CancellationHandle::default(),
            ),
        )
        .await
        .map_err(|_| SupervisorError::Provider)?
        .map_err(|_| SupervisorError::Provider)?;
        complete_session_if_active(&self.repository, &session.id).await?;
        self.repository
            .v3()
            .create_artifact(
                CreateArtifact {
                    task_id: task_id.clone(),
                    kind: "repair_provider_result".into(),
                    display_name: result.provider_id.to_string(),
                    content_hash: None,
                    metadata: serde_json::json!({
                        "provider_id":result.provider_id,
                        "model_id":result.model_id,
                        "rounds":result.rounds,
                        "tool_calls":result.tool_calls,
                    }),
                },
                now_ms(),
            )
            .await
            .map_err(|_| SupervisorError::Storage)?;
        Ok(())
    }

    /// Fails unfinished tasks closed on restart. A worktree must reconcile
    /// before provider inspection is attempted. Provider resume is deliberately
    /// not inferred from process disappearance or persisted text.
    pub async fn enter_recovery_after_restart(&mut self) -> Result<(), SupervisorError> {
        if self.reconciled_startup {
            return Ok(());
        }
        self.reconciled_startup = true;
        let tasks = self
            .repository
            .v3()
            .list_tasks()
            .await
            .map_err(|_| SupervisorError::Storage)?
            .into_iter()
            .filter(|task| !task.lifecycle.terminal() && task.lifecycle != TaskLifecycle::Draft)
            .collect::<Vec<_>>();
        for task in tasks {
            let previous_lifecycle = task.lifecycle;
            let belongs_to_repository = self
                .repository
                .v3()
                .get_task_worktree(&task.id)
                .await
                .is_ok_and(|worktree| Path::new(&worktree.repository_root) == self.primary_root);
            if !belongs_to_repository {
                continue;
            }
            let task = self
                .repository
                .v3()
                .restore_unfinished_task(&task.id, now_ms())
                .await
                .map_err(|_| SupervisorError::Storage)?;
            let worktree = match WorktreeTransaction::reopen(
                self.repository.clone(),
                task.id.clone(),
                &self.primary_root,
            )
            .await
            {
                Ok(worktree) => worktree,
                Err(_) => {
                    self.persist_reconciliation(&task.id, "worktree", "recovery_required")
                        .await?;
                    continue;
                }
            };
            if previous_lifecycle == TaskLifecycle::Validating {
                self.repository
                    .v3()
                    .recover_interrupted_validations_for_task(&task.id, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                self.persist_reconciliation(&task.id, "validation", "interrupted")
                    .await?;
                continue;
            }
            if previous_lifecycle == TaskLifecycle::ReadyForHuman {
                let packet_exists = self
                    .repository
                    .v3()
                    .list_artifacts(&task.id)
                    .await
                    .map_err(|_| SupervisorError::Storage)?
                    .iter()
                    .any(|artifact| artifact.kind == "final_approval_packet");
                let approval_exists = self
                    .repository
                    .v3()
                    .list_approvals_for_task(&task.id)
                    .await
                    .map_err(|_| SupervisorError::Storage)?
                    .iter()
                    .any(|approval| approval.action_kind == "v3_final_git_action");
                if packet_exists && approval_exists {
                    self.repository
                        .v3()
                        .transition_task(&task, TaskLifecycle::ReadyForHuman, now_ms())
                        .await
                        .map_err(|_| SupervisorError::Storage)?;
                    self.persist_reconciliation(&task.id, "final_approval", "restored")
                        .await?;
                } else {
                    self.persist_reconciliation(&task.id, "final_approval", "missing_evidence")
                        .await?;
                }
                continue;
            }
            let sessions = self
                .repository
                .v3()
                .list_sessions_for_task(&task.id)
                .await
                .map_err(|_| SupervisorError::Storage)?;
            if sessions.len() != 1 {
                self.persist_reconciliation(&task.id, "provider", "missing_or_ambiguous")
                    .await?;
                continue;
            }
            match sessions[0].provider.as_str() {
                CODEX_PROVIDER => {
                    let starter =
                        CodexTaskStarter::new(self.programs.codex.clone(), self.repository.clone());
                    match starter
                        .reconcile_recovering_task(task.clone(), Path::new(&worktree.worktree_path))
                        .await
                    {
                        Ok(CodexTaskReconciliation::Resumed(started)) => {
                            self.owned
                                .insert(task.id.clone(), OwnedProvider::Codex(started));
                            self.persist_reconciliation(&task.id, "codex", "resumed")
                                .await?;
                        }
                        Ok(CodexTaskReconciliation::Inactive) => {
                            self.persist_reconciliation(&task.id, "codex", "inactive")
                                .await?;
                        }
                        Ok(CodexTaskReconciliation::Missing) => {
                            self.persist_reconciliation(&task.id, "codex", "missing")
                                .await?;
                        }
                        Ok(CodexTaskReconciliation::Ambiguous) | Err(_) => {
                            self.persist_reconciliation(&task.id, "codex", "provider_error")
                                .await?;
                        }
                    }
                }
                CLAUDE_PROVIDER => {
                    // A CLI process from the previous app lifetime is not an
                    // owned live child. Persist the uncertainty and keep the
                    // task recovery-required; never infer a successful resume.
                    self.persist_reconciliation(&task.id, "claude_code", "unproven")
                        .await?;
                }
                _ => {
                    self.persist_reconciliation(&task.id, "provider", "unsupported")
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn persist_reconciliation(
        &self,
        task_id: &TaskId,
        provider: &str,
        outcome: &str,
    ) -> Result<(), SupervisorError> {
        self.repository
            .v3()
            .create_artifact(
                CreateArtifact {
                    task_id: task_id.clone(),
                    kind: "provider_reconciliation".into(),
                    display_name: provider.into(),
                    content_hash: None,
                    metadata: serde_json::json!({"provider":provider,"outcome":outcome}),
                },
                now_ms(),
            )
            .await
            .map(|_| ())
            .map_err(|_| SupervisorError::Storage)
    }
}

async fn persist_api_run_outcome(
    repository: &RunRepository,
    task_id: &TaskId,
    session_id: &sentinel_core::v3::SessionId,
    provider_id: &ProviderId,
    outcome: &Result<AgentLoopResult, ProviderError>,
) -> Result<(), SupervisorError> {
    let session = repository
        .v3()
        .get_session(session_id)
        .await
        .map_err(|_| SupervisorError::Storage)?;
    match outcome {
        Ok(result) => {
            if session.lifecycle == SessionLifecycle::Active {
                repository
                    .v3()
                    .transition_session(&session, SessionLifecycle::Completed, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
            }
            repository
                .v3()
                .append_event(&NormalizedEventEnvelope {
                    event_id: EventId::new(),
                    task_id: task_id.clone(),
                    session_id: Some(session_id.clone()),
                    provider: provider_id.to_string(),
                    kind: EventKind::ToolCompleted,
                    schema_version: 1,
                    occurred_at_ms: now_ms(),
                    sequence_number: last_event_sequence(repository, task_id)
                        .await?
                        .saturating_add(1),
                    causation_id: None,
                    correlation_id: None,
                    payload: serde_json::json!({
                        "api_agent":"completed",
                        "model_id":result.model_id,
                        "rounds":result.rounds,
                        "tool_calls":result.tool_calls,
                    }),
                    raw_diagnostic_payload: None,
                })
                .await
                .map_err(|_| SupervisorError::Storage)?;
        }
        Err(error) => {
            if matches!(error, ProviderError::Cancelled)
                && session.lifecycle == SessionLifecycle::Cancelling
            {
                repository
                    .v3()
                    .transition_session(&session, SessionLifecycle::Cancelled, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
                return Ok(());
            }
            if session.lifecycle == SessionLifecycle::Active {
                repository
                    .v3()
                    .transition_session(&session, SessionLifecycle::Failed, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
            }
            repository
                .v3()
                .create_artifact(
                    CreateArtifact {
                        task_id: task_id.clone(),
                        kind: "api_provider_failure".into(),
                        display_name: provider_id.to_string(),
                        content_hash: None,
                        metadata: serde_json::json!({"error":redact(&error.to_string())}),
                    },
                    now_ms(),
                )
                .await
                .map_err(|_| SupervisorError::Storage)?;
            let task = repository
                .v3()
                .get_task(task_id)
                .await
                .map_err(|_| SupervisorError::Storage)?;
            if !task.lifecycle.terminal() {
                repository
                    .v3()
                    .transition_task(&task, TaskLifecycle::Blocked, now_ms())
                    .await
                    .map_err(|_| SupervisorError::Storage)?;
            }
        }
    }
    Ok(())
}

async fn activate_session(
    repository: &RunRepository,
    session_id: &sentinel_core::v3::SessionId,
) -> Result<(), SupervisorError> {
    let durable = repository
        .v3()
        .get_session(session_id)
        .await
        .map_err(|_| SupervisorError::Storage)?;
    let starting = repository
        .v3()
        .transition_session(&durable, SessionLifecycle::Starting, now_ms())
        .await
        .map_err(|_| SupervisorError::Storage)?;
    repository
        .v3()
        .transition_session(&starting, SessionLifecycle::Active, now_ms())
        .await
        .map_err(|_| SupervisorError::Storage)?;
    Ok(())
}

async fn activate_session_if_needed(
    repository: &RunRepository,
    session_id: &sentinel_core::v3::SessionId,
) -> Result<(), SupervisorError> {
    let session = repository
        .v3()
        .get_session(session_id)
        .await
        .map_err(|_| SupervisorError::Storage)?;
    if session.lifecycle == SessionLifecycle::Created {
        activate_session(repository, session_id).await?;
    }
    Ok(())
}

async fn complete_session_if_active(
    repository: &RunRepository,
    session_id: &sentinel_core::v3::SessionId,
) -> Result<(), SupervisorError> {
    let session = repository
        .v3()
        .get_session(session_id)
        .await
        .map_err(|_| SupervisorError::Storage)?;
    if session.lifecycle == SessionLifecycle::Active {
        repository
            .v3()
            .transition_session(&session, SessionLifecycle::Completed, now_ms())
            .await
            .map_err(|_| SupervisorError::Storage)?;
    }
    Ok(())
}

fn provider_turn_completed(event: &NormalizedEventEnvelope) -> bool {
    match event.provider.as_str() {
        CODEX_PROVIDER => {
            event.kind == EventKind::ToolCompleted
                && event
                    .payload
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    == Some("turn/completed")
        }
        CLAUDE_PROVIDER => {
            event.kind == EventKind::ToolCompleted
                && event
                    .payload
                    .get("claude_event")
                    .and_then(|value| value.get("type"))
                    .and_then(serde_json::Value::as_str)
                    == Some("result")
        }
        _ => {
            event.kind == EventKind::ToolCompleted
                && event
                    .payload
                    .get("api_agent")
                    .and_then(serde_json::Value::as_str)
                    == Some("completed")
        }
    }
}

async fn last_event_sequence(
    repository: &RunRepository,
    task_id: &TaskId,
) -> Result<u64, SupervisorError> {
    repository
        .v3()
        .list_events(task_id)
        .await
        .map_err(|_| SupervisorError::Storage)
        .map(|events| {
            events
                .last()
                .map(|event| event.sequence_number)
                .unwrap_or_default()
        })
}

async fn wait_for_provider_completion(
    repository: &RunRepository,
    task_id: &TaskId,
    provider: &str,
    after_sequence: u64,
) -> Result<(), SupervisorError> {
    let mut updates = repository.subscribe_v3_changes();
    time::timeout(PROVIDER_STAGE_TIMEOUT, async {
        loop {
            let completed = repository
                .v3()
                .list_events(task_id)
                .await
                .map_err(|_| SupervisorError::Storage)?
                .iter()
                .any(|event| {
                    event.sequence_number > after_sequence
                        && event.provider == provider
                        && provider_turn_completed(event)
                });
            if completed {
                return Ok(());
            }
            updates
                .recv()
                .await
                .map_err(|_| SupervisorError::Provider)?;
        }
    })
    .await
    .map_err(|_| SupervisorError::Provider)?
}

async fn latest_claude_result(
    repository: &RunRepository,
    task_id: &TaskId,
    after_sequence: u64,
) -> Result<String, SupervisorError> {
    repository
        .v3()
        .list_events(task_id)
        .await
        .map_err(|_| SupervisorError::Storage)?
        .into_iter()
        .rev()
        .find(|event| {
            event.sequence_number > after_sequence
                && event.provider == CLAUDE_PROVIDER
                && provider_turn_completed(event)
        })
        .and_then(|event| {
            event
                .payload
                .get("claude_event")
                .and_then(|value| value.get("result"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .ok_or(SupervisorError::Provider)
}

async fn latest_codex_message(
    repository: &RunRepository,
    task_id: &TaskId,
    after_sequence: u64,
) -> Result<String, SupervisorError> {
    for event in repository
        .v3()
        .list_events(task_id)
        .await
        .map_err(|_| SupervisorError::Storage)?
        .into_iter()
        .rev()
        .filter(|event| event.sequence_number > after_sequence && event.provider == CODEX_PROVIDER)
    {
        if event
            .payload
            .get("method")
            .and_then(serde_json::Value::as_str)
            != Some("item/completed")
        {
            continue;
        }
        let item = event
            .payload
            .get("params")
            .and_then(|params| params.get("item"));
        if let Some(text) = item
            .and_then(|item| item.get("text"))
            .and_then(serde_json::Value::as_str)
        {
            return Ok(text.to_owned());
        }
        if let Some(text) = item
            .and_then(|item| item.get("content"))
            .and_then(serde_json::Value::as_array)
            .and_then(|content| {
                content
                    .iter()
                    .find_map(|part| part.get("text").and_then(serde_json::Value::as_str))
            })
        {
            return Ok(text.to_owned());
        }
    }
    Err(SupervisorError::Provider)
}

#[derive(Deserialize)]
struct ReviewerOutput {
    findings: Vec<ReviewerFinding>,
}

#[derive(Deserialize)]
struct ReviewerFinding {
    severity: String,
    summary: String,
    file: String,
    line: u32,
    evidence: String,
    #[serde(default)]
    validation_check: Option<String>,
    #[serde(default)]
    validation_outcome: Option<String>,
}

fn parse_review_output(output: &str) -> Result<Vec<ReviewCandidate>, SupervisorError> {
    if output.len() > 64 * 1024 {
        return Err(SupervisorError::Provider);
    }
    let parsed: ReviewerOutput =
        serde_json::from_str(output.trim()).map_err(|_| SupervisorError::Provider)?;
    if parsed.findings.len() > 32 {
        return Err(SupervisorError::Provider);
    }
    parsed
        .findings
        .into_iter()
        .map(|finding| {
            let severity = match finding.severity.as_str() {
                "blocker" | "high" => FindingSeverity::Blocker,
                "warning" => FindingSeverity::Warning,
                "suggestion" => FindingSeverity::Suggestion,
                _ => return Err(SupervisorError::Provider),
            };
            Ok(ReviewCandidate {
                severity,
                summary: finding.summary,
                file: finding.file,
                line: finding.line,
                evidence: finding.evidence,
                validation_check: finding.validation_check,
                validation_outcome: finding.validation_outcome,
            })
        })
        .collect()
}

fn review_prompt(packet: &ReviewPacket) -> Result<String, SupervisorError> {
    let facts = serde_json::to_string(packet).map_err(|_| SupervisorError::Storage)?;
    let bounded = facts.chars().take(6_500).collect::<String>();
    Ok(redact(&format!(
        "Read-only cross-model review. Return ONLY JSON as {{\"findings\":[{{\"severity\":\"blocker|warning|suggestion\",\"summary\":\"...\",\"file\":\"relative/path\",\"line\":1,\"evidence\":\"...\",\"validation_check\":null,\"validation_outcome\":null}}]}}. Findings require concrete evidence. Deterministic validation facts override unsupported claims. Task evidence: {bounded}"
    )))
}

fn repair_prompt(packet: &RepairPacket) -> Result<String, SupervisorError> {
    let facts = serde_json::to_string(packet).map_err(|_| SupervisorError::Storage)?;
    Ok(redact(&format!(
        "Repair only the confirmed findings in this Sentinel repair packet. Keep changes minimal and inside the current task worktree. Do not claim completion; Sentinel will validate and re-review. Packet: {}",
        facts.chars().take(7_000).collect::<String>()
    )))
}

fn implementation_system_prompt() -> String {
    "You are the implementation worker for one Sentinel-owned Git worktree. Use only the provided bounded file tools. Inspect before editing, keep changes scoped to the user's task, and do not claim validation, approval, merge, or task completion. Sentinel remains the workflow authority.".into()
}

fn review_system_prompt() -> String {
    "You are a read-only reviewer. You have no filesystem or execution tools. Evaluate only the supplied task diff and deterministic validation evidence. Return the exact requested JSON shape; never claim approval or task completion.".into()
}

fn managed_selection(provider: &str) -> ProviderSelection {
    ProviderSelection {
        provider_id: ProviderId::new(provider).expect("managed provider ID is constant"),
        model_id: "cli-owned".into(),
    }
}

fn provider_alias(provider: &str) -> &str {
    match provider {
        "claude" | "claude-code" => CLAUDE_PROVIDER_ID,
        value => value,
    }
}

fn repair_finding_from_durable(
    finding: &sentinel_core::v3::ReviewFinding,
) -> Result<RepairFinding, SupervisorError> {
    let file = finding
        .evidence
        .get("file")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty() && !value.starts_with('/') && !value.contains(".."))
        .ok_or(SupervisorError::NotAvailable)?;
    let line = finding
        .evidence
        .get("line")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or(SupervisorError::NotAvailable)?;
    let evidence = finding
        .evidence
        .get("evidence")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(SupervisorError::NotAvailable)?;
    Ok(RepairFinding {
        id: finding.id.to_string(),
        summary: finding.summary.clone(),
        file: file.into(),
        line,
        evidence: evidence.into(),
    })
}

async fn validation_facts(
    repository: &RunRepository,
    task_id: &TaskId,
) -> Result<Vec<ReviewValidation>, SupervisorError> {
    let mut facts = Vec::new();
    for result in repository
        .v3()
        .list_validation_results(task_id)
        .await
        .map_err(|_| SupervisorError::Storage)?
    {
        facts.push(ReviewValidation {
            check_name: result.check_name,
            lifecycle: result.lifecycle,
            outcome: repository
                .v3()
                .get_validation_execution(&result.id)
                .await
                .ok()
                .map(|execution| execution.outcome),
        });
    }
    Ok(facts)
}

async fn original_implementer_session(
    repository: &RunRepository,
    task_id: &TaskId,
) -> Result<Option<sentinel_core::v3::AgentSession>, SupervisorError> {
    let task = repository
        .v3()
        .get_task(task_id)
        .await
        .map_err(|_| SupervisorError::Storage)?;
    let provider = match task.workflow_id.strip_prefix("v3:") {
        Some(CODEX_PROVIDER_ID) => CODEX_PROVIDER,
        Some(CLAUDE_PROVIDER_ID) => CLAUDE_PROVIDER,
        Some(provider) => provider,
        None if task.workflow_id.contains(CODEX_PROVIDER) => CODEX_PROVIDER,
        None if task.workflow_id.contains(CLAUDE_PROVIDER) => CLAUDE_PROVIDER,
        None => return Ok(None),
    };
    repository
        .v3()
        .list_sessions_for_task(task_id)
        .await
        .map_err(|_| SupervisorError::Storage)
        .map(|sessions| {
            sessions
                .into_iter()
                .find(|session| session.provider == provider)
        })
}

#[derive(Clone)]
struct FixedReviewer(Vec<ReviewCandidate>);

impl Reviewer for FixedReviewer {
    fn review(&self, _packet: &ReviewPacket) -> Result<Vec<ReviewCandidate>, ReviewError> {
        Ok(self.0.clone())
    }
}

struct NoRepair;

impl RepairImplementer for NoRepair {
    fn repair(
        &self,
        _session: Option<&sentinel_core::v3::AgentSession>,
        _packet: &RepairPacket,
    ) -> Result<RepairEvidence, RepairError> {
        Err(RepairError::NoEligibleFindings)
    }
}

async fn fail_task(repository: &RunRepository, task: &Task) {
    if let Ok(current) = repository.v3().get_task(&task.id).await {
        if !current.lifecycle.terminal() {
            let _ = repository
                .v3()
                .transition_task(&current, TaskLifecycle::Failed, now_ms())
                .await;
        }
    }
}

fn map_worktree(error: TransactionError) -> SupervisorError {
    match error {
        TransactionError::RecoveryRequired => SupervisorError::RecoveryRequired,
        TransactionError::Invalid | TransactionError::Conflict => SupervisorError::Ownership,
        TransactionError::Storage => SupervisorError::Storage,
        _ => SupervisorError::Ownership,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::v3::{CreateApproval, CreateTask};
    use sentinel_provider_api::{
        CredentialReference, CredentialStore, ModelLimits, ProviderAdapter, ProviderCapabilities,
        ProviderCompletion, ProviderConfig, ProviderFuture, ProviderRun, ProviderTransport,
        SecretString, TokenUsage, ToolCall,
    };
    use std::{collections::VecDeque, fs, process::Command, sync::Mutex};
    use tempfile::TempDir;

    struct Fixture {
        _temp: TempDir,
        main: PathBuf,
        worktrees: PathBuf,
        repository: RunRepository,
        codex: PathBuf,
        claude: PathBuf,
        cwd_evidence: PathBuf,
    }

    struct ToolCallingAdapter {
        config: ProviderConfig,
        completions: Mutex<VecDeque<ProviderCompletion>>,
    }

    impl ProviderAdapter for ToolCallingAdapter {
        fn config(&self) -> &ProviderConfig {
            &self.config
        }

        fn start<'a>(
            &'a self,
            _request: ProviderRequest,
            _credential: SecretString,
            _events: Arc<dyn sentinel_provider_api::ProviderEventSink>,
        ) -> ProviderFuture<'a, ProviderRun> {
            let completion = self.completions.lock().unwrap().pop_front().unwrap();
            Box::pin(async move {
                Ok(ProviderRun::new(
                    CancellationHandle::default(),
                    tokio::spawn(async move { Ok(completion) }),
                ))
            })
        }
    }

    async fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("main");
        let worktrees = temp.path().join("worktrees");
        fs::create_dir_all(&main).unwrap();
        fs::create_dir_all(&worktrees).unwrap();
        git(&main, &["init", "-q"]);
        git(&main, &["config", "user.email", "sentinel@example.invalid"]);
        git(&main, &["config", "user.name", "Sentinel Test"]);
        fs::write(main.join("tracked.txt"), "primary\n").unwrap();
        git(&main, &["add", "tracked.txt"]);
        git(&main, &["commit", "-qm", "base"]);
        let repository = RunRepository::open(&format!(
            "sqlite://{}",
            temp.path().join("state.sqlite3").display()
        ))
        .await
        .unwrap();
        let cwd_evidence = temp.path().join("provider-cwd.txt");
        let codex = temp.path().join("fake-codex.py");
        let script = format!(
            r#"#!/usr/bin/python3
import json, os, sys
evidence = {evidence:?}
cwd = ""
for raw in sys.stdin:
    value = json.loads(raw)
    if "id" not in value:
        continue
    method = value.get("method", "")
    notify = None
    if method == "initialize": result = {{"protocolVersion":"1"}}
    elif method == "account/rateLimits/read": result = {{}}
    elif method == "thread/start":
        cwd = value.get("params", {{}}).get("cwd", "")
        open(evidence, "w").write(cwd)
        result = {{"thread":{{"id":"thread-test"}}}}
    elif method == "thread/read": result = {{"thread":{{"id":"thread-test","status":{{"type":"active"}},"turns":[{{"id":"turn-test","status":"inProgress"}}]}}}}
    elif method == "thread/resume": result = {{"thread":{{"id":"thread-test"}}}}
    elif method == "turn/start":
        if cwd: open(os.path.join(cwd, "implemented.txt"), "w").write("implemented\n")
        result = {{"turn":{{"id":"turn-test"}}}}
        notify = {{"jsonrpc":"2.0","method":"turn/completed","params":{{"threadId":"thread-test","turn":{{"id":"turn-test"}}}}}}
    elif method == "turn/interrupt": result = {{}}
    else: result = {{}}
    print(json.dumps({{"jsonrpc":"2.0","id":value["id"],"result":result}}), flush=True)
    if notify: print(json.dumps(notify), flush=True)
"#,
            evidence = cwd_evidence.to_string_lossy().to_string()
        );
        fs::write(&codex, script).unwrap();
        let claude = temp.path().join("fake-claude.py");
        fs::write(
            &claude,
            r#"#!/usr/bin/python3
import json
print(json.dumps({"type":"system","subtype":"init","session_id":"claude-review"}), flush=True)
print(json.dumps({"type":"result","session_id":"claude-review","result":"{\"findings\":[]}"}), flush=True)
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&codex, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(&claude, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Fixture {
            _temp: temp,
            main,
            worktrees,
            repository,
            codex,
            claude,
            cwd_evidence,
        }
    }

    fn git(root: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn task_start_creates_owned_worktree_before_provider_and_preserves_primary() {
        let fixture = fixture().await;
        let program = CodexProgram::from_executable(&fixture.codex).unwrap();
        let mut supervisor = SentinelSupervisor::new(
            fixture.repository.clone(),
            &fixture.main,
            &fixture.worktrees,
            SupervisorPrograms {
                codex: Some(program),
                claude: None,
            },
        )
        .unwrap();
        let task = supervisor
            .start_task(StartTaskIntent {
                provider: ProviderKind::Codex,
                summary: "change the task tree".into(),
                prompt: "make a change".into(),
            })
            .await
            .unwrap();
        let worktree = fixture
            .repository
            .v3()
            .get_task_worktree(&task.id)
            .await
            .unwrap();
        assert_ne!(Path::new(&worktree.worktree_path), fixture.main);
        assert!(Path::new(&worktree.worktree_path)
            .starts_with(fixture.worktrees.canonicalize().unwrap()));
        assert_eq!(
            fs::read_to_string(&fixture.cwd_evidence).unwrap(),
            worktree.worktree_path
        );
        assert_eq!(
            fs::read_to_string(fixture.main.join("tracked.txt")).unwrap(),
            "primary\n"
        );
        assert_eq!(task.lifecycle, TaskLifecycle::Implementing);
        supervisor.stop(&task.id).await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_api_implementer_edits_only_the_owned_worktree_and_pins_roles() {
        let fixture = fixture().await;
        let provider_id = ProviderId::new("fake_api").unwrap();
        let credential = CredentialReference::for_provider(&provider_id);
        let credentials = Arc::new(MemoryCredentialStore::default());
        credentials
            .set(&credential, SecretString::new("test-key").unwrap())
            .unwrap();
        let completion = |text: &str, tool_calls: Vec<ToolCall>| ProviderCompletion {
            provider_id: provider_id.clone(),
            request_id: "fake-request".into(),
            model_id: "fake-model".into(),
            text: text.into(),
            reasoning: None,
            tool_calls,
            finish_reason: Some("stop".into()),
            usage: TokenUsage::default(),
            rate_limits: None,
        };
        let adapter = Arc::new(ToolCallingAdapter {
            config: ProviderConfig {
                id: provider_id.clone(),
                display_name: "Fake API".into(),
                model_id: "fake-model".into(),
                base_url: Some("https://example.invalid/v1".into()),
                enabled: true,
                transport: ProviderTransport::Api,
                capabilities: ProviderCapabilities {
                    streaming: true,
                    cancellation: true,
                    tool_calling: true,
                    structured_output: true,
                    model_selection: true,
                    rate_limits: false,
                    implementation: true,
                    read_only_review: true,
                    repair: true,
                },
                limits: ModelLimits {
                    context_tokens: None,
                    max_output_tokens: None,
                },
                credential: Some(credential),
            },
            completions: Mutex::new(VecDeque::from([
                completion(
                    "",
                    vec![ToolCall {
                        id: "write-1".into(),
                        name: "write_file".into(),
                        arguments: serde_json::json!({
                            "path":"api-implemented.txt",
                            "content":"owned worktree only\n"
                        }),
                    }],
                ),
                completion("implementation ready", Vec::new()),
            ])),
        });
        let mut registry = ProviderRegistry::with_managed_cli_providers(credentials, false, true);
        registry.register_api(adapter).unwrap();
        let selection = ProviderSelection {
            provider_id: provider_id.clone(),
            model_id: "fake-model".into(),
        };
        let workflow = WorkflowProviderConfiguration {
            implementer: selection.clone(),
            reviewer: managed_selection(CLAUDE_PROVIDER_ID),
            repair: Some(selection.clone()),
        };
        let mut supervisor = SentinelSupervisor::with_provider_runtime(
            fixture.repository.clone(),
            &fixture.main,
            &fixture.worktrees,
            SupervisorPrograms {
                codex: None,
                claude: Some(ClaudeProgram::from_executable(&fixture.claude).unwrap()),
            },
            Arc::new(registry),
            workflow.clone(),
        )
        .unwrap();
        let task = supervisor
            .start_configured_task(None, "API implementation".into(), "make the file".into())
            .await
            .unwrap();
        let worktree = fixture
            .repository
            .v3()
            .get_task_worktree(&task.id)
            .await
            .unwrap();
        for _ in 0..50 {
            let completed = fixture
                .repository
                .v3()
                .list_events(&task.id)
                .await
                .unwrap()
                .iter()
                .any(provider_turn_completed);
            if completed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            fs::read_to_string(Path::new(&worktree.worktree_path).join("api-implemented.txt"))
                .unwrap(),
            "owned worktree only\n"
        );
        assert!(!fixture.main.join("api-implemented.txt").exists());
        let roles = supervisor.roles_for_task(&task.id).await.unwrap();
        assert_eq!(roles, workflow);
        assert!(fixture
            .repository
            .v3()
            .list_events(&task.id)
            .await
            .unwrap()
            .iter()
            .any(provider_turn_completed));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn active_projection_ignores_utility_and_draft_records() {
        let fixture = fixture().await;
        let supervisor = SentinelSupervisor::new(
            fixture.repository.clone(),
            &fixture.main,
            &fixture.worktrees,
            SupervisorPrograms {
                codex: None,
                claude: None,
            },
        )
        .unwrap();
        let coding = fixture
            .repository
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: "v3:openai.codex.app_server".into(),
                    summary: "coding".into(),
                },
                1,
            )
            .await
            .unwrap();
        let coding = fixture
            .repository
            .v3()
            .transition_task(&coding, TaskLifecycle::Preparing, 2)
            .await
            .unwrap();
        WorktreeTransaction::create(
            fixture.repository.clone(),
            coding.id.clone(),
            &fixture.main,
            &fixture.worktrees,
        )
        .await
        .unwrap();
        fixture
            .repository
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: "native-provider-usage".into(),
                    summary: "usage".into(),
                },
                99,
            )
            .await
            .unwrap();
        assert_eq!(supervisor.active_task().await.unwrap(), Some(coding));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_reconciles_the_owned_worktree_and_resumable_codex_thread() {
        let fixture = fixture().await;
        let program = CodexProgram::from_executable(&fixture.codex).unwrap();
        let task_id = {
            let mut supervisor = SentinelSupervisor::new(
                fixture.repository.clone(),
                &fixture.main,
                &fixture.worktrees,
                SupervisorPrograms {
                    codex: Some(program.clone()),
                    claude: None,
                },
            )
            .unwrap();
            supervisor
                .start_task(StartTaskIntent {
                    provider: ProviderKind::Codex,
                    summary: "restart".into(),
                    prompt: "work".into(),
                })
                .await
                .unwrap()
                .id
        };
        let before = fixture
            .repository
            .v3()
            .get_task_worktree(&task_id)
            .await
            .unwrap();
        let mut restarted = SentinelSupervisor::new(
            fixture.repository.clone(),
            &fixture.main,
            &fixture.worktrees,
            SupervisorPrograms {
                codex: Some(program),
                claude: None,
            },
        )
        .unwrap();
        restarted.enter_recovery_after_restart().await.unwrap();
        let task = fixture.repository.v3().get_task(&task_id).await.unwrap();
        let after = fixture
            .repository
            .v3()
            .get_task_worktree(&task_id)
            .await
            .unwrap();
        assert_eq!(task.lifecycle, TaskLifecycle::Implementing);
        assert_eq!(task.recovery_condition, RecoveryCondition::None);
        assert_eq!(before, after);
        assert!(restarted.available_actions(&task).await.unwrap().stop);
        restarted.enter_recovery_after_restart().await.unwrap();
        assert_eq!(
            fixture.repository.v3().get_task(&task_id).await.unwrap(),
            task
        );
        restarted.stop(&task_id).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn provider_completion_advances_through_validation_review_and_human_gate() {
        let fixture = fixture().await;
        fs::create_dir_all(fixture.main.join(".agent-sentinel")).unwrap();
        fs::write(
            fixture.main.join(".agent-sentinel/validation.json"),
            r#"{"profiles":[{"id":"default","steps":[{"name":"implemented","argv":["/bin/test","-f","implemented.txt"],"kind":"test","cwd":".","env":{},"required":true,"timeout_ms":5000}]}]}"#,
        )
        .unwrap();
        git(&fixture.main, &["add", ".agent-sentinel/validation.json"]);
        git(&fixture.main, &["commit", "-qm", "validation profile"]);
        let mut supervisor = SentinelSupervisor::new(
            fixture.repository.clone(),
            &fixture.main,
            &fixture.worktrees,
            SupervisorPrograms {
                codex: Some(CodexProgram::from_executable(&fixture.codex).unwrap()),
                claude: Some(ClaudeProgram::from_executable(&fixture.claude).unwrap()),
            },
        )
        .unwrap();
        let task = supervisor
            .start_task(StartTaskIntent {
                provider: ProviderKind::Codex,
                summary: "complete workflow".into(),
                prompt: "implement it".into(),
            })
            .await
            .unwrap();
        supervisor.reconcile_progress().await.unwrap();
        assert_eq!(
            fixture
                .repository
                .v3()
                .get_task(&task.id)
                .await
                .unwrap()
                .lifecycle,
            TaskLifecycle::Validating
        );
        supervisor.reconcile_progress().await.unwrap();
        assert_eq!(
            fixture
                .repository
                .v3()
                .get_task(&task.id)
                .await
                .unwrap()
                .lifecycle,
            TaskLifecycle::Reviewing
        );
        supervisor.reconcile_progress().await.unwrap();
        let ready = fixture.repository.v3().get_task(&task.id).await.unwrap();
        assert_eq!(ready.lifecycle, TaskLifecycle::ReadyForHuman);
        let actions = supervisor.available_actions(&ready).await.unwrap();
        assert!(actions.approve && actions.reject);
        assert!(!actions.stop);
        let approval_id = actions.approval_id.unwrap();
        supervisor
            .decide_final_approval(&task.id, &approval_id, false)
            .await
            .unwrap();
        assert_eq!(
            fixture
                .repository
                .v3()
                .get_approval(&approval_id)
                .await
                .unwrap()
                .lifecycle,
            ApprovalLifecycle::Denied
        );
        assert_eq!(
            fixture
                .repository
                .v3()
                .get_task(&task.id)
                .await
                .unwrap()
                .lifecycle,
            TaskLifecycle::Blocked
        );
        assert!(fixture
            .repository
            .v3()
            .get_task_worktree_merge_preparation(&task.id)
            .await
            .is_ok());
        assert_eq!(
            fs::read_to_string(fixture.main.join("tracked.txt")).unwrap(),
            "primary\n"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_restores_a_proven_final_approval_boundary_without_a_provider() {
        let fixture = fixture().await;
        let task = fixture
            .repository
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: "v3:openai.codex.app_server".into(),
                    summary: "approval restart".into(),
                },
                1,
            )
            .await
            .unwrap();
        let preparing = fixture
            .repository
            .v3()
            .transition_task(&task, TaskLifecycle::Preparing, 2)
            .await
            .unwrap();
        WorktreeTransaction::create(
            fixture.repository.clone(),
            preparing.id.clone(),
            &fixture.main,
            &fixture.worktrees,
        )
        .await
        .unwrap();
        let implementing = fixture
            .repository
            .v3()
            .transition_task(&preparing, TaskLifecycle::Implementing, 3)
            .await
            .unwrap();
        let validating = fixture
            .repository
            .v3()
            .transition_task(&implementing, TaskLifecycle::Validating, 4)
            .await
            .unwrap();
        let ready = fixture
            .repository
            .v3()
            .transition_task(&validating, TaskLifecycle::ReadyForHuman, 5)
            .await
            .unwrap();
        fixture
            .repository
            .v3()
            .create_artifact(
                CreateArtifact {
                    task_id: ready.id.clone(),
                    kind: "final_approval_packet".into(),
                    display_name: "final-approval".into(),
                    content_hash: None,
                    metadata: serde_json::json!({"safe":true}),
                },
                6,
            )
            .await
            .unwrap();
        fixture
            .repository
            .v3()
            .create_approval(
                CreateApproval {
                    task_id: ready.id.clone(),
                    session_id: None,
                    action_kind: "v3_final_git_action".into(),
                    summary: "human required".into(),
                },
                7,
            )
            .await
            .unwrap();
        let mut restarted = SentinelSupervisor::new(
            fixture.repository.clone(),
            &fixture.main,
            &fixture.worktrees,
            SupervisorPrograms {
                codex: None,
                claude: None,
            },
        )
        .unwrap();
        restarted.enter_recovery_after_restart().await.unwrap();
        let restored = fixture.repository.v3().get_task(&ready.id).await.unwrap();
        assert_eq!(restored.lifecycle, TaskLifecycle::ReadyForHuman);
        assert_eq!(restored.recovery_condition, RecoveryCondition::None);
        assert!(
            restarted
                .available_actions(&restored)
                .await
                .unwrap()
                .approve
        );
    }
}
