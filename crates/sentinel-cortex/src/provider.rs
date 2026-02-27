use async_trait::async_trait;

use crate::client::CortexResponse;
use crate::prompt::LlmRequest;
use crate::response::LlmResponse;
use sentinel_core::config::AiConfig;
use sentinel_core::types::{ServiceId, TokenCost};
use sentinel_membrane::credentials::{CredentialVault, ServiceCredentials};

/// Trait abstracting an AI language model provider.
///
/// Sentinel is designed and tuned for Claude (Anthropic). Other providers are
/// supported but results may vary — the system prompt, response format
/// expectations, and prompt structure are optimised for Claude's behaviour.
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Send a structured request and parse the response as an `LlmResponse`.
    async fn complete(&self, request: LlmRequest) -> anyhow::Result<CortexResponse>;

    /// Send a request and return only the text response (for freeform
    /// conversations like onboarding that don't use the structured format).
    async fn chat(&self, request: LlmRequest) -> anyhow::Result<String>;

    /// The model name in use.
    fn model(&self) -> &str;

    /// The provider name (e.g. "anthropic", "deepseek", "gemini").
    fn provider_name(&self) -> &str;
}

// ── OpenAI-compatible provider ──────────────────────────────────
//
// Works with any service that implements the OpenAI chat completions API:
// OpenAI, DeepSeek, Gemini, Groq, Mistral, Together, Fireworks, etc.

const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";
const OLLAMA_API_URL: &str = "http://localhost:11434/v1/chat/completions";

/// Well-known defaults for providers that use the OpenAI-compatible API.
fn well_known_defaults(provider: &str) -> (&str, &str) {
    // Returns (default_api_base, default_model)
    match provider {
        "openai" => (OPENAI_API_URL, "gpt-4o"),
        "deepseek" => ("https://api.deepseek.com/v1/chat/completions", "deepseek-chat"),
        "gemini" => ("https://generativelanguage.googleapis.com/v1beta/openai/chat/completions", "gemini-2.0-flash"),
        "groq" => ("https://api.groq.com/openai/v1/chat/completions", "llama-3.3-70b-versatile"),
        "mistral" => ("https://api.mistral.ai/v1/chat/completions", "mistral-large-latest"),
        "together" => ("https://api.together.xyz/v1/chat/completions", "meta-llama/Llama-3.3-70B-Instruct-Turbo"),
        "ollama" => (OLLAMA_API_URL, "llama3.1"),
        _ => (OPENAI_API_URL, ""),
    }
}

pub struct OpenAiCompatibleProvider {
    http: reqwest::Client,
    api_key: String,
    model: String,
    api_base: String,
    name: String,
}

