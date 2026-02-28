use std::time::Duration;

use anyhow::{Context, Result};
use async_imap::types::Fetch;
use async_imap::Session;
use sentinel_core::capability::EmailId;
use sentinel_core::config::{EmailAccountConfig, EmailTriageConfig};
use sentinel_core::events::{EmailEvent, WatchEvent};
use sentinel_core::sanitize::sanitize_text;
use sentinel_core::types::Urgency;
use sentinel_memory::state::StateManager;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_native_tls::TlsStream;

/// IMAP IDLE email watcher with reconnection logic.
pub struct ImapWatcher {
    account: EmailAccountConfig,
    triage: TriageRules,
    state: StateManager,
}

/// Compiled triage rules for fast local classification.
struct TriageRules {
    priority_senders: Vec<String>,
    ignore_senders: Vec<String>,
    preview_max_chars: usize,
}

impl TriageRules {
    fn from_config(config: Option<&EmailTriageConfig>) -> Self {
        match config {
            Some(c) => Self {
                priority_senders: c.priority_senders.clone(),
                ignore_senders: c.ignore_senders.clone(),
                preview_max_chars: c.preview_max_chars.unwrap_or(500),
            },
            None => Self {
                priority_senders: vec![],
                ignore_senders: vec![],
                preview_max_chars: 500,
            },
        }
    }
}

type ImapSession = Session<TlsStream<TcpStream>>;

impl ImapWatcher {
    pub fn new(
        account: EmailAccountConfig,
        triage: Option<&EmailTriageConfig>,
        state: StateManager,
    ) -> Self {
        Self {
            account,
            triage: TriageRules::from_config(triage),
            state,
        }
    }

