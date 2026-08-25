//! Shared Codex V3 task-start supervisor used by desktop presentation shells.

use crate::{
    CodexAppServer, CodexError, CodexProgram, CodexSession, CodexThreadState, CodexTurn, PROVIDER,
};
use sentinel_core::{
    v3::{CreateTask, SessionLifecycle, Task, TaskLifecycle},
    RunRepository,
};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// An owned provider process/thread pair. Keeping this value alive is required
/// for later turns and cancellation; callers cannot manufacture one from IDs.
pub struct StartedCodexTask {
    task: Task,
    server: CodexAppServer,
    session: CodexSession,
    turn: Option<CodexTurn>,
    repository: RunRepository,
}

impl StartedCodexTask {
    pub fn task(&self) -> &Task {
        &self.task
    }

    pub async fn start_turn(&mut self, prompt: &str) -> Result<CodexTurn, CodexError> {
        self.start_turn_with_model(prompt, None).await
    }

    pub async fn start_turn_with_model(
        &mut self,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<CodexTurn, CodexError> {
        let turn = self
            .server
            .start_turn_with_model(&self.session, prompt, model)
            .await?;
        self.turn = Some(turn.clone());
        Ok(turn)
    }

    pub async fn ensure_requested_selection_enforced(&self) -> Result<(), CodexError> {
        self.server.ensure_requested_selection_enforced().await
    }

    pub async fn cancel(&mut self) -> Result<(), CodexError> {
        let Some(turn) = self.turn.as_ref() else {
            return Ok(());
        };
        let durable = self
            .repository
            .v3()
            .get_session(&self.session.session_id)
            .await
            .map_err(|_| CodexError::Storage)?;
        let durable = self
            .repository
            .v3()
            .transition_session(&durable, SessionLifecycle::Cancelling, now_ms())
            .await
            .map_err(|_| CodexError::Storage)?;
        self.server.interrupt_turn(&self.session, turn).await?;
        self.repository
            .v3()
            .transition_session(&durable, SessionLifecycle::Cancelled, now_ms())
            .await
            .map_err(|_| CodexError::Storage)?;
        let task = self
            .repository
            .v3()
            .get_task(&self.task.id)
            .await
            .map_err(|_| CodexError::Storage)?;
        if !task.lifecycle.terminal() {
            self.task = self
                .repository
                .v3()
                .transition_task(&task, TaskLifecycle::Cancelled, now_ms())
                .await
                .map_err(|_| CodexError::Storage)?;
        }
        self.turn = None;
        Ok(())
    }

    /// Releases the completed implementation transport.  A later repair, if
    /// required, starts its own owned App Server rather than retaining an idle
    /// implementation child through validation and review.
    pub async fn complete(mut self) -> Result<(), CodexError> {
        self.server.shutdown().await?;
        let durable = self
            .repository
            .v3()
            .get_session(&self.session.session_id)
            .await
            .map_err(|_| CodexError::Storage)?;
        if durable.lifecycle == SessionLifecycle::Active {
            self.repository
                .v3()
                .transition_session(&durable, SessionLifecycle::Completed, now_ms())
                .await
                .map_err(|_| CodexError::Storage)?;
        }
        Ok(())
    }

    pub fn into_parts(self) -> (Task, CodexAppServer, CodexSession) {
        (self.task, self.server, self.session)
    }
}

#[derive(Clone)]
pub struct CodexTaskStarter {
    program: Option<CodexProgram>,
    repository: RunRepository,
}

pub enum CodexTaskReconciliation {
    Resumed(StartedCodexTask),
    Inactive,
    Missing,
    Ambiguous,
}

impl CodexTaskStarter {
    pub fn new(program: Option<CodexProgram>, repository: RunRepository) -> Self {
        Self {
            program,
            repository,
        }
    }

