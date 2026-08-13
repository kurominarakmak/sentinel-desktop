//! Shared Codex V3 task-start supervisor used by desktop presentation shells.

use crate::{CodexAppServer, CodexError, CodexProgram, CodexSession, CodexTurn};
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
}

impl StartedCodexTask {
    pub fn task(&self) -> &Task {
        &self.task
    }

    pub async fn start_turn(&mut self, prompt: &str) -> Result<CodexTurn, CodexError> {
        self.server.start_turn(&self.session, prompt).await
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
        })
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
