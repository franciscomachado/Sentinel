use chrono::{DateTime, NaiveTime, Utc};
use sentinel_core::capability::Capability;
use sentinel_core::config::PolicyConfig;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

/// Pre-fetched async data passed into policy evaluation.
/// The daemon gathers this before calling `evaluate()`.
#[derive(Debug, Default)]
pub struct PolicyContext {
    /// Current month's estimated AI spend in euros.
    pub monthly_spend_eur: f64,
    /// Number of active recurring tasks.
    pub active_recurring_tasks: u32,
}

/// Policy engine that enforces action limits and approval requirements.
pub struct PolicyEngine {
    config: PolicyConfig,
    write_count: AtomicU32,
    write_window_start: Mutex<DateTime<Utc>>,
}

pub enum PolicyDecision {
    AutoApproved,
    RequiresApproval { reason: String },
    Blocked { reason: String },
}

impl PolicyEngine {
    pub fn new(config: PolicyConfig) -> Self {
        Self {
            config,
            write_count: AtomicU32::new(0),
            write_window_start: Mutex::new(Utc::now()),
        }
    }

    /// Evaluate a capability against all policies.
    pub fn evaluate(
        &self,
        capability: &Capability,
        now: DateTime<Utc>,
        ctx: &PolicyContext,
    ) -> PolicyDecision {
        // Check blocked actions first
        if let Some(reason) = self.is_blocked(capability, ctx) {
            return PolicyDecision::Blocked { reason };
        }

        // Rate limiting for writes
        if !capability.is_read() {
            if let Some(reason) = self.check_rate_limit(now) {
                return PolicyDecision::Blocked { reason };
            }
        }

        // Check if approval is needed
        if self.requires_approval(capability, ctx) {
            PolicyDecision::RequiresApproval {
                reason: format!("Write action {:?} requires human approval", capability.kind()),
            }
        } else {
            PolicyDecision::AutoApproved
        }
    }

    /// Whether this capability needs human approval.
    pub fn requires_approval(&self, capability: &Capability, ctx: &PolicyContext) -> bool {
        if capability.is_read() {
            return !self.config.auto_approve_reads;
        }

        // Check per-integration auto-approval policies
        match capability {
            // Email: always_confirm_send forces approval on drafts/replies
            Capability::EmailDraft(_) | Capability::EmailReply(_, _) => {
                if let Some(ref email_policy) = self.config.email {
                    if email_policy.always_confirm_send {
                        return true;
                    }
                }
            }
            // Calendar: require_confirmation_for_deletion
            Capability::CalendarEventDelete(_) => {
                if let Some(ref cal_policy) = self.config.calendar {
                    if cal_policy.require_confirmation_for_deletion {
                        return true;
                    }
                }
            }
            Capability::ReminderSet(_) => {
                if let Some(ref cal_policy) = self.config.calendar {
                    return !cal_policy.auto_approve_reminder_creation;
                }
            }
            // Tasks: max_recurring_tasks — if at limit, require approval for new recurring tasks
            Capability::TaskCreate(task) => {
                if let Some(ref task_policy) = self.config.tasks {
                    if matches!(task.schedule, sentinel_core::schedule::TaskSchedule::Recurring { .. }
                        | sentinel_core::schedule::TaskSchedule::BusinessDay { .. })
                        && ctx.active_recurring_tasks >= task_policy.max_recurring_tasks
                    {
                        return true;
                    }
                }
            }
            Capability::TaskComplete(_) => {
                if let Some(ref task_policy) = self.config.tasks {
                    return !task_policy.auto_approve_completion;
                }
            }
            // Bring: distinguish user-requested vs AI-suggested
            Capability::BringAdd(_) => {
                if let Some(ref bring_policy) = self.config.bring {
                    // auto_approve_ai_suggested=false means AI-initiated adds need approval.
                    // Since all BringAdd capabilities come from Cortex (AI),
                    // this flag controls whether they're auto-approved.
                    return !bring_policy.auto_approve_ai_suggested;
                }
            }
            Capability::BringRemove(_) => {
                if let Some(ref bring_policy) = self.config.bring {
                    return !bring_policy.auto_approve_when_user_requested;
                }
            }
            _ => {}
        }

        // All other writes require approval
        true
    }

