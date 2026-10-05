use crate::school_portal::model::{date, identifier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub en: String,
    pub id: String,
}
impl Text {
    fn valid(&self, max: usize) -> bool {
        [&self.en, &self.id]
            .iter()
            .all(|s| !s.trim().is_empty() && s.len() <= max && !s.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t')))
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Meeting {
    pub id: String,
    pub date: String,
    pub start: String,
    pub end: String,
    pub title: Text,
    pub location: Text,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Criterion {
    pub id: String,
    pub title: Text,
    pub levels: Vec<Text>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyllabusStep {
    pub title: Text,
    pub body: Text,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Recognition {
    pub value: Text,
    pub detail: Text,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Facility {
    pub title: Text,
    pub caption: Text,
    pub capacity: u32,
    pub image: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogPresentation {
    pub division: Option<Text>,
    pub recognition: Option<Recognition>,
    pub facility: Option<Facility>,
    pub event_label: Option<Text>,
    pub coach_title: Option<Text>,
    pub credential: Option<Text>,
    pub syllabus: Vec<SyllabusStep>,
}
fn approved_asset(s: &str) -> bool {
    s.starts_with("/assets/")
        && s.len() <= 256
        && !s.contains("..")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_.-".contains(&b))
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Activity {
    pub id: String,
    pub title: Text,
    pub summary: Text,
    pub category: String,
    pub image: Option<String>,
    pub coach_id: String,
    pub section_ids: Vec<String>,
    pub capacity: u32,
    pub parent_consent: bool,
    pub published: bool,
    pub meetings: Vec<Meeting>,
    pub criteria: Vec<Criterion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<CatalogPresentation>,
}
fn unique<'a>(ids: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = BTreeSet::new();
    ids.into_iter().all(|id| identifier(id) && seen.insert(id))
}
fn time(s: &str) -> bool {
    s.is_ascii()
        && s.len() == 5
        && s.as_bytes()[2] == b':'
        && s[..2].parse::<u8>().is_ok_and(|h| h < 24)
        && s[3..].parse::<u8>().is_ok_and(|m| m < 60)
}
impl Activity {
    pub fn valid(&self) -> bool {
        identifier(&self.id)
            && self.title.valid(160)
            && self.summary.valid(4000)
            && matches!(
                self.category.as_str(),
                "athletics" | "stem" | "creative-arts" | "leadership"
            )
            && identifier(&self.coach_id)
            && self.capacity > 0
            && self.capacity <= 200
            && !self.section_ids.is_empty()
            && self.section_ids.len() <= 100
            && unique(self.section_ids.iter().map(String::as_str))
            && self.image.as_ref().is_none_or(|s| {
                s.starts_with("/assets/")
                    && s.len() <= 256
                    && !s.contains("..")
                    && !s.contains(['?', '#', '\\'])
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"/_.-".contains(&b))
            })
            && self.meetings.len() <= 100
            && unique(self.meetings.iter().map(|m| m.id.as_str()))
            && self.meetings.iter().all(|m| {
                date(&m.date)
                    && time(&m.start)
                    && time(&m.end)
                    && m.start < m.end
                    && m.title.valid(160)
                    && m.location.valid(160)
            })
            && self.criteria.len() <= 30
            && unique(self.criteria.iter().map(|c| c.id.as_str()))
            && self.criteria.iter().all(|c| {
                c.title.valid(160)
                    && c.levels.len() >= 2
                    && c.levels.len() <= 10
                    && c.levels.iter().all(|l| l.valid(160))
            })
            && self.presentation.as_ref().is_none_or(|p| {
                [&p.division, &p.event_label, &p.coach_title, &p.credential]
                    .iter()
                    .all(|t| t.as_ref().is_none_or(|t| t.valid(160)))
                    && p.recognition
                        .as_ref()
                        .is_none_or(|r| r.value.valid(160) && r.detail.valid(160))
                    && p.facility.as_ref().is_none_or(|f| {
                        f.title.valid(160)
                            && f.caption.valid(500)
                            && f.capacity > 0
                            && f.capacity <= 200
                            && f.image.as_ref().is_none_or(|s| approved_asset(s))
                    })
                    && p.syllabus.len() <= 100
                    && p.syllabus
                        .iter()
                        .all(|s| s.title.valid(160) && s.body.valid(4000))
            })
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub school_id: String,
    pub tenant_id: String,
    pub academic_year: String,
    pub student_id: Option<String>,
    pub after: Option<String>,
}
impl Scope {
    pub fn valid(&self) -> bool {
        identifier(&self.school_id)
            && identifier(&self.tenant_id)
            && !self.academic_year.is_empty()
            && self.academic_year.len() <= 32
            && !self.academic_year.contains('|')
            && self.student_id.as_ref().is_none_or(|s| identifier(s))
            && self.after.as_ref().is_none_or(|s| identifier(s))
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mark {
    pub student_id: String,
    pub status: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Skill {
    pub criterion_id: String,
    pub level: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Save {
        activity: Activity,
    },
    Archive {},
    Enroll {},
    Approve {},
    Reject {},
    Withdraw {},
    Attendance {
        #[serde(rename = "meetingId")]
        meeting_id: String,
        marks: Vec<Mark>,
    },
    Outcome {
        skills: Vec<Skill>,
        feedback: Text,
        achievements: Vec<Text>,
    },
    Release {},
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub scope: Scope,
    pub activity_id: String,
    pub revision: u32,
    pub request_id: String,
    pub command: Command,
}
impl Change {
    pub fn valid(&self) -> bool {
        self.scope.valid()
            && self.scope.after.is_none()
            && identifier(&self.activity_id)
            && identifier(&self.request_id)
            && self.revision < i32::MAX as u32
            && match &self.command {
                Command::Save { activity } => activity.id == self.activity_id && activity.valid(),
                Command::Attendance { meeting_id, marks } => {
                    identifier(meeting_id)
                        && !marks.is_empty()
                        && marks.len() <= 200
                        && unique(marks.iter().map(|m| m.student_id.as_str()))
                        && marks.iter().all(|m| {
                            matches!(m.status.as_str(), "present" | "absent" | "late" | "excused")
                        })
                }
                Command::Outcome {
                    skills,
                    feedback,
                    achievements,
                } => {
                    !skills.is_empty()
                        && skills.len() <= 30
                        && unique(skills.iter().map(|s| s.criterion_id.as_str()))
                        && feedback.valid(4000)
                        && achievements.len() <= 30
                        && achievements.iter().all(|t| t.valid(500))
                }
                _ => true,
            }
    }
}

#[derive(Clone, Debug)]
pub struct Actor {
    pub id: String,
    pub role: String,
}
