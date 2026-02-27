use sentinel_core::events::{ScheduledTrigger, WatchEvent};
use sentinel_core::schedule::{ScheduleEntry, ScheduledTriggerKind};
use tokio::sync::mpsc::Sender;

/// Time-based trigger watcher for scheduled events.
///
/// Fires ScheduledTrigger events at the configured times based on
/// the user's local timezone offset.
pub struct TimeWatcher {
    pub schedule: Vec<ScheduleEntry>,
    pub tz_offset: chrono::FixedOffset,
}

impl TimeWatcher {
    pub fn new(schedule: Vec<ScheduleEntry>, tz_offset: chrono::FixedOffset) -> Self {
        Self {
            schedule,
            tz_offset,
        }
    }

    /// Run the schedule loop, emitting trigger events.
    pub async fn run(self, tx: Sender<WatchEvent>) -> anyhow::Result<()> {
        if self.schedule.is_empty() {
            tracing::info!("time watcher: no scheduled entries, exiting");
            return Ok(());
        }

        loop {
            let now = chrono::Utc::now();

            // Find the soonest next fire time across all schedule entries
            let mut soonest: Option<(std::time::Duration, &ScheduleEntry)> = None;

            for entry in &self.schedule {
                if let Some(next_fire) = entry.next_fire(now, self.tz_offset) {
                    let delta = next_fire - now;
                    if let Ok(dur) = delta.to_std() {
                        match &soonest {
                            None => soonest = Some((dur, entry)),
                            Some((current_min, _)) if dur < *current_min => {
                                soonest = Some((dur, entry));
                            }
                            _ => {}
                        }
                    }
                }
            }

            let Some((sleep_dur, entry)) = soonest else {
                tracing::warn!("time watcher: no next fire time found, sleeping 60s");
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                continue;
            };

            let trigger_kind = entry.trigger_kind().clone();
            tracing::info!(
                trigger = ?trigger_kind,
                sleep_secs = sleep_dur.as_secs(),
                "time watcher: waiting for next trigger"
            );

            tokio::time::sleep(sleep_dur).await;

            let event = kind_to_event(&trigger_kind);
            if tx.send(event).await.is_err() {
                return Ok(()); // channel closed
            }
        }
    }
}

fn kind_to_event(kind: &ScheduledTriggerKind) -> WatchEvent {
    let trigger = match kind {
        ScheduledTriggerKind::RhythmEngineRun => ScheduledTrigger::RhythmEngineRun,
        ScheduledTriggerKind::MorningReflection => ScheduledTrigger::MorningReflection,
        ScheduledTriggerKind::MorningBriefing => ScheduledTrigger::MorningBriefing,
        ScheduledTriggerKind::DepartureCheck => ScheduledTrigger::DepartureCheck,
        ScheduledTriggerKind::WeeklyReflection => ScheduledTrigger::WeeklyReflection,
        ScheduledTriggerKind::WeeklyPlanning => ScheduledTrigger::WeeklyPlanning,
        ScheduledTriggerKind::MonthlyReflection => ScheduledTrigger::MonthlyReflection,
    };
    WatchEvent::Schedule(trigger)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_to_event_mapping() {
        let event = kind_to_event(&ScheduledTriggerKind::MorningBriefing);
        assert!(matches!(
            event,
            WatchEvent::Schedule(ScheduledTrigger::MorningBriefing)
        ));

        let event = kind_to_event(&ScheduledTriggerKind::WeeklyPlanning);
        assert!(matches!(
            event,
            WatchEvent::Schedule(ScheduledTrigger::WeeklyPlanning)
        ));
    }
}
