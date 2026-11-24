//! A 6–7 field crontab (`秒 分 时 日 月 星期 [年]`), parsed and evaluated in the server's local
//! timezone.
//!
//! Written here instead of pulling in a crate because two behaviours a generic parser would not
//! give us are load-bearing for this feature: **standard numeric weekdays** (`0` and `7` both mean
//! Sunday, `1` Monday) and the Vixie rule that a restricted day-of-month and a restricted
//! day-of-week are ORed. Both are also what makes `0 0 3 * * 1` mean "Monday 03:00" to an operator
//! who knows crontab. It also lets every rejection name the offending field, which is what the
//! settings UI shows.
//!
//! Matching is done on whole local seconds. A time that does not exist (a spring-forward gap) is
//! skipped; a time that exists twice (a fall-back hour) resolves to the earlier instant.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Datelike, Local, LocalResult, TimeZone, Timelike};

/// The default expression: every Monday at 03:00:00, local time. One full backup a week is what a
/// production-sized library wants unless the operator asks for more.
pub const DEFAULT_CRON: &str = "0 0 3 * * 1";

/// How far ahead the day walk will look. The widest legal expression is a single leap day in a
/// specific year (`0 0 0 29 2 * 2100`), so the bound is the year field's own ceiling.
const LAST_YEAR: i32 = 2100;

/// A parsed crontab expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    seconds: BTreeSet<u32>,
    minutes: BTreeSet<u32>,
    hours: BTreeSet<u32>,
    days_of_month: BTreeSet<u32>,
    months: BTreeSet<u32>,
    /// 0 = Sunday … 6 = Saturday. `7` is normalized to `0` on parse.
    days_of_week: BTreeSet<u32>,
    years: BTreeSet<i32>,
    /// Whether the field was written as `*` / `?`. Vixie cron ANDs the two day fields when either
    /// one is a star and ORs them when both are restricted.
    dom_any: bool,
    dow_any: bool,
}

/// A rejection that names the field it came from, so the settings UI can say which segment is bad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronError {
    /// `expression` for whole-expression problems, otherwise the segment's own name
    /// (`seconds`, `minutes`, …).
    pub field: &'static str,
    pub message: String,
}

impl fmt::Display for CronError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.field == "expression" {
            write!(f, "{}", self.message)
        } else {
            write!(
                f,
                "{} (segment {}): {}",
                self.field,
                self.index(),
                self.message
            )
        }
    }
}

impl CronError {
    /// 1-based position of the named field in the expression.
    fn index(&self) -> usize {
        FIELDS
            .iter()
            .position(|spec| spec.name == self.field)
            .map(|index| index + 1)
            .unwrap_or(0)
    }
}

impl std::error::Error for CronError {}

impl CronSchedule {
    /// The first matching instant strictly after `after`, or `None` when the expression can never
    /// match again (a year in the past, a single impossible day).
    pub fn next_after(&self, after: DateTime<Local>) -> Option<DateTime<Local>> {
        let mut date = after.date_naive();
        while date.year() <= LAST_YEAR {
            if self.months.contains(&date.month())
                && self.years.contains(&date.year())
                && self.day_matches(date)
            {
                for hour in &self.hours {
                    for minute in &self.minutes {
                        for second in &self.seconds {
                            // Cheap skip of everything below the cursor on its own day, so a
                            // `* * * * * *` schedule does not build 86,400 instants to find the
                            // next second.
                            if date == after.date_naive() {
                                let before = (*hour, *minute, *second)
                                    <= (after.hour(), after.minute(), after.second());
                                if before {
                                    continue;
                                }
                            }
                            match Local.with_ymd_and_hms(
                                date.year(),
                                date.month(),
                                date.day(),
                                *hour,
                                *minute,
                                *second,
                            ) {
                                LocalResult::Single(at) => return Some(at),
                                // A doubled hour resolves to the earlier instant, matching the
                                // rule the pre-cron scheduler used so a restart cannot see the
                                // same slot twice.
                                LocalResult::Ambiguous(earlier, _) => {
                                    if earlier > after {
                                        return Some(earlier);
                                    }
                                }
                                // A skipped hour: this candidate does not exist today.
                                LocalResult::None => {}
                            }
                        }
                    }
                }
            }
            date = date.succ_opt()?;
        }
        None
    }