    /// Starts exactly one Sentinel-owned Codex task and provider thread.
    /// Task/session state transitions remain durable and provider completion is
    /// deliberately not interpreted as Sentinel task completion.
    pub async fn start_task(
        &self,
        summary: String,
        cwd: &Path,
    ) -> Result<StartedCodexTask, CodexError> {
        if summary.trim().is_empty() {
            return Err(CodexError::InvalidInput);
        }
        let task = self
            .repository
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: "codex-v3".into(),
                    summary,
                },
                now_ms(),
            )
            .await
            .map_err(|_| CodexError::Storage)?;
        let task = self
            .repository
            .v3()
            .transition_task(&task, TaskLifecycle::Preparing, now_ms())
            .await
            .map_err(|_| CodexError::Storage)?;
        self.start_prepared_task(task, cwd).await
    }

    /// Starts a provider thread for an already prepared Sentinel task.
    ///
    /// The workflow supervisor uses this entry point only after it has created
    /// and reconciled the task's owned worktree. Keeping task creation separate
    /// from provider startup prevents presentation bridges from accidentally
    /// starting Codex in the primary checkout.
    pub async fn start_prepared_task(
        &self,
        task: Task,
        cwd: &Path,
    ) -> Result<StartedCodexTask, CodexError> {
        if task.lifecycle != TaskLifecycle::Preparing || !cwd.is_dir() {
            return Err(CodexError::InvalidInput);
        }
        let program = self.program.clone().ok_or(CodexError::MissingExecutable)?;
        let server =
            CodexAppServer::start(program, self.repository.clone(), task.id.clone(), cwd).await?;
        let session = match server.start_thread(cwd).await {
            Ok(session) => session,
            Err(error) => {
                let _ = self
                    .repository
                    .v3()
                    .transition_task(&task, TaskLifecycle::Failed, now_ms())
                    .await;
                return Err(error);
            }
        };
        let durable = self
            .repository
            .v3()
            .get_session(&session.session_id)
            .await
            .map_err(|_| CodexError::Storage)?;
        let durable = self
            .repository
            .v3()
            .transition_session(&durable, SessionLifecycle::Starting, now_ms())
            .await
            .map_err(|_| CodexError::Storage)?;
        self.repository
            .v3()
            .transition_session(&durable, SessionLifecycle::Active, now_ms())
            .await
            .map_err(|_| CodexError::Storage)?;
        let task = self
            .repository
            .v3()
            .transition_task(&task, TaskLifecycle::Implementing, now_ms())
            .await
            .map_err(|_| CodexError::Storage)?;
        Ok(StartedCodexTask {
            task,
            server,
            session,
            turn: None,
            repository: self.repository.clone(),
        })
    }

    /// Reconciles exactly one persisted Sentinel-owned Codex thread after a
    /// restart. It never creates a replacement thread. Only provider-inspected
    /// active state can move the durable task out of recovery.
    pub async fn reconcile_recovering_task(
        &self,
        task: Task,
        cwd: &Path,
    ) -> Result<CodexTaskReconciliation, CodexError> {
        if task.lifecycle != TaskLifecycle::Recovering || !cwd.is_dir() {
            return Err(CodexError::InvalidInput);
        }
        let durable = self
            .repository
            .v3()
            .list_sessions_for_task(&task.id)
            .await
            .map_err(|_| CodexError::Storage)?
            .into_iter()
            .filter(|session| session.provider == PROVIDER)
            .collect::<Vec<_>>();
        if durable.len() != 1 {
            return Ok(CodexTaskReconciliation::Missing);
        }
        let program = self.program.clone().ok_or(CodexError::MissingExecutable)?;
        let mut server =
            CodexAppServer::start(program, self.repository.clone(), task.id.clone(), cwd).await?;
        let provider_ref = durable[0].provider_session_ref.clone();
        match server.inspect_thread(&provider_ref).await? {
            CodexThreadState::Inactive => {
                let _ = server.shutdown().await;
                Ok(CodexTaskReconciliation::Inactive)
            }
            CodexThreadState::Missing => {
                let _ = server.shutdown().await;
                Ok(CodexTaskReconciliation::Missing)
            }
            CodexThreadState::Ambiguous => {
                let _ = server.shutdown().await;
                Ok(CodexTaskReconciliation::Ambiguous)
            }
            CodexThreadState::Resumable => {
                let session = server.resume_thread(&provider_ref).await?;
                if session.session_id != durable[0].id {
                    let _ = server.shutdown().await;
                    return Ok(CodexTaskReconciliation::Ambiguous);
                }
                let stored = self
                    .repository
                    .v3()
                    .get_session(&session.session_id)
                    .await
                    .map_err(|_| CodexError::Storage)?;
                if stored.lifecycle == SessionLifecycle::RecoveryRequired {
                    self.repository
                        .v3()
                        .transition_session(&stored, SessionLifecycle::Active, now_ms())
                        .await
                        .map_err(|_| CodexError::Storage)?;
                }
                let turn = self
                    .repository
                    .v3()
                    .list_events(&task.id)
                    .await
                    .map_err(|_| CodexError::Storage)?
                    .into_iter()
                    .rev()
                    .find_map(|event| {
                        event
                            .payload
                            .get("turn_id")
                            .and_then(|value| value.as_str())
                            .filter(|value| !value.is_empty())
                            .map(|turn_id| CodexTurn {
                                turn_id: turn_id.into(),
                            })
                    });
                let task = self
                    .repository
                    .v3()
                    .transition_task(&task, TaskLifecycle::Implementing, now_ms())
                    .await
                    .map_err(|_| CodexError::Storage)?;
                Ok(CodexTaskReconciliation::Resumed(StartedCodexTask {
                    task,
                    server,
                    session,
                    turn,
                    repository: self.repository.clone(),
                }))
            }
        }
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
