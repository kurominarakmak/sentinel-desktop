//! Durable API-provider configuration without credential material.

use crate::{
    gemini::GeminiAdapter,
    managed_claude_config, managed_codex_config, managed_omp_config,
    openai_compatible::{CustomProviderSpec, OpenAiCompatibleAdapter},
    CredentialError, CredentialState, CredentialStore, ProviderAdapter, ProviderCapabilities,
    ProviderConfig, ProviderError, ProviderId, ProviderRegistry, ProviderRole, ProviderSelection,
    ProviderTransport, SecretString, WorkflowProviderConfiguration, CLAUDE_PROVIDER_ID,
    CODEX_PROVIDER_ID, OMP_PROVIDER_ID,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use thiserror::Error;

const MAX_CONFIG_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettingsDocument {
    pub version: u64,
    pub providers: Vec<ProviderConfig>,
    pub workflow: WorkflowProviderConfiguration,
}

impl Default for ProviderSettingsDocument {
    fn default() -> Self {
        let mut gemini = GeminiAdapter::new(crate::gemini::DEFAULT_GEMINI_MODEL)
            .expect("default Gemini configuration")
            .config()
            .clone();
        gemini.enabled = false;
        Self {
            version: 1,
            providers: vec![
                managed_codex_config(true),
                managed_claude_config(true),
                managed_omp_config(false),
                gemini,
            ],
            workflow: WorkflowProviderConfiguration {
                implementer: ProviderSelection {
                    provider_id: ProviderId::new(CODEX_PROVIDER_ID).expect("constant provider ID"),
                    model_id: "cli-owned".into(),
                    reasoning_effort: None,
                },
                reviewer: ProviderSelection {
                    provider_id: ProviderId::new(CLAUDE_PROVIDER_ID).expect("constant provider ID"),
                    model_id: "cli-owned".into(),
                    reasoning_effort: None,
                },
                repair: None,
            },
        }
    }
}

impl ProviderSettingsDocument {
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.version == 0 || self.providers.len() > 64 {
            return Err(SettingsError::InvalidConfiguration);
        }
        let mut ids = HashSet::new();
        for provider in &self.providers {
            provider
                .validate()
                .map_err(|_| SettingsError::InvalidConfiguration)?;
            if !ids.insert(provider.id.clone()) {
                return Err(SettingsError::InvalidConfiguration);
            }
            validate_provider_shape(provider)?;
        }
        validate_selection(
            &self.providers,
            &self.workflow.implementer,
            ProviderRole::Implementer,
        )?;
        validate_selection(
            &self.providers,
            &self.workflow.reviewer,
            ProviderRole::Reviewer,
        )?;
        validate_selection(
            &self.providers,
            self.workflow.repair_selection(),
            ProviderRole::Repair,
        )?;
        Ok(())
    }
}

fn validate_provider_shape(provider: &ProviderConfig) -> Result<(), SettingsError> {
    match provider.id.as_str() {
        CODEX_PROVIDER_ID | CLAUDE_PROVIDER_ID => {
            if provider.transport != ProviderTransport::ManagedCli || provider.credential.is_some()
            {
                return Err(SettingsError::InvalidConfiguration);
            }
        }
        OMP_PROVIDER_ID => {
            if provider.transport != ProviderTransport::ManagedCli || provider.credential.is_some()
            {
                return Err(SettingsError::InvalidConfiguration);
            }
        }
        "gemini" => {
            GeminiAdapter::from_config(provider.clone())
                .map_err(|_| SettingsError::InvalidConfiguration)?;
        }
        _ => {
            OpenAiCompatibleAdapter::from_config(provider.clone())
                .map_err(|_| SettingsError::InvalidConfiguration)?;
        }
    }
    Ok(())
}

