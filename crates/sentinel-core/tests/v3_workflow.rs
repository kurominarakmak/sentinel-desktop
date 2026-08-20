use sentinel_core::{
    v3::{
        ApprovalLifecycle, CreateApproval, CreateArtifact, CreateRepairRound, CreateReviewFinding,
        CreateSession, CreateTask, CreateValidationResult, EventId, EventKind, FindingDisposition,
        NormalizedEventEnvelope, RecoveryCondition, RepairRoundLifecycle, SessionLifecycle,
        SupervisorRequestReservation, TaskLifecycle, ValidationLifecycle,
    },
    CoreError, RunRepository,
};
use serde_json::json;
use sqlx::SqlitePool;
use tempfile::TempDir;

const TIME: i64 = 1_725_000_000_000;

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

async fn task(repository: &RunRepository) -> sentinel_core::v3::Task {
    repository
        .v3()
        .create_task(
            CreateTask {
                project_id: Some("project-opaque-id".into()),
                workflow_id: "default".into(),
                summary: "Add fixture".into(),
            },
            TIME,
        )
        .await
        .expect("task")
}

fn event(
    task_id: sentinel_core::v3::TaskId,
    event_id: EventId,
    sequence: u64,
    kind: EventKind,
) -> NormalizedEventEnvelope {
    NormalizedEventEnvelope {
        event_id,
        task_id,
        session_id: None,
        provider: "fixture.provider".into(),
        kind,
        schema_version: 1,
        occurred_at_ms: TIME,
        sequence_number: sequence,
        causation_id: Some("cause-1".into()),
        correlation_id: Some("correlation-1".into()),
        payload: json!({"message":"safe"}),
        raw_diagnostic_payload: Some(json!({"provider_shape":"retained"})),
    }
}

#[tokio::test]
async fn turn_completed_round_trips_as_a_distinct_durable_kind() {
    let (_directory, _url, repository) = repository().await;
    let task = task(&repository).await;
    let completed = event(task.id.clone(), EventId::new(), 1, EventKind::TurnCompleted);
    repository.v3().append_event(&completed).await.unwrap();
    assert_eq!(
        repository.v3().list_events(&task.id).await.unwrap()[0].kind,
        EventKind::TurnCompleted
    );
}

#[test]
fn normalized_event_round_trips_and_unknown_kinds_are_explicit() {
    let envelope = event(
        sentinel_core::v3::TaskId::new(),
        EventId::new(),
        3,
        EventKind::Unknown {
            discriminator: "future/provider.event".into(),
        },
    );
    let encoded = serde_json::to_string(&envelope).expect("serialize");
    let decoded: NormalizedEventEnvelope = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, envelope);
}

#[test]
fn state_transitions_are_closed_for_every_v3_lifecycle() {
    assert!(matches!(
        TaskLifecycle::Draft.transition(TaskLifecycle::Preparing),
        Ok(TaskLifecycle::Preparing)
    ));
    assert!(TaskLifecycle::Draft
        .transition(TaskLifecycle::Finalized)
        .is_err());
    assert!(matches!(
        SessionLifecycle::Created.transition(SessionLifecycle::Starting),
        Ok(SessionLifecycle::Starting)
    ));
    assert!(SessionLifecycle::Completed
        .transition(SessionLifecycle::Active)
        .is_err());
    assert!(matches!(
        ApprovalLifecycle::Pending.transition(ApprovalLifecycle::Approved),
        Ok(ApprovalLifecycle::Approved)
    ));
    assert!(ApprovalLifecycle::Approved
        .transition(ApprovalLifecycle::Denied)
        .is_err());
    assert!(matches!(
        ValidationLifecycle::Pending.transition(ValidationLifecycle::Running),
        Ok(ValidationLifecycle::Running)
    ));
    assert!(ValidationLifecycle::Passed
        .transition(ValidationLifecycle::Running)
        .is_err());
    assert!(matches!(
        FindingDisposition::Reported.transition(FindingDisposition::ConfirmedBlocking),
        Ok(FindingDisposition::ConfirmedBlocking)
    ));
    assert!(FindingDisposition::Informational
        .transition(FindingDisposition::Repaired)
        .is_err());
    assert!(matches!(
        RepairRoundLifecycle::Active.transition(RepairRoundLifecycle::AwaitingValidation),
        Ok(RepairRoundLifecycle::AwaitingValidation)
    ));
    assert!(RepairRoundLifecycle::Resolved
        .transition(RepairRoundLifecycle::Active)
        .is_err());
}

