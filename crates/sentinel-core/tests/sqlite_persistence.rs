use sentinel_core::{
    redact, CoreError, NormalizedAgentEvent, RunId, RunRepository, RunStatus, SafeRunError,
    TaskRequest, EVENT_SCHEMA_VERSION, MAX_EVENT_PAYLOAD_BYTES, MAX_SAFE_ERROR_CATEGORY_BYTES,
    MAX_SAFE_ERROR_MESSAGE_BYTES, MAX_TASK_BYTES,
};
use serde_json::json;
use sqlx::{Row, SqlitePool};
use tempfile::TempDir;

fn database_url(directory: &TempDir) -> String {
    format!(
        "sqlite://{}",
        directory.path().join("sentinel.sqlite").display()
    )
}

async fn repository() -> (TempDir, String, RunRepository) {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let url = database_url(&directory);
    let repository = RunRepository::open(&url).await.expect("open repository");
    (directory, url, repository)
}

fn event(run_id: RunId, sequence_number: u64, payload: serde_json::Value) -> NormalizedAgentEvent {
    NormalizedAgentEvent {
        run_id,
        sequence_number,
        event_type: "message".into(),
        schema_version: EVENT_SCHEMA_VERSION,
        occurred_at_ms: 1_725_000_000_000,
        payload,
    }
}

#[tokio::test]
async fn fresh_database_migrates_with_required_schema_and_configuration() {
    let (_directory, url, repository) = repository().await;
    let settings = repository.sqlite_settings().await.expect("settings");
    assert!(settings.foreign_keys);
    assert_eq!(settings.busy_timeout_ms, 5_000);
    assert!(matches!(settings.journal_mode.as_str(), "wal" | "memory"));

    let pool = SqlitePool::connect(&url).await.expect("inspect database");
    let objects: Vec<(String, String)> = sqlx::query("SELECT type, name FROM sqlite_master WHERE name IN ('runs', 'run_events', 'runs_recent_order', 'run_events_order') ORDER BY name")
        .fetch_all(&pool).await.expect("schema objects").into_iter()
        .map(|row| (row.get("type"), row.get("name"))).collect();
    assert_eq!(
        objects,
        vec![
            ("table".into(), "run_events".into()),
            ("index".into(), "run_events_order".into()),
            ("table".into(), "runs".into()),
            ("index".into(), "runs_recent_order".into()),
        ]
    );
    let project_columns: Vec<String> = sqlx::query("PRAGMA table_info(projects)")
        .fetch_all(&pool)
        .await
        .expect("project schema")
        .into_iter()
        .map(|row| row.get("name"))
        .collect();
    assert!(project_columns.contains(&"repository_fingerprint".into()));
    assert!(project_columns.contains(&"fingerprint_scheme".into()));
}

#[tokio::test]
async fn run_persistence_round_trip_and_recent_order_are_deterministic() {
    let (_directory, _url, repository) = repository().await;
    let first = repository
        .create_run(TaskRequest {
            task_text: "first task".into(),
        })
        .await
        .expect("first run");
    let second = repository
        .create_run(TaskRequest {
            task_text: "second task".into(),
        })
        .await
        .expect("second run");
    assert_eq!(
        repository.get_run(&first.id).await.expect("round trip"),
        first
    );

    let recent = repository.list_recent_runs().await.expect("recent runs");
    assert_eq!(recent.len(), 2);
    assert!(recent
        .windows(2)
        .all(|pair| (pair[0].created_at_ms, pair[0].id.to_string())
            >= (pair[1].created_at_ms, pair[1].id.to_string())));
    assert!(recent.iter().any(|run| run == &first));
    assert!(recent.iter().any(|run| run == &second));
}

#[tokio::test]
async fn event_persistence_round_trip_and_sequence_ordering_are_stable() {
    let (_directory, _url, repository) = repository().await;
    let run = repository
        .create_run(TaskRequest {
            task_text: "events".into(),
        })
        .await
        .expect("run");
    let later = event(run.id.clone(), 20, json!({"nested": [true, 7]}));
    let earlier = event(run.id.clone(), 3, json!({"message": "safe json"}));
    repository.append_event(&later).await.expect("later event");
    repository
        .append_event(&earlier)
        .await
        .expect("earlier event");

    let events = repository.list_events(&run.id).await.expect("events");
    assert_eq!(events, vec![earlier, later]);
    assert_eq!(events[0].schema_version, EVENT_SCHEMA_VERSION);
    assert_eq!(events[0].occurred_at_ms, 1_725_000_000_000);
}

#[tokio::test]
async fn duplicate_sequence_is_rejected_without_changing_original_event() {
    let (_directory, _url, repository) = repository().await;
    let run = repository
        .create_run(TaskRequest {
            task_text: "duplicate".into(),
        })
        .await
        .expect("run");
    let original = event(run.id.clone(), 1, json!({"original": true}));
    repository
        .append_event(&original)
        .await
        .expect("original event");
    assert!(matches!(
        repository
            .append_event(&event(run.id.clone(), 1, json!({"replacement": true})))
            .await,
        Err(CoreError::Storage)
    ));
    assert_eq!(
        repository.list_events(&run.id).await.expect("events"),
        vec![original]
    );
}