fn validate_selection(
    providers: &[ProviderConfig],
    selection: &ProviderSelection,
    role: ProviderRole,
) -> Result<(), SettingsError> {
    let provider = providers
        .iter()
        .find(|provider| provider.id == selection.provider_id)
        .ok_or(SettingsError::InvalidSelection)?;
    let allowed = provider.enabled
        && provider.model_id == selection.model_id
        && match role {
            ProviderRole::Implementer => provider.capabilities.implementation,
            ProviderRole::Reviewer => provider.capabilities.read_only_review,
            ProviderRole::Repair => provider.capabilities.repair,
        };
    if !allowed {
        return Err(SettingsError::InvalidSelection);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettingsItem {
    pub id: ProviderId,
    pub display_name: String,
    pub model_id: String,
    pub supported_models: Vec<String>,
    pub base_url: Option<String>,
    pub enabled: bool,
    pub capabilities: ProviderCapabilities,
    pub credential_state: CredentialState,
    pub custom: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettingsSnapshot {
    pub version: u64,
    pub providers: Vec<ProviderSettingsItem>,
    pub workflow: WorkflowProviderConfiguration,
}

pub struct ProviderSettingsStore {
    path: PathBuf,
    document: Mutex<ProviderSettingsDocument>,
    credentials: Arc<dyn CredentialStore>,
}

impl ProviderSettingsStore {
    pub fn load(
        path: impl AsRef<Path>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self, SettingsError> {
        let path = path.as_ref().to_path_buf();
        let mut document = if path.exists() {
            let bytes = std::fs::read(&path).map_err(|_| SettingsError::Storage)?;
            if bytes.len() > MAX_CONFIG_BYTES {
                return Err(SettingsError::Corrupt);
            }
            serde_json::from_slice::<ProviderSettingsDocument>(&bytes)
                .map_err(|_| SettingsError::Corrupt)?
        } else {
            ProviderSettingsDocument::default()
        };
        migrate_glm_kimi_to_omp(&mut document);
        document.validate()?;
        Ok(Self {
            path,
            document: Mutex::new(document),
            credentials,
        })
    }

    pub fn snapshot(&self) -> ProviderSettingsSnapshot {
        let document = self.document.lock().expect("provider settings lock");
        ProviderSettingsSnapshot {
            version: document.version,
            providers: document
                .providers
                .iter()
                .map(|provider| ProviderSettingsItem {
                    id: provider.id.clone(),
                    display_name: provider.display_name.clone(),
                    model_id: provider.model_id.clone(),
                    supported_models: supported_models(provider),
                    base_url: provider.base_url.clone(),
                    enabled: provider.enabled,
                    capabilities: provider.capabilities,
                    credential_state: provider
                        .credential
                        .as_ref()
                        .map(|reference| self.credentials.state(reference))
                        .unwrap_or(CredentialState::NotRequired),
                    custom: provider.id.as_str().starts_with("custom."),
                })
                .collect(),
            workflow: document.workflow.clone(),
        }
    }

    pub fn set_enabled(
        &self,
        provider_id: &ProviderId,
        enabled: bool,
    ) -> Result<ProviderSettingsSnapshot, SettingsError> {
        self.update(|document| {
            let provider = document
                .providers
                .iter_mut()
                .find(|provider| provider.id == *provider_id)
                .ok_or(SettingsError::ProviderMissing)?;
            provider.enabled = enabled;
            Ok(())
        })
    }

    pub fn set_model(
        &self,
        provider_id: &ProviderId,
        model_id: String,
    ) -> Result<ProviderSettingsSnapshot, SettingsError> {
        self.update(|document| {
            let provider = document
                .providers
                .iter_mut()
                .find(|provider| provider.id == *provider_id)
                .ok_or(SettingsError::ProviderMissing)?;
            provider.model_id = model_id;
            for selection in [
                &mut document.workflow.implementer,
                &mut document.workflow.reviewer,
            ] {
                if selection.provider_id == *provider_id {
                    selection.model_id = provider.model_id.clone();
                }
            }
            if let Some(selection) = &mut document.workflow.repair {
                if selection.provider_id == *provider_id {
                    selection.model_id = provider.model_id.clone();
                }
            }
            Ok(())
        })
    }

    pub fn set_workflow(
        &self,
        workflow: WorkflowProviderConfiguration,
    ) -> Result<ProviderSettingsSnapshot, SettingsError> {
        self.update(|document| {
            document.workflow = workflow;
            Ok(())
        })
    }

    pub fn upsert_custom(
        &self,
        spec: CustomProviderSpec,
    ) -> Result<ProviderSettingsSnapshot, SettingsError> {
        let config = OpenAiCompatibleAdapter::new(spec)
            .map_err(|_| SettingsError::InvalidConfiguration)?
            .config()
            .clone();
        self.update(move |document| {
            if let Some(existing) = document
                .providers
                .iter_mut()
                .find(|provider| provider.id == config.id)
            {
                if !existing.id.as_str().starts_with("custom.") {
                    return Err(SettingsError::InvalidConfiguration);
                }
                *existing = config;
            } else {
                document.providers.push(config);
            }
            Ok(())
        })
    }

    pub fn remove_custom(
        &self,
        provider_id: &ProviderId,
    ) -> Result<ProviderSettingsSnapshot, SettingsError> {
        if !provider_id.as_str().starts_with("custom.") {
            return Err(SettingsError::InvalidConfiguration);
        }
        self.update(|document| {
            let before = document.providers.len();
            document
                .providers
                .retain(|provider| provider.id != *provider_id);
            if before == document.providers.len() {
                return Err(SettingsError::ProviderMissing);
            }
            Ok(())
        })
    }

    pub fn set_credential(
        &self,
        provider_id: &ProviderId,
        secret: SecretString,
    ) -> Result<ProviderSettingsSnapshot, SettingsError> {
        let reference = self.credential_reference(provider_id)?;
        self.credentials
            .set(&reference, secret)
            .map_err(SettingsError::Credential)?;
        Ok(self.snapshot())
    }

    pub fn remove_credential(
        &self,
        provider_id: &ProviderId,
    ) -> Result<ProviderSettingsSnapshot, SettingsError> {
        let reference = self.credential_reference(provider_id)?;
        self.credentials
            .remove(&reference)
            .map_err(SettingsError::Credential)?;
        Ok(self.snapshot())
    }

    pub fn build_registry(
        &self,
        codex_available: bool,
        claude_available: bool,
        omp_available: bool,
    ) -> Result<ProviderRegistry, SettingsError> {
        let document = self
            .document
            .lock()
            .expect("provider settings lock")
            .clone();
        let mut registry = ProviderRegistry::new(self.credentials.clone());
        for mut provider in document.providers {
            match provider.id.as_str() {
                CODEX_PROVIDER_ID => {
                    provider.enabled &= codex_available;
                    registry.register_managed(provider)?;
                }
                CLAUDE_PROVIDER_ID => {
                    provider.enabled &= claude_available;
                    registry.register_managed(provider)?;
                }
                OMP_PROVIDER_ID => {
                    provider.enabled &= omp_available;
                    registry.register_managed(provider)?;
                }
                "gemini" => {
                    registry.register_api(Arc::new(GeminiAdapter::from_config(provider)?))?
                }
                _ => registry
                    .register_api(Arc::new(OpenAiCompatibleAdapter::from_config(provider)?))?,
            }
        }
        Ok(registry)
    }

    fn credential_reference(
        &self,
        provider_id: &ProviderId,
    ) -> Result<crate::CredentialReference, SettingsError> {
        self.document
            .lock()
            .expect("provider settings lock")
            .providers
            .iter()
            .find(|provider| provider.id == *provider_id)
            .and_then(|provider| provider.credential.clone())
            .ok_or(SettingsError::ProviderMissing)
    }

    fn update<F>(&self, mutation: F) -> Result<ProviderSettingsSnapshot, SettingsError>
    where
        F: FnOnce(&mut ProviderSettingsDocument) -> Result<(), SettingsError>,
    {
        let mut document = self.document.lock().expect("provider settings lock");
        let mut candidate = document.clone();
        mutation(&mut candidate)?;
        candidate.version = candidate
            .version
            .checked_add(1)
            .ok_or(SettingsError::InvalidConfiguration)?;
        candidate.validate()?;
        save_document(&self.path, &candidate)?;
        *document = candidate;
        drop(document);
        Ok(self.snapshot())
    }
}

/// Older settings stored direct GLM/Kimi API descriptors. OMP owns those
/// provider protocols now, so retain the user's selected model while dropping
/// the direct credential reference (OMP reads its own login/key configuration).
fn migrate_glm_kimi_to_omp(document: &mut ProviderSettingsDocument) {
    let selected = [&document.workflow.implementer, &document.workflow.reviewer]
        .into_iter()
        .chain(document.workflow.repair.iter())
        .find_map(|selection| match selection.provider_id.as_str() {
            "glm" => Some(format!("zai/{}", selection.model_id)),
            "kimi" => Some(format!("moonshot/{}", selection.model_id)),
            _ => None,
        });
    let had_legacy = document
        .providers
        .iter()
        .any(|provider| matches!(provider.id.as_str(), "glm" | "kimi"));
    if !had_legacy {
        return;
    }
    document
        .providers
        .retain(|provider| !matches!(provider.id.as_str(), "glm" | "kimi"));
    let enabled = selected.is_some();
    let mut omp = managed_omp_config(enabled);
    if let Some(model) = selected {
        omp.model_id = model;
    }
    document.providers.push(omp.clone());
    for selection in [
        &mut document.workflow.implementer,
        &mut document.workflow.reviewer,
    ] {
        if matches!(selection.provider_id.as_str(), "glm" | "kimi") {
            selection.provider_id = ProviderId::new(OMP_PROVIDER_ID).expect("constant");
            selection.model_id = omp.model_id.clone();
        }
    }
    if let Some(selection) = &mut document.workflow.repair {
        if matches!(selection.provider_id.as_str(), "glm" | "kimi") {
            selection.provider_id = ProviderId::new(OMP_PROVIDER_ID).expect("constant");
            selection.model_id = omp.model_id;
        }
    }
}

fn supported_models(provider: &ProviderConfig) -> Vec<String> {
    let documented = match provider.id.as_str() {
        "gemini" => crate::gemini::SUPPORTED_GEMINI_MODELS,
        _ => return vec![provider.model_id.clone()],
    };
    documented.iter().map(|model| (*model).to_owned()).collect()
}

fn save_document(path: &Path, document: &ProviderSettingsDocument) -> Result<(), SettingsError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| SettingsError::Storage)?;
    }
    let bytes = serde_json::to_vec_pretty(document).map_err(|_| SettingsError::Storage)?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(SettingsError::InvalidConfiguration);
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| SettingsError::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| SettingsError::Storage)?;
    }
    file.write_all(&bytes).map_err(|_| SettingsError::Storage)?;
    file.sync_all().map_err(|_| SettingsError::Storage)?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(|_| SettingsError::Storage)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SettingsError {
    #[error("provider settings are invalid")]
    InvalidConfiguration,
    #[error("provider selection is invalid")]
    InvalidSelection,
    #[error("provider is missing")]
    ProviderMissing,
    #[error("provider settings are corrupt")]
    Corrupt,
    #[error("provider settings storage failed")]
    Storage,
    #[error("secure credential operation failed")]
    Credential(#[from] CredentialError),
    #[error("provider operation failed")]
    Provider(#[from] ProviderError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryCredentialStore, ModelLimits};

    #[test]
    fn settings_persist_provider_and_role_selection_without_secrets() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("providers.json");
        let credentials = Arc::new(MemoryCredentialStore::default());
        let store = ProviderSettingsStore::load(&path, credentials.clone()).unwrap();
        let gemini = ProviderId::new("gemini").unwrap();
        let omp = ProviderId::new(OMP_PROVIDER_ID).unwrap();
        store
            .set_credential(
                &gemini,
                SecretString::new("never-persist-this-key").unwrap(),
            )
            .unwrap();
        store.set_enabled(&omp, true).unwrap();
        let codex = ProviderSelection {
            provider_id: ProviderId::new("codex").unwrap(),
            model_id: "cli-owned".into(),
            reasoning_effort: None,
        };
        store
            .set_workflow(WorkflowProviderConfiguration {
                implementer: omp_selection(),
                reviewer: codex.clone(),
                repair: Some(omp_selection()),
            })
            .unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("never-persist-this-key"));
        assert!(raw.contains("\"providerId\": \"omp\""));

        let reopened = ProviderSettingsStore::load(&path, credentials).unwrap();
        let snapshot = reopened.snapshot();
        assert_eq!(snapshot.workflow.implementer.provider_id, omp);
        assert_eq!(
            snapshot
                .providers
                .iter()
                .find(|provider| provider.id.as_str() == "gemini")
                .unwrap()
                .credential_state,
            CredentialState::Configured
        );
    }

    #[test]
    fn credential_replace_remove_and_restart_are_keychain_contract_only() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("providers.json");
        let credentials = Arc::new(MemoryCredentialStore::default());
        let store = ProviderSettingsStore::load(&path, credentials.clone()).unwrap();
        let glm = ProviderId::new("gemini").unwrap();
        store
            .set_credential(&glm, SecretString::new("first").unwrap())
            .unwrap();
        store
            .set_credential(&glm, SecretString::new("replacement").unwrap())
            .unwrap();
        assert_eq!(
            store
                .snapshot()
                .providers
                .iter()
                .find(|provider| provider.id == glm)
                .unwrap()
                .credential_state,
            CredentialState::Configured
        );
        store.remove_credential(&glm).unwrap();
        let reopened = ProviderSettingsStore::load(&path, credentials).unwrap();
        assert_eq!(
            reopened
                .snapshot()
                .providers
                .iter()
                .find(|provider| provider.id == glm)
                .unwrap()
                .credential_state,
            CredentialState::Missing
        );
    }

    #[test]
    fn custom_provider_is_validated_persisted_and_registered() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("providers.json");
        let credentials = Arc::new(MemoryCredentialStore::default());
        let store = ProviderSettingsStore::load(&path, credentials).unwrap();
        let snapshot = store
            .upsert_custom(CustomProviderSpec {
                provider_id: "custom.company".into(),
                display_name: "Company Gateway".into(),
                base_url: "https://models.example.com/v1".into(),
                model_id: "coding-model".into(),
                enabled: true,
                capabilities: ProviderCapabilities {
                    streaming: true,
                    cancellation: true,
                    tool_calling: true,
                    structured_output: true,
                    model_selection: true,
                    rate_limits: true,
                    implementation: true,
                    read_only_review: true,
                    repair: true,
                },
                limits: ModelLimits {
                    context_tokens: Some(100_000),
                    max_output_tokens: Some(10_000),
                },
            })
            .unwrap();
        assert!(snapshot
            .providers
            .iter()
            .any(|provider| provider.id.as_str() == "custom.company" && provider.custom));
        let registry = store.build_registry(true, true, true).unwrap();
        assert!(registry
            .provider(&ProviderId::new("custom.company").unwrap())
            .is_some());
    }

    #[test]
    fn corrupt_or_secret_bearing_unknown_configuration_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("providers.json");
        std::fs::write(&path, b"{\"version\":1,\"apiKey\":\"secret\"}").unwrap();
        assert!(matches!(
            ProviderSettingsStore::load(&path, Arc::new(MemoryCredentialStore::default())),
            Err(SettingsError::Corrupt)
        ));
    }

    fn omp_selection() -> ProviderSelection {
        ProviderSelection {
            provider_id: ProviderId::new(OMP_PROVIDER_ID).unwrap(),
            model_id: "zai/glm-5.2".into(),
            reasoning_effort: None,
        }
    }
}