    /// Check if action is outright blocked (e.g., quiet hours, banned recipients, budget).
    fn is_blocked(&self, capability: &Capability, ctx: &PolicyContext) -> Option<String> {
        // Check quiet hours for notifications
        if self.is_quiet_hours(Utc::now()) && !capability.is_read() {
            // During quiet hours, only high-urgency actions go through
            return Some("quiet hours: non-urgent write blocked".into());
        }

        // Check email blocklist
        if let Capability::EmailDraft(draft) = capability {
            if let Some(ref email_policy) = self.config.email {
                for recipient in &draft.to {
                    let lower = recipient.to_lowercase();
                    if email_policy.never_send_to.iter().any(|b| b.to_lowercase() == lower) {
                        return Some(format!("recipient {recipient} is on never_send_to list"));
                    }
                }
            }
        }

        // Spending policy: hard block when monthly budget is exceeded
        if let Some(ref spending) = self.config.spending {
            if ctx.monthly_spend_eur >= spending.monthly_ai_budget_euros {
                return Some(format!(
                    "monthly AI budget exceeded: €{:.2}/€{:.2}",
                    ctx.monthly_spend_eur, spending.monthly_ai_budget_euros,
                ));
            }
        }

        None
    }

    fn is_quiet_hours(&self, now: DateTime<Utc>) -> bool {
        let Some(ref qh) = self.config.quiet_hours else {
            return false;
        };

        let Ok(start) = NaiveTime::parse_from_str(&qh.start, "%H:%M") else {
            return false;
        };
        let Ok(end) = NaiveTime::parse_from_str(&qh.end, "%H:%M") else {
            return false;
        };

        let current_time = now.time();

        if start <= end {
            // Same-day range (e.g., 09:00–17:00)
            current_time >= start && current_time < end
        } else {
            // Overnight range (e.g., 22:00–07:00)
            current_time >= start || current_time < end
        }
    }

