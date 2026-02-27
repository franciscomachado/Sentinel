use sentinel_core::integration::Integration;
use sentinel_core::types::{CredentialRequirement, IntegrationCategory};

/// Telegram integration via Telegram Bot API.
pub struct TelegramIntegration;

#[async_trait::async_trait]
impl Integration for TelegramIntegration {
    fn id(&self) -> &str {
        "telegram"
    }

    fn category(&self) -> IntegrationCategory {
        IntegrationCategory::Messaging
    }

    fn capabilities(&self) -> Vec<String> {
        vec![
            "Send messages via Telegram bot".into(),
            "Receive messages via Telegram bot".into(),
        ]
    }

    fn credential_requirements(&self) -> Vec<CredentialRequirement> {
        vec![
            CredentialRequirement {
                key: "telegram_bot_token".into(),
                description: "Telegram Bot API Token".into(),
                secret: true,
            },
            CredentialRequirement {
                key: "telegram_chat_id".into(),
                description: "Telegram Chat ID for notifications".into(),
                secret: false,
            },
        ]
    }

    async fn validate(&self) -> anyhow::Result<()> {
        anyhow::bail!("Telegram integration not yet implemented")
    }
}