    /// Run the watcher with automatic reconnection.
    pub async fn run(self, tx: mpsc::Sender<WatchEvent>) -> Result<()> {
        loop {
            match self.watch_loop(&tx).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::error!(
                        account = %self.account.name,
                        error = %e,
                        "IMAP watcher error, reconnecting in 30s"
                    );
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            }
        }
    }

    async fn watch_loop(&self, tx: &mpsc::Sender<WatchEvent>) -> Result<()> {
        let mut session = self.connect().await?;
        let mailbox = session.select("INBOX").await.context("select INBOX")?;

        tracing::info!(
            account = %self.account.name,
            messages = mailbox.exists,
            "IMAP connected, INBOX selected"
        );

        let highwater_key = format!("uid_highwater_{}", self.account.name);

        let mut last_uid: u32 = self
            .state
            .get_watcher_state("email", &highwater_key)
            .await?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // If first run (no highwater), just set it to current max — don't flood with old emails
        if last_uid == 0 {
            let max_uid = self.get_max_uid(&mut session).await?;
            if max_uid > 0 {
                last_uid = max_uid;
                self.save_highwater(&highwater_key, last_uid).await?;
                tracing::info!(
                    account = %self.account.name,
                    uid = last_uid,
                    "first run, setting highwater to current max UID"
                );
            }
        } else {
            // Process any messages that arrived while we were offline
            let new_uids = self.search_new_uids(&mut session, last_uid).await?;
            if !new_uids.is_empty() {
                tracing::info!(
                    account = %self.account.name,
                    count = new_uids.len(),
                    "processing emails received while offline"
                );
                for uid in &new_uids {
                    if let Err(e) = self.process_uid(&mut session, *uid, tx).await {
                        tracing::warn!(uid, error = %e, "failed to process email");
                    }
                    if *uid > last_uid {
                        last_uid = *uid;
                    }
                }
                self.save_highwater(&highwater_key, last_uid).await?;
            }
        }

        // Enter IDLE loop
        loop {
            tracing::debug!(account = %self.account.name, "entering IDLE");

            let mut idle = session.idle();
            idle.init().await.context("IDLE init")?;

            // RFC 2177: re-issue IDLE every 29 minutes
            let (idle_wait, _interrupt) = idle.wait_with_timeout(Duration::from_secs(29 * 60));
            let _result = idle_wait.await.context("IDLE wait")?;

            session = idle.done().await.context("IDLE done")?;

            // Check for new messages
            let new_uids = self.search_new_uids(&mut session, last_uid).await?;
            for uid in &new_uids {
                if let Err(e) = self.process_uid(&mut session, *uid, tx).await {
                    tracing::warn!(uid, error = %e, "failed to process email");
                }
                if *uid > last_uid {
                    last_uid = *uid;
                }
            }
            if !new_uids.is_empty() {
                self.save_highwater(&highwater_key, last_uid).await?;
            }
        }
    }

    /// Connect to IMAP server with TLS.
    async fn connect(&self) -> Result<ImapSession> {
        let addr = format!("{}:{}", self.account.imap_host, self.account.imap_port);

        let tcp = TcpStream::connect(&addr)
            .await
            .with_context(|| format!("TCP connect to {addr}"))?;

        let tls = tokio_native_tls::TlsConnector::from(
            native_tls::TlsConnector::new().context("TLS connector")?,
        );

        let tls_stream = tls
            .connect(&self.account.imap_host, tcp)
            .await
            .with_context(|| format!("TLS handshake with {}", self.account.imap_host))?;

        let client = async_imap::Client::new(tls_stream);

        let username = std::env::var(format!(
            "SENTINEL_IMAP_USERNAME_{}",
            self.account.name.to_uppercase()
        ))
        .or_else(|_| std::env::var("SENTINEL_IMAP_USERNAME"))
        .context("IMAP username not set (SENTINEL_IMAP_USERNAME[_ACCOUNT])")?;

        let password = std::env::var(format!(
            "SENTINEL_IMAP_PASSWORD_{}",
            self.account.name.to_uppercase()
        ))
        .or_else(|_| std::env::var("SENTINEL_IMAP_PASSWORD"))
        .context("IMAP password not set (SENTINEL_IMAP_PASSWORD[_ACCOUNT])")?;

        let session = client
            .login(&username, &password)
            .await
            .map_err(|(e, _)| e)
            .context("IMAP login")?;

        tracing::info!(
            account = %self.account.name,
            host = %self.account.imap_host,
            "IMAP authenticated"
        );

        Ok(session)
    }

    /// Get the highest UID in the mailbox.
    async fn get_max_uid(&self, session: &mut ImapSession) -> Result<u32> {
        let uids = session
            .uid_search("ALL")
            .await
            .context("UID SEARCH ALL")?;
        Ok(uids.into_iter().max().unwrap_or(0))
    }

    /// Search for UIDs greater than the highwater mark.
    async fn search_new_uids(
        &self,
        session: &mut ImapSession,
        since_uid: u32,
    ) -> Result<Vec<u32>> {
        let query = format!("UID {}:*", since_uid + 1);
        let uids = session
            .uid_search(&query)
            .await
            .context("UID SEARCH new")?;

        // Filter out the since_uid itself (IMAP range is inclusive)
        let mut result: Vec<u32> = uids
            .into_iter()
            .filter(|&uid| uid > since_uid)
            .collect();
        result.sort();
        Ok(result)
    }

    /// Fetch a single email by UID, classify, and send as event.
    async fn process_uid(
        &self,
        session: &mut ImapSession,
        uid: u32,
        tx: &mpsc::Sender<WatchEvent>,
    ) -> Result<()> {
        let fetch_query = format!("{uid}");
        let mut messages = session
            .uid_fetch(
                &fetch_query,
                "(ENVELOPE BODY.PEEK[TEXT]<0.2048> FLAGS BODYSTRUCTURE)",
            )
            .await
            .context("UID FETCH")?;

        use futures_util::StreamExt;
        let msg: Option<Result<Fetch, _>> = messages.next().await;
        let Some(Ok(msg)) = msg else {
            tracing::debug!(uid, "UID FETCH returned no results");
            return Ok(());
        };

        let envelope = msg.envelope().context("missing envelope")?;

        let from = extract_address(
            envelope.from.as_deref().unwrap_or_default(),
        );

        let to: Vec<String> = envelope
            .to
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|addr| format_address(addr))
            .collect();

        let subject = envelope
            .subject
            .as_ref()
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .unwrap_or_default();

        let is_reply = envelope.in_reply_to.is_some();

        // Extract preview from body text
        let preview_raw = msg
            .text()
            .map(|t| String::from_utf8_lossy(t).into_owned())
            .unwrap_or_default();
        let preview: String = sanitize_text(&preview_raw)
            .chars()
            .take(self.triage.preview_max_chars)
            .collect();

        // Check for attachments via BODYSTRUCTURE
        let has_attachments = msg
            .bodystructure()
            .map(|bs| format!("{bs:?}").contains("attachment"))
            .unwrap_or(false);

        let urgency = self.classify_urgency(&from);

        let timestamp = envelope
        .date
        .as_ref()
        .and_then(|d| {
            let s = String::from_utf8_lossy(d);
            chrono::DateTime::parse_from_rfc2822(s.trim())
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc))
        })
        .unwrap_or_else(chrono::Utc::now);

        let event = EmailEvent {
            id: EmailId::new(self.account.name.clone(), uid),
            from: sanitize_text(&from),
            to,
            subject: sanitize_text(&subject),
            preview,
            //timestamp: chrono::Utc::now(),
            timestamp,
            is_reply,
            has_attachments,
            urgency,
        };

        tracing::info!(
            account = %self.account.name,
            uid,
            from = %event.from,
            subject = %event.subject,
            date = %event.timestamp.format("%Y-%m-%d %H:%M UTC"),
            urgency = ?event.urgency,
            "new email"
        );

        tx.send(WatchEvent::Email(event))
            .await
            .context("event channel closed")?;

        Ok(())
    }

    /// Classify urgency locally — no AI cost.
    fn classify_urgency(&self, from: &str) -> Urgency {
        let from_lower = from.to_lowercase();

        // Priority senders get high urgency
        for pattern in &self.triage.priority_senders {
            if matches_sender_pattern(&from_lower, pattern) {
                return Urgency::High;
            }
        }

        // Ignored senders get zero urgency
        for pattern in &self.triage.ignore_senders {
            if matches_sender_pattern(&from_lower, pattern) {
                return Urgency::Ignore;
            }
        }

        // No-reply addresses are low priority
        if from_lower.contains("noreply") || from_lower.contains("no-reply") {
            return Urgency::Low;
        }

        Urgency::Medium
    }

    async fn save_highwater(&self, key: &str, uid: u32) -> Result<()> {
        self.state
            .set_watcher_state("email", key, &uid.to_string())
            .await?;
        Ok(())
    }
}

