use sentinel_core::integration::Integration;
use sentinel_core::types::{CredentialRequirement, IntegrationCategory};

/// Gmail integration via Google API + OAuth.
pub struct GmailIntegration;

#[async_trait::async_trait]
impl Integration for GmailIntegration {
    fn id(&self) -> &str {
        "gmail"
    }

    fn category(&self) -> IntegrationCategory {
        IntegrationCategory::Email
    }

    fn capabilities(&self) -> Vec<String> {
        vec![
            "Read emails via Gmail API".into(),
            "Send emails via Gmail API".into(),
            "Search email history".into(),
        ]
    }

    fn credential_requirements(&self) -> Vec<CredentialRequirement> {
        vec![
            CredentialRequirement {
                key: "google_client_id".into(),
                description: "Google OAuth Client ID".into(),
                secret: false,
            },
            CredentialRequirement {
                key: "google_client_secret".into(),
                description: "Google OAuth Client Secret".into(),
                secret: true,
            },
        ]
    }

    async fn validate(&self) -> anyhow::Result<()> {
        // TODO: validate OAuth tokens
        anyhow::bail!("Gmail integration not yet implemented")
    }
}
