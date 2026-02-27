use serde::Deserialize;
use sentinel_core::config::SignalConfig;
use sentinel_core::events::{SignalMessage, WatchEvent};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// Signal message watcher via signal-cli's Unix socket JSON-RPC API.
///
/// Connects to the signal-cli daemon over its Unix socket and subscribes to
/// incoming messages via `subscribeReceive`. Envelopes arrive as JSON-RPC
/// notifications. Filters by `allow_from` and emits `WatchEvent::Signal`
/// for each accepted message.
pub struct SignalWatcher {
    config: SignalConfig,
}

/// Envelope wrapper used for processing incoming messages.
#[derive(Debug, Deserialize)]
struct WsEnvelope {
    #[serde(default)]
    envelope: Option<Envelope>,
}

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default, alias = "sourceNumber")]
    source_number: Option<String>,
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default, alias = "dataMessage")]
    data_message: Option<DataMessage>,
}

#[derive(Debug, Deserialize)]
struct DataMessage {
    #[serde(default)]
    message: Option<String>,
    #[serde(default, alias = "groupInfo")]
    group_info: Option<GroupInfo>,
    #[serde(default)]
    attachments: Option<Vec<Attachment>>,
}

#[derive(Debug, Deserialize)]
struct GroupInfo {
    #[serde(default, alias = "groupId")]
    group_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Attachment {
    #[serde(default)]
    id: Option<String>,
}


impl SignalWatcher {
    pub fn new(config: SignalConfig) -> Self {
        Self { config }
    }

    /// Run the watcher loop. Connects to signal-cli's Unix socket and
    /// subscribes for incoming messages via `subscribeReceive`.
    pub async fn run(&self, tx: tokio::sync::mpsc::Sender<WatchEvent>) -> anyhow::Result<()> {
        tracing::info!(account = %self.config.account, "Signal watcher starting");

        loop {
            match self.subscribe_and_listen(&tx).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!(error = %e, "Signal socket connection failed, retrying in 5s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    /// Connect to the Unix socket, subscribe, and forward messages until
    /// the socket closes or the event channel is dropped.
    async fn subscribe_and_listen(
        &self,
        tx: &tokio::sync::mpsc::Sender<WatchEvent>,
    ) -> anyhow::Result<()> {
        let socket_path = self.config.signal_socket();
        tracing::debug!(path = %socket_path, "connecting to signal-cli socket");

        let stream = tokio::net::UnixStream::connect(&socket_path).await?;
        let (reader, mut writer) = stream.into_split();

        let subscribe = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "subscribeReceive",
            "params": { "account": self.config.account },
            "id": "sentinel-watcher"
        });
        let mut msg = serde_json::to_string(&subscribe)?;
        msg.push('\n');
        writer.write_all(msg.as_bytes()).await?;
        writer.flush().await?;

        let mut buf_reader = tokio::io::BufReader::new(reader);
        let mut line = String::new();

        // Read subscription ack
        buf_reader.read_line(&mut line).await?;
        let ack: serde_json::Value = serde_json::from_str(line.trim())?;
        if let Some(err) = ack.get("error") {
            anyhow::bail!("signal-cli subscription failed: {err}");
        }
        tracing::info!("Signal subscription active");

        loop {
            line.clear();
            let n = buf_reader.read_line(&mut line).await?;
            if n == 0 {
                anyhow::bail!("signal-cli socket closed");
            }

            if let Some(msg) = self.parse_notification(line.trim()) {
                if tx.send(WatchEvent::Signal(msg)).await.is_err() {
                    tracing::info!("event channel closed, Signal watcher stopping");
                    return Ok(());
                }
            }
        }
    }

    /// Parse a JSON-RPC subscription notification from signal-cli.
    ///
    /// Expected format:
    /// ```json
    /// {"jsonrpc":"2.0","method":"receive","params":{"subscription":"…","result":{…envelope…}}}
    /// ```
    fn parse_notification(&self, text: &str) -> Option<SignalMessage> {
        let body: serde_json::Value = serde_json::from_str(text).ok()?;

        // Extract the envelope from the subscription notification
        let result = body.pointer("/params/result")?;
        let envelope_val = result.get("envelope").unwrap_or(result);

        let ws_env = WsEnvelope {
            envelope: Some(Envelope {
                source_number: envelope_val
                    .get("sourceNumber")
                    .or_else(|| envelope_val.get("source"))
                    .and_then(|s| s.as_str())
                    .map(String::from),
                timestamp: envelope_val
                    .get("timestamp")
                    .and_then(|t| t.as_i64()),
                data_message: envelope_val.get("dataMessage").and_then(|d| {
                    serde_json::from_value::<DataMessage>(d.clone()).ok()
                }),
            }),
        };

        self.process_envelope(ws_env)
    }

    /// Process a single envelope: validate sender, filter groups, extract message.
    fn process_envelope(&self, result: WsEnvelope) -> Option<SignalMessage> {
        let envelope = result.envelope?;
        let sender = envelope.source_number.as_deref()?;
        let data = envelope.data_message?;

        // Filter: sender must be in allow_from
        if !self.is_allowed_sender(sender) {
            tracing::debug!(sender, "Signal message from unlisted number — dropped");
            return None;
        }

        // Filter: group messages according to policy
        if let Some(ref group) = data.group_info {
            if !self.is_allowed_group(group) {
                tracing::debug!(sender, "Signal group message — dropped by policy");
                return None;
            }
        }

        // Extract message text
        let text = data.message.unwrap_or_default();
        if text.trim().is_empty() {
            return None;
        }

        let attachments = data
            .attachments
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| a.id)
            .collect();

        let timestamp = envelope
            .timestamp
            .map(|ts| {
                chrono::DateTime::from_timestamp_millis(ts)
                    .unwrap_or_else(chrono::Utc::now)
            })
            .unwrap_or_else(chrono::Utc::now);

        Some(SignalMessage {
            sender: sender.to_string(),
            text,
            timestamp,
            attachments,
        })
    }

