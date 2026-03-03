use chrono::{Datelike, NaiveDate, Weekday};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct HolidayFile {
    pub country: CountryInfo,
    pub fixed_holidays: Vec<FixedHoliday>,
    #[serde(default)]
    pub easter_relative: Vec<EasterRelativeHoliday>,
    #[serde(default)]
    pub nth_weekday_holidays: Vec<NthWeekdayHoliday>,
    #[serde(default)]
    pub regions: HashMap<String, RegionConfig>,
}

#[derive(Debug, Deserialize)]
pub struct CountryInfo {
    pub name: String,
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct FixedHoliday {
    pub name: String,
    pub month: u32,
    pub day: u32,
}

#[derive(Debug, Deserialize)]
pub struct EasterRelativeHoliday {
    pub name: String,
    pub offset: i32,
}

/// Holiday defined as "nth weekday of month" (e.g., 3rd Monday of January).
/// Use `nth = -1` for "last weekday of month" (e.g., last Monday of May).
#[derive(Debug, Deserialize)]
pub struct NthWeekdayHoliday {
    pub name: String,
    pub month: u32,
    pub weekday: String,
    pub nth: i8,
}

#[derive(Debug, Deserialize)]
pub struct RegionConfig {
    pub name: String,
    #[serde(default)]
    pub holidays: Vec<FixedHoliday>,
}

/// A rule for computing a holiday that falls on the nth weekday of a month.
#[derive(Debug, Clone)]
pub struct NthWeekdayRule {
    pub month: u32,
    pub weekday: Weekday,
    pub nth: i8,
}

/// Describes what kind of day it is when home and work municipalities differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayKind {
    /// Normal business day — no municipal holidays apply.
    Normal,
    /// Home municipality holiday only — kids home, you still work.
    HomeHoliday,
    /// Work municipality holiday only — you're off, kids at school.
    WorkHoliday,
    /// National holiday or both municipalities — full day off.
    FullHoliday,
}

pub struct HolidayCalendar {
    pub country_code: String,
    pub fixed: Vec<(u32, u32)>,
    pub easter_offsets: Vec<i32>,
    pub nth_weekday_rules: Vec<NthWeekdayRule>,
    /// Municipal holiday for the home region (affects school, local services).
    pub home_municipal: Option<(u32, u32)>,
    /// Municipal holiday for the work region (affects whether you go to office).
    pub work_municipal: Option<(u32, u32)>,
}

impl HolidayCalendar {
    /// Load a holiday calendar from a parsed TOML file.
    /// Accepts separate home and work region keys to resolve municipal holidays.
    pub fn from_file(
        file: &HolidayFile,
        home_region: Option<&str>,
        work_region: Option<&str>,
    ) -> Self {
        let fixed = file
            .fixed_holidays
            .iter()
            .map(|h| (h.month, h.day))
            .collect();

        let easter_offsets = file.easter_relative.iter().map(|h| h.offset).collect();

        let nth_weekday_rules = file
            .nth_weekday_holidays
            .iter()
            .filter_map(|h| {
                let weekday = parse_weekday(&h.weekday)?;
                Some(NthWeekdayRule {
                    month: h.month,
                    weekday,
                    nth: h.nth,
                })
            })
            .collect();

        let home_municipal = home_region
            .and_then(|r| file.regions.get(r))
            .and_then(|rc| rc.holidays.first())
            .map(|h| (h.month, h.day));

        let work_municipal = work_region
            .and_then(|r| file.regions.get(r))
            .and_then(|rc| rc.holidays.first())
            .map(|h| (h.month, h.day));

        Self {
            country_code: file.country.code.clone(),
            fixed,
            easter_offsets,
            nth_weekday_rules,
            home_municipal,
            work_municipal,
        }
    }

    /// Load a holiday calendar from a TOML string.
    pub fn from_toml(
        toml_str: &str,
        home_region: Option<&str>,
        work_region: Option<&str>,
    ) -> Result<Self, toml::de::Error> {
        let file: HolidayFile = toml::from_str(toml_str)?;
        Ok(Self::from_file(&file, home_region, work_region))
    }

    pub fn is_weekend(date: NaiveDate) -> bool {
        matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
    }

