//! Supervisor-owned Codex task/session lifecycle.
//!
//! The desktop process owns every App Server it starts.  Provider `turn/completed`
//! is retained as a normalized event only; it never advances a Sentinel task.

use sentinel_codex::{CodexAppServer, CodexError, CodexProgram, CodexSession, CodexTurn, PROVIDER};
use sentinel_core::{
    v3::{CreateTask, SessionLifecycle, Task, TaskId, TaskLifecycle},
    RunRepository,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

struct ManagedSession {
    server: CodexAppServer,
    session: CodexSession,
    turn: Option<CodexTurn>,
}

#[derive(Clone)]
pub struct CodexSessionManager {
    program: Option<CodexProgram>,
    repository: RunRepository,
    default_cwd: PathBuf,
    sessions: Arc<Mutex<HashMap<TaskId, ManagedSession>>>,
}

impl CodexSessionManager {
    pub fn new(
        program: Option<CodexProgram>,
        repository: RunRepository,
        default_cwd: PathBuf,
    ) -> Self {
        Self {
            program,
            repository,
            default_cwd,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn start_task(&self, summary: String, cwd: &Path) -> Result<Task, CodexError> {
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
        let mut owned = self.sessions.lock().await;
        let server = self.start_server(&task.id, cwd).await?;
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
        owned.insert(
            task.id.clone(),
            ManagedSession {
                server,
                session,
                turn: None,
            },
        );
        Ok(task)
    }

    pub async fn start_turn(
        &self,
        task_id: &TaskId,
        prompt: &str,
    ) -> Result<CodexTurn, CodexError> {
        let mut owned = self.sessions.lock().await;
        let managed = owned.get_mut(task_id).ok_or(CodexError::InvalidInput)?;
        if !managed.server.is_alive() {
            return Err(CodexError::UnexpectedExit);
        }
        let turn = managed.server.start_turn(&managed.session, prompt).await?;
        managed.turn = Some(turn.clone());
        Ok(turn)
    }

    pub async fn cancel(&self, task_id: &TaskId) -> Result<(), CodexError> {
        let mut owned = self.sessions.lock().await;
        let managed = owned.get_mut(task_id).ok_or(CodexError::InvalidInput)?;
        let Some(turn) = managed.turn.as_ref() else {
            return Ok(());
        };
        let durable = self
            .repository
            .v3()
            .get_session(&managed.session.session_id)
            .await
            .map_err(|_| CodexError::Storage)?;
        let durable = self
            .repository
            .v3()
            .transition_session(&durable, SessionLifecycle::Cancelling, now_ms())
            .await
            .map_err(|_| CodexError::Storage)?;
        managed
            .server
            .interrupt_turn(&managed.session, turn)
            .await?;
        self.repository
            .v3()
            .transition_session(&durable, SessionLifecycle::Cancelled, now_ms())
            .await
            .map_err(|_| CodexError::Storage)?;
        let task = self
            .repository
            .v3()
            .get_task(task_id)
            .await
            .map_err(|_| CodexError::Storage)?;
        if !task.lifecycle.terminal() {
            let _ = self
                .repository
                .v3()
                .transition_task(&task, TaskLifecycle::Cancelled, now_ms())
                .await;
        }
        managed.turn = None;
        Ok(())
    }

    /// Reopens only a durable, Sentinel-owned Codex thread. Missing or
    /// unavailable sessions remain in explicit recovery; no provider history
    /// scan or inferred completion is attempted.
    pub async fn resume_task(&self, task_id: &TaskId) -> Result<Task, CodexError> {
        let task = self
            .repository
            .v3()
            .get_task(task_id)
            .await
            .map_err(|_| CodexError::Storage)?;
        // A repeated reconciliation is a no-op while the exact Sentinel-owned
        // App Server and durable thread are still live.  In particular, do not
        // start a second App Server or a replacement provider thread.
        {
            let owned = self.sessions.lock().await;
            if owned
                .get(task_id)
                .is_some_and(|managed| managed.server.is_alive())
            {
                return Ok(task);
            }
        }
        let session = self
            .repository
            .v3()
            .list_sessions_for_task(task_id)
            .await
            .map_err(|_| CodexError::Storage)?
            .into_iter()
            .find(|session| session.provider == PROVIDER)
            .ok_or(CodexError::InvalidInput)?;
        let mut owned = self.sessions.lock().await;
        // A dead App Server is ours to replace; the durable provider thread is
        // not. `thread/resume` is the proof step and errors leave recovery
        // intact rather than creating a new Sentinel session/thread.
        owned.remove(task_id);
        let server = self.start_server(task_id, &self.default_cwd).await?;
        let live = server.resume_thread(&session.provider_session_ref).await?;
        if live.session_id != session.id {
            return Err(CodexError::InvalidInput);
        }
        let durable = self
            .repository
            .v3()
            .get_session(&live.session_id)
            .await
            .map_err(|_| CodexError::Storage)?;
        let durable = match durable.lifecycle {
            SessionLifecycle::RecoveryRequired => {
                self.repository
                    .v3()
                    .transition_session(&durable, SessionLifecycle::Active, now_ms())
                    .await
            }
            SessionLifecycle::Created => {
                let starting = self
                    .repository
                    .v3()
                    .transition_session(&durable, SessionLifecycle::Starting, now_ms())
                    .await
                    .map_err(|_| CodexError::Storage)?;
                self.repository
                    .v3()
                    .transition_session(&starting, SessionLifecycle::Active, now_ms())
                    .await
            }
            _ => Ok(durable),
        }
        .map_err(|_| CodexError::Storage)?;
        let _ = durable;
        let task = if task.lifecycle == TaskLifecycle::Recovering {
            self.repository
                .v3()
                .transition_task(&task, TaskLifecycle::Implementing, now_ms())
                .await
                .map_err(|_| CodexError::Storage)?
        } else {
            task
        };
        owned.insert(
            task.id.clone(),
            ManagedSession {
                server,
                session: live,
                turn: None,
            },
        );
        Ok(task)
    }

    pub async fn reconcile_after_restart(&self) -> Result<(), CodexError> {
        for task in self
            .repository
            .v3()
            .list_tasks()
            .await
            .map_err(|_| CodexError::Storage)?
        {
            if task.lifecycle == TaskLifecycle::Recovering {
                // Resume failure deliberately leaves the durable recovery state intact.
                let _ = self.resume_task(&task.id).await;
            }
        }
        Ok(())
    }

    async fn start_server(
        &self,
        task_id: &TaskId,
        cwd: &Path,
    ) -> Result<CodexAppServer, CodexError> {
        let program = self.program.clone().ok_or(CodexError::MissingExecutable)?;
        CodexAppServer::start(program, self.repository.clone(), task_id.clone(), cwd).await
    }

    #[cfg(test)]
    async fn pid_for_test(&self, task_id: &TaskId) -> Option<u32> {
        self.sessions
            .lock()
            .await
            .get(task_id)
            .and_then(|managed| managed.server.pid())
    }

    #[cfg(test)]
    async fn shutdown(&self) {
        let mut owned = self.sessions.lock().await;
        for (_, mut managed) in owned.drain() {
            let _ = managed.server.shutdown().await;
        }
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
    use sentinel_core::v3::{EventKind, TaskLifecycle};
    use std::{process::Command, sync::Mutex as StdMutex};
    use tempfile::TempDir;

    static PROCESS_LOCK: StdMutex<()> = StdMutex::new(());

    async fn fixture() -> (TempDir, RunRepository, CodexSessionManager) {
        let temp = tempfile::tempdir().unwrap();
        let repository = RunRepository::open(&format!(
            "sqlite://{}",
            temp.path().join("sessions.sqlite").display()
        ))
        .await
        .unwrap();
        let executable = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/debug/fake-codex-app-server");
        assert!(
            executable.is_file(),
            "build fake-codex-app-server before desktop session tests"
        );
        let manager = CodexSessionManager::new(
            Some(CodexProgram::from_executable(executable).unwrap()),
            repository.clone(),
            temp.path().to_owned(),
        );
        (temp, repository, manager)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn start_streams_events_and_persists_thread_and_turn() {
        let _guard = PROCESS_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (temp, repository, manager) = fixture().await;
        let task = manager
            .start_task("fixture".into(), temp.path())
            .await
            .unwrap();
        manager.start_turn(&task.id, "stream this").await.unwrap();
        let sessions = repository
            .v3()
            .list_sessions_for_task(&task.id)
            .await
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider_session_ref, "thread-test");
        assert_eq!(sessions[0].lifecycle, SessionLifecycle::Active);
        assert_eq!(
            repository.v3().get_task(&task.id).await.unwrap().lifecycle,
            TaskLifecycle::Implementing
        );
        let events = repository.v3().list_events(&task.id).await.unwrap();
        assert!(events
            .iter()
            .any(|event| matches!(event.kind, EventKind::ToolStarted)));
        assert!(events
            .iter()
            .any(|event| matches!(event.kind, EventKind::ToolCompleted)));
        assert_ne!(
            repository.v3().get_task(&task.id).await.unwrap().lifecycle,
            TaskLifecycle::Finalized
        );
        manager.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancel_interrupts_the_owned_turn_and_task() {
        let _guard = PROCESS_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (temp, repository, manager) = fixture().await;
        let task = manager
            .start_task("fixture".into(), temp.path())
            .await
            .unwrap();
        manager.start_turn(&task.id, "cancel this").await.unwrap();
        manager.cancel(&task.id).await.unwrap();
        assert_eq!(
            repository.v3().get_task(&task.id).await.unwrap().lifecycle,
            TaskLifecycle::Cancelled
        );
        assert_eq!(
            repository
                .v3()
                .list_sessions_for_task(&task.id)
                .await
                .unwrap()[0]
                .lifecycle,
            SessionLifecycle::Cancelled
        );
        manager.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_reconciles_then_resumes_the_persisted_thread() {
        let _guard = PROCESS_LOCK.lock().unwrap();
        let (temp, repository, manager) = fixture().await;
        let task = manager
            .start_task("fixture".into(), temp.path())
            .await
            .unwrap();
        manager.shutdown().await;
        repository
            .v3()
            .restore_unfinished_tasks(now_ms())
            .await
            .unwrap();
        assert_eq!(
            repository.v3().get_task(&task.id).await.unwrap().lifecycle,
            TaskLifecycle::Recovering
        );
        let replacement = CodexSessionManager::new(
            manager.program.clone(),
            repository.clone(),
            temp.path().to_owned(),
        );
        replacement.resume_task(&task.id).await.unwrap();
        assert_eq!(
            repository.v3().get_task(&task.id).await.unwrap().lifecycle,
            TaskLifecycle::Implementing
        );
        assert_eq!(
            repository
                .v3()
                .list_sessions_for_task(&task.id)
                .await
                .unwrap()[0]
                .lifecycle,
            SessionLifecycle::Active
        );
        replacement.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn repeated_reconciliation_reuses_the_owned_process_and_thread() {
        let _guard = PROCESS_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (temp, repository, manager) = fixture().await;
        let task = manager
            .start_task("fixture".into(), temp.path())
            .await
            .unwrap();
        let first = manager.pid_for_test(&task.id).await.unwrap();
        manager.resume_task(&task.id).await.unwrap();
        assert_eq!(manager.pid_for_test(&task.id).await.unwrap(), first);
        assert_eq!(
            repository
                .v3()
                .list_sessions_for_task(&task.id)
                .await
                .unwrap()
                .len(),
            1
        );
        manager.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_session_and_dead_server_leave_or_recreate_explicitly() {
        let _guard = PROCESS_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (temp, repository, manager) = fixture().await;
        let missing = repository
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: "codex-v3".into(),
                    summary: "missing".into(),
                },
                now_ms(),
            )
            .await
            .unwrap();
        let missing = repository
            .v3()
            .transition_task(&missing, TaskLifecycle::Preparing, now_ms())
            .await
            .unwrap();
        let missing = repository
            .v3()
            .transition_task(&missing, TaskLifecycle::Implementing, now_ms())
            .await
            .unwrap();
        repository
            .v3()
            .restore_unfinished_tasks(now_ms())
            .await
            .unwrap();
        assert_eq!(
            manager.resume_task(&missing.id).await,
            Err(CodexError::InvalidInput)
        );
        assert_eq!(
            repository
                .v3()
                .get_task(&missing.id)
                .await
                .unwrap()
                .lifecycle,
            TaskLifecycle::Recovering
        );

        let task = manager
            .start_task("fixture".into(), temp.path())
            .await
            .unwrap();
        let first = manager.pid_for_test(&task.id).await.unwrap();
        Command::new("kill")
            .args(["-TERM", &first.to_string()])
            .status()
            .unwrap();
        manager.resume_task(&task.id).await.unwrap();
        assert_ne!(manager.pid_for_test(&task.id).await.unwrap(), first);
        manager.shutdown().await;
    }
}
