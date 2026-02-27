use serde::{Deserialize, Serialize};
use sentinel_core::config::SignalConfig;
use sentinel_core::types::Urgency;

/// Signal client for sending messages via signal-cli's HTTP JSON-RPC API.
///
/// This is only the *sending* side. The watcher (in sentinel-watchers) handles
/// receiving. This separation mirrors the architecture: gate sends, watcher listens.
pub struct SignalClient {
    http: reqwest::Client,
    config: SignalConfig,
}

/// A JSON-RPC 2.0 request for signal-cli.
#[derive(Debug, Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    id: &'a str,
    params: serde_json::Value,
}

/// A JSON-RPC 2.0 response from signal-cli.
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    id: Option<String>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: Option<i64>,
    message: String,
}

impl SignalClient {
    pub fn new(config: SignalConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client");
        Self { http, config }
    }

    /// Send a plain text message to a specific recipient.
    pub async fn send_message(&self, recipient: &str, text: &str) -> anyhow::Result<()> {
        let params = serde_json::json!({
            "account": self.config.account,
            "recipients": [recipient],
            "message": text,
        });
        self.rpc_call("send", &params).await?;
        Ok(())
    }

    /// Send a message to all allowed numbers.
    pub async fn broadcast(&self, text: &str) -> anyhow::Result<()> {
        for recipient in &self.config.allow_from {
            if let Err(e) = self.send_message(recipient, text).await {
                tracing::warn!(recipient, error = %e, "failed to send Signal message");
            }
        }
        Ok(())
    }

    /// Send a read receipt for a given timestamp (message ID in Signal).
    pub async fn send_read_receipt(
        &self,
        sender: &str,
        timestamp: i64,
    ) -> anyhow::Result<()> {
        if !self.config.send_read_receipts {
            return Ok(());
        }
        let params = serde_json::json!({
            "account": self.config.account,
            "recipientAddress": { "number": sender },
            "type": "read",
            "timestamps": [timestamp],
        });
        // Best-effort — don't fail the pipeline on receipt errors
        if let Err(e) = self.rpc_call("sendReceipt", &params).await {
            tracing::debug!(error = %e, "read receipt failed (non-critical)");
        }
        Ok(())
    }

    /// Format and send a notification via Signal.
    pub async fn send_notification(
        &self,
        urgency: &Urgency,
        title: &str,
        body: &str,
    ) -> anyhow::Result<()> {
        let icon = match urgency {
            Urgency::Ignore | Urgency::Low => "ℹ️",
            Urgency::Medium => "📋",
            Urgency::High => "⚠️",
            Urgency::Urgent => "🚨",
        };
        let text = format!("{icon} {title}\n{body}");
        self.broadcast(&text).await
    }

    /// Send an approval request and return the formatted message.
    pub fn format_approval_request(
        &self,
        action_id: &str,
        capability_kind: &str,
        explanation: &str,
    ) -> String {
        format!(
            "📅 Sentinel suggests:\n\n\
             {explanation}\n\n\
             → {capability_kind}\n\
             Reply \"yes {action_id}\" to approve, \
             \"no {action_id}\" to reject."
        )
    }

    /// Send an approval request via Signal.
    pub async fn send_approval_request(
        &self,
        action_id: &str,
        capability_kind: &str,
        explanation: &str,
    ) -> anyhow::Result<()> {
        let text = self.format_approval_request(action_id, capability_kind, explanation);
        self.broadcast(&text).await
    }

    /// Check if signal-cli is reachable.
    pub async fn health_check(&self) -> bool {
        let params = serde_json::json!({
            "account": self.config.account,
        });
        self.rpc_call("listGroups", &params).await.is_ok()
    }

    /// The account phone number this client is configured for.
    pub fn account(&self) -> &str {
        &self.config.account
    }

    /// The allowed senders list.
    pub fn allow_from(&self) -> &[String] {
        &self.config.allow_from
    }

    /// Make a JSON-RPC call to signal-cli.
    async fn rpc_call(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            method,
            id: "sentinel",
            params: params.clone(),
        };

        let resp = self
            .http
            .post(&self.config.signal_url())
            .json(&request)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("signal-cli HTTP request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("signal-cli returned HTTP {status}: {body}");
        }

        let rpc_resp: JsonRpcResponse = resp.json().await?;
        if let Some(err) = rpc_resp.error {
            anyhow::bail!("signal-cli RPC error: {}", err.message);
        }

        Ok(rpc_resp.result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SignalConfig {
        SignalConfig {
            enabled: true,
            account: "+351919191919".into(),
            port: 8083,
            http_url: None,
            socket_path: None,
            allow_from: vec!["+351969696969".into()],
            group_policy: "ignore".into(),
            allowed_groups: vec![],
            send_read_receipts: true,
        }
    }

    #[test]
    fn format_approval_request_includes_action_id() {
        let client = SignalClient::new(test_config());
        let msg = client.format_approval_request(
            "abc123",
            "CalendarEventCreate",
            "Move dentist to Wednesday?",
        );
        assert!(msg.contains("abc123"));
        assert!(msg.contains("CalendarEventCreate"));
        assert!(msg.contains("yes abc123"));
        assert!(msg.contains("no abc123"));
    }

    #[test]
    fn client_exposes_config() {
        let client = SignalClient::new(test_config());
        assert_eq!(client.account(), "+351919191919");
        assert_eq!(client.allow_from(), &["+351969696969"]);
    }
}
