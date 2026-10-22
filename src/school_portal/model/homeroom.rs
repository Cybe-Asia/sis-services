//! The homeroom summary of one class (ADR 0015 layer B): attendance counts over a date range and
//! the official grades of one term, per enrolled Student. Reads only.
use chrono::NaiveDate;
use serde::Serialize;

/// Longest range one read covers; a semester is about 120 school days.
pub const MAX_RANGE_DAYS: i64 = 200;

/// A valid `from..=to` within the bound, or `None`.
pub fn range(from: &str, to: &str) -> Option<(NaiveDate, NaiveDate)> {
    let parse = |v: &str| {
        NaiveDate::parse_from_str(v, "%Y-%m-%d")
            .ok()
            .filter(|d| d.to_string() == v)
    };
    let (from, to) = (parse(from)?, parse(to)?);
    let days = (to - from).num_days();
    (0..MAX_RANGE_DAYS).contains(&days).then_some((from, to))
}
/// A term label as SIS stores it: free text, trimmed, short.
pub fn term(value: &str) -> bool {
    !value.trim().is_empty() && value.trim() == value && value.len() <= 64
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendance {
    pub present: u32,
    pub late: u32,
    pub absent: u32,
    pub excused: u32,
    /// Days with any record, including a status this read does not name.
    pub recorded_days: u32,
}
impl Attendance {
    /// One record per day and Student (the record key includes the date).
    pub fn tally<'a>(statuses: impl IntoIterator<Item = &'a str>) -> Self {
        let mut a = Self::default();
        for status in statuses {
            a.recorded_days += 1;
            match status {
                "present" => a.present += 1,
                "late" => a.late += 1,
                "absent" => a.absent += 1,
                "excused" => a.excused += 1,
                _ => {}
            }
        }
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_range_is_ordered_exact_and_bounded() {
        assert!(range("2026-07-14", "2026-10-22").is_some());
        assert!(range("2026-10-22", "2026-10-22").is_some(), "one day");
        assert!(range("2026-10-22", "2026-10-21").is_none(), "reversed");
        assert!(
            range("2026-01-01", "2026-07-20").is_none(),
            "200 days or more"
        );
        assert!(range("2026-01-01", "2026-07-19").is_some(), "199 days");
        for bad in [
            "2026-7-14",
            "2026-02-30",
            "14-07-2026",
            "",
            "2026-07-14T00:00",
        ] {
            assert!(range(bad, "2026-10-22").is_none(), "{bad}");
        }
    }
    #[test]
    fn a_term_is_trimmed_short_text() {
        assert!(term("Semester 1"));
        for bad in ["", " ", " Semester 1", "Semester 1 ", &"x".repeat(65)] {
            assert!(!term(bad), "{bad:?}");
        }
    }
    #[test]
    fn attendance_counts_each_status_and_every_recorded_day() {
        let a = Attendance::tally(["present", "late", "present", "excused", "absent", "unknown"]);
        assert_eq!(
            a,
            Attendance {
                present: 2,
                late: 1,
                absent: 1,
                excused: 1,
                recorded_days: 6,
            }
        );
        assert_eq!(
            serde_json::to_value(&a).unwrap()["recordedDays"],
            serde_json::json!(6)
        );
    }
}