    /// The next `count` matching instants, strictly increasing.
    pub fn next_runs(&self, from: DateTime<Local>, count: usize) -> Vec<DateTime<Local>> {
        let mut runs = Vec::with_capacity(count);
        let mut cursor = from;
        while runs.len() < count {
            let Some(at) = self.next_after(cursor) else {
                break;
            };
            runs.push(at);
            cursor = at;
        }
        runs
    }

    fn day_matches(&self, date: chrono::NaiveDate) -> bool {
        let dom_ok = self.days_of_month.contains(&date.day());
        let dow_ok = self
            .days_of_week
            .contains(&date.weekday().num_days_from_sunday());
        if self.dom_any || self.dow_any {
            dom_ok && dow_ok
        } else {
            dom_ok || dow_ok
        }
    }
}

impl Default for CronSchedule {
    fn default() -> Self {
        DEFAULT_CRON
            .parse()
            .expect("the built-in default expression always parses")
    }
}

impl FromStr for CronSchedule {
    type Err = CronError;

    fn from_str(expression: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = expression.split_whitespace().collect();
        if parts.len() < 6 || parts.len() > 7 {
            return Err(CronError {
                field: "expression",
                message: format!(
                    "expected 6 or 7 fields (seconds minutes hours day-of-month month weekday \
                     [year]), got {}: {expression:?}",
                    parts.len()
                ),
            });
        }

        let mut parsed = Vec::with_capacity(parts.len());
        for (index, raw) in parts.iter().enumerate() {
            parsed.push(parse_field(raw, &FIELDS[index])?);
        }
        let (seconds, _) = parsed[0].clone();
        let (minutes, _) = parsed[1].clone();
        let (hours, _) = parsed[2].clone();
        let (days_of_month, dom_any) = parsed[3].clone();
        let (months, _) = parsed[4].clone();
        let (days_of_week, dow_any) = parsed[5].clone();
        let years = match parsed.get(6) {
            Some((years, _)) => years.iter().map(|year| *year as i32).collect(),
            None => (1970..=LAST_YEAR).collect(),
        };

        Ok(Self {
            seconds,
            minutes,
            hours,
            days_of_month,
            months,
            days_of_week,
            years,
            dom_any,
            dow_any,
        })
    }
}

struct FieldSpec {
    name: &'static str,
    min: u32,
    max: u32,
    /// Case-insensitive names accepted in this field (`JAN`/`MON`, …).
    names: &'static [(&'static str, u32)],
    /// The weekday field accepts the standard `0` and `7` as two spellings of Sunday.
    sunday_is_seven: bool,
}

const MONTH_NAMES: &[(&str, u32)] = &[
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];

const DOW_NAMES: &[(&str, u32)] = &[
    ("sun", 0),
    ("mon", 1),
    ("tue", 2),
    ("wed", 3),
    ("thu", 4),
    ("fri", 5),
    ("sat", 6),
];

const FIELDS: [FieldSpec; 7] = [
    FieldSpec {
        name: "seconds",
        min: 0,
        max: 59,
        names: &[],
        sunday_is_seven: false,
    },
    FieldSpec {
        name: "minutes",
        min: 0,
        max: 59,
        names: &[],
        sunday_is_seven: false,
    },
    FieldSpec {
        name: "hours",
        min: 0,
        max: 23,
        names: &[],
        sunday_is_seven: false,
    },
    FieldSpec {
        name: "days-of-month",
        min: 1,
        max: 31,
        names: &[],
        sunday_is_seven: false,
    },
    FieldSpec {
        name: "months",
        min: 1,
        max: 12,
        names: MONTH_NAMES,
        sunday_is_seven: false,
    },
    FieldSpec {
        name: "weekdays",
        min: 0,
        max: 7,
        names: DOW_NAMES,
        sunday_is_seven: true,
    },
    FieldSpec {
        name: "years",
        min: 1970,
        max: LAST_YEAR as u32,
        names: &[],
        sunday_is_seven: false,
    },
];

