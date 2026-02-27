use anyhow::Context;
use async_trait::async_trait;
use serde::Deserialize;
use crate::prompt::LlmRequest;
use crate::provider::AiProvider;
use crate::response::LlmResponse;
use sentinel_core::types::TokenCost;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic Messages API client.
pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: String,
    model: String,
    api_base: String,
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text { text: String },
    Thinking { thinking: String },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct Usage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

pub struct CortexResponse {
    pub parsed: LlmResponse,
    pub token_cost: TokenCost,
    pub raw_text: String,
}

fn extract_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

impl AnthropicClient {
    pub fn new(api_key: String, model: String, api_base: Option<String>) -> Self {
        let http = reqwest::Client::new();
        let api_base = api_base.unwrap_or_else(|| ANTHROPIC_API_URL.to_string());
        Self { 
            http,
            api_key,
            model,
            api_base,
        }
    }

    /// Send a raw request to the Anthropic API and return the parsed response.
    async fn send_request(&self, request: &LlmRequest) -> anyhow::Result<ApiResponse> {
        let response = self
            .http
            .post(&self.api_base)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(request)
            .send()
            .await
            .context("failed to send request to Anthropic")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error {status}: {body}");
        }

        response
            .json()
            .await
            .context("failed to parse Anthropic response")
    }
}

#[async_trait]
impl AiProvider for AnthropicClient {
    async fn complete(&self, request: LlmRequest) -> anyhow::Result<CortexResponse> {
        tracing::debug!(model = %request.model, "sending request to Anthropic API");

        let api_response = self.send_request(&request).await?;

        let raw_text = extract_text(&api_response.content);

        tracing::debug!(
            input_tokens = api_response.usage.input_tokens,
            output_tokens = api_response.usage.output_tokens,
            "Anthropic API response received"
        );

        let parsed: LlmResponse = serde_json::from_str(&raw_text).with_context(|| {
            format!(
                "failed to parse LLM JSON response: {}",
                &raw_text[..raw_text.len().min(20000)]
            )
        })?;

        let token_cost = TokenCost {
            input_tokens: api_response.usage.input_tokens,
            output_tokens: api_response.usage.output_tokens,
            cached_tokens: api_response.usage.cache_read_input_tokens,
        };

        Ok(CortexResponse {
            parsed,
            token_cost,
            raw_text,
        })
    }

    async fn chat(&self, request: LlmRequest) -> anyhow::Result<String> {
        let api_response = self.send_request(&request).await?;
        Ok(extract_text(&api_response.content))
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn provider_name(&self) -> &str {
        "anthropic"
    }
}