    /// Check if the date is a national holiday (fixed, Easter-relative, or nth-weekday).
    fn is_national_holiday(&self, date: NaiveDate) -> bool {
        let md = (date.month(), date.day());

        if self.fixed.contains(&md) {
            return true;
        }

        let easter = easter_sunday(date.year());
        for &offset in &self.easter_offsets {
            if let Some(holiday) = easter.checked_add_signed(chrono::Duration::days(offset as i64))
            {
                if holiday == date {
                    return true;
                }
            }
        }

        for rule in &self.nth_weekday_rules {
            if date.month() == rule.month && date.weekday() == rule.weekday {
                if let Some(resolved) =
                    resolve_nth_weekday(date.year(), rule.month, rule.weekday, rule.nth)
                {
                    if resolved == date {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Returns true if the date is a holiday in **either** municipality or nationally.
    pub fn is_holiday(&self, date: NaiveDate) -> bool {
        if self.is_national_holiday(date) {
            return true;
        }
        let md = (date.month(), date.day());
        if self.home_municipal == Some(md) || self.work_municipal == Some(md) {
            return true;
        }
        false
    }

    /// Classify what kind of day this is, considering the dual-municipality scenario.
    /// Weekends return `FullHoliday`. National holidays return `FullHoliday`.
    /// Municipal holidays return the appropriate variant.
    pub fn day_kind(&self, date: NaiveDate) -> DayKind {
        if Self::is_weekend(date) || self.is_national_holiday(date) {
            return DayKind::FullHoliday;
        }

        let md = (date.month(), date.day());
        let home = self.home_municipal == Some(md);
        let work = self.work_municipal == Some(md);

        match (home, work) {
            (true, true) => DayKind::FullHoliday,
            (true, false) => DayKind::HomeHoliday,
            (false, true) => DayKind::WorkHoliday,
            (false, false) => DayKind::Normal,
        }
    }

    /// A business day is a weekday that is not a national or *work* municipality holiday.
    pub fn is_business_day(&self, date: NaiveDate) -> bool {
        if Self::is_weekend(date) || self.is_national_holiday(date) {
            return false;
        }
        let md = (date.month(), date.day());
        if self.work_municipal == Some(md) {
            return false;
        }
        true
    }

    /// Find the next business day on or after the given date.
    pub fn next_business_day(&self, date: NaiveDate) -> NaiveDate {
        let mut d = date;
        while !self.is_business_day(d) {
            d = d.succ_opt().expect("date overflow");
        }
        d
    }

    /// Find the next business day strictly after the given date.
    pub fn next_business_day_after(&self, date: NaiveDate) -> NaiveDate {
        self.next_business_day(date.succ_opt().expect("date overflow"))
    }

    /// Find the nth business day of a given month/year (1-indexed).
    pub fn nth_business_day(&self, year: i32, month: u32, n: u8) -> Option<NaiveDate> {
        let mut count = 0u8;
        let mut day = NaiveDate::from_ymd_opt(year, month, 1)?;

        loop {
            if day.month() != month {
                return None; // ran past the month
            }
            if self.is_business_day(day) {
                count += 1;
                if count == n {
                    return Some(day);
                }
            }
            day = day.succ_opt()?;
        }
    }

    /// Find the last business day of a given month/year.
    pub fn last_business_day(&self, year: i32, month: u32) -> Option<NaiveDate> {
        // Start from the last day of the month
        let next_month = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1)?
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1)?
        };
        let mut day = next_month.pred_opt()?;

        while !self.is_business_day(day) {
            day = day.pred_opt()?;
            if day.month() != month {
                return None;
            }
        }
        Some(day)
    }
}

impl Default for HolidayCalendar {
    /// A calendar with no holidays — treats every weekday as a business day.
    fn default() -> Self {
        Self {
            country_code: String::new(),
            fixed: vec![],
            easter_offsets: vec![],
            nth_weekday_rules: vec![],
            home_municipal: None,
            work_municipal: None,
        }
    }
}

/// Load the built-in holiday calendar for a known ISO country code (case-insensitive).
/// Returns `None` for unrecognised codes; the caller can fall back to `HolidayCalendar::default()`.
pub fn load_calendar_for_country(country_code: &str) -> Option<HolidayCalendar> {
    let toml_str = match country_code.to_lowercase().as_str() {
        "pt" => include_str!("../../../config/holidays/pt.toml"),
        "de" => include_str!("../../../config/holidays/de.toml"),
        "us" => include_str!("../../../config/holidays/us.toml"),
        _ => return None,
    };
    HolidayCalendar::from_toml(toml_str, None, None).ok()
}

/// Resolve the nth occurrence of a weekday in a given month.
/// `nth = 1` → first, `nth = 2` → second, ..., `nth = -1` → last.
fn resolve_nth_weekday(year: i32, month: u32, weekday: Weekday, nth: i8) -> Option<NaiveDate> {
    if nth == 0 {
        return None;
    }

    if nth > 0 {
        // Count forward from the 1st
        let mut count = 0i8;
        let mut d = NaiveDate::from_ymd_opt(year, month, 1)?;
        while d.month() == month {
            if d.weekday() == weekday {
                count += 1;
                if count == nth {
                    return Some(d);
                }
            }
            d = d.succ_opt()?;
        }
        None
    } else {
        // Count backward from the last day of the month
        let next_month_first = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1)?
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1)?
        };
        let mut d = next_month_first.pred_opt()?;
        let target = -nth; // e.g., -1 → find the 1st from the end
        let mut count = 0i8;
        while d.month() == month {
            if d.weekday() == weekday {
                count += 1;
                if count == target {
                    return Some(d);
                }
            }
            d = d.pred_opt()?;
        }
        None
    }
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