#[tokio::test]
async fn events_cannot_reference_missing_runs() {
    let (_directory, _url, repository) = repository().await;
    assert!(matches!(
        repository
            .append_event(&event(RunId::new(), 1, json!({})))
            .await,
        Err(CoreError::Storage)
    ));
}

#[tokio::test]
async fn transitions_persist_timestamps_and_terminal_runs_are_immutable() {
    let (_directory, _url, repository) = repository().await;
    let run = repository
        .create_run(TaskRequest {
            task_text: "transition".into(),
        })
        .await
        .expect("run");
    let preparing = repository
        .transition(&run.id, RunStatus::Preparing, None)
        .await
        .expect("prepare");
    assert_eq!(preparing.started_at_ms, None);
    let running = repository
        .transition(&run.id, RunStatus::Running, None)
        .await
        .expect("run");
    assert!(running.started_at_ms.is_some());
    let completed = repository
        .transition(&run.id, RunStatus::Completed, None)
        .await
        .expect("complete");
    assert!(completed.finished_at_ms.is_some());
    assert!(completed.finished_at_ms >= completed.started_at_ms);
    assert!(matches!(
        repository
            .transition(&run.id, RunStatus::Running, None)
            .await,
        Err(CoreError::Transition(_))
    ));
    assert_eq!(
        repository
            .get_run(&run.id)
            .await
            .expect("unchanged terminal run"),
        completed
    );
}

#[tokio::test]
async fn terminal_outcome_persists_exit_code_atomically_with_its_event() {
    let (_directory, _url, repository) = repository().await;
    let run = repository
        .create_run(TaskRequest {
            task_text: "terminal outcome".into(),
        })
        .await
        .expect("run");
    repository
        .transition(&run.id, RunStatus::Preparing, None)
        .await
        .expect("prepare");
    repository
        .transition(&run.id, RunStatus::Running, None)
        .await
        .expect("run");
    let event = event(run.id.clone(), 1, json!({"outcome": "failed"}));
    let failed = repository
        .finish_run(
            &run.id,
            RunStatus::Failed,
            Some(23),
            Some(SafeRunError {
                category: "process".into(),
                message: "safe failure".into(),
            }),
            Some(&event),
        )
        .await
        .expect("terminal outcome");
    assert_eq!(failed.exit_code, Some(23));
    assert_eq!(failed.status, RunStatus::Failed);
    assert_eq!(
        repository.list_events(&run.id).await.expect("events"),
        vec![event]
    );
}

#[tokio::test]
async fn invalid_transition_leaves_persisted_run_unchanged() {
    let (_directory, _url, repository) = repository().await;
    let run = repository
        .create_run(TaskRequest {
            task_text: "rollback".into(),
        })
        .await
        .expect("run");
    assert!(matches!(
        repository
            .transition(&run.id, RunStatus::Completed, None)
            .await,
        Err(CoreError::Transition(_))
    ));
    assert_eq!(
        repository.get_run(&run.id).await.expect("unchanged run"),
        run
    );
}

#[tokio::test]
async fn transition_and_event_are_atomic_on_success_and_failure() {
    let (_directory, _url, repository) = repository().await;
    let run = repository
        .create_run(TaskRequest {
            task_text: "atomic".into(),
        })
        .await
        .expect("run");
    let preparing_event = event(run.id.clone(), 1, json!({"state": "preparing"}));
    let preparing = repository
        .transition_with_event(&run.id, RunStatus::Preparing, None, &preparing_event)
        .await
        .expect("atomic prepare");
    assert_eq!(preparing.status, RunStatus::Preparing);
    assert_eq!(
        repository.list_events(&run.id).await.expect("events"),
        vec![preparing_event]
    );

    let duplicate = event(run.id.clone(), 1, json!({"state": "running"}));
    assert!(matches!(
        repository
            .transition_with_event(&run.id, RunStatus::Running, None, &duplicate)
            .await,
        Err(CoreError::Storage)
    ));
    assert_eq!(
        repository
            .get_run(&run.id)
            .await
            .expect("rolled back run")
            .status,
        RunStatus::Preparing
    );
    assert_eq!(
        repository
            .list_events(&run.id)
            .await
            .expect("rolled back events")
            .len(),
        1
    );
}

