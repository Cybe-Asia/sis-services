use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    Appointment,
    Leave,
}
impl RequestKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Appointment => "appointment",
            Self::Leave => "leave",
        }
    }
}
pub const PENDING_REVIEW: &str = "pending_review";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParentRequestInput {
    pub idempotency_key: String,
    pub student_id: String,
    pub kind: RequestKind,
    pub date: String,
    pub time: Option<String>,
    pub teacher_email: Option<String>,
    pub reason: Option<String>,
    pub note: String,
}
impl ParentRequestInput {
    pub fn valid(&self) -> bool {
        let Ok(date) = NaiveDate::parse_from_str(&self.date, "%Y-%m-%d") else {
            return false;
        };
        let today = (chrono::Utc::now() + chrono::Duration::hours(7)).date_naive();
        if uuid::Uuid::parse_str(&self.idempotency_key).is_err()
            || self.student_id.is_empty()
            || self.student_id.len() > 128
            || self.note.len() > 2000
            || date < today
            || date > today + chrono::Duration::days(365)
        {
            return false;
        }
        match self.kind {
            RequestKind::Leave => {
                self.teacher_email.is_none()
                    && self.time.is_none()
                    && self
                        .reason
                        .as_deref()
                        .is_some_and(|r| ["sick", "other"].contains(&r))
            }
            RequestKind::Appointment => {
                self.reason.is_none()
                    && !self.note.trim().is_empty()
                    && self
                        .teacher_email
                        .as_ref()
                        .is_some_and(|e| e.len() <= 254 && !e.is_empty())
                    && self
                        .time
                        .as_ref()
                        .is_some_and(|t| chrono::NaiveTime::parse_from_str(t, "%H:%M").is_ok())
            }
        }
    }
}
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParentRequest {
    pub id: String,
    pub student_id: String,
    pub kind: String,
    pub date: String,
    pub time: Option<String>,
    pub teacher_email: Option<String>,
    pub reason: Option<String>,
    pub note: String,
    pub status: String,
    pub created_at: String,
    pub review_note: Option<String>,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_future_bounded_reports() {
        let mut input = ParentRequestInput {
            idempotency_key: uuid::Uuid::new_v4().to_string(),
            student_id: "owned".into(),
            kind: RequestKind::Leave,
            date: chrono::Utc::now().date_naive().to_string(),
            time: None,
            teacher_email: None,
            reason: Some("sick".into()),
            note: "".into(),
        };
        assert!(input.valid());
        input.date = "2000-01-01".into();
        assert!(!input.valid());
        input.date = chrono::Utc::now().date_naive().to_string();
        input.teacher_email = Some("foreign@example.test".into());
        assert!(!input.valid());
    }
}
