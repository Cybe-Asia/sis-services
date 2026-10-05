use super::super::model::identifier;
use serde::{Deserialize, Serialize};
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Import {
    pub course_id: String,
    pub assessment_id: String,
    pub attempt_id: String,
    pub attempt_revision: u32,
    pub term: String,
}
impl Import {
    pub fn valid(&self) -> bool {
        [&self.course_id, &self.assessment_id, &self.attempt_id]
            .iter()
            .all(|s| identifier(s))
            && self.attempt_revision > 0
            && (!self.term.trim().is_empty() && self.term.len() <= 128)
            && self.term.trim() == self.term
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Export {
    pub schema_version: u8,
    pub school_id: String,
    pub tenant_id: String,
    pub class_id: String,
    pub course_id: String,
    pub course_revision: u32,
    pub assessment_id: String,
    pub assessment_revision: u32,
    pub attempt_id: String,
    pub attempt_revision: u32,
    pub student_id: String,
    pub author_id: String,
    pub subject: String,
    pub score_minor: u32,
    pub maximum_minor: u32,
    pub submitted_at: u64,
    pub results_available_at: Option<u64>,
}
impl Export {
    pub fn matches(&self, input: &Import, section: &str, actor: &str) -> bool {
        self.matches_for(input, section, actor, false)
    }
    pub fn matches_for(&self, input: &Import, section: &str, actor: &str, owner: bool) -> bool {
        self.schema_version == 1
            && [
                &self.school_id,
                &self.tenant_id,
                &self.class_id,
                &self.course_id,
                &self.assessment_id,
                &self.attempt_id,
                &self.student_id,
                &self.author_id,
            ]
            .iter()
            .all(|s| identifier(s))
            && self.class_id == section
            && (owner || self.author_id == actor)
            && self.course_id == input.course_id
            && self.assessment_id == input.assessment_id
            && self.attempt_id == input.attempt_id
            && self.attempt_revision == input.attempt_revision
            && self.course_revision > 0
            && self.assessment_revision > 0
            && self.maximum_minor > 0
            && self.maximum_minor <= 5_000_000
            && self.score_minor <= self.maximum_minor
            && self.submitted_at > 0
            && self.submitted_at <= 4_102_444_800_000
            && self
                .results_available_at
                .is_none_or(|v| v <= 4_102_444_800_000)
            && [
                "mathematics",
                "science",
                "physics",
                "chemistry",
                "computing",
                "english",
                "humanities",
                "geography",
                "economics",
                "religion",
                "indonesian",
                "biology",
                "sociology",
                "history",
                "civics",
            ]
            .contains(&self.subject.as_str())
    }
}