#[tokio::test]
async fn bounds_are_checked_before_persistence() {
    let (_directory, _url, repository) = repository().await;
    let run = repository
        .create_run(TaskRequest {
            task_text: "x".repeat(MAX_TASK_BYTES),
        })
        .await
        .expect("maximum task");
    assert!(matches!(
        repository
            .create_run(TaskRequest {
                task_text: "x".repeat(MAX_TASK_BYTES + 1)
            })
            .await,
        Err(CoreError::InvalidTask)
    ));
    let accepted = event(
        run.id.clone(),
        1,
        json!("x".repeat(MAX_EVENT_PAYLOAD_BYTES - 2)),
    );
    repository
        .append_event(&accepted)
        .await
        .expect("maximum payload");
    let rejected = event(
        run.id.clone(),
        2,
        json!("x".repeat(MAX_EVENT_PAYLOAD_BYTES - 1)),
    );
    assert!(matches!(
        repository.append_event(&rejected).await,
        Err(CoreError::PayloadTooLarge)
    ));
    assert_eq!(
        repository.list_events(&run.id).await.expect("events"),
        vec![accepted]
    );
}

#[tokio::test]
async fn safe_errors_redact_credentials_without_hiding_ordinary_context() {
    let (_directory, _url, repository) = repository().await;
    let run = repository
        .create_run(TaskRequest {
            task_text: "errors".into(),
        })
        .await
        .expect("run");
    repository
        .transition(&run.id, RunStatus::Preparing, None)
        .await
        .expect("prepare");
    let failed = repository.transition(&run.id, RunStatus::Failed, Some(SafeRunError {
        category: "remote token=category-secret".into(),
        message: "request token expired; api_key=api-secret; password: hunter2; Authorization: Bearer bearer-secret; sk-live-secret".into(),
    })).await.expect("failure");
    let error = failed.error.expect("safe error");
    for secret in [
        "category-secret",
        "api-secret",
        "hunter2",
        "bearer-secret",
        "sk-live-secret",
    ] {
        assert!(!error.message.contains(secret) && !error.category.contains(secret));
    }
    assert!(error.message.contains("request token expired"));
    assert_eq!(
        redact("ordinary token refresh failed"),
        "ordinary token refresh failed"
    );
}

#[tokio::test]
async fn repository_bounds_safe_errors_on_every_public_transition_path() {
    let (_directory, _url, repository) = repository().await;
    let ordinary = repository
        .create_run(TaskRequest {
            task_text: "ordinary error".into(),
        })
        .await
        .expect("run");
    repository
        .transition(&ordinary.id, RunStatus::Preparing, None)
        .await
        .expect("prepare");
    let ordinary_error = SafeRunError {
        category: "process".into(),
        message: "ordinary diagnostic".into(),
    };
    let ordinary_result = repository
        .transition(
            &ordinary.id,
            RunStatus::Failed,
            Some(ordinary_error.clone()),
        )
        .await
        .expect("transition");
    assert_eq!(ordinary_result.error, Some(ordinary_error));

    let direct = repository
        .create_run(TaskRequest {
            task_text: "direct transition".into(),
        })
        .await
        .expect("run");
    repository
        .transition(&direct.id, RunStatus::Preparing, None)
        .await
        .expect("prepare");
    let direct_result = repository
        .transition(
            &direct.id,
            RunStatus::Failed,
            Some(SafeRunError {
                category: "日本語".repeat(300),
                message: format!("prefix token=super-secret {}", "😀".repeat(300)),
            }),
        )
        .await
        .expect("direct transition");
    assert_bounded_safe_error(direct_result.error.expect("safe error"));

    let atomic = repository
        .create_run(TaskRequest {
            task_text: "atomic transition".into(),
        })
        .await
        .expect("run");
    let event = event(atomic.id.clone(), 1, json!({"state": "preparing"}));
    let atomic_result = repository
        .transition_with_event(
            &atomic.id,
            RunStatus::Preparing,
            Some(SafeRunError {
                category: "ภาษาไทย".repeat(200),
                message: "emoji ".to_owned() + &"😀".repeat(300),
            }),
            &event,
        )
        .await
        .expect("atomic transition");
    assert_bounded_safe_error(atomic_result.error.expect("safe error"));

    let terminal = repository
        .create_run(TaskRequest {
            task_text: "terminal error".into(),
        })
        .await
        .expect("run");
    repository
        .transition(&terminal.id, RunStatus::Preparing, None)
        .await
        .expect("prepare");
    repository
        .transition(&terminal.id, RunStatus::Running, None)
        .await
        .expect("running");
    let terminal_result = repository
        .finish_run(
            &terminal.id,
            RunStatus::Failed,
            Some(1),
            Some(SafeRunError {
                category: "combining e\u{301}".repeat(200),
                message: "password=very-secret ".to_owned() + &"日本語".repeat(300),
            }),
            None,
        )
        .await
        .expect("finish");
    assert_bounded_safe_error(terminal_result.error.expect("safe error"));
}

fn assert_bounded_safe_error(error: SafeRunError) {
    assert!(error.category.len() <= MAX_SAFE_ERROR_CATEGORY_BYTES);
    assert!(error.message.len() <= MAX_SAFE_ERROR_MESSAGE_BYTES);
    assert!(std::str::from_utf8(error.category.as_bytes()).is_ok());
    assert!(std::str::from_utf8(error.message.as_bytes()).is_ok());
    assert!(!error.message.contains("super-secret") && !error.message.contains("very-secret"));
}
