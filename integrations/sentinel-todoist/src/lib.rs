use sentinel_core::integration::Integration;
use sentinel_core::types::{CredentialRequirement, IntegrationCategory};

/// Todoist integration via Todoist API.
pub struct TodoistIntegration;

#[async_trait::async_trait]
impl Integration for TodoistIntegration {
    fn id(&self) -> &str {
        "todoist"
    }

    fn category(&self) -> IntegrationCategory {
        IntegrationCategory::Tasks
    }

    fn capabilities(&self) -> Vec<String> {
        vec![
            "Read tasks and projects".into(),
            "Create and complete tasks".into(),
            "Watch for task changes".into(),
        ]
    }

    fn credential_requirements(&self) -> Vec<CredentialRequirement> {
        vec![CredentialRequirement {
            key: "todoist_api_token".into(),
            description: "Todoist API Token".into(),
            secret: true,
        }]
    }

    async fn validate(&self) -> anyhow::Result<()> {
        anyhow::bail!("Todoist integration not yet implemented")
    }
}
