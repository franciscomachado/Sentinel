use crate::types::{CredentialRequirement, IntegrationCategory};

/// Trait for all Sentinel integrations (Gmail, Calendar, Todoist, Telegram, etc.)
///
/// Each integration declares its identity, capabilities, and credential needs.
/// The daemon validates credentials at startup and invokes `watch` for push-based
/// integrations or `execute` for on-demand actions.
#[async_trait::async_trait]
pub trait Integration: Send + Sync {
    /// Unique identifier for this integration (e.g. "gmail", "todoist").
    fn id(&self) -> &str;

    /// Category of this integration (Email, Calendar, Tasks, Messaging, etc.)
    fn category(&self) -> IntegrationCategory;

    /// Human-readable capabilities summary.
    fn capabilities(&self) -> Vec<String>;

    /// Credentials this integration requires for setup.
    fn credential_requirements(&self) -> Vec<CredentialRequirement>;

    /// Validate that the integration is correctly configured and credentials work.
    async fn validate(&self) -> anyhow::Result<()>;
}