/// Format an IMAP address into `mailbox@host`.
fn format_address(addr: &async_imap::imap_proto::types::Address) -> String {
    let mailbox = addr
        .mailbox
        .as_ref()
        .map(|m| String::from_utf8_lossy(m).into_owned())
        .unwrap_or_default();
    let host = addr
        .host
        .as_ref()
        .map(|h| String::from_utf8_lossy(h).into_owned())
        .unwrap_or_default();
    if host.is_empty() {
        mailbox
    } else {
        format!("{mailbox}@{host}")
    }
}

/// Extract the first sender address, or "unknown".
fn extract_address(addrs: &[async_imap::imap_proto::types::Address]) -> String {
    addrs.first().map(format_address).unwrap_or_else(|| "unknown".into())
}

/// Match a sender address against a glob pattern.
/// Supports `*` as wildcard prefix/suffix: `*@newsletter.*`, `noreply@*`, `ana@work.com`
fn matches_sender_pattern(sender: &str, pattern: &str) -> bool {
    let pattern = pattern.to_lowercase();

    if !pattern.contains('*') {
        return sender == pattern;
    }

    // Split on '*' and check that all parts appear in order
    let parts: Vec<&str> = pattern.split('*').collect();

    // Leading * means sender doesn't need to start with first part
    let must_start = !pattern.starts_with('*');
    let must_end = !pattern.ends_with('*');

    let mut remaining = sender;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 && must_start {
            if !remaining.starts_with(part) {
                return false;
            }
            remaining = &remaining[part.len()..];
        } else if i == parts.len() - 1 && must_end {
            if !remaining.ends_with(part) {
                return false;
            }
            return true;
        } else if let Some(pos) = remaining.find(part) {
            remaining = &remaining[pos + part.len()..];
        } else {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_pattern_exact() {
        assert!(matches_sender_pattern("ana@work.com", "ana@work.com"));
        assert!(!matches_sender_pattern("bob@work.com", "ana@work.com"));
    }

    #[test]
    fn sender_pattern_wildcard_prefix() {
        assert!(matches_sender_pattern(
            "news@newsletter.example.com",
            "*@newsletter.*"
        ));
        assert!(matches_sender_pattern("a@newsletter.co", "*@newsletter.*"));
        assert!(!matches_sender_pattern("a@news.com", "*@newsletter.*"));
    }

    #[test]
    fn sender_pattern_wildcard_suffix() {
        assert!(matches_sender_pattern("noreply@anything.com", "noreply@*"));
        assert!(!matches_sender_pattern("info@anything.com", "noreply@*"));
    }

    #[test]
    fn sender_pattern_domain_wildcard() {
        assert!(matches_sender_pattern("anyone@family.com", "*@family.com"));
        assert!(!matches_sender_pattern("anyone@work.com", "*@family.com"));
    }

    #[tokio::test]
    async fn classify_priority_sender() {
        let triage = TriageRules {
            priority_senders: vec!["ana@work.com".into(), "*@kidschool.pt".into()],
            ignore_senders: vec!["*@newsletter.*".into()],
            preview_max_chars: 500,
        };
        let watcher = ImapWatcher {
            account: EmailAccountConfig {
                name: "test".into(),
                imap_host: "localhost".into(),
                imap_port: 993,
                smtp_host: None,
                smtp_port: None,
                triage: None,
            },
            triage,
            state: test_state().await,
        };

        assert_eq!(watcher.classify_urgency("ana@work.com"), Urgency::High);
        assert_eq!(
            watcher.classify_urgency("teacher@kidschool.pt"),
            Urgency::High
        );
        assert_eq!(
            watcher.classify_urgency("promo@newsletter.example.com"),
            Urgency::Ignore
        );
        assert_eq!(
            watcher.classify_urgency("noreply@service.com"),
            Urgency::Low
        );
        assert_eq!(
            watcher.classify_urgency("colleague@work.com"),
            Urgency::Medium
        );
    }
}

// Test helper — creates a StateManager with an in-memory SQLite pool.
#[cfg(test)]
async fn test_state() -> StateManager {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let pool = sentinel_memory::db::open(&db_path).await.unwrap();
    // Leak the tempdir to keep it alive for the test duration
    std::mem::forget(dir);
    StateManager::new(pool)
}
