// GradeEntry — per (section × student × subject × term) score record.
//
// Kept minimal for v0.1: a numeric score + max plus a free-text subject
// and term label. Later: linked curriculum, weighting, grade scale
// conversion. Admin UI validates score in [0, max_score].

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[allow(non_snake_case)]
pub struct GradeEntry {
    pub entryId: String,
    pub sectionId: String,
    pub applicantStudentId: String,
    /// Free-text for v0.1 — e.g. "Math", "English", "Science".
    pub subject: String,
    /// Free-text for v0.1 — e.g. "Term 1", "Semester 1".
    pub term: String,
    pub score: f64,
    pub maxScore: f64,
    pub recordedAt: String,
    pub recordedBy: Option<String>,
    pub notes: Option<String>,
}
