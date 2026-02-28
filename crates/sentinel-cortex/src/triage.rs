use sentinel_core::events::{ScheduledTrigger, WatchEvent};
use sentinel_core::types::Urgency;

/// Local event triage — no AI cost.
pub enum Triage {
    /// Drop the event silently.
    Ignore,
    /// Pass through as a notification without AI involvement.
    PassThrough(String),
    /// Send to the LLM for processing.
    NeedsAI(TriggerType),
}

pub enum TriggerType {
    MorningBriefing,
    MorningReflection,
    WeeklyPlanning,
    WeeklyReflection,
    MonthlyReflection,
    EmailTriage(EmailTrigger),
    DepartureAlert(DepartureTrigger),
    SignalQuery(SignalTrigger),
    UserNote(UserNoteTrigger),
    CalendarChange,
    TaskEvent,
    WeatherUpdate,
}

pub struct EmailTrigger {
    pub from: String,
    pub subject: String,
    pub preview: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub struct DepartureTrigger {
    pub destination: String,
    pub event_time: chrono::DateTime<chrono::Utc>,
    pub travel_minutes: u32,
    pub leave_by: chrono::DateTime<chrono::Utc>,
}

pub struct SignalTrigger {
    pub text: String,
}

pub struct UserNoteTrigger {
    pub text: String,
}

/// Perform local triage on an event before deciding whether to involve the AI.
pub fn local_triage(event: &WatchEvent) -> Triage {
    match event {
        WatchEvent::Email(email) => {
            // Low-urgency emails from unknown senders → ignore
            if email.urgency == Urgency::Ignore {
                return Triage::Ignore;
            }

            Triage::NeedsAI(TriggerType::EmailTriage(EmailTrigger {
                from: email.from.clone(),
                subject: email.subject.clone(),
                preview: email.preview.clone(),
                timestamp: email.timestamp,
            }))
        }

        WatchEvent::Calendar(_change) => Triage::NeedsAI(TriggerType::CalendarChange),

        WatchEvent::Task(_task_event) => Triage::NeedsAI(TriggerType::TaskEvent),

        WatchEvent::Departure(dep) => {
            Triage::NeedsAI(TriggerType::DepartureAlert(DepartureTrigger {
                destination: dep.destination.clone(),
                event_time: dep.event_time,
                travel_minutes: dep.travel_minutes,
                leave_by: dep.leave_by,
            }))
        }

        WatchEvent::Signal(msg) => {
            if msg.text.trim().is_empty() {
                return Triage::Ignore;
            }
            // Heuristic: if it looks like a question or command, it's a query.
            // If it looks like a statement/observation, it's a user note.
            let trimmed = msg.text.trim();
            if is_likely_query(trimmed) {
                Triage::NeedsAI(TriggerType::SignalQuery(SignalTrigger {
                    text: msg.text.clone(),
                }))
            } else {
                Triage::NeedsAI(TriggerType::UserNote(UserNoteTrigger {
                    text: msg.text.clone(),
                }))
            }
        }

        WatchEvent::Weather(update) => {
            // Only escalate to AI if conditions are notable
            let dominated_by_bad = update.conditions.to_lowercase();
            if dominated_by_bad.contains("storm")
                || dominated_by_bad.contains("warning")
                || dominated_by_bad.contains("extreme")
            {
                Triage::NeedsAI(TriggerType::WeatherUpdate)
            } else {
                // Store but don't call AI for normal weather
                Triage::PassThrough(format!(
                    "Weather: {}°C, {}",
                    update.temperature_c, update.conditions
                ))
            }
        }

        WatchEvent::Sports(alert) => {
            // Sports alerts are pre-filtered by the watcher (notify policy).
            // Pass through as a notification — no AI needed.
            if alert.spoiler_protect {
                Triage::PassThrough(format!(
                    "🏁 {} — {} available soon (spoiler-free reminder)",
                    alert.series_name, alert.round_name,
                ))
            } else {
                let mins = (alert.start_utc - chrono::Utc::now()).num_minutes();
                let time_str = if mins > 0 {
                    format!("in {} min", mins)
                } else {
                    "now".into()
                };
                Triage::PassThrough(format!(
                    "🏁 {} {} — {} {time_str}",
                    alert.series_name, alert.round_name, alert.session_name,
                ))
            }
        }

        WatchEvent::Cultural(alert) => {
            // Cultural alerts are pre-scored by the taste engine.
            // Pass through — no AI cost.
            let venue_str = alert.venue.as_deref()
                .map(|v| format!(" at {v}"))
                .unwrap_or_default();
            let date_str = alert.date
                .map(|d| d.format("%a %H:%M").to_string())
                .unwrap_or_else(|| "TBD".into());
            Triage::PassThrough(format!(
                "🎭 {}{venue_str} — {date_str} ({:.0}% match)",
                alert.title, alert.match_score * 100.0,
            ))
        }

        WatchEvent::Schedule(trigger) => match trigger {
            ScheduledTrigger::MorningBriefing => {
                Triage::NeedsAI(TriggerType::MorningBriefing)
            }
            ScheduledTrigger::MorningReflection => {
                Triage::NeedsAI(TriggerType::MorningReflection)
            }
            ScheduledTrigger::WeeklyPlanning => {
                Triage::NeedsAI(TriggerType::WeeklyPlanning)
            }
            ScheduledTrigger::WeeklyReflection => {
                Triage::NeedsAI(TriggerType::WeeklyReflection)
            }
            ScheduledTrigger::MonthlyReflection => {
                Triage::NeedsAI(TriggerType::MonthlyReflection)
            }
            ScheduledTrigger::DepartureCheck => {
                // Departure check is handled by the watcher emitting DepartureEvent
                Triage::Ignore
            }
            ScheduledTrigger::RhythmEngineRun => {
                // Rhythm engine runs internally, doesn't need AI
                Triage::Ignore
            }
        },
    }
}

/// Heuristic to distinguish questions/commands from freeform notes.
///
/// Questions: "what's for dinner?", "any emails from Ana?"
/// Commands: "add milk to the shopping list", "remind me to..."
/// Notes: "blood pressure 140/85", "went to the gym", "feeling tired"
///
/// Both go through the LLM, but with different context and prompting.
fn is_likely_query(text: &str) -> bool {
    let lower = text.to_lowercase();

    // Ends with question mark → query
    if lower.trim_end().ends_with('?') {
        return true;
    }

    // Starts with question words → query
    let question_starters = [
        "what", "when", "where", "who", "how", "why", "which",
        "is ", "are ", "do ", "does ", "did ", "can ", "could ",
        "will ", "would ", "should ", "any ",
    ];
    if question_starters.iter().any(|q| lower.starts_with(q)) {
        return true;
    }

    // Starts with command verbs → query (imperative request)
    let command_starters = [
        "add ", "remove ", "delete ", "cancel ", "remind ",
        "schedule ", "set ", "move ", "show ", "tell ", "list ",
        "check ", "send ", "draft ", "reply ",
    ];
    if command_starters.iter().any(|c| lower.starts_with(c)) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_question_mark() {
        assert!(is_likely_query("What's for dinner tonight?"));
    }

    #[test]
    fn query_question_word() {
        assert!(is_likely_query("when is the dentist appointment"));
    }

    #[test]
    fn query_command_verb() {
        assert!(is_likely_query("add milk to the shopping list"));
        assert!(is_likely_query("remind me to call the plumber"));
    }

    #[test]
    fn note_blood_pressure() {
        assert!(!is_likely_query("blood pressure 140/85"));
    }

    #[test]
    fn note_observation() {
        assert!(!is_likely_query("feeling exhausted this week"));
    }

    #[test]
    fn note_meal_logged() {
        assert!(!is_likely_query("made chicken curry for dinner"));
    }

    #[test]
    fn note_freeform() {
        assert!(!is_likely_query("saw a nice Renault 5 at the dealership"));
    }
}
