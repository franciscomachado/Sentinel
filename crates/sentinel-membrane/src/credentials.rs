use sentinel_core::types::ServiceId;

/// Credential vault. Phase 1 uses environment variables;
/// will migrate to OS keychain (keyring crate) later.
pub struct CredentialVault {
    _private: (),
}

pub enum ServiceCredentials {
    Imap {
        host: String,
        port: u16,
        username: String,
        password: String,
    },
    Smtp {
        host: String,
        port: u16,
        username: String,
        password: String,
    },
    CalDav {
        url: String,
        username: String,
        password: String,
    },
    ApiKey {
        key: String,
    },
    Routing {
        endpoint: String,
        api_key: Option<String>,
    },
    Bring {
        email: String,
        password: String,
    },
}

impl CredentialVault {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Retrieve credentials for a given service.
    /// Phase 1: looks up environment variables.
    /// Convention: SENTINEL_{SERVICE}_{FIELD} (e.g., SENTINEL_ANTHROPIC_API_KEY).
    pub fn get_credentials(&self, service: ServiceId) -> anyhow::Result<ServiceCredentials> {
        match service {
            ServiceId::Anthropic => {
                let key = std::env::var("SENTINEL_ANTHROPIC_API_KEY").or_else(|_| {
                    std::env::var("ANTHROPIC_API_KEY")
                }).map_err(|_| {
                    anyhow::anyhow!("SENTINEL_ANTHROPIC_API_KEY or ANTHROPIC_API_KEY not set")
                })?;
                Ok(ServiceCredentials::ApiKey { key })
            }
            ServiceId::Ai(ref name) => {
                // Dynamic credential lookup: SENTINEL_{NAME}_API_KEY or {NAME}_API_KEY.
                // Allows any provider (openai, deepseek, gemini, groq, mistral, ...)
                // to work without code changes.
                let upper = name.to_uppercase();
                let sentinel_var = format!("SENTINEL_{upper}_API_KEY");
                let plain_var = format!("{upper}_API_KEY");
                let key = std::env::var(&sentinel_var).or_else(|_| {
                    std::env::var(&plain_var)
                }).map_err(|_| {
                    anyhow::anyhow!("{sentinel_var} or {plain_var} not set")
                })?;
                Ok(ServiceCredentials::ApiKey { key })
            }
            ServiceId::Imap(ref host) => {
                let username = env_or_err("SENTINEL_IMAP_USERNAME")?;
                let password = env_or_err("SENTINEL_IMAP_PASSWORD")?;
                let port = std::env::var("SENTINEL_IMAP_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(993);
                Ok(ServiceCredentials::Imap {
                    host: host.clone(),
                    port,
                    username,
                    password,
                })
            }
            ServiceId::Smtp(ref host) => {
                let username = env_or_err("SENTINEL_SMTP_USERNAME")?;
                let password = env_or_err("SENTINEL_SMTP_PASSWORD")?;
                let port = std::env::var("SENTINEL_SMTP_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(587);
                Ok(ServiceCredentials::Smtp {
                    host: host.clone(),
                    port,
                    username,
                    password,
                })
            }
            ServiceId::CalDav => {
                let url = env_or_err("SENTINEL_CALDAV_URL")?;
                let username = env_or_err("SENTINEL_CALDAV_USERNAME")?;
                let password = env_or_err("SENTINEL_CALDAV_PASSWORD")?;
                Ok(ServiceCredentials::CalDav {
                    url,
                    username,
                    password,
                })
            }
            ServiceId::Routing => {
                let endpoint = env_or_err("SENTINEL_ROUTING_ENDPOINT")?;
                let api_key = std::env::var("SENTINEL_ROUTING_API_KEY").ok();
                Ok(ServiceCredentials::Routing { endpoint, api_key })
            }
            ServiceId::Bring => {
                let email = env_or_err("SENTINEL_BRING_EMAIL")?;
                let password = env_or_err("SENTINEL_BRING_PASSWORD")?;
                Ok(ServiceCredentials::Bring { email, password })
            }
        }
    }
}

fn env_or_err(key: &str) -> anyhow::Result<String> {
    std::env::var(key).map_err(|_| anyhow::anyhow!("{key} not set"))
}
