//! One desktop-owned, lazily started Codex App Server used only for account
//! usage.  It deliberately owns no user threads: an App Server exit therefore
//! invalidates the process but never implies a session can be resumed.

use sentinel_codex::{CodexAppServer, CodexError, CodexProgram, CodexRateLimits};
use sentinel_core::{v3::CreateTask, RunRepository};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{watch, Mutex};

#[derive(Clone)]
pub struct CodexUsageManager {
    program: Option<CodexProgram>,
    repository: RunRepository,
    cwd: PathBuf,
    server: Arc<Mutex<Option<CodexAppServer>>>,
    latest: watch::Sender<Option<CodexRateLimits>>,
}

impl CodexUsageManager {
    pub fn new(program: Option<CodexProgram>, repository: RunRepository, cwd: PathBuf) -> Self {
        let (latest, _) = watch::channel(None);
        Self {
            program,
            repository,
            cwd,
            server: Arc::new(Mutex::new(None)),
            latest,
        }
    }

    /// Serializes startup with server ownership.  If the child exited, the
    /// next caller starts a fresh server; no old thread/session is retained.
    pub async fn ensure_started(&self) -> Result<(), CodexError> {
        let mut owned = self.server.lock().await;
        if owned.as_ref().is_some_and(CodexAppServer::is_alive) {
            return Ok(());
        }
        let Some(program) = self.program.clone() else {
            return Err(CodexError::MissingExecutable);
        };
        let task = self
            .repository
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: "provider-usage".into(),
                    summary: "Codex account usage".into(),
                },
                now_ms(),
            )
            .await
            .map_err(|_| CodexError::Storage)?;
        let server =
            CodexAppServer::start(program, self.repository.clone(), task.id, &self.cwd).await?;
        let mut updates = server.subscribe_rate_limits();
        let latest = self.latest.clone();
        tokio::spawn(async move {
            while updates.changed().await.is_ok() {
                let _ = latest.send(updates.borrow().clone());
            }
        });
        if let Some(snapshot) = server.latest_rate_limits() {
            let _ = self.latest.send(Some(snapshot));
        }
        *owned = Some(server);
        Ok(())
    }

    pub async fn rate_limits(&self) -> Result<Option<CodexRateLimits>, CodexError> {
        self.ensure_started().await?;
        Ok(self.latest.borrow().clone())
    }

    pub fn subscribe(&self) -> watch::Receiver<Option<CodexRateLimits>> {
        self.latest.subscribe()
    }

    #[allow(dead_code)]
    pub async fn shutdown(&self) -> Result<(), CodexError> {
        if let Some(mut server) = self.server.lock().await.take() {
            server.shutdown().await?;
        }
        Ok(())
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
