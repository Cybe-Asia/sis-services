//! Persistence port: application decisions use typed states, not transport/graph rows.
use super::model::*;
use serde::{Deserialize, Serialize};
#[derive(Clone)]
pub struct StoredActivity {
    pub activity: Activity,
    pub revision: u32,
    pub archived: bool,
    pub student_id: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Enrollment {
    pub student_id: String,
    pub revision: u32,
    pub status: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub student_id: String,
    pub revision: u32,
    pub released: bool,
    pub skills: Vec<Skill>,
    pub feedback: Text,
    pub achievements: Vec<Text>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub revision: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub student_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub released: Option<bool>,
}
impl Receipt {
    pub fn revision(revision: u32) -> Self {
        Self {
            revision,
            student_id: None,
            status: None,
            released: None,
        }
    }
}
pub enum Replay {
    None,
    Match(Receipt),
    Conflict,
}
pub trait ChangeStore {
    type Error;
    async fn authorized(&mut self) -> Result<bool, Self::Error>;
    async fn current(&mut self) -> Result<Option<StoredActivity>, Self::Error>;
    async fn replay(&mut self) -> Result<Replay, Self::Error>;
    async fn catalog_references(&mut self, activity: &Activity) -> Result<bool, Self::Error>;
    async fn enrollment_count(&mut self, reserved_only: bool) -> Result<u32, Self::Error>;
    async fn save_catalog(&mut self, activity: &Activity, revision: u32)
        -> Result<(), Self::Error>;
    async fn archive(&mut self, revision: u32) -> Result<(), Self::Error>;
    async fn enrollment(&mut self, student: &str) -> Result<Option<Enrollment>, Self::Error>;
    async fn save_enrollment(&mut self, enrollment: &Enrollment) -> Result<(), Self::Error>;
    async fn roster_authorized(&mut self, students: &[String]) -> Result<bool, Self::Error>;
    async fn meeting_revision(&mut self, meeting: &str) -> Result<u32, Self::Error>;
    async fn save_attendance(
        &mut self,
        meeting: &str,
        revision: u32,
        marks: &[Mark],
    ) -> Result<(), Self::Error>;
    async fn outcome(&mut self, student: &str) -> Result<Option<Outcome>, Self::Error>;
    async fn save_outcome(&mut self, outcome: &Outcome) -> Result<(), Self::Error>;
    async fn audit(&mut self, receipt: &Receipt) -> Result<(), Self::Error>;
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassContext {
    pub school_id: String,
    pub tenant_id: String,
    pub academic_year: String,
    pub section_id: String,
    pub name: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaffContext {
    pub id: String,
    pub name: String,
    pub school_ids: Vec<String>,
    pub tenant_ids: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogContext {
    pub contract_version: u8,
    pub classes: Vec<ClassContext>,
    pub staff: Vec<StaffContext>,
}
pub trait ContextReader {
    type Error;
    async fn read(
        &self,
        actor: &Actor,
        student: Option<String>,
        schools: Vec<String>,
        tenants: Vec<String>,
    ) -> Result<CatalogContext, Self::Error>;
}
