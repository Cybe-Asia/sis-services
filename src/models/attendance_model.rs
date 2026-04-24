// AttendanceRecord — one row per (section × student × date). Records
// the section-level roll-call result for a single school day.
//
// Lives alongside Section in admission-services for v0.1 (same graph,
// same service). Moves to sis-services when that process is carved out.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const ATTENDANCE_PRESENT: &str = "present";
pub const ATTENDANCE_ABSENT: &str = "absent";
pub const ATTENDANCE_LATE: &str = "late";
pub const ATTENDANCE_EXCUSED: &str = "excused";

pub const ATTENDANCE_ALL_STATUSES: &[&str] = &[
    ATTENDANCE_PRESENT,
    ATTENDANCE_ABSENT,
    ATTENDANCE_LATE,
    ATTENDANCE_EXCUSED,
];

pub fn is_valid_attendance_status(s: &str) -> bool {
    ATTENDANCE_ALL_STATUSES.contains(&s)
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[allow(non_snake_case)]
pub struct AttendanceRecord {
    pub recordId: String,
    pub sectionId: String,
    /// Always the ApplicantStudent (:Student) id — keeps one id-space
    /// across the admissions + SIS graph so admin tooling can follow
    /// the kid from EOI to attendance without translation.
    pub applicantStudentId: String,
    pub date: String,
    pub status: String,
    pub recordedAt: String,
    pub recordedBy: Option<String>,
    pub notes: Option<String>,
}
