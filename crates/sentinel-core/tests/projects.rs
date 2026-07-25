use sentinel_core::{
    CoreError, ProjectFingerprintScheme, ProjectRegistration, ProjectValidationState, RunRepository,
};
use sqlx::SqlitePool;
use tempfile::TempDir;

fn database_url(directory: &TempDir) -> String {
    format!(
        "sqlite://{}",
        directory.path().join("sentinel.sqlite").display()
    )
}

fn registration(identity: &str, name: &str) -> ProjectRegistration {
    ProjectRegistration {
        display_name: name.into(),
        repository_identity: identity.into(),
        repository_fingerprint: format!("unix:fixture:{identity}"),
        fingerprint_scheme: ProjectFingerprintScheme::StrongV1,
        repository_root: format!("/private/fixture/{identity}"),
        primary_root: format!("/private/fixture/{identity}"),
        git_common_dir: format!("/private/fixture/{identity}/.git"),
        branch: Some("main".into()),
        head: Some("abc123".into()),
        validation_state: ProjectValidationState::Valid,
        is_primary_worktree: true,
    }
}

#[tokio::test]
async fn projects_persist_list_deterministically_and_detect_duplicates() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let url = database_url(&directory);
    let repository = RunRepository::open(&url).await.expect("open repository");
    let alpha = repository
        .register_project(registration("alpha", "Alpha"))
        .await
        .expect("register alpha");
    assert!(matches!(
        repository
            .register_project(registration("alpha", "Again"))
            .await,
        Err(CoreError::DuplicateProject)
    ));
    let beta = repository
        .register_project(registration("beta", "Beta"))
        .await
        .expect("register beta");

    let listed = repository.list_projects().await.expect("list projects");
    assert_eq!(listed.len(), 2);
    assert!(listed
        .windows(2)
        .all(|pair| (pair[0].created_at_ms, pair[0].id.to_string())
            >= (pair[1].created_at_ms, pair[1].id.to_string())));
    assert_eq!(
        repository.get_project(&alpha.id).await.expect("alpha"),
        alpha
    );
    assert!(!alpha.repository_fingerprint.is_empty());

    drop(repository);
    let reopened = RunRepository::open(&url).await.expect("reopen database");
    assert_eq!(reopened.get_project(&beta.id).await.expect("beta"), beta);
}

#[tokio::test]
async fn fingerprint_mismatch_cannot_retarget_a_project() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let repository = RunRepository::open(&database_url(&directory))
        .await
        .expect("open repository");
    let project = repository
        .register_project(registration("replaceable", "Original"))
        .await
        .expect("register project");
    let original_fingerprint = project.repository_fingerprint.clone();
    let mut replacement = registration("replaceable", "Replacement");
    replacement.repository_fingerprint = "unix:replacement:object".into();
    assert!(matches!(
        repository
            .revalidate_project(&project.id, replacement)
            .await,
        Err(CoreError::RepositoryIdentityChanged)
    ));
    let persisted = repository
        .get_project(&project.id)
        .await
        .expect("original remains");
    assert_eq!(persisted.display_name, "Original");
    assert_eq!(persisted.repository_fingerprint, original_fingerprint);
}

#[tokio::test]
async fn revalidation_and_unregister_change_only_registry_rows() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let repository = RunRepository::open(&database_url(&directory))
        .await
        .expect("open repository");
    let project = repository
        .register_project(registration("project", "Original"))
        .await
        .expect("register project");
    let repository_fixture = directory.path().join("user repository");
    std::fs::create_dir_all(&repository_fixture).expect("repository fixture");
    std::fs::write(repository_fixture.join("keep.txt"), "must remain").expect("fixture file");

    let mut updated = registration("project", "Validated");
    updated.validation_state = ProjectValidationState::Detached;
    updated.branch = None;
    let revalidated = repository
        .revalidate_project(&project.id, updated)
        .await
        .expect("revalidate project");
    assert_eq!(revalidated.display_name, "Validated");
    assert_eq!(
        revalidated.validation_state,
        ProjectValidationState::Detached
    );

    repository
        .unregister_project(&project.id)
        .await
        .expect("remove registry row");
    assert!(matches!(
        repository.get_project(&project.id).await,
        Err(CoreError::NotFound)
    ));
    assert_eq!(
        std::fs::read_to_string(repository_fixture.join("keep.txt")).expect("fixture remains"),
        "must remain"
    );
}

#[tokio::test]
async fn legacy_and_weak_rows_require_explicit_one_time_trusted_upgrade() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let url = database_url(&directory);
    let repository = RunRepository::open(&url).await.expect("open repository");
    let registered = repository
        .register_project(registration("legacy-fixture", "Legacy fixture"))
        .await
        .expect("register strong project");
    let pool = SqlitePool::connect(&url).await.expect("inspect database");
    sqlx::query("UPDATE projects SET repository_fingerprint = '', fingerprint_scheme = 'legacy_unverified' WHERE id = ?")
        .bind(registered.id.to_string())
        .execute(&pool)
        .await
        .expect("make pre-strong fixture");

    let legacy = repository
        .get_project(&registered.id)
        .await
        .expect("legacy project remains listable");
    assert_eq!(
        legacy.fingerprint_scheme,
        ProjectFingerprintScheme::LegacyUnverified
    );
    assert_eq!(
        legacy.validation_state,
        ProjectValidationState::RequiresTrustedRevalidation
    );

    let upgraded = repository
        .revalidate_project(
            &registered.id,
            registration("legacy-fixture", "Legacy fixture"),
        )
        .await
        .expect("explicit trusted upgrade");
    assert_eq!(
        upgraded.fingerprint_scheme,
        ProjectFingerprintScheme::StrongV1
    );
    assert_eq!(upgraded.validation_state, ProjectValidationState::Valid);
    assert!(!upgraded.repository_fingerprint.is_empty());

    let mut replacement = registration("legacy-fixture", "Legacy fixture");
    replacement.repository_fingerprint = "strong_v1:macos_object_v1:replacement".into();
    assert!(matches!(
        repository
            .revalidate_project(&registered.id, replacement)
            .await,
        Err(CoreError::RepositoryIdentityChanged)
    ));

    sqlx::query("UPDATE projects SET repository_fingerprint = 'unix:1:2', fingerprint_scheme = 'weak_v0' WHERE id = ?")
        .bind(registered.id.to_string())
        .execute(&pool)
        .await
        .expect("make weak fixture");
    let weak = repository
        .get_project(&registered.id)
        .await
        .expect("weak project");
    assert_eq!(weak.fingerprint_scheme, ProjectFingerprintScheme::WeakV0);
    assert_eq!(
        weak.validation_state,
        ProjectValidationState::RequiresTrustedRevalidation
    );
}
