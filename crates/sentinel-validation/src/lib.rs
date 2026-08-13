use sentinel_core::{
    redact,
    v3::{
        CreateArtifact, CreateValidationResult, TaskId, ValidationExecution, ValidationLifecycle,
    },
    RunRepository,
};
use sentinel_process::{ProcessEvent, SupervisedProcess};
use sentinel_worktree::WorktreeTransaction;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::{sync::watch, time};

pub const OUTPUT_LIMIT: usize = 16 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationProfile {
    pub id: String,
    pub steps: Vec<ValidationStep>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationStep {
    pub name: String,
    pub argv: Vec<String>,
    pub required: bool,
    pub timeout_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationReport {
    pub results: Vec<ValidationExecution>,
}
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("invalid validation profile")]
    InvalidProfile,
    #[error("worktree unavailable")]
    Worktree,
    #[error("storage failure")]
    Storage,
}

pub struct ValidationRunner;
impl ValidationRunner {
    pub async fn run_profile(
        repository: RunRepository,
        task_id: TaskId,
        main: &std::path::Path,
        profile: ValidationProfile,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<ValidationReport, ValidationError> {
        if profile.id.trim().is_empty()
            || profile.steps.is_empty()
            || profile.steps.iter().any(invalid_step)
        {
            return Err(ValidationError::InvalidProfile);
        }
        let worktree = WorktreeTransaction::reopen(repository.clone(), task_id.clone(), main)
            .await
            .map_err(|_| ValidationError::Worktree)?;
        let mut results = Vec::new();
        for step in profile.steps {
            let result = repository
                .v3()
                .create_validation_result(
                    CreateValidationResult {
                        task_id: task_id.clone(),
                        profile_id: profile.id.clone(),
                        check_name: step.name.clone(),
                        required: step.required,
                        summary: None,
                    },
                    now(),
                )
                .await
                .map_err(|_| ValidationError::Storage)?;
            let running = repository
                .v3()
                .transition_validation_result(&result, ValidationLifecycle::Running, None, now())
                .await
                .map_err(|_| ValidationError::Storage)?;
            let execution = execute(
                &step,
                std::path::Path::new(&worktree.worktree_path),
                cancellation,
                running.id.clone(),
            )
            .await;
            let lifecycle = match execution.outcome.as_str() {
                "passed" => ValidationLifecycle::Passed,
                "failed" => ValidationLifecycle::Failed,
                "timed_out" => ValidationLifecycle::Incomplete,
                "cancelled" => ValidationLifecycle::Incomplete,
                _ => ValidationLifecycle::Incomplete,
            };
            let summary = Some(execution.outcome.clone());
            repository
                .v3()
                .save_validation_execution(&execution)
                .await
                .map_err(|_| ValidationError::Storage)?;
            repository.v3().create_artifact(CreateArtifact { task_id: task_id.clone(), kind: "validation_output".into(), display_name: step.name, content_hash: None, metadata: serde_json::json!({"stdout": execution.stdout, "stderr": execution.stderr, "outcome": execution.outcome}) }, now()).await.map_err(|_| ValidationError::Storage)?;
            repository
                .v3()
                .transition_validation_result(&running, lifecycle, summary, now())
                .await
                .map_err(|_| ValidationError::Storage)?;
            results.push(execution);
        }
        Ok(ValidationReport { results })
    }
}
fn invalid_step(step: &ValidationStep) -> bool {
    step.name.trim().is_empty()
        || step.argv.is_empty()
        || step.argv[0].trim().is_empty()
        || step.timeout_ms == 0
}
async fn execute(
    step: &ValidationStep,
    cwd: &std::path::Path,
    cancellation: &mut watch::Receiver<bool>,
    validation_id: sentinel_core::v3::ValidationResultId,
) -> ValidationExecution {
    let started = now();
    let started_instant = std::time::Instant::now();
    let args: Vec<&str> = step.argv.iter().skip(1).map(String::as_str).collect();
    let mut stdout = String::new();
    let mut stderr = String::new();
    let (mut process, mut events) =
        match SupervisedProcess::start_in_sanitized(&step.argv[0], &args, Some(cwd)).await {
            Ok(value) => value,
            Err(_) => {
                return execution(
                    validation_id,
                    step,
                    None,
                    started,
                    started_instant,
                    stdout,
                    stderr,
                    "invalid",
                )
            }
        };
    let deadline = time::Instant::now() + Duration::from_millis(step.timeout_ms);
    let exit: Option<i32>;
    let outcome;
    loop {
        tokio::select! {
            changed = cancellation.changed() => if changed.is_ok() && *cancellation.borrow() { exit = process.cancel(Duration::from_secs(1)).await.ok().flatten(); outcome = "cancelled"; break; },
            _ = time::sleep_until(deadline) => { exit = process.cancel(Duration::from_secs(1)).await.ok().flatten(); outcome = "timed_out"; break; },
            event = events.recv() => match event { Some(ProcessEvent::Stdout(line)) => append(&mut stdout, &line), Some(ProcessEvent::Stderr(line)) => append(&mut stderr, &line), Some(ProcessEvent::Exited(code)) => { exit=code; outcome=if code == Some(0) { "passed" } else { "failed" }; break; }, Some(ProcessEvent::OutputError) | None => { exit = process.wait().await.ok().flatten(); outcome=if exit == Some(0) { "passed" } else { "failed" }; break; } }
        }
    }
    execution(
        validation_id,
        step,
        exit.map(i64::from),
        started,
        started_instant,
        stdout,
        stderr,
        outcome,
    )
}
fn execution(
    id: sentinel_core::v3::ValidationResultId,
    step: &ValidationStep,
    exit_code: Option<i64>,
    started: i64,
    instant: std::time::Instant,
    stdout: String,
    stderr: String,
    outcome: &str,
) -> ValidationExecution {
    ValidationExecution {
        validation_id: id,
        command_json: serde_json::to_string(&step.argv).unwrap_or_default(),
        exit_code,
        duration_ms: instant.elapsed().as_millis() as i64,
        stdout,
        stderr,
        outcome: outcome.into(),
        started_at_ms: started,
        finished_at_ms: now(),
    }
}
fn append(target: &mut String, line: &str) {
    let value = redact(line);
    if target.len() < OUTPUT_LIMIT {
        let remaining = OUTPUT_LIMIT - target.len();
        target.push_str(&value[..value.len().min(remaining)]);
        if target.len() < OUTPUT_LIMIT {
            target.push('\n');
        }
    }
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::v3::{CreateTask, ValidationLifecycle};
    use std::{fs, path::Path, process::Command};
    use tempfile::TempDir;
    fn git(dir: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .current_dir(dir)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    async fn fixture() -> (TempDir, TempDir, RunRepository, TaskId) {
        let main = tempfile::tempdir().unwrap();
        git(main.path(), &["init"]);
        git(main.path(), &["config", "user.email", "a@b.c"]);
        git(main.path(), &["config", "user.name", "A"]);
        fs::write(main.path().join("x"), "x").unwrap();
        git(main.path(), &["add", "x"]);
        git(main.path(), &["commit", "-m", "x"]);
        let db = tempfile::tempdir().unwrap();
        let repo = RunRepository::open(&format!(
            "sqlite://{}",
            db.path().join("v.sqlite").display()
        ))
        .await
        .unwrap();
        let task = repo
            .v3()
            .create_task(
                CreateTask {
                    project_id: None,
                    workflow_id: "p".into(),
                    summary: "s".into(),
                },
                1,
            )
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().to_path_buf();
        std::mem::forget(root);
        sentinel_worktree::WorktreeTransaction::create(
            repo.clone(),
            task.id.clone(),
            main.path(),
            &root_path,
        )
        .await
        .unwrap();
        (main, db, repo, task.id)
    }
    fn step(name: &str, argv: &[&str]) -> ValidationStep {
        ValidationStep {
            name: name.into(),
            argv: argv.iter().map(|v| (*v).into()).collect(),
            required: true,
            timeout_ms: 500,
        }
    }
    async fn run(
        repo: RunRepository,
        task: TaskId,
        main: &Path,
        steps: Vec<ValidationStep>,
    ) -> ValidationReport {
        let (_tx, mut cancel) = watch::channel(false);
        ValidationRunner::run_profile(
            repo.clone(),
            task.clone(),
            main,
            ValidationProfile {
                id: "p".into(),
                steps,
            },
            &mut cancel,
        )
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn passing_and_failing_commands_are_classified() {
        let (main, _db, repo, task) = fixture().await;
        let r = run(
            repo.clone(),
            task.clone(),
            main.path(),
            vec![
                step("pass", &["/usr/bin/true"]),
                step("fail", &["/usr/bin/false"]),
            ],
        )
        .await;
        assert_eq!(
            r.results
                .iter()
                .map(|r| r.outcome.as_str())
                .collect::<Vec<_>>(),
            vec!["passed", "failed"]
        );
    }
    #[tokio::test]
    async fn timeout_and_cancellation_are_explicit() {
        let (main, _db, repo, task) = fixture().await;
        let r = run(
            repo.clone(),
            task.clone(),
            main.path(),
            vec![step("slow", &["/bin/sh", "-c", "sleep 1"])],
        )
        .await;
        assert_eq!(r.results[0].outcome, "timed_out");
        let (tx, mut rx) = watch::channel(false);
        let repo2 = repo.clone();
        let main_path = main.path().to_path_buf();
        let task2 = task.clone();
        let join = tokio::spawn(async move {
            ValidationRunner::run_profile(
                repo2,
                task2,
                &main_path,
                ValidationProfile {
                    id: "c".into(),
                    steps: vec![step("slow", &["/bin/sh", "-c", "sleep 1"])],
                },
                &mut rx,
            )
            .await
            .unwrap()
        });
        tx.send(true).unwrap();
        assert_eq!(join.await.unwrap().results[0].outcome, "cancelled");
    }
    #[tokio::test]
    async fn invalid_profile_and_wrong_worktree_are_refused() {
        let (main, _db, repo, task) = fixture().await;
        let (_tx, mut rx) = watch::channel(false);
        assert!(matches!(
            ValidationRunner::run_profile(
                repo.clone(),
                task.clone(),
                main.path(),
                ValidationProfile {
                    id: "".into(),
                    steps: vec![]
                },
                &mut rx
            )
            .await,
            Err(ValidationError::InvalidProfile)
        ));
        assert!(matches!(
            ValidationRunner::run_profile(
                repo,
                task,
                Path::new("/tmp"),
                ValidationProfile {
                    id: "p".into(),
                    steps: vec![step("x", &["/usr/bin/true"])]
                },
                &mut rx
            )
            .await,
            Err(ValidationError::Worktree)
        ));
    }
    #[tokio::test]
    async fn truncates_redacts_orders_and_persists() {
        let (main, _db, repo, task) = fixture().await;
        let huge = "x".repeat(OUTPUT_LIMIT + 100);
        let script = format!("printf 'token=secret'; printf '{}';", huge);
        let r = run(
            repo.clone(),
            task.clone(),
            main.path(),
            vec![
                step("one", &["/bin/sh", "-c", &script]),
                step("two", &["/usr/bin/true"]),
            ],
        )
        .await;
        assert_eq!(r.results.len(), 2);
        assert!(r.results[0].stdout.len() <= OUTPUT_LIMIT);
        assert!(!r.results[0].stdout.contains("secret"));
        let result = repo
            .v3()
            .get_validation_execution(&r.results[0].validation_id)
            .await
            .unwrap();
        assert_eq!(result.stdout, r.results[0].stdout);
    }
    #[tokio::test]
    async fn restart_recovery_marks_running_incomplete() {
        let (main, _db, repo, task) = fixture().await;
        let value = repo
            .v3()
            .create_validation_result(
                CreateValidationResult {
                    task_id: task,
                    profile_id: "p".into(),
                    check_name: "x".into(),
                    required: true,
                    summary: None,
                },
                1,
            )
            .await
            .unwrap();
        let running = repo
            .v3()
            .transition_validation_result(&value, ValidationLifecycle::Running, None, 2)
            .await
            .unwrap();
        assert_eq!(
            repo.v3().recover_interrupted_validations(3).await.unwrap(),
            1
        );
        assert_eq!(
            repo.v3()
                .get_validation_result(&running.id)
                .await
                .unwrap()
                .lifecycle,
            ValidationLifecycle::Incomplete
        );
        drop(main);
    }
}