impl OpenAiCompatibleProvider {
    pub fn new(name: String, api_key: String, model: String, api_base: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key,
            model,
            api_base,
            name,
        }
    }

    /// Build the OpenAI-format JSON request body from an LlmRequest.
    fn build_request_body(&self, request: &LlmRequest) -> serde_json::Value {
        let mut messages = Vec::new();

        if !request.system.is_empty() {
            messages.push(serde_json::json!({
                "role": "system",
                "content": request.system,
            }));
        }

        for msg in &request.messages {
            messages.push(serde_json::json!({
                "role": msg.role,
                "content": msg.content,
            }));
        }

        serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
        })
    }

    async fn send_request(&self, request: &LlmRequest) -> anyhow::Result<serde_json::Value> {
        let body = self.build_request_body(request);

        let mut req = self
            .http
            .post(&self.api_base)
            .header("Content-Type", "application/json");

        // Only send Authorization header if we have an API key
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = req
            .json(&body)
            .send()
            .await
            .context("failed to send request to AI provider")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("{} API error {status}: {body}", self.name);
        }

        response
            .json()
            .await
            .context("failed to parse AI provider response")
    }

    fn extract_text(response: &serde_json::Value) -> String {
        response
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|msg| msg.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string()
    }

    fn extract_token_cost(response: &serde_json::Value) -> TokenCost {
        let usage = response.get("usage");
        TokenCost {
            input_tokens: usage
                .and_then(|u| u.get("prompt_tokens"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as u32,
            output_tokens: usage
                .and_then(|u| u.get("completion_tokens"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as u32,
            cached_tokens: 0,
        }
    }
}

use anyhow::Context;

#[async_trait]
impl AiProvider for OpenAiCompatibleProvider {
    async fn complete(&self, request: LlmRequest) -> anyhow::Result<CortexResponse> {
        tracing::debug!(model = %request.model, provider = %self.name, "sending request to AI provider");

        let response = self.send_request(&request).await?;
        let raw_text = Self::extract_text(&response);
        let token_cost = Self::extract_token_cost(&response);

        tracing::debug!(
            input_tokens = token_cost.input_tokens,
            output_tokens = token_cost.output_tokens,
            provider = %self.name,
            "AI provider response received"
        );

        let parsed: LlmResponse = serde_json::from_str(&raw_text).with_context(|| {
            format!(
                "failed to parse LLM JSON response: {}",
                &raw_text[..raw_text.len().min(200)]
            )
        })?;

        Ok(CortexResponse {
            parsed,
            token_cost,
            raw_text,
        })
    }

    async fn chat(&self, request: LlmRequest) -> anyhow::Result<String> {
        let response = self.send_request(&request).await?;
        Ok(Self::extract_text(&response))
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn provider_name(&self) -> &str {
        &self.name
    }
}

// ── Factory ─────────────────────────────────────────────────────

/// Create an AI provider from configuration.
///
/// Reads the `[ai]` config section (defaulting to Anthropic if absent) and
/// resolves credentials from the vault. The `SENTINEL_MODEL` env var can
/// override the model selection for any provider.
///
/// - `"anthropic"` uses the Anthropic Messages API (unique format).
/// - `"ollama"` uses the OpenAI-compatible API with no authentication and
///   localhost defaults.
/// - Any other name (e.g. `"openai"`, `"deepseek"`, `"gemini"`, `"groq"`,
///   `"mistral"`, `"together"`) uses the OpenAI-compatible chat completions
///   API. Well-known providers have built-in default URLs; for others
///   you must set `api_base` in the config.
pub fn create_provider(
    ai_config: Option<&AiConfig>,
    vault: &CredentialVault,
) -> anyhow::Result<Box<dyn AiProvider>> {
    let config = ai_config.cloned().unwrap_or_default();

    // SENTINEL_MODEL env var overrides config
    let model_override = std::env::var("SENTINEL_MODEL").ok();
    let provider_name = config.provider.as_str();

    match provider_name {
        // Anthropic has its own API format
        "anthropic" => {
            let creds = vault.get_credentials(ServiceId::Anthropic)?;
            let api_key = match creds {
                ServiceCredentials::ApiKey { key } => key,
                _ => anyhow::bail!("unexpected credential type for Anthropic"),
            };
            let model = model_override
                .or(config.model)
                .unwrap_or_else(|| "claude-sonnet-4-20250514".into());
            Ok(Box::new(crate::client::AnthropicClient::new(
                api_key,
                model,
                config.api_base,
            )))
        }
        // Ollama runs locally — no API key, localhost default
        "ollama" => {
            let (default_base, default_model) = well_known_defaults("ollama");
            let model = model_override
                .or(config.model)
                .unwrap_or_else(|| default_model.into());
            let api_base = config.api_base.unwrap_or_else(|| default_base.into());
            Ok(Box::new(OpenAiCompatibleProvider::new(
                "ollama".into(),
                String::new(), // no auth
                model,
                api_base,
            )))
        }
        // Everything else: OpenAI-compatible with Bearer token auth
        name => {
            let creds = vault.get_credentials(ServiceId::Ai(name.into()))?;
            let api_key = match creds {
                ServiceCredentials::ApiKey { key } => key,
                _ => anyhow::bail!("unexpected credential type for {name}"),
            };
            let (default_base, default_model) = well_known_defaults(name);
            let model = match model_override.or(config.model) {
                Some(m) => m,
                None if !default_model.is_empty() => default_model.into(),
                None => anyhow::bail!(
                    "model is required for provider \"{name}\" — set [ai] model in config or SENTINEL_MODEL env var"
                ),
            };
            let api_base = config.api_base.unwrap_or_else(|| default_base.into());
            Ok(Box::new(OpenAiCompatibleProvider::new(
                name.into(),
                api_key,
                model,
                api_base,
            )))
        }
    }
}
