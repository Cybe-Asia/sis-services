use chrono::{Days, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
pub mod growth;

pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max
}
pub fn school_today() -> NaiveDate {
    (Utc::now() + chrono::Duration::hours(7)).date_naive()
}
pub fn date(value: &str) -> bool {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok_and(|d| {
        d.to_string() == value
            && d >= school_today().checked_sub_days(Days::new(365)).unwrap()
            && d <= school_today().checked_add_days(Days::new(365)).unwrap()
    })
}
fn time(value: &Option<String>) -> bool {
    value.as_ref().is_none_or(|v| {
        chrono::NaiveTime::parse_from_str(v, "%H:%M")
            .is_ok_and(|t| t.format("%H:%M").to_string() == *v)
    })
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttendanceInput {
    pub student_id: String,
    pub status: String,
    pub arrived_at: Option<String>,
    pub dismissed_at: Option<String>,
    pub note: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RollCall {
    pub date: String,
    pub entries: Vec<AttendanceInput>,
}
impl RollCall {
    pub fn valid(&self) -> bool {
        date(&self.date)
            && self.date <= school_today().to_string()
            && !self.entries.is_empty()
            && self.entries.len() <= 200
            && self.entries.iter().all(|e| {
                identifier(&e.student_id)
                    && ["present", "late", "absent", "excused"].contains(&e.status.as_str())
                    && e.note.len() <= 2000
                    && time(&e.arrived_at)
                    && time(&e.dismissed_at)
                    && (matches!(e.status.as_str(), "present" | "late")
                        || (e.arrived_at.is_none() && e.dismissed_at.is_none()))
                    && !(e.arrived_at.is_some()
                        && e.dismissed_at.is_some()
                        && e.arrived_at > e.dismissed_at)
            })
            && self
                .entries
                .iter()
                .map(|e| &e.student_id)
                .collect::<std::collections::HashSet<_>>()
                .len()
                == self.entries.len()
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublishedRecord {
    Growth {
        id: String,
        student_id: String,
        report: growth::GrowthReport,
    },
    Event {
        id: String,
        title: String,
        date: String,
        start: Option<String>,
        end: Option<String>,
        location: String,
        category: String,
        rsvp: bool,
    },
    Project {
        id: String,
        student_id: String,
        title: String,
        subject: String,
        summary: String,
        state: String,
    },
    Report {
        id: String,
        student_id: String,
        term: String,
        gpa: f64,
        maximum: f64,
    },
    Grade {
        id: String,
        student_id: String,
        subject: String,
        term: String,
        score: f64,
        maximum: f64,
    },
}
impl PublishedRecord {
    pub fn id(&self) -> &str {
        match self {
            Self::Growth { id, .. }
            | Self::Event { id, .. }
            | Self::Project { id, .. }
            | Self::Report { id, .. }
            | Self::Grade { id, .. } => id,
        }
    }
    pub fn student(&self) -> Option<&str> {
        match self {
            Self::Event { .. } => None,
            Self::Growth { student_id, .. }
            | Self::Project { student_id, .. }
            | Self::Report { student_id, .. }
            | Self::Grade { student_id, .. } => Some(student_id),
        }
    }
    pub fn kind(&self) -> &str {
        match self {
            Self::Growth { .. } => "growth",
            Self::Event { .. } => "event",
            Self::Project { .. } => "project",
            Self::Report { .. } => "report",
            Self::Grade { .. } => "grade",
        }
    }
    pub fn valid(&self) -> bool {
        if !identifier(self.id()) || self.student().is_some_and(|v| !identifier(v)) {
            return false;
        }
        match self {
            Self::Growth { report, .. } => report.valid(),
            Self::Event {
                title,
                date: d,
                start,
                end,
                location,
                category,
                ..
            } => {
                text(title, 256)
                    && date(d)
                    && time(start)
                    && time(end)
                    && !(start.is_some() && end.is_some() && start > end)
                    && location.len() <= 256
                    && ["meeting", "holiday", "exam", "lesson", "school"]
                        .contains(&category.as_str())
            }
            Self::Project {
                title,
                subject,
                summary,
                state,
                ..
            } => {
                text(title, 256)
                    && text(subject, 128)
                    && text(summary, 4000)
                    && ["planned", "in_progress", "completed"].contains(&state.as_str())
            }
            Self::Report {
                term, gpa, maximum, ..
            } => {
                text(term, 128)
                    && gpa.is_finite()
                    && maximum.is_finite()
                    && *maximum > 0.0
                    && *maximum <= 100.0
                    && *gpa >= 0.0
                    && gpa <= maximum
            }
            Self::Grade {
                subject,
                term,
                score,
                maximum,
                ..
            } => {
                text(subject, 128)
                    && text(term, 128)
                    && score.is_finite()
                    && maximum.is_finite()
                    && *maximum > 0.0
                    && *maximum <= 10000.0
                    && *score >= 0.0
                    && score <= maximum
            }
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    pub expected_status: String,
    pub status: String,
    pub note: String,
}
impl Review {
    pub fn valid(&self) -> bool {
        self.expected_status == "pending_review"
            && ["approved", "declined"].contains(&self.status.as_str())
            && self.note.len() <= 2000
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub version: u32,
    pub locale: String,
    pub theme: String,
    pub notifications: bool,
}
impl Preferences {
    pub fn valid(&self) -> bool {
        self.version < i32::MAX as u32
            && ["en", "id"].contains(&self.locale.as_str())
            && ["light", "dark", "system"].contains(&self.theme.as_str())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rsvp {
    pub student_id: String,
    pub response: String,
}
impl Rsvp {
    pub fn valid(&self) -> bool {
        identifier(&self.student_id)
            && ["attending", "declined", "clear"].contains(&self.response.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_school_records() {
        let mut call = RollCall {
            date: school_today().to_string(),
            entries: vec![AttendanceInput {
                student_id: "child-1".into(),
                status: "present".into(),
                arrived_at: Some("07:00".into()),
                dismissed_at: Some("14:00".into()),
                note: String::new(),
            }],
        };
        assert!(call.valid());
        call.entries[0].status = "absent".into();
        assert!(!call.valid());
        call.entries[0].status = "present".into();
        call.entries[0].dismissed_at = Some("06:00".into());
        assert!(!call.valid());
        call.entries[0].dismissed_at = None;
        call.entries.push(call.entries[0].clone());
        assert!(!call.valid());
        assert!(!PublishedRecord::Report {
            id: "report".into(),
            student_id: "child".into(),
            term: "Term1".into(),
            gpa: 5.0,
            maximum: 4.0
        }
        .valid());
        assert!(!date("2026-02-30"));
        assert!(!identifier("../other"));
    }
}
