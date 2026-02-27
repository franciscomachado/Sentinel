use sentinel_core::integration::Integration;
use sentinel_core::types::{CredentialRequirement, IntegrationCategory};

/// Google Calendar integration via Google Calendar API.
pub struct GoogleCalendarIntegration;

#[async_trait::async_trait]
impl Integration for GoogleCalendarIntegration {
    fn id(&self) -> &str {
        "google-calendar"
    }

    fn category(&self) -> IntegrationCategory {
        IntegrationCategory::Calendar
    }

    fn capabilities(&self) -> Vec<String> {
        vec![
            "Read calendar events".into(),
            "Create calendar events".into(),
            "Watch for calendar changes".into(),
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
        anyhow::bail!("Google Calendar integration not yet implemented")
    }
}
