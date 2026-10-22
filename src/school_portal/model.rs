use chrono::{Days, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
pub mod growth;
pub mod homeroom;

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
        /// Optional term breakdown the Parent sees under Capaian Akademik. Left out, the
        /// payload is byte-for-byte the one earlier releases stored.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<GradeDetail>,
    },
    /// A class announcement on the Parent School Board. `note` is the "Note for Parents" (may be
    /// empty). An important announcement stays on top until the Parent reads it or, with
    /// `approval`, answers it for the child; the due date and time are on the school clock.
    Announcement {
        id: String,
        title: String,
        body: String,
        note: String,
        priority: String,
        approval: bool,
        due_date: Option<String>,
        due_time: Option<String>,
    },
}
impl PublishedRecord {
    pub fn id(&self) -> &str {
        match self {
            Self::Growth { id, .. }
            | Self::Event { id, .. }
            | Self::Project { id, .. }
            | Self::Report { id, .. }
            | Self::Grade { id, .. }
            | Self::Announcement { id, .. } => id,
        }
    }
    pub fn student(&self) -> Option<&str> {
        match self {
            Self::Event { .. } | Self::Announcement { .. } => None,
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
            Self::Announcement { .. } => "announcement",
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
                detail,
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
                    && detail.as_ref().is_none_or(|d| d.valid(*maximum))
            }
            Self::Announcement {
                title,
                body,
                note,
                priority,
                approval,
                due_date,
                due_time,
                ..
            } => {
                text(title, 256)
                    && text(body, 4000)
                    && note.len() <= 2000
                    && ["important", "general"].contains(&priority.as_str())
                    && (!approval || priority == "important")
                    && due_date.as_ref().is_none_or(|d| date(d))
                    && time(due_time)
                    && !(due_time.is_some() && due_date.is_none())
            }
        }
    }
}
/// The parts of a term grade a school publishes beside the final score: the assessment
/// components (Tugas, PTS, PAS), the subject group (rumpun) the subject belongs to, the
/// learning outcome (Capaian Pembelajaran) and the teacher's top-performer mark.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GradeDetail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midterm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_exam: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_group_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learning_outcome: Option<String>,
    #[serde(default)]
    pub top_performer: bool,
}
impl GradeDetail {
    /// Components use the grade's own scale; a group summary needs its group.
    pub fn valid(&self, maximum: f64) -> bool {
        [self.assignment, self.midterm, self.final_exam]
            .into_iter()
            .flatten()
            .all(|v| v.is_finite() && v >= 0.0 && v <= maximum)
            && self.subject_group.as_ref().is_none_or(|v| text(v, 128))
            && self
                .subject_group_summary
                .as_ref()
                .is_none_or(|v| text(v, 256) && self.subject_group.is_some())
            && self.learning_outcome.as_ref().is_none_or(|v| text(v, 2000))
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
/// The three notification switches of a Parent. When sent, all three are required.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationCategories {
    pub class_schedule: bool,
    pub announcements: bool,
    pub messages: bool,
}
impl NotificationCategories {
    pub fn all(value: bool) -> Self {
        Self {
            class_schedule: value,
            announcements: value,
            messages: value,
        }
    }
    pub fn any(&self) -> bool {
        self.class_schedule || self.announcements || self.messages
    }
    /// The categories to report for what is stored on the User. They count only when all three
    /// were saved and still agree with the single switch; otherwise the single switch is the
    /// newer truth (never saved, or last saved by a release that predates the categories) and
    /// each category equals it.
    pub fn stored(notifications: bool, saved: [Option<bool>; 3]) -> Self {
        match saved {
            [Some(class_schedule), Some(announcements), Some(messages)] => {
                let categories = Self {
                    class_schedule,
                    announcements,
                    messages,
                };
                if categories.any() == notifications {
                    categories
                } else {
                    Self::all(notifications)
                }
            }
            _ => Self::all(notifications),
        }
    }
}
fn preference_fields(version: u32, locale: &str, theme: &str) -> bool {
    version < i32::MAX as u32
        && ["en", "id"].contains(&locale)
        && ["light", "dark", "system"].contains(&theme)
}
/// The six notification switches of a Student (Profile, Account & Preferences). When sent, all
/// six are required.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StudentNotificationCategories {
    pub assignments: bool,
    pub grades: bool,
    pub class_schedule: bool,
    pub announcements: bool,
    pub messages: bool,
    pub learning_reminders: bool,
}
impl StudentNotificationCategories {
    pub const STORED: [&'static str; 6] = [
        "assignments",
        "grades",
        "class_schedule",
        "announcements",
        "messages",
        "learning_reminders",
    ];
    pub fn all(value: bool) -> Self {
        Self::from([value; 6])
    }
    pub fn values(&self) -> [bool; 6] {
        [
            self.assignments,
            self.grades,
            self.class_schedule,
            self.announcements,
            self.messages,
            self.learning_reminders,
        ]
    }
    pub fn any(&self) -> bool {
        self.values().contains(&true)
    }
    /// Same rule as the Parent categories: the saved six count only when all were saved and still
    /// agree with the single switch; otherwise each category equals the single switch.
    pub fn stored(notifications: bool, saved: [Option<bool>; 6]) -> Self {
        if saved.iter().all(Option::is_some) {
            let categories = Self::from(saved.map(|v| v.unwrap_or(notifications)));
            if categories.any() == notifications {
                return categories;
            }
        }
        Self::all(notifications)
    }
}
impl From<[bool; 6]> for StudentNotificationCategories {
    fn from(
        [assignments, grades, class_schedule, announcements, messages, learning_reminders]: [bool;
            6],
    ) -> Self {
        Self {
            assignments,
            grades,
            class_schedule,
            announcements,
            messages,
            learning_reminders,
        }
    }
}
/// Student preferences. The Parent body is `ParentPreferences`.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub version: u32,
    pub locale: String,
    pub theme: String,
    pub notifications: bool,
    /// Optional: a client that predates the categories sends only the single switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_categories: Option<StudentNotificationCategories>,
}
impl Preferences {
    pub fn valid(&self) -> bool {
        preference_fields(self.version, &self.locale, &self.theme)
    }
    /// What a save stores: with categories, the single switch becomes their OR; without them,
    /// each category takes the value of the single switch.
    pub fn notification_switches(&self) -> (bool, StudentNotificationCategories) {
        match self.notification_categories {
            Some(categories) => (categories.any(), categories),
            None => (
                self.notifications,
                StudentNotificationCategories::all(self.notifications),
            ),
        }
    }
}
/// Parent preferences: the same four fields plus the optional notification categories.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParentPreferences {
    pub version: u32,
    pub locale: String,
    pub theme: String,
    pub notifications: bool,
    /// Optional: a client that predates the categories sends only the single switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_categories: Option<NotificationCategories>,
}
impl ParentPreferences {
    pub fn valid(&self) -> bool {
        preference_fields(self.version, &self.locale, &self.theme)
    }
    /// What a save stores: with categories, the single switch becomes their OR; without them,
    /// each category takes the value of the single switch.
    pub fn notification_switches(&self) -> (bool, NotificationCategories) {
        match self.notification_categories {
            Some(categories) => (categories.any(), categories),
            None => (
                self.notifications,
                NotificationCategories::all(self.notifications),
            ),
        }
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
            && ["attending", "declined", "clear", "approved", "rejected"]
                .contains(&self.response.as_str())
    }
    /// "approved" and "rejected" answer an announcement that asks for approval; the other values
    /// answer an event invitation.
    pub fn approval(&self) -> bool {
        matches!(self.response.as_str(), "approved" | "rejected")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn student_notification_categories_are_optional_and_agree_with_the_switch() {
        let old: Preferences = serde_json::from_str(
            r#"{"version":1,"locale":"id","theme":"system","notifications":false}"#,
        )
        .unwrap();
        assert_eq!(
            old.notification_switches(),
            (false, StudentNotificationCategories::all(false))
        );
        let new: Preferences = serde_json::from_str(r#"{"version":1,"locale":"en","theme":"system","notifications":false,"notificationCategories":{"assignments":true,"grades":false,"classSchedule":false,"announcements":false,"messages":false,"learningReminders":false}}"#).unwrap();
        let (single, categories) = new.notification_switches();
        assert!(single && categories.assignments && !categories.grades);
        assert!(serde_json::from_str::<Preferences>(r#"{"version":1,"locale":"en","theme":"system","notifications":true,"notificationCategories":{"assignments":true}}"#).is_err());
        let saved = [
            Some(true),
            Some(false),
            Some(false),
            Some(false),
            Some(false),
            Some(false),
        ];
        assert!(StudentNotificationCategories::stored(true, saved).assignments);
        // A release without categories switched everything off after the six were saved.
        assert_eq!(
            StudentNotificationCategories::stored(false, saved),
            StudentNotificationCategories::all(false)
        );
        assert_eq!(
            StudentNotificationCategories::stored(true, [None; 6]),
            StudentNotificationCategories::all(true)
        );
        assert_eq!(
            serde_json::to_value(StudentNotificationCategories::all(true)).unwrap(),
            serde_json::json!({"assignments":true,"grades":true,"classSchedule":true,"announcements":true,"messages":true,"learningReminders":true})
        );
    }
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
    #[test]
    fn grade_detail_is_optional_and_bounded() {
        let plain = r#"{"kind":"grade","id":"g","student_id":"child","subject":"Matematika","term":"Semester Ganjil 2026/2027","score":94.0,"maximum":100.0}"#;
        let record: PublishedRecord = serde_json::from_str(plain).unwrap();
        assert!(record.valid());
        // A grade without detail is stored exactly as before.
        assert_eq!(
            serde_json::to_value(&record).unwrap(),
            serde_json::from_str::<serde_json::Value>(plain).unwrap()
        );
        let with = |detail: &str| {
            serde_json::from_str::<PublishedRecord>(&plain.replace(
                r#""maximum":100.0}"#,
                &format!(r#""maximum":100.0,"detail":{detail}}}"#),
            ))
        };
        let full = with(
            r#"{"assignment":94,"midterm":90,"final_exam":92,"subject_group":"Rumpun STEM & Inovasi Digital","subject_group_summary":"Fondasi logika kuantitatif","learning_outcome":"Sangat mandiri.","top_performer":true}"#,
        )
        .unwrap();
        assert!(full.valid());
        let PublishedRecord::Grade {
            detail: Some(detail),
            ..
        } = &full
        else {
            panic!("detail")
        };
        assert!(detail.top_performer && detail.final_exam == Some(92.0));
        // Each part is optional; top_performer defaults to false.
        let partial = with(r#"{"midterm":80}"#).unwrap();
        assert!(partial.valid());
        assert_eq!(
            serde_json::to_value(&partial).unwrap()["detail"],
            serde_json::json!({"midterm":80.0,"top_performer":false})
        );
        // Components stay on the grade's scale, texts are bounded, a summary needs its group.
        assert!(!with(r#"{"assignment":101}"#).unwrap().valid());
        assert!(!with(r#"{"midterm":-1}"#).unwrap().valid());
        assert!(!with(r#"{"subject_group":" "}"#).unwrap().valid());
        assert!(!with(r#"{"subject_group_summary":"Fondasi"}"#)
            .unwrap()
            .valid());
        assert!(
            !with(&format!(r#"{{"learning_outcome":"{}"}}"#, "a".repeat(2001)))
                .unwrap()
                .valid()
        );
        assert!(with(r#"{"rank":1}"#).is_err());
        assert!(with(r#"{"top_performer":"yes"}"#).is_err());
    }
    #[test]
    fn validates_announcements() {
        let announcement =
            |priority: &str, approval, due_date: Option<&str>, due_time: Option<&str>| {
                PublishedRecord::Announcement {
                    id: "notice".into(),
                    title: "Museum visit".into(),
                    body: "Year 7 visits the science museum.".into(),
                    note: String::new(),
                    priority: priority.into(),
                    approval,
                    due_date: due_date.map(Into::into),
                    due_time: due_time.map(Into::into),
                }
            };
        let today = school_today().to_string();
        assert!(announcement("general", false, None, None).valid());
        assert!(announcement("important", true, Some(&today), Some("17:00")).valid());
        assert!(announcement("important", false, Some(&today), None).valid());
        // Approval is an action on an important announcement; a time needs its date.
        assert!(!announcement("general", true, None, None).valid());
        assert!(!announcement("important", true, None, Some("17:00")).valid());
        assert!(!announcement("urgent", false, None, None).valid());
        assert!(!announcement("important", true, Some("2026-02-30"), None).valid());
        let record: PublishedRecord = serde_json::from_str(
            r#"{"kind":"announcement","id":"a","title":"T","body":"B","note":"","priority":"general","approval":false,"due_date":null,"due_time":null}"#,
        )
        .unwrap();
        assert_eq!(record.kind(), "announcement");
        assert!(record.student().is_none());
        assert!(serde_json::from_str::<PublishedRecord>(
            r#"{"kind":"announcement","id":"a","title":"T","body":"B","note":"","priority":"general","approval":false,"student_id":"child"}"#
        )
        .is_err());
        let reply = |response: &str| Rsvp {
            student_id: "child".into(),
            response: response.into(),
        };
        assert!(reply("approved").valid() && reply("approved").approval());
        assert!(reply("attending").valid() && !reply("attending").approval());
        assert!(!reply("maybe").valid());
    }
    fn preferences(body: &str) -> Result<ParentPreferences, serde_json::Error> {
        serde_json::from_str(body)
    }
    const LIVE_CLIENT: &str = r#"{"version":3,"locale":"id","theme":"dark","notifications":false}"#;
    #[test]
    fn preferences_without_categories_behave_as_before() {
        // The body the live client sends: four fields, no categories.
        let off = preferences(LIVE_CLIENT).unwrap();
        assert!(off.valid() && off.notification_categories.is_none());
        assert_eq!(
            off.notification_switches(),
            (false, NotificationCategories::all(false))
        );
        let on = preferences(&LIVE_CLIENT.replace("false", "true")).unwrap();
        assert_eq!(
            on.notification_switches(),
            (true, NotificationCategories::all(true))
        );
        // An explicit null is the same as leaving the object out.
        let null = preferences(
            r#"{"version":0,"locale":"en","theme":"system","notifications":true,"notificationCategories":null}"#,
        )
        .unwrap();
        assert!(null.notification_categories.is_none());
        // A value that came from such a client serialises exactly as it arrived.
        assert_eq!(
            serde_json::to_value(&off).unwrap(),
            serde_json::from_str::<serde_json::Value>(LIVE_CLIENT).unwrap()
        );
        // The single switch stays required.
        assert!(preferences(r#"{"version":0,"locale":"en","theme":"system"}"#).is_err());
    }
    #[test]
    fn preferences_with_categories_store_them_and_their_or() {
        let with = |notifications: bool, categories: &str| {
            preferences(&format!(
                r#"{{"version":0,"locale":"en","theme":"system","notifications":{notifications},"notificationCategories":{categories}}}"#
            ))
        };
        let one = with(
            false,
            r#"{"classSchedule":false,"announcements":true,"messages":false}"#,
        )
        .unwrap();
        assert!(one.valid());
        // The single switch is derived from the categories, whatever the client sent for it.
        assert_eq!(
            one.notification_switches(),
            (
                true,
                NotificationCategories {
                    class_schedule: false,
                    announcements: true,
                    messages: false
                }
            )
        );
        let none = with(
            true,
            r#"{"classSchedule":false,"announcements":false,"messages":false}"#,
        )
        .unwrap();
        assert_eq!(
            none.notification_switches(),
            (false, NotificationCategories::all(false))
        );
        let all = with(
            false,
            r#"{"classSchedule":true,"announcements":true,"messages":true}"#,
        )
        .unwrap();
        assert_eq!(
            all.notification_switches(),
            (true, NotificationCategories::all(true))
        );
        // All three are required, typed, and nothing else is accepted.
        for rejected in [
            r#"{}"#,
            r#"{"classSchedule":true,"announcements":true}"#,
            r#"{"classSchedule":true,"announcements":true,"messages":"yes"}"#,
            r#"{"classSchedule":true,"announcements":true,"messages":1}"#,
            r#"{"classSchedule":true,"announcements":true,"messages":true,"marketing":true}"#,
            r#"{"class_schedule":true,"announcements":true,"messages":true}"#,
            r#"true"#,
            r#"[true,true]"#,
            r#""all""#,
        ] {
            assert!(with(true, rejected).is_err(), "{rejected}");
        }
        // Unknown top-level fields are still rejected.
        assert!(preferences(
            r#"{"version":0,"locale":"en","theme":"system","notifications":true,"userId":"other"}"#
        )
        .is_err());
        // Locale, theme and version limits are the ones the four-field body always had.
        let mut limits = all.clone();
        limits.locale = "fr".into();
        assert!(!limits.valid());
        limits.locale = "en".into();
        limits.theme = "sepia".into();
        assert!(!limits.valid());
        limits.theme = "light".into();
        limits.version = i32::MAX as u32;
        assert!(!limits.valid());
    }
    #[test]
    fn student_preferences_do_not_accept_parent_categories() {
        assert!(serde_json::from_str::<Preferences>(LIVE_CLIENT).is_ok_and(|v| v.valid()));
        assert!(serde_json::from_str::<Preferences>(
            r#"{"version":0,"locale":"en","theme":"system","notifications":true,"notificationCategories":{"classSchedule":true,"announcements":true,"messages":true}}"#
        )
        .is_err());
    }
    #[test]
    fn snapshot_categories_default_to_the_single_switch() {
        let never = [None, None, None];
        // Never saved: each category equals the single switch (which itself defaults to on).
        assert_eq!(
            NotificationCategories::stored(true, never),
            NotificationCategories::all(true)
        );
        assert_eq!(
            NotificationCategories::stored(false, never),
            NotificationCategories::all(false)
        );
        // Saved categories are reported as saved.
        let saved = [Some(true), Some(false), Some(false)];
        assert_eq!(
            NotificationCategories::stored(true, saved),
            NotificationCategories {
                class_schedule: true,
                announcements: false,
                messages: false
            }
        );
        assert_eq!(
            NotificationCategories::stored(false, [Some(false); 3]),
            NotificationCategories::all(false)
        );
        // A release without categories changed the single switch afterwards: it wins.
        assert_eq!(
            NotificationCategories::stored(false, saved),
            NotificationCategories::all(false)
        );
        assert_eq!(
            NotificationCategories::stored(true, [Some(false); 3]),
            NotificationCategories::all(true)
        );
        // A partial set is not a saved choice.
        assert_eq!(
            NotificationCategories::stored(true, [Some(false), None, Some(false)]),
            NotificationCategories::all(true)
        );
        // The snapshot shape the client reads.
        assert_eq!(
            serde_json::to_value(NotificationCategories::stored(true, saved)).unwrap(),
            serde_json::json!({"classSchedule":true,"announcements":false,"messages":false})
        );
    }
}
