use crate::school_portal::model::identifier;
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Meeting {
    pub id: String,
    pub school_id: String,
    pub tenant_id: String,
    pub class_id: String,
    pub teacher_id: String,
    pub course_id: String,
    pub subject: String,
    pub title: String,
    pub starts_at: u64,
    pub ends_at: u64,
    pub academic_year: String,
    pub room_id: String,
}
impl Meeting {
    pub fn valid(&self) -> bool {
        [
            &self.id,
            &self.school_id,
            &self.tenant_id,
            &self.class_id,
            &self.teacher_id,
            &self.course_id,
            &self.subject,
            &self.room_id,
        ]
        .iter()
        .all(|s| identifier(s))
            && !self.academic_year.trim().is_empty()
            && self.academic_year.len() <= 32
            && !self.academic_year.chars().any(char::is_control)
            && !self.academic_year.contains('|')
            && !self.title.trim().is_empty()
            && self.title.len() <= 200
            && self.starts_at > 0
            && self.starts_at < self.ends_at
            && self.ends_at <= 4_102_444_800_000
            && self.ends_at - self.starts_at <= 12 * 3_600_000
    }
    pub fn dates(&self) -> Option<(NaiveDate, NaiveDate)> {
        let local = |ms| {
            chrono::DateTime::from_timestamp_millis(ms as i64 + 25_200_000).map(|t| t.date_naive())
        };
        Some((local(self.starts_at)?, local(self.ends_at.checked_sub(1)?)?))
    }
    pub fn time_issues(&self, now: u64) -> Vec<&'static str> {
        let mut issues = vec![];
        if self.starts_at <= now {
            issues.push("PAST_MEETING");
        }
        match self.dates() {
            Some((start, end)) => {
                if start != end {
                    issues.push("CROSSES_SCHOOL_DAY");
                }
                if start.weekday().number_from_monday() > 5
                    || end.weekday().number_from_monday() > 5
                {
                    issues.push("NON_SCHOOL_DAY");
                }
            }
            None => issues.push("INVALID_TIME"),
        }
        issues
    }
    pub fn key(&self) -> String {
        format!(
            "{}:{}{}:{}{}:{}{}:{}",
            self.school_id.len(),
            self.school_id,
            self.tenant_id.len(),
            self.tenant_id,
            self.class_id.len(),
            self.class_id,
            self.id.len(),
            self.id
        )
    }
    pub fn school_key(&self) -> String {
        format!("{}|{}", self.tenant_id, self.school_id)
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    SaveDraft,
    Validate,
    Publish,
    Archive,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub meeting: Meeting,
    pub expected_version: u32,
    pub expected_published_version: u32,
    pub action: Action,
    pub operation_id: String,
}
impl Change {
    pub fn valid(&self) -> bool {
        self.meeting.valid()
            && self.expected_version < i32::MAX as u32
            && self.expected_published_version < i32::MAX as u32
            && uuid::Uuid::parse_str(&self.operation_id)
                .is_ok_and(|id| id.hyphenated().to_string() == self.operation_id)
            && (self.action == Action::SaveDraft || self.expected_version > 0)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub school_id: String,
    pub tenant_id: String,
    pub class_id: String,
}
impl Scope {
    pub fn valid(&self) -> bool {
        [&self.school_id, &self.tenant_id, &self.class_id]
            .iter()
            .all(|s| identifier(s))
    }
}
#[cfg(test)]
mod tests;
