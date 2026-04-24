// Section (spec §3.4 — school structure, SIS side).
//
// A Section is a named group of enrolled students for one academic year.
// The Section carries its school_id and year_group for cheap filtering;
// students attach via (EnrolledStudent)-[:ENROLLED_IN]->(Section).
//
// Lives in admission-services for v0.1 — we already own EnrolledStudent
// here. When the SIS domain grows (attendance / grades / timetable),
// we extract this file + sis_repository + sis_handler into a dedicated
// sis-services process.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const SECTION_STATUS_ACTIVE: &str = "active";
pub const SECTION_STATUS_ARCHIVED: &str = "archived";

pub const SECTION_ALL_STATUSES: &[&str] = &[SECTION_STATUS_ACTIVE, SECTION_STATUS_ARCHIVED];

pub fn is_valid_section_status(s: &str) -> bool {
    SECTION_ALL_STATUSES.contains(&s)
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[allow(non_snake_case)]
pub struct Section {
    pub sectionId: String,
    pub tenantId: String,
    pub schoolId: String,
    /// Human-readable label shown on parent dashboards, e.g. "Grade 7A".
    pub name: String,
    /// Shared shape with Student.targetGradeLevel — "Grade 7", "Grade 10".
    pub yearGroup: String,
    /// "2026", "2026/2027" — matches Offer.academic_year.
    pub academicYear: String,
    pub status: String,
    #[serde(default)]
    pub enrolledCount: i64,
    /// Free-text name of the homeroom teacher. Stored as a string rather
    /// than as a :Teacher node because we don't have a teacher domain yet
    /// — a proper (Teacher)-[:TEACHES]->(Section) edge replaces this
    /// when sis-services grows.
    #[serde(default)]
    pub homeroomTeacherName: Option<String>,
    /// Optional contact email. Surfaced on the parent card.
    #[serde(default)]
    pub homeroomTeacherEmail: Option<String>,
    pub createdAt: DateTime<Utc>,
    pub updatedAt: DateTime<Utc>,
}