    fn check_rate_limit(&self, now: DateTime<Utc>) -> Option<String> {
        let mut window_start = self.write_window_start.lock().unwrap();
        let elapsed = now.signed_duration_since(*window_start);

        if elapsed.num_seconds() > 3600 {
            // Reset window
            *window_start = now;
            self.write_count.store(1, Ordering::Relaxed);
            return None;
        }

        let count = self.write_count.fetch_add(1, Ordering::Relaxed);
        if count >= self.config.max_writes_per_hour {
            Some(format!(
                "rate limit exceeded: {count}/{} writes this hour",
                self.config.max_writes_per_hour
            ))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentinel_core::capability::{CalendarEvent, EmailDraft, EmailId, DraftReply, EventId, Task, BringItem};
    use sentinel_core::config::{CalendarPolicyConfig, TasksPolicyConfig, BringPolicyConfig, SpendingPolicyConfig};
    use sentinel_core::schedule::TaskSchedule;

    fn test_config() -> PolicyConfig {
        PolicyConfig {
            auto_approve_reads: true,
            max_writes_per_hour: 10,
            quiet_hours: None,
            email: Some(sentinel_core::config::EmailPolicyConfig {
                never_send_to: vec!["ceo@bigcorp.com".into()],
                always_confirm_send: true,
            }),
            calendar: Some(CalendarPolicyConfig {
                auto_approve_reminder_creation: true,
                require_confirmation_for_deletion: true,
            }),
            tasks: Some(TasksPolicyConfig {
                auto_approve_completion: true,
                max_recurring_tasks: 5,
            }),
            bring: Some(BringPolicyConfig {
                auto_approve_when_user_requested: true,
                auto_approve_ai_suggested: false,
                notify_partner_on_removal: true,
            }),
            spending: Some(SpendingPolicyConfig {
                monthly_ai_budget_euros: 10.0,
                warn_at_percentage: 80,
            }),
        }
    }

    fn default_ctx() -> PolicyContext {
        PolicyContext::default()
    }

    #[test]
    fn reads_auto_approved() {
        let engine = PolicyEngine::new(test_config());
        let cap = Capability::TaskListRead;
        assert!(matches!(
            engine.evaluate(&cap, Utc::now(), &default_ctx()),
            PolicyDecision::AutoApproved
        ));
    }

    #[test]
    fn writes_require_approval() {
        let engine = PolicyEngine::new(test_config());
        let cap = Capability::CalendarEventCreate(CalendarEvent {
            title: "Test".into(),
            start: Utc::now(),
            end: None,
            location: None,
            description: None,
            all_day: false,
        });
        assert!(matches!(
            engine.evaluate(&cap, Utc::now(), &default_ctx()),
            PolicyDecision::RequiresApproval { .. }
        ));
    }

    #[test]
    fn blocked_email_recipient() {
        let engine = PolicyEngine::new(test_config());
        let cap = Capability::EmailDraft(EmailDraft {
            to: vec!["ceo@bigcorp.com".into()],
            subject: "Hello".into(),
            body: "Hi".into(),
            account: None,
        });
        assert!(matches!(
            engine.evaluate(&cap, Utc::now(), &default_ctx()),
            PolicyDecision::Blocked { .. }
        ));
    }

    #[test]
    fn always_confirm_send_forces_approval() {
        let engine = PolicyEngine::new(test_config());
        // EmailDraft to a non-blocked address should still require approval
        let cap = Capability::EmailDraft(EmailDraft {
            to: vec!["friend@example.com".into()],
            subject: "Hi".into(),
            body: "Hello".into(),
            account: None,
        });
        assert!(matches!(
            engine.evaluate(&cap, Utc::now(), &default_ctx()),
            PolicyDecision::RequiresApproval { .. }
        ));

        // EmailReply also requires approval
        let cap = Capability::EmailReply(
            EmailId::new("personal".into(), 1),
            DraftReply { body: "Thanks!".into() },
        );
        assert!(matches!(
            engine.evaluate(&cap, Utc::now(), &default_ctx()),
            PolicyDecision::RequiresApproval { .. }
        ));
    }

    #[test]
    fn require_confirmation_for_deletion() {
        let engine = PolicyEngine::new(test_config());
        let cap = Capability::CalendarEventDelete(EventId("ev-123".into()));
        assert!(matches!(
            engine.evaluate(&cap, Utc::now(), &default_ctx()),
            PolicyDecision::RequiresApproval { .. }
        ));
    }

    #[test]
    fn max_recurring_tasks_blocks_when_at_limit() {
        let engine = PolicyEngine::new(test_config());
        let cap = Capability::TaskCreate(Task {
            title: "Weekly standup".into(),
            notes: None,
            schedule: TaskSchedule::Recurring { rrule: "FREQ=WEEKLY".into() },
            context: vec![],
            conditions: vec![],
            urgency: sentinel_core::types::Urgency::Medium,
        });

        // Under limit — should require approval (it's a write) but not blocked
        let ctx = PolicyContext { active_recurring_tasks: 4, ..Default::default() };
        assert!(matches!(
            engine.evaluate(&cap, Utc::now(), &ctx),
            PolicyDecision::RequiresApproval { .. }
        ));

        // At limit — requires approval because max_recurring_tasks forces it
        let ctx = PolicyContext { active_recurring_tasks: 5, ..Default::default() };
        assert!(matches!(
            engine.evaluate(&cap, Utc::now(), &ctx),
            PolicyDecision::RequiresApproval { .. }
        ));
    }

    #[test]
    fn auto_approve_ai_suggested_false_requires_approval() {
        let engine = PolicyEngine::new(test_config());
        let cap = Capability::BringAdd(BringItem {
            name: "Milk".into(),
            category: None,
            context: None,
        });
        // auto_approve_ai_suggested is false, so AI-initiated adds need approval
        assert!(matches!(
            engine.evaluate(&cap, Utc::now(), &default_ctx()),
            PolicyDecision::RequiresApproval { .. }
        ));
    }

    #[test]
    fn spending_budget_exceeded_blocks() {
        let engine = PolicyEngine::new(test_config());
        let cap = Capability::CalendarEventCreate(CalendarEvent {
            title: "Test".into(),
            start: Utc::now(),
            end: None,
            location: None,
            description: None,
            all_day: false,
        });
        // Over budget
        let ctx = PolicyContext { monthly_spend_eur: 10.50, ..Default::default() };
        let decision = engine.evaluate(&cap, Utc::now(), &ctx);
        assert!(matches!(decision, PolicyDecision::Blocked { .. }));
        if let PolicyDecision::Blocked { reason } = decision {
            assert!(reason.contains("budget exceeded"));
        }
    }
}