/// Compute Easter Sunday for a given year using the Anonymous Gregorian algorithm.
/// Valid for all years in the Gregorian calendar.
pub fn easter_sunday(year: i32) -> NaiveDate {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;

    NaiveDate::from_ymd_opt(year, month as u32, day as u32).expect("invalid Easter date computed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easter_known_dates() {
        assert_eq!(
            easter_sunday(2024),
            NaiveDate::from_ymd_opt(2024, 3, 31).unwrap()
        );
        assert_eq!(
            easter_sunday(2025),
            NaiveDate::from_ymd_opt(2025, 4, 20).unwrap()
        );
        assert_eq!(
            easter_sunday(2026),
            NaiveDate::from_ymd_opt(2026, 4, 5).unwrap()
        );
    }

    #[test]
    fn portugal_holidays() {
        let toml_str = r#"
[country]
name = "Portugal"
code = "PT"

[[fixed_holidays]]
name = "Dia da Liberdade"
month = 4
day = 25

[[fixed_holidays]]
name = "Natal"
month = 12
day = 25

[[easter_relative]]
name = "Sexta-feira Santa"
offset = -2

[[easter_relative]]
name = "Corpo de Deus"
offset = 60
"#;
        let cal = HolidayCalendar::from_toml(toml_str, None, None).unwrap();

        // April 25 is a fixed holiday
        assert!(cal.is_holiday(NaiveDate::from_ymd_opt(2026, 4, 25).unwrap()));

        // Good Friday 2026 = April 3 (Easter April 5 - 2)
        assert!(cal.is_holiday(NaiveDate::from_ymd_opt(2026, 4, 3).unwrap()));

        // Corpo de Deus 2026 = June 4 (Easter April 5 + 60)
        assert!(cal.is_holiday(NaiveDate::from_ymd_opt(2026, 6, 4).unwrap()));

        // Same holidays work for different years without changing the file
        // Good Friday 2025 = April 18 (Easter April 20 - 2)
        assert!(cal.is_holiday(NaiveDate::from_ymd_opt(2025, 4, 18).unwrap()));

        // Regular day
        assert!(!cal.is_holiday(NaiveDate::from_ymd_opt(2026, 3, 10).unwrap()));
    }

    #[test]
    fn us_nth_weekday_holidays() {
        let toml_str = r#"
[country]
name = "United States"
code = "US"

[[fixed_holidays]]
name = "New Year's Day"
month = 1
day = 1

[[nth_weekday_holidays]]
name = "MLK Day"
month = 1
weekday = "monday"
nth = 3

[[nth_weekday_holidays]]
name = "Presidents' Day"
month = 2
weekday = "monday"
nth = 3

[[nth_weekday_holidays]]
name = "Memorial Day"
month = 5
weekday = "monday"
nth = -1

[[nth_weekday_holidays]]
name = "Labor Day"
month = 9
weekday = "monday"
nth = 1

[[nth_weekday_holidays]]
name = "Thanksgiving"
month = 11
weekday = "thursday"
nth = 4
"#;
        let cal = HolidayCalendar::from_toml(toml_str, None, None).unwrap();

        // MLK Day 2026 = 3rd Monday of January = Jan 19
        assert!(cal.is_holiday(NaiveDate::from_ymd_opt(2026, 1, 19).unwrap()));
        assert!(!cal.is_holiday(NaiveDate::from_ymd_opt(2026, 1, 12).unwrap())); // 2nd Monday

        // Memorial Day 2026 = last Monday of May = May 25
        assert!(cal.is_holiday(NaiveDate::from_ymd_opt(2026, 5, 25).unwrap()));

        // Labor Day 2026 = 1st Monday of September = Sep 7
        assert!(cal.is_holiday(NaiveDate::from_ymd_opt(2026, 9, 7).unwrap()));

        // Thanksgiving 2026 = 4th Thursday of November = Nov 26
        assert!(cal.is_holiday(NaiveDate::from_ymd_opt(2026, 11, 26).unwrap()));

        // Works for other years too — MLK Day 2025 = Jan 20
        assert!(cal.is_holiday(NaiveDate::from_ymd_opt(2025, 1, 20).unwrap()));
    }

    #[test]
    fn resolve_nth_weekday_edge_cases() {
        // 5th Monday of February 2026 doesn't exist
        assert!(resolve_nth_weekday(2026, 2, Weekday::Mon, 5).is_none());

        // nth = 0 is invalid
        assert!(resolve_nth_weekday(2026, 1, Weekday::Mon, 0).is_none());

        // Last Friday of January 2026 = Jan 30
        assert_eq!(
            resolve_nth_weekday(2026, 1, Weekday::Fri, -1),
            Some(NaiveDate::from_ymd_opt(2026, 1, 30).unwrap())
        );
    }

    #[test]
    fn business_day_computation() {
        let cal = HolidayCalendar {
            country_code: "PT".into(),
            fixed: vec![(1, 1)],
            easter_offsets: vec![],
            nth_weekday_rules: vec![],
            home_municipal: None,
            work_municipal: None,
        };

        // Jan 1 2026 is a Thursday but a holiday
        let jan1 = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert!(!cal.is_business_day(jan1));

        // Jan 2 2026 is a Friday, business day
        let jan2 = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
        assert!(cal.is_business_day(jan2));

        // Next business day after Jan 1 = Jan 2
        assert_eq!(cal.next_business_day_after(jan1), jan2);
    }

    #[test]
    fn nth_business_day_of_month() {
        let cal = HolidayCalendar {
            country_code: "TEST".into(),
            fixed: vec![],
            easter_offsets: vec![],
            nth_weekday_rules: vec![],
            home_municipal: None,
            work_municipal: None,
        };

        // Feb 2026: 1st is Sunday, so 1st business day = Feb 2 (Monday)
        let first = cal.nth_business_day(2026, 2, 1).unwrap();
        assert_eq!(first, NaiveDate::from_ymd_opt(2026, 2, 2).unwrap());
    }

    #[test]
    fn dual_municipality_day_kind() {
        // Scenario: live in Porto, work in Lisbon
        let toml_str = r#"
[country]
name = "Portugal"
code = "PT"

[[fixed_holidays]]
name = "Dia da Liberdade"
month = 4
day = 25

[regions.porto]
name = "Porto"
[[regions.porto.holidays]]
name = "São João"
month = 6
day = 24

[regions.lisbon]
name = "Lisboa"
[[regions.lisbon.holidays]]
name = "Santo António"
month = 6
day = 13
"#;
        let cal =
            HolidayCalendar::from_toml(toml_str, Some("porto"), Some("lisbon")).unwrap();

        let sao_joao = NaiveDate::from_ymd_opt(2026, 6, 24).unwrap();
        let santo_antonio = NaiveDate::from_ymd_opt(2026, 6, 13).unwrap(); // Saturday in 2026
        let liberation = NaiveDate::from_ymd_opt(2026, 4, 25).unwrap(); // Saturday in 2026
        let normal_day = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(); // Monday

        // São João: Porto holiday → kids home, but Lisbon office open
        assert_eq!(cal.day_kind(sao_joao), DayKind::HomeHoliday);
        // You still have to work (it's a business day for your work municipality)
        assert!(cal.is_business_day(sao_joao));

        // Santo António 2026 falls on a Saturday, so it's already a weekend
        assert_eq!(cal.day_kind(santo_antonio), DayKind::FullHoliday);

        // National holiday → full holiday regardless of municipality
        assert_eq!(cal.day_kind(liberation), DayKind::FullHoliday);

        // Regular Monday → normal
        assert_eq!(cal.day_kind(normal_day), DayKind::Normal);

        // Both are considered holidays for is_holiday() (any municipal match)
        assert!(cal.is_holiday(sao_joao));
        assert!(cal.is_holiday(santo_antonio));

        // Test the same-municipality scenario (live and work in Porto)
        let cal_porto =
            HolidayCalendar::from_toml(toml_str, Some("porto"), Some("porto")).unwrap();
        assert_eq!(cal_porto.day_kind(sao_joao), DayKind::FullHoliday);
        assert!(!cal_porto.is_business_day(sao_joao));
    }
}
