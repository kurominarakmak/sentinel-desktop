use sentinel_core::{
    v3::{CreateTask, CreateTaskWorktree, TaskId, TaskLifecycle},
    RunRepository,
};
use sentinel_review::{ActionApprovalContext, ActionAuthorizationError, ActionAuthorizationGuard};
use tempfile::tempdir;

const SHA: &str = "1111111111111111111111111111111111111111";

async fn fixture() -> (
    tempfile::TempDir,
    RunRepository,
    TaskId,
    ActionApprovalContext,
) {
    let directory = tempdir().unwrap();
    let database = directory.path().join("state.db");
    let repo = RunRepository::open(&format!("sqlite://{}", database.display()))
        .await
        .unwrap();
    let task = repo
        .v3()
        .create_task(
            CreateTask {
                project_id: None,
                workflow_id: "test".into(),
                summary: "test".into(),
            },
            1,
        )
        .await
        .unwrap();
    let root = directory.path().join("repo");
    let worktree = directory.path().join("worktree");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    repo.v3()
        .create_task_worktree(
            CreateTaskWorktree {
                task_id: task.id.clone(),
                repository_root: root.display().to_string(),
                worktree_path: worktree.display().to_string(),
                branch: "agent-sentinel/v3-test".into(),
                base_commit: SHA.into(),
            },
            1,
        )
        .await
        .unwrap();
    let context = ActionApprovalContext {
        task_id: task.id.to_string(),
        action: "discard".into(),
        worktree_path: worktree.display().to_string(),
        repository_root: root.display().to_string(),
        branch: "agent-sentinel/v3-test".into(),
        head: SHA.into(),
        base_commit: SHA.into(),
        evidence_digest: "digest-a".into(),
    };
    (directory, repo, task.id, context)
}

#[tokio::test]
async fn exact_capability_is_single_use_and_audited() {
    let (_dir, repo, _id, context) = fixture().await;
    let approval = ActionAuthorizationGuard::issue(repo.clone(), context.clone(), 2)
        .await
        .unwrap();
    ActionAuthorizationGuard::validate_and_consume(repo.clone(), &approval.id, &context, 3)
        .await
        .unwrap();
    assert_eq!(
        ActionAuthorizationGuard::validate_and_consume(repo.clone(), &approval.id, &context, 4)
            .await,
        Err(ActionAuthorizationError::Denied)
    );
    let audit = repo
        .v3()
        .list_artifacts(&TaskId(context.task_id.clone()))
        .await
        .unwrap();
    assert_eq!(
        audit
            .iter()
            .filter(|item| item.kind == "authorization_audit")
            .count(),
        1
    );
}

#[tokio::test]
async fn capability_mismatches_and_recovery_fail_without_consuming() {
    let (_dir, repo, id, context) = fixture().await;
    let approval = ActionAuthorizationGuard::issue(repo.clone(), context.clone(), 2)
        .await
        .unwrap();
    for changed in [
        ActionApprovalContext {
            task_id: "wrong".into(),
            ..context.clone()
        },
        ActionApprovalContext {
            action: "merge".into(),
            ..context.clone()
        },
        ActionApprovalContext {
            branch: "other".into(),
            ..context.clone()
        },
        ActionApprovalContext {
            head: "2222222222222222222222222222222222222222".into(),
            ..context.clone()
        },
        ActionApprovalContext {
            evidence_digest: "changed".into(),
            ..context.clone()
        },
    ] {
        assert_eq!(
            ActionAuthorizationGuard::validate_and_consume(repo.clone(), &approval.id, &changed, 3)
                .await,
            Err(ActionAuthorizationError::Denied)
        );
    }
    assert_eq!(
        repo.v3()
            .get_approval(&approval.id)
            .await
            .unwrap()
            .lifecycle,
        sentinel_core::v3::ApprovalLifecycle::Pending
    );
    let task = repo.v3().get_task(&id).await.unwrap();
    let _task = repo
        .v3()
        .transition_task(&task, TaskLifecycle::Preparing, 4)
        .await
        .unwrap();
    repo.v3().restore_unfinished_tasks(5).await.unwrap();
    assert_eq!(
        ActionAuthorizationGuard::validate_and_consume(repo, &approval.id, &context, 6).await,
        Err(ActionAuthorizationError::RecoveryRequired)
    );
}

#[tokio::test]
async fn concurrent_consumers_have_exactly_one_winner() {
    let (_dir, repo, _id, context) = fixture().await;
    let approval = ActionAuthorizationGuard::issue(repo.clone(), context.clone(), 2)
        .await
        .unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    let one = {
        let repo = repo.clone();
        let context = context.clone();
        let barrier = barrier.clone();
        let approval = approval.id.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            ActionAuthorizationGuard::validate_and_consume(repo, &approval, &context, 3)
                .await
                .is_ok()
        })
    };
    let two = {
        let repo = repo.clone();
        let context = context.clone();
        let barrier = barrier.clone();
        let approval = approval.id.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            ActionAuthorizationGuard::validate_and_consume(repo, &approval, &context, 3)
                .await
                .is_ok()
        })
    };
    barrier.wait().await;
    assert_ne!(one.await.unwrap(), two.await.unwrap());
}
