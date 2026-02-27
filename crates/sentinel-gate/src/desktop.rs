use notify_rust::{Notification, Urgency as NotifyUrgency};
use sentinel_core::types::Urgency;

/// Desktop notifications via D-Bus / libnotify.
///
/// Maps Sentinel's urgency levels to freedesktop notification urgency
/// and uses appropriate icons. Notifications are non-blocking — if the
/// D-Bus session bus isn't available (e.g., headless server), the send
/// gracefully fails and logs a warning.
pub struct DesktopNotifier {
    app_name: String,
}

impl DesktopNotifier {
    pub fn new(name: &str) -> Self {
        Self {
            app_name: name.to_string(),
        }
    }

    /// Send a desktop notification with urgency-appropriate styling.
    pub fn send(&self, urgency: &Urgency, title: &str, body: &str) -> anyhow::Result<()> {
        let notify_urgency = match urgency {
            Urgency::Ignore | Urgency::Low => NotifyUrgency::Low,
            Urgency::Medium => NotifyUrgency::Normal,
            Urgency::High | Urgency::Urgent => NotifyUrgency::Critical,
        };

        let icon = match urgency {
            Urgency::Ignore | Urgency::Low => "mail-unread",
            Urgency::Medium => "dialog-information",
            Urgency::High => "dialog-warning",
            Urgency::Urgent => "dialog-error",
        };

        // Timeout: low urgency expires after 10s, critical stays until dismissed
        let timeout_ms = match urgency {
            Urgency::Ignore | Urgency::Low => 10_000,
            Urgency::Medium => 15_000,
            Urgency::High | Urgency::Urgent => 0, // 0 = never expire
        };

        Notification::new()
            .appname(&self.app_name)
            .summary(title)
            .body(body)
            .icon(icon)
            .urgency(notify_urgency)
            .timeout(timeout_ms)
            .show()?;

        Ok(())
    }

    /// Send a simple informational notification (Medium urgency).
    pub fn info(&self, title: &str, body: &str) -> anyhow::Result<()> {
        self.send(&Urgency::Medium, title, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifier_constructs() {
        let notifier = DesktopNotifier::new("Archibald");
        assert_eq!(notifier.app_name, "Archibald");
    }
}