#[tokio::test]
async fn events_are_ordered_unknown_events_are_preserved_and_replay_is_idempotent() {
    let (_directory, _url, repository) = repository().await;
    let task = task(&repository).await;
    let first = event(task.id.clone(), EventId::new(), 1, EventKind::Message);
    let second = event(
        task.id.clone(),
        EventId::new(),
        2,
        EventKind::Unknown {
            discriminator: "new.event".into(),
        },
    );
    repository
        .v3()
        .append_event(&first)
        .await
        .expect("first event");
    repository
        .v3()
        .append_event(&second)
        .await
        .expect("second event");
    repository
        .v3()
        .append_event(&second)
        .await
        .expect("same replay is no-op");
    assert!(matches!(
        repository
            .v3()
            .append_event(&event(
                task.id.clone(),
                EventId::new(),
                2,
                EventKind::Message
            ))
            .await,
        Err(CoreError::V3Conflict)
    ));
    let events = repository.v3().list_events(&task.id).await.expect("events");
    assert_eq!(events, vec![first, second]);
}

#[tokio::test]
async fn task_transition_and_event_are_atomic() {
    let (_directory, _url, repository) = repository().await;
    let task = task(&repository).await;
    let event = event(
        task.id.clone(),
        EventId::new(),
        1,
        EventKind::SessionStarted,
    );
    let preparing = repository
        .v3()
        .transition_task_with_event(&task, TaskLifecycle::Preparing, &event, TIME + 1)
        .await
        .expect("atomic transition");
    assert_eq!(preparing.lifecycle, TaskLifecycle::Preparing);
    assert_eq!(
        repository
            .v3()
            .list_events(&task.id)
            .await
            .expect("event list"),
        vec![event]
    );
    assert!(matches!(
        repository
            .v3()
            .transition_task(&task, TaskLifecycle::Preparing, TIME + 2)
            .await,
        Err(CoreError::V3Conflict)
    ));
}

#[tokio::test]
async fn sessions_approvals_validation_findings_rounds_and_artifacts_survive_reopen() {
    let (directory, url, repository) = repository().await;
    let task = task(&repository).await;
    let v3 = repository.v3();
    let session = v3
        .create_session(
            CreateSession {
                task_id: task.id.clone(),
                provider: "fixture.provider".into(),
                provider_session_ref: "opaque-session".into(),
            },
            TIME,
        )
        .await
        .expect("session");
    let starting = v3
        .transition_session(&session, SessionLifecycle::Starting, TIME + 1)
        .await
        .expect("starting");
    let approval = v3
        .create_approval(
            CreateApproval {
                task_id: task.id.clone(),
                session_id: Some(session.id.clone()),
                action_kind: "provider_permission".into(),
                summary: "Needs a decision".into(),
            },
            TIME,
        )
        .await
        .expect("approval");
    let approved = v3
        .transition_approval(&approval, ApprovalLifecycle::Approved, TIME + 1)
        .await
        .expect("approve");
    let validation = v3
        .create_validation_result(
            CreateValidationResult {
                task_id: task.id.clone(),
                profile_id: "default".into(),
                check_name: "test".into(),
                required: true,
                summary: None,
            },
            TIME,
        )
        .await
        .expect("validation");
    let running = v3
        .transition_validation_result(
            &validation,
            ValidationLifecycle::Running,
            Some("started".into()),
            TIME + 1,
        )
        .await
        .expect("running");
    let round = v3
        .create_repair_round(
            CreateRepairRound {
                task_id: task.id.clone(),
                round_number: 1,
            },
            TIME,
        )
        .await
        .expect("round");
    let active_round = v3
        .transition_repair_round(&round, RepairRoundLifecycle::Active, TIME + 1)
        .await
        .expect("active round");
    let finding = v3
        .create_review_finding(
            CreateReviewFinding {
                task_id: task.id.clone(),
                repair_round_id: Some(round.id.clone()),
                review_generation_id: None,
                severity: "high".into(),
                summary: "fixture finding".into(),
                evidence: json!({"path":"src/lib.rs","line":1}),
            },
            TIME,
        )
        .await
        .expect("finding");
    let confirmed = v3
        .transition_review_finding(&finding, FindingDisposition::ConfirmedBlocking, TIME + 1)
        .await
        .expect("confirmed");
    let artifact = v3
        .create_artifact(
            CreateArtifact {
                task_id: task.id.clone(),
                kind: "diff".into(),
                display_name: "fixture diff".into(),
                content_hash: Some("abc".into()),
                metadata: json!({"bytes":12}),
            },
            TIME,
        )
        .await
        .expect("artifact");
    drop(repository);
    let reopened = RunRepository::open(&url).await.expect("reopen");
    let v3 = reopened.v3();
    assert_eq!(
        v3.get_session(&session.id).await.expect("session"),
        starting
    );
    assert_eq!(
        v3.get_approval(&approval.id).await.expect("approval"),
        approved
    );
    assert_eq!(
        v3.get_validation_result(&validation.id)
            .await
            .expect("validation"),
        running
    );
    assert_eq!(
        v3.get_repair_round(&round.id).await.expect("round"),
        active_round
    );
    assert_eq!(
        v3.get_review_finding(&finding.id).await.expect("finding"),
        confirmed
    );
    assert_eq!(
        v3.get_artifact(&artifact.id).await.expect("artifact"),
        artifact
    );
    drop(directory);
}

