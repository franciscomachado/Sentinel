use chrono::{DateTime, Datelike, NaiveTime, TimeZone, Utc, Weekday};
use serde::{Deserialize, Serialize};

use crate::holidays::HolidayCalendar;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskSchedule {
    Once {
        due: DateTime<Utc>,
    },
    Recurring {
        rrule: String,
    },
    BusinessDay {
        day_spec: BusinessDaySpec,
        holidays: String,
    },
    Triggered {
        trigger: TaskTrigger,
    },
    RelativeToEvent {
        event_pattern: String,
        offset_minutes: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BusinessDaySpec {
    FirstOfMonth,
    LastOfMonth,
    NthOfMonth(u8),
    FirstOfQuarter,
    LastFridayOfMonth,
    EveryNthBusinessDay(u8),
}

impl BusinessDaySpec {
    /// Resolve the next occurrence of this business-day spec on or after `after`.
    pub fn next_occurrence(
        &self,
        after: chrono::NaiveDate,
        calendar: &HolidayCalendar,
    ) -> Option<chrono::NaiveDate> {
        match self {
            Self::FirstOfMonth => {
                // Try current month first, then next month
                let candidate = calendar.nth_business_day(after.year(), after.month(), 1)?;
                if candidate >= after {
                    return Some(candidate);
                }
                let (y, m) = next_month(after.year(), after.month());
                calendar.nth_business_day(y, m, 1)
            }
            Self::LastOfMonth => {
                let candidate = calendar.last_business_day(after.year(), after.month())?;
                if candidate >= after {
                    return Some(candidate);
                }
                let (y, m) = next_month(after.year(), after.month());
                calendar.last_business_day(y, m)
            }
            Self::NthOfMonth(n) => {
                let candidate = calendar.nth_business_day(after.year(), after.month(), *n)?;
                if candidate >= after {
                    return Some(candidate);
                }
                let (y, m) = next_month(after.year(), after.month());
                calendar.nth_business_day(y, m, *n)
            }
            Self::FirstOfQuarter => {
                let quarter_starts = [1u32, 4, 7, 10];
                for &month in &quarter_starts {
                    if month >= after.month() {
                        let candidate = calendar.nth_business_day(after.year(), month, 1)?;
                        if candidate >= after {
                            return Some(candidate);
                        }
                    }
                }
                calendar.nth_business_day(after.year() + 1, 1, 1)
            }
            Self::LastFridayOfMonth => {
                // Find last Friday of the month, then check if it's a business day
                let candidate = last_friday_of_month(after.year(), after.month())?;
                if candidate >= after && calendar.is_business_day(candidate) {
                    return Some(candidate);
                }
                let (y, m) = next_month(after.year(), after.month());
                let candidate = last_friday_of_month(y, m)?;
                if calendar.is_business_day(candidate) {
                    Some(candidate)
                } else {
                    // Fall back to previous business day
                    let mut d = candidate;
                    while !calendar.is_business_day(d) {
                        d = d.pred_opt()?;
                    }
                    Some(d)
                }
            }
            Self::EveryNthBusinessDay(_n) => {
                // This needs a reference start date to count from, which we don't have here.
                // For now, return the next business day.
                Some(calendar.next_business_day(after))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTrigger {
    pub condition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ScheduleEntry {
    Daily {
        time: String,
        trigger: ScheduledTriggerKind,
    },
    Weekly {
        day: String,
        time: String,
        trigger: ScheduledTriggerKind,
    },
    Interval {
        every_seconds: u64,
        trigger: ScheduledTriggerKind,
    },
}

impl ScheduleEntry {
    /// Compute the next fire time for this schedule entry after `now`.
    /// `tz` is the user's timezone (e.g., "Europe/Lisbon" → chrono_tz offset).
    pub fn next_fire(&self, now: DateTime<Utc>, tz_offset: chrono::FixedOffset) -> Option<DateTime<Utc>> {
        match self {
            Self::Daily { time, .. } => {
                let local_now = now.with_timezone(&tz_offset);
                let t = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
                let today_fire = local_now.date_naive().and_time(t);
                let today_fire_utc = tz_offset.from_local_datetime(&today_fire).single()?.to_utc();

                if today_fire_utc > now {
                    Some(today_fire_utc)
                } else {
                    let tomorrow = local_now.date_naive().succ_opt()?;
                    let fire = tomorrow.and_time(t);
                    Some(tz_offset.from_local_datetime(&fire).single()?.to_utc())
                }
            }
            Self::Weekly { day, time, .. } => {
                let target_weekday = parse_weekday(day)?;
                let t = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
                let local_now = now.with_timezone(&tz_offset);
                let today = local_now.date_naive();

                // Find the next occurrence of target_weekday
                let mut d = today;
                for _ in 0..8 {
                    if d.weekday() == target_weekday {
                        let fire = d.and_time(t);
                        let fire_utc = tz_offset.from_local_datetime(&fire).single()?.to_utc();
                        if fire_utc > now {
                            return Some(fire_utc);
                        }
                    }
                    d = d.succ_opt()?;
                }
                None
            }
            Self::Interval { every_seconds, .. } => {
                Some(now + chrono::Duration::seconds(*every_seconds as i64))
            }
        }
    }

    pub fn trigger_kind(&self) -> &ScheduledTriggerKind {
        match self {
            Self::Daily { trigger, .. }
            | Self::Weekly { trigger, .. }
            | Self::Interval { trigger, .. } => trigger,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScheduledTriggerKind {
    RhythmEngineRun,
    MorningReflection,
    MorningBriefing,
    DepartureCheck,
    WeeklyReflection,
    WeeklyPlanning,
    MonthlyReflection,
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

fn last_friday_of_month(year: i32, month: u32) -> Option<chrono::NaiveDate> {
    let (ny, nm) = next_month(year, month);
    let first_next = chrono::NaiveDate::from_ymd_opt(ny, nm, 1)?;
    let last_day = first_next.pred_opt()?;
    let days_after_friday = (last_day.weekday().num_days_from_monday() + 7 - 4) % 7;
    last_day.checked_sub_signed(chrono::Duration::days(days_after_friday as i64))
}

fn parse_weekday(s: &str) -> Option<Weekday> {
    match s.to_lowercase().as_str() {
        "monday" | "mon" => Some(Weekday::Mon),
        "tuesday" | "tue" => Some(Weekday::Tue),
        "wednesday" | "wed" => Some(Weekday::Wed),
        "thursday" | "thu" => Some(Weekday::Thu),
        "friday" | "fri" => Some(Weekday::Fri),
        "saturday" | "sat" => Some(Weekday::Sat),
        "sunday" | "sun" => Some(Weekday::Sun),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_schedule_next_fire() {
        use chrono::Timelike;
        let tz = chrono::FixedOffset::east_opt(0).unwrap(); // UTC
        let now = Utc.with_ymd_and_hms(2026, 3, 15, 6, 0, 0).unwrap();

        let entry = ScheduleEntry::Daily {
            time: "07:30".into(),
            trigger: ScheduledTriggerKind::MorningBriefing,
        };
        let next = entry.next_fire(now, tz).unwrap();
        assert_eq!(next.hour(), 7);
        assert_eq!(next.minute(), 30);
        assert_eq!(next.day(), 15); // Same day, later
    }

    #[test]
    fn daily_schedule_wraps_to_tomorrow() {
        use chrono::Timelike;
        let tz = chrono::FixedOffset::east_opt(0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 3, 15, 20, 0, 0).unwrap();

        let entry = ScheduleEntry::Daily {
            time: "07:30".into(),
            trigger: ScheduledTriggerKind::MorningBriefing,
        };
        let next = entry.next_fire(now, tz).unwrap();
        assert_eq!(next.day(), 16); // Next day
        assert_eq!(next.hour(), 7);
    }

    #[test]
    fn weekly_schedule() {
        use chrono::Timelike;
        let tz = chrono::FixedOffset::east_opt(0).unwrap();
        // March 15 2026 is a Sunday
        let now = Utc.with_ymd_and_hms(2026, 3, 15, 10, 0, 0).unwrap();

        let entry = ScheduleEntry::Weekly {
            day: "monday".into(),
            time: "09:00".into(),
            trigger: ScheduledTriggerKind::WeeklyPlanning,
        };
        let next = entry.next_fire(now, tz).unwrap();
        assert_eq!(next.weekday(), Weekday::Mon);
        assert_eq!(next.hour(), 9);
    }

    #[test]
    fn first_business_day_of_month() {
        let cal = HolidayCalendar {
            country_code: "TEST".into(),
            fixed: vec![(1, 1)], // New Year
            easter_offsets: vec![],
            nth_weekday_rules: vec![],
            home_municipal: None,
            work_municipal: None,
        };
        let spec = BusinessDaySpec::FirstOfMonth;
        let after = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let result = spec.next_occurrence(after, &cal).unwrap();
        // Jan 1 is holiday, Jan 2 is Friday → first business day
        assert_eq!(
            result,
            chrono::NaiveDate::from_ymd_opt(2026, 1, 2).unwrap()
        );
    }
}