fn parse_field(raw: &str, spec: &FieldSpec) -> Result<(BTreeSet<u32>, bool), CronError> {
    let fail = |message: String| CronError {
        field: spec.name,
        message,
    };
    let mut values = BTreeSet::new();
    let mut any = false;

    for item in raw.split(',') {
        let item = item.trim();
        if item.is_empty() {
            return Err(fail(format!("empty item (consecutive commas in {raw:?})")));
        }
        let (base, step) = match item.split_once('/') {
            Some((base, step)) => {
                let step: u32 = step
                    .parse()
                    .map_err(|_| fail(format!("step in \"{item}\" is not a number")))?;
                if step == 0 {
                    return Err(fail(format!("step in \"{item}\" must be at least 1")));
                }
                (base, Some(step))
            }
            None => (item, None),
        };

        let (start, end) = if base == "*" || base == "?" {
            any |= step.is_none();
            (spec.min, spec.max)
        } else if let Some((start, end)) = base.split_once('-') {
            (point(start, spec, &fail)?, point(end, spec, &fail)?)
        } else {
            let value = point(base, spec, &fail)?;
            // `5/10` means "from 5, every 10" up to the field's own maximum, as in Vixie cron.
            match step {
                Some(_) => (value, spec.max),
                None => (value, value),
            }
        };
        if start > end {
            return Err(fail(format!("\"{item}\" starts after it ends")));
        }

        let step = step.unwrap_or(1);
        let mut value = start;
        while value <= end {
            values.insert(if spec.sunday_is_seven && value == 7 {
                0
            } else {
                value
            });
            value += step;
        }
    }

    Ok((values, any))
}