#[tokio::test]
async fn restart_restores_active_tasks_without_assuming_a_live_provider() {
    let (_directory, _url, repository) = repository().await;
    let active_task = task(&repository).await;
    let untouched_draft = task(&repository).await;
    let preparing = repository
        .v3()
        .transition_task(&active_task, TaskLifecycle::Preparing, TIME + 1)
        .await
        .expect("preparing");
    let implementing = repository
        .v3()
        .transition_task(&preparing, TaskLifecycle::Implementing, TIME + 2)
        .await
        .expect("implementing");
    let session = repository
        .v3()
        .create_session(
            CreateSession {
                task_id: implementing.id.clone(),
                provider: "fixture.provider".into(),
                provider_session_ref: "opaque".into(),
            },
            TIME,
        )
        .await
        .expect("session");
    let starting = repository
        .v3()
        .transition_session(&session, SessionLifecycle::Starting, TIME + 1)
        .await
        .expect("starting");
    let active = repository
        .v3()
        .transition_session(&starting, SessionLifecycle::Active, TIME + 2)
        .await
        .expect("active");
    let restored = repository
        .v3()
        .restore_unfinished_tasks(TIME + 3)
        .await
        .expect("restore");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].lifecycle, TaskLifecycle::Recovering);
    assert_eq!(
        restored[0].recovery_previous_lifecycle,
        Some(TaskLifecycle::Implementing)
    );
    assert_eq!(
        restored[0].recovery_condition,
        RecoveryCondition::NoLiveProcessAssumed
    );
    assert_eq!(
        repository
            .v3()
            .get_session(&active.id)
            .await
            .expect("session")
            .lifecycle,
        SessionLifecycle::RecoveryRequired
    );
    assert_eq!(
        repository
            .v3()
            .get_task(&untouched_draft.id)
            .await
            .expect("draft")
            .lifecycle,
        TaskLifecycle::Draft
    );
    assert!(repository
        .v3()
        .restore_unfinished_tasks(TIME + 4)
        .await
        .expect("idempotent recovery")
        .is_empty());
}

#[tokio::test]
async fn existing_v1_database_migrates_additively_and_corruption_fails_closed() {
    let (directory, url, repository) = repository().await;
    let task = task(&repository).await;
    let pool = SqlitePool::connect(&url).await.expect("inspection pool");
    let v1_runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs")
        .fetch_one(&pool)
        .await
        .expect("legacy table persists");
    assert_eq!(v1_runs, 0);
    let v3_table: String =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name='v3_tasks'")
            .fetch_one(&pool)
            .await
            .expect("v3 migration");
    assert_eq!(v3_table, "v3_tasks");
    sqlx::query("UPDATE v3_tasks SET lifecycle='recovering', recovery_condition='none', recovery_previous_lifecycle=NULL WHERE id=?")
        .bind(task.id.to_string()).execute(&pool).await.expect("incomplete fixture");
    assert!(matches!(
        repository.v3().get_task(&task.id).await,
        Err(CoreError::CorruptV3State)
    ));
    sqlx::query("UPDATE v3_tasks SET lifecycle='not_a_lifecycle' WHERE id=?")
        .bind(task.id.to_string())
        .execute(&pool)
        .await
        .expect("corrupt fixture");
    assert!(matches!(
        repository.v3().get_task(&task.id).await,
        Err(CoreError::CorruptV3State)
    ));
    drop(directory);
}

#[tokio::test]
async fn supervisor_mutation_responses_are_durable_and_intent_bound() {
    let (directory, url, repository) = repository().await;
    assert_eq!(
        repository
            .v3()
            .reserve_supervisor_request("request-1", "start_task", "sha256:one", TIME)
            .await
            .expect("reserve"),
        SupervisorRequestReservation::New
    );
    assert_eq!(
        repository
            .v3()
            .reserve_supervisor_request("request-1", "start_task", "sha256:one", TIME + 1)
            .await
            .expect("pending replay"),
        SupervisorRequestReservation::Pending
    );
    repository
        .v3()
        .complete_supervisor_request(
            "request-1",
            r#"{"kind":"task_start_result","accepted":true}"#,
            TIME + 2,
        )
        .await
        .expect("complete");
    drop(repository);
    let reopened = RunRepository::open(&url).await.expect("reopen");
    assert_eq!(
        reopened
            .v3()
            .reserve_supervisor_request("request-1", "start_task", "sha256:one", TIME + 3)
            .await
            .expect("completed replay"),
        SupervisorRequestReservation::Completed(
            r#"{"kind":"task_start_result","accepted":true}"#.into()
        )
    );
    assert!(matches!(
        reopened
            .v3()
            .reserve_supervisor_request("request-1", "attention_action", "sha256:two", TIME + 4)
            .await,
        Err(CoreError::V3Conflict)
    ));
    drop(directory);
}
