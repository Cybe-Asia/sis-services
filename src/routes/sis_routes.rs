use axum::{routing::{get, post}, Router};

use crate::handlers::sis_handler::{
    admin_assign_students_handler, admin_attendance_roster_handler, admin_attendance_upsert_handler,
    admin_create_section_handler, admin_grade_roster_handler, admin_grades_upsert_handler,
    admin_list_sections_handler, admin_section_detail_handler, admin_update_homeroom_teacher_handler,
    admin_update_section_status_handler, parent_list_attendance_handler, parent_list_grades_handler,
    parent_list_sections_handler,
};
use crate::AppState;

pub fn sis_router() -> Router<AppState> {
    Router::new()
        // Admin.
        .route("/api/leads/v1/admin/sis/sections", get(admin_list_sections_handler).post(admin_create_section_handler))
        .route("/api/leads/v1/admin/sis/sections/:section_id", get(admin_section_detail_handler))
        .route("/api/leads/v1/admin/sis/sections/:section_id/status", post(admin_update_section_status_handler))
        .route("/api/leads/v1/admin/sis/sections/:section_id/assign", post(admin_assign_students_handler))
        .route("/api/leads/v1/admin/sis/sections/:section_id/homeroom", post(admin_update_homeroom_teacher_handler))
        // Attendance — admin records, parent reads.
        .route(
            "/api/leads/v1/admin/sis/sections/:section_id/attendance",
            get(admin_attendance_roster_handler).post(admin_attendance_upsert_handler),
        )
        // Grades — admin records, parent reads.
        .route(
            "/api/leads/v1/admin/sis/sections/:section_id/grades",
            get(admin_grade_roster_handler).post(admin_grades_upsert_handler),
        )
        // Parent.
        .route("/api/leads/v1/me/sections", get(parent_list_sections_handler))
        .route("/api/leads/v1/me/attendance", get(parent_list_attendance_handler))
        .route("/api/leads/v1/me/grades", get(parent_list_grades_handler))
}