fn point(
    raw: &str,
    spec: &FieldSpec,
    fail: &impl Fn(String) -> CronError,
) -> Result<u32, CronError> {
    if let Ok(value) = raw.parse::<u32>() {
        if value < spec.min || value > spec.max {
            return Err(fail(format!(
                "\"{raw}\" is outside {}-{}",
                spec.min, spec.max
            )));
        }
        return Ok(value);
    }
    let lowered = raw.to_ascii_lowercase();
    for (name, value) in spec.names {
        // Accept both the three-letter form and the full English word (`sep` / `september`).
        if lowered == *name || lowered.starts_with(name) && lowered.len() > name.len() {
            if *value < spec.min || *value > spec.max {
                break;
            }
            return Ok(*value);
        }
    }
    Err(fail(format!(
        "\"{raw}\" is not a number and not a name this field accepts"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(date: &str, hour: u32, minute: u32, second: u32) -> DateTime<Local> {
        let date =
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").expect("test date is well formed");
        Local
            .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, second)
            .single()
            .expect("test instant exists in this timezone")
    }

    fn schedule(expression: &str) -> CronSchedule {
        expression.parse().expect("test expression parses")
    }

    #[test]
    fn parses_the_default_expression() {
        let parsed = schedule(DEFAULT_CRON);
        // 2026-11-17 is a Tuesday, so the next Monday 03:00:00 is the 23rd.
        assert_eq!(
            parsed.next_after(local("2026-11-17", 12, 0, 0)),
            Some(local("2026-11-23", 3, 0, 0))
        );
    }

    #[test]
    fn seconds_granularity_matches_every_ten_seconds() {
        let parsed = schedule("*/10 * * * * *");
        assert_eq!(
            parsed.next_after(local("2026-11-17", 12, 0, 3)),
            Some(local("2026-11-17", 12, 0, 10))
        );
        assert_eq!(
            parsed.next_after(local("2026-11-17", 12, 0, 10)),
            Some(local("2026-11-17", 12, 0, 20)),
            "strictly after, so a served second is never served twice"
        );
        assert_eq!(
            parsed.next_after(local("2026-11-17", 12, 0, 53)),
            Some(local("2026-11-17", 12, 1, 0)),
            "it crosses the minute"
        );
    }

    #[test]
    fn hourly_crosses_the_hour_and_the_day() {
        let parsed = schedule("0 0 * * * *");
        assert_eq!(
            parsed.next_after(local("2026-11-17", 12, 30, 0)),
            Some(local("2026-11-17", 13, 0, 0))
        );
        assert_eq!(
            parsed.next_after(local("2026-11-17", 23, 30, 0)),
            Some(local("2026-11-18", 0, 0, 0))
        );
    }

    #[test]
    fn weekly_monday_and_monthly_first_cross_days() {
        let weekly = schedule("0 0 3 * * 1");
        assert_eq!(
            weekly.next_after(local("2026-11-17", 12, 0, 0)),
            Some(local("2026-11-23", 3, 0, 0))
        );
        let monthly = schedule("0 0 0 1 * *");
        assert_eq!(
            monthly.next_after(local("2026-11-17", 12, 0, 0)),
            Some(local("2026-12-01", 0, 0, 0))
        );
    }

    #[test]
    fn a_year_field_is_honoured() {
        let parsed = schedule("0 0 0 1 1 * 2030");
        assert_eq!(
            parsed.next_after(local("2026-11-17", 12, 0, 0)),
            Some(local("2030-01-01", 0, 0, 0))
        );
        // Nothing left after its only year.
        assert_eq!(parsed.next_after(local("2030-06-01", 0, 0, 0)), None);
    }

    #[test]
    fn a_restricted_day_of_month_and_weekday_are_ored() {
        // "the 1st of the month, or any Monday", the Vixie rule. 2026-12-01 is a Tuesday and
        // 2026-11-23 is a Monday, so from the 17th the Monday comes first.
        let parsed = schedule("0 0 0 1 * 1");
        assert_eq!(
            parsed.next_after(local("2026-11-17", 12, 0, 0)),
            Some(local("2026-11-23", 0, 0, 0))
        );
        assert_eq!(
            parsed.next_after(local("2026-11-23", 0, 0, 0)),
            Some(local("2026-11-30", 0, 0, 0))
        );
    }

    #[test]
    fn named_months_and_weekdays_parse() {
        let parsed = schedule("0 0 3 * JAN-MAR MON");
        assert_eq!(
            parsed.next_after(local("2026-11-17", 12, 0, 0)),
            Some(local("2027-01-04", 3, 0, 0)),
            "the first Monday of the next January"
        );
    }

    #[test]
    fn both_spellings_of_sunday_are_sunday() {
        let zero = schedule("0 0 3 * * 0");
        let seven = schedule("0 0 3 * * 7");
        let from = local("2026-11-17", 12, 0, 0);
        assert_eq!(zero.next_after(from), Some(local("2026-11-22", 3, 0, 0)));
        assert_eq!(zero.next_after(from), seven.next_after(from));
    }

    #[test]
    fn next_runs_is_a_strictly_increasing_sequence() {
        let runs = schedule("0 0 3 * * 1").next_runs(local("2026-11-17", 12, 0, 0), 10);
        assert_eq!(runs.len(), 10);
        assert_eq!(runs[0], local("2026-11-23", 3, 0, 0));
        for pair in runs.windows(2) {
            assert!(pair[0] < pair[1], "runs must be strictly increasing");
        }
    }

    #[test]
    fn rejects_a_wrong_segment_count() {
        let error = "0 3 * * 1".parse::<CronSchedule>().unwrap_err();
        assert_eq!(error.field, "expression");
        assert!(error.to_string().contains("6 or 7 fields"), "{error}");
    }

    #[test]
    fn names_the_offending_segment() {
        let error = "61 0 3 * * 1".parse::<CronSchedule>().unwrap_err();
        assert_eq!(error.field, "seconds");
        assert!(error.to_string().contains("seconds (segment 1)"), "{error}");
        assert!(error.to_string().contains("0-59"), "{error}");

        let error = "0 0 24 * * 1".parse::<CronSchedule>().unwrap_err();
        assert_eq!(error.field, "hours");
        assert!(error.to_string().contains("hours (segment 3)"), "{error}");

        let error = "0 0 3 * 13 *".parse::<CronSchedule>().unwrap_err();
        assert_eq!(error.field, "months");
        assert!(error.to_string().contains("1-12"), "{error}");

        let error = "0 0 3 * * 8".parse::<CronSchedule>().unwrap_err();
        assert_eq!(error.field, "weekdays");
        assert!(error.to_string().contains("0-7"), "{error}");
    }

    #[test]
    fn rejects_impossible_ranges_and_steps() {
        assert!("0 0 3 * * 5-1".parse::<CronSchedule>().is_err());
        assert!("*/0 * * * * *".parse::<CronSchedule>().is_err());
        assert!("0 0 3 * NOK *".parse::<CronSchedule>().is_err());
        assert!("0 0 3 * * 1,".parse::<CronSchedule>().is_err());
    }

    #[test]
    fn a_leap_day_is_only_reachable_in_a_leap_year() {
        let parsed = schedule("0 0 0 29 2 *");
        // 2026 and 2027 are not leap years; 2028 is.
        assert_eq!(
            parsed.next_after(local("2026-03-01", 0, 0, 0)),
            Some(local("2028-02-29", 0, 0, 0))
        );
    }
}