    fn is_allowed_sender(&self, sender: &str) -> bool {
        if self.config.allow_from.is_empty() {
            // Empty allowlist = accept nothing (secure default)
            return false;
        }
        self.config.allow_from.iter().any(|allowed| allowed == sender)
    }

    fn is_allowed_group(&self, group: &GroupInfo) -> bool {
        match self.config.group_policy.as_str() {
            "allowlist" => {
                if let Some(ref gid) = group.group_id {
                    self.config.allowed_groups.iter().any(|g| g == gid)
                } else {
                    false
                }
            }
            _ => false, // "ignore" or any other value → reject groups
        }
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
            allow_from: vec!["+351969696969".into(), "+351959595959".into()],
            group_policy: "ignore".into(),
            allowed_groups: vec![],
            send_read_receipts: true,
        }
    }

    fn make_result(sender: &str, text: &str, group: Option<&str>) -> WsEnvelope {
        WsEnvelope {
            envelope: Some(Envelope {
                source_number: Some(sender.into()),
                timestamp: Some(1708500000000),
                data_message: Some(DataMessage {
                    message: Some(text.into()),
                    group_info: group.map(|g| GroupInfo {
                        group_id: Some(g.into()),
                    }),
                    attachments: None,
                }),
            }),
        }
    }

    #[test]
    fn accepts_allowed_sender() {
        let watcher = SignalWatcher::new(test_config());
        let result = make_result("+351969696969", "hello", None);
        let msg = watcher.process_envelope(result);
        assert!(msg.is_some());
        assert_eq!(msg.unwrap().text, "hello");
    }

    #[test]
    fn rejects_unlisted_sender() {
        let watcher = SignalWatcher::new(test_config());
        let result = make_result("+351000000000", "hack attempt", None);
        assert!(watcher.process_envelope(result).is_none());
    }

    #[test]
    fn rejects_empty_allowlist() {
        let mut config = test_config();
        config.allow_from = vec![];
        let watcher = SignalWatcher::new(config);
        let result = make_result("+351969696969", "hello", None);
        assert!(watcher.process_envelope(result).is_none());
    }

    #[test]
    fn rejects_group_message_with_ignore_policy() {
        let watcher = SignalWatcher::new(test_config());
        let result = make_result("+351969696969", "group msg", Some("group123"));
        assert!(watcher.process_envelope(result).is_none());
    }

    #[test]
    fn accepts_group_on_allowlist() {
        let mut config = test_config();
        config.group_policy = "allowlist".into();
        config.allowed_groups = vec!["group123".into()];
        let watcher = SignalWatcher::new(config);
        let result = make_result("+351969696969", "group msg", Some("group123"));
        assert!(watcher.process_envelope(result).is_some());
    }

    #[test]
    fn rejects_group_not_on_allowlist() {
        let mut config = test_config();
        config.group_policy = "allowlist".into();
        config.allowed_groups = vec!["group123".into()];
        let watcher = SignalWatcher::new(config);
        let result = make_result("+351969696969", "msg", Some("other-group"));
        assert!(watcher.process_envelope(result).is_none());
    }

    #[test]
    fn rejects_empty_message() {
        let watcher = SignalWatcher::new(test_config());
        let result = make_result("+351969696969", "   ", None);
        assert!(watcher.process_envelope(result).is_none());
    }

    #[test]
    fn rejects_missing_envelope() {
        let watcher = SignalWatcher::new(test_config());
        let result = WsEnvelope { envelope: None };
        assert!(watcher.process_envelope(result).is_none());
    }

    #[test]
    fn extracts_sender_field() {
        let watcher = SignalWatcher::new(test_config());
        let result = make_result("+351969696969", "test", None);
        let msg = watcher.process_envelope(result).unwrap();
        assert_eq!(msg.sender, "+351969696969");
    }
}
