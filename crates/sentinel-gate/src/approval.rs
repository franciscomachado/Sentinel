use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use sentinel_core::capability::{Capability, CapabilityKind};
use sentinel_core::types::Decision;

/// A pending action awaiting human approval via Signal.
#[derive(Debug, Clone)]
pub struct PendingAction {
    /// Short unique ID shown in approval messages (e.g. "a3f").
    pub id: String,
    pub capability: Capability,
    pub explanation: String,
    pub cortex_reasoning: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// The result of parsing a user's reply.
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalReply {
    /// User approved a specific action.
    Approved(String),
    /// User rejected a specific action.
    Rejected(String),
    /// Not an approval reply — treat as a normal message.
    NotAnApproval,
}

/// Manages pending actions awaiting approval.
///
/// Thread-safe: wrapped in `Arc<Mutex>` internally so the daemon can
/// share it between the event processing pipeline and the Signal watcher.
#[derive(Clone)]
pub struct ApprovalManager {
    pending: Arc<Mutex<HashMap<String, PendingAction>>>,
}

impl ApprovalManager {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Add a pending action. Returns the short ID for the approval message.
    pub async fn add(&self, capability: Capability, explanation: String, reasoning: String) -> String {
        let id = generate_short_id();
        let action = PendingAction {
            id: id.clone(),
            capability,
            explanation,
            cortex_reasoning: reasoning,
            created_at: chrono::Utc::now(),
        };
        self.pending.lock().await.insert(id.clone(), action);
        id
    }

    /// Try to resolve a pending action by ID, returning it if found.
    pub async fn resolve(&self, id: &str) -> Option<PendingAction> {
        self.pending.lock().await.remove(id)
    }

    /// Get the capability kind for a pending action (for display).
    pub async fn get_kind(&self, id: &str) -> Option<CapabilityKind> {
        self.pending.lock().await.get(id).map(|a| a.capability.kind())
    }

    /// List all pending actions (for status display).
    pub async fn list_pending(&self) -> Vec<PendingAction> {
        self.pending.lock().await.values().cloned().collect()
    }

    /// Expire actions older than the given duration.
    pub async fn expire_old(&self, max_age: chrono::Duration) {
        let cutoff = chrono::Utc::now() - max_age;
        self.pending.lock().await.retain(|_, a| a.created_at > cutoff);
    }

    /// Parse a user's message to see if it's an approval/rejection reply.
    pub fn parse_reply(text: &str) -> ApprovalReply {
        let trimmed = text.trim().to_lowercase();

        // "yes <id>" or "approve <id>" or "ok <id>"
        if let Some(id) = trimmed
            .strip_prefix("yes ")
            .or_else(|| trimmed.strip_prefix("approve "))
            .or_else(|| trimmed.strip_prefix("ok "))
        {
            let id = id.trim();
            if !id.is_empty() {
                return ApprovalReply::Approved(id.to_string());
            }
        }

        // "no <id>" or "reject <id>"
        if let Some(id) = trimmed
            .strip_prefix("no ")
            .or_else(|| trimmed.strip_prefix("reject "))
        {
            let id = id.trim();
            if !id.is_empty() {
                return ApprovalReply::Rejected(id.to_string());
            }
        }

        ApprovalReply::NotAnApproval
    }

    /// Process an approval reply: resolve the action and return the decision.
    pub async fn process_reply(&self, reply: &ApprovalReply) -> Option<(PendingAction, Decision)> {
        match reply {
            ApprovalReply::Approved(id) => {
                self.resolve(id).await.map(|a| (a, Decision::HumanApproved))
            }
            ApprovalReply::Rejected(id) => {
                self.resolve(id).await.map(|a| (a, Decision::HumanRejected))
            }
            ApprovalReply::NotAnApproval => None,
        }
    }
}

/// Generate a short human-friendly ID (3 hex chars = 4096 possibilities).
/// Collisions are extremely unlikely in the small pending set.
fn generate_short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{:03x}", nanos % 0xFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_yes_reply() {
        assert_eq!(
            ApprovalManager::parse_reply("yes a3f"),
            ApprovalReply::Approved("a3f".into())
        );
    }

    #[test]
    fn parse_approve_reply() {
        assert_eq!(
            ApprovalManager::parse_reply("approve a3f"),
            ApprovalReply::Approved("a3f".into())
        );
    }

    #[test]
    fn parse_ok_reply() {
        assert_eq!(
            ApprovalManager::parse_reply("OK b12"),
            ApprovalReply::Approved("b12".into())
        );
    }

    #[test]
    fn parse_no_reply() {
        assert_eq!(
            ApprovalManager::parse_reply("no a3f"),
            ApprovalReply::Rejected("a3f".into())
        );
    }

    #[test]
    fn parse_reject_reply() {
        assert_eq!(
            ApprovalManager::parse_reply("reject c7e"),
            ApprovalReply::Rejected("c7e".into())
        );
    }

    #[test]
    fn parse_normal_message() {
        assert_eq!(
            ApprovalManager::parse_reply("What's for dinner tonight?"),
            ApprovalReply::NotAnApproval
        );
    }

    #[test]
    fn parse_case_insensitive() {
        assert_eq!(
            ApprovalManager::parse_reply("YES A3F"),
            ApprovalReply::Approved("a3f".into())
        );
    }

    #[test]
    fn parse_bare_yes_is_not_approval() {
        // "yes" alone without an ID is not an approval command
        assert_eq!(
            ApprovalManager::parse_reply("yes"),
            ApprovalReply::NotAnApproval
        );
    }

    #[test]
    fn parse_no_without_id() {
        assert_eq!(
            ApprovalManager::parse_reply("no"),
            ApprovalReply::NotAnApproval
        );
    }

    #[tokio::test]
    async fn add_and_resolve() {
        let mgr = ApprovalManager::new();
        let cap = sentinel_core::capability::Capability::TaskListRead;
        let id = mgr.add(cap, "test".into(), "reasoning".into()).await;
        assert!(!id.is_empty());

        let action = mgr.resolve(&id).await;
        assert!(action.is_some());

        // Second resolve returns None (already consumed)
        assert!(mgr.resolve(&id).await.is_none());
    }

    #[tokio::test]
    async fn process_approved_reply() {
        let mgr = ApprovalManager::new();
        let cap = sentinel_core::capability::Capability::TaskListRead;
        let id = mgr.add(cap, "test".into(), "reasoning".into()).await;

        let reply = ApprovalReply::Approved(id);
        let result = mgr.process_reply(&reply).await;
        assert!(result.is_some());
        let (_, decision) = result.unwrap();
        assert!(matches!(decision, Decision::HumanApproved));
    }

    #[tokio::test]
    async fn unknown_id_returns_none() {
        let mgr = ApprovalManager::new();
        let reply = ApprovalReply::Approved("nonexistent".into());
        assert!(mgr.process_reply(&reply).await.is_none());
    }
}
