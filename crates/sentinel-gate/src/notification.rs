use sentinel_core::types::Urgency;

use crate::desktop::DesktopNotifier;

/// Notification routing to desktop (and eventually Signal).
///
/// The router tries each configured channel in order. Desktop notifications
/// are best-effort — if D-Bus isn't available, we log a warning and continue.
/// The terminal fallback always works.
pub struct NotificationRouter {
    desktop: Option<DesktopNotifier>,
    /// The assistant's display name, used in terminal echo and notifications.
    name: String,
    /// Whether to also print to stdout (useful for daemon logs / testing).
    terminal_echo: bool,
}

impl NotificationRouter {
    /// Create a router with desktop notifications enabled.
    pub fn with_desktop(name: &str) -> Self {
        Self {
            desktop: Some(DesktopNotifier::new(name)),
            name: name.to_string(),
            terminal_echo: true,
        }
    }

    /// Create a terminal-only router (no desktop notifications).
    pub fn terminal_only(name: &str) -> Self {
        Self {
            desktop: None,
            name: name.to_string(),
            terminal_echo: true,
        }
    }

    /// Send a notification through all configured channels.
    pub fn notify(&self, urgency: &Urgency, title: &str, body: &str) {
        // Desktop notification (best-effort)
        if let Some(desktop) = &self.desktop {
            if let Err(e) = desktop.send(urgency, title, body) {
                tracing::warn!(error = %e, "desktop notification failed — is D-Bus available?");
            }
        }

        // Terminal echo
        if self.terminal_echo {
            println!("[{} | {urgency}] {title}", self.name);
            if !body.is_empty() {
                println!("  {body}");
            }
            println!();
        }
    }

    /// Send an informational message (no urgency prefix).
    pub fn info(&self, message: &str) {
        if let Some(desktop) = &self.desktop {
            if let Err(e) = desktop.info(&self.name, message) {
                tracing::warn!(error = %e, "desktop notification failed");
            }
        }
        if self.terminal_echo {
            println!("[{}] {message}", self.name);
        }
    }
}
