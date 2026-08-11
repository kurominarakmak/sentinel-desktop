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
        if let Some(snapshot) = self.latest.borrow().clone() {
            return Ok(Some(snapshot));
        }
        Ok(self
            .server
            .lock()
            .await
            .as_ref()
            .and_then(CodexAppServer::latest_rate_limits))
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

    #[cfg(test)]
    async fn pid_for_test(&self) -> Option<u32> {
        self.server
            .lock()
            .await
            .as_ref()
            .and_then(|server| server.pid())
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
    use std::{
        process::{Command, Stdio},
        sync::Mutex as StdMutex,
        time::Duration,
    };
    use tempfile::TempDir;

    static PROCESS_LOCK: StdMutex<()> = StdMutex::new(());

    async fn fixture() -> (TempDir, RunRepository, CodexUsageManager, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repository = RunRepository::open(&format!(
            "sqlite://{}",
            temp.path().join("usage.sqlite").display()
        ))
        .await
        .unwrap();
        let executable = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/debug/fake-codex-app-server");
        assert!(
            executable.is_file(),
            "build the sentinel-codex fake App Server before these focused tests"
        );
        let manager = CodexUsageManager::new(
            Some(CodexProgram::from_executable(&executable).unwrap()),
            repository.clone(),
            temp.path().to_owned(),
        );
        (temp, repository, manager, executable)
    }

    async fn wait_for_exit(manager: &CodexUsageManager) {
        for _ in 0..50 {
            if manager
                .server
                .lock()
                .await
                .as_ref()
                .is_some_and(|server| !server.is_alive())
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("fake App Server did not exit");
    }

    fn is_running(pid: u32) -> bool {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .is_ok_and(|output| output.status.success())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn first_concurrent_requests_lazily_create_one_app_server() {
        let _guard = PROCESS_LOCK.lock().unwrap();
        let (_temp, repository, manager, _executable) = fixture().await;
        assert_eq!(manager.pid_for_test().await, None);
        assert!(repository.v3().list_tasks().await.unwrap().is_empty());
        let (one, two, three, four) = tokio::join!(
            manager.rate_limits(),
            manager.rate_limits(),
            manager.rate_limits(),
            manager.rate_limits(),
        );
        for result in [one, two, three, four] {
            assert_eq!(result.unwrap().unwrap().0["primary"]["usedPercent"], 42);
        }
        assert!(manager.pid_for_test().await.is_some());
        assert_eq!(repository.v3().list_tasks().await.unwrap().len(), 1);
        manager.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn repeated_requests_reuse_the_healthy_owned_process() {
        let _guard = PROCESS_LOCK.lock().unwrap();
        let (_temp, repository, manager, _executable) = fixture().await;
        manager.rate_limits().await.unwrap();
        let pid = manager.pid_for_test().await.unwrap();
        manager.rate_limits().await.unwrap();
        manager.rate_limits().await.unwrap();
        assert_eq!(manager.pid_for_test().await, Some(pid));
        assert_eq!(repository.v3().list_tasks().await.unwrap().len(), 1);
        manager.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dead_child_is_cleared_and_next_request_creates_a_fresh_process() {
        let _guard = PROCESS_LOCK.lock().unwrap();
        let (_temp, repository, manager, _executable) = fixture().await;
        manager.rate_limits().await.unwrap();
        let first = manager.pid_for_test().await.unwrap();
        Command::new("kill")
            .args(["-TERM", &first.to_string()])
            .status()
            .unwrap();
        wait_for_exit(&manager).await;
        manager.rate_limits().await.unwrap();
        let second = manager.pid_for_test().await.unwrap();
        assert_ne!(first, second);
        assert_eq!(repository.v3().list_tasks().await.unwrap().len(), 2);
        manager.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_terminates_and_reaps_the_owned_process() {
        let _guard = PROCESS_LOCK.lock().unwrap();
        let (_temp, _repository, manager, _executable) = fixture().await;
        manager.rate_limits().await.unwrap();
        let pid = manager.pid_for_test().await.unwrap();
        manager.shutdown().await.unwrap();
        assert_eq!(manager.pid_for_test().await, None);
        assert!(!is_running(pid));
        manager.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_after_shutdown_lazily_starts_a_replacement() {
        let _guard = PROCESS_LOCK.lock().unwrap();
        let (_temp, repository, manager, _executable) = fixture().await;
        manager.rate_limits().await.unwrap();
        let first = manager.pid_for_test().await.unwrap();
        manager.shutdown().await.unwrap();
        manager.rate_limits().await.unwrap();
        let second = manager.pid_for_test().await.unwrap();
        assert_ne!(first, second);
        assert_eq!(repository.v3().list_tasks().await.unwrap().len(), 2);
        manager.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_does_not_affect_an_unrelated_codex_process() {
        let _guard = PROCESS_LOCK.lock().unwrap();
        let (_temp, _repository, manager, executable) = fixture().await;
        let mut unrelated = Command::new(executable)
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        manager.rate_limits().await.unwrap();
        let owned_pid = manager.pid_for_test().await.unwrap();
        manager.shutdown().await.unwrap();
        assert!(!is_running(owned_pid));
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
    }
}
