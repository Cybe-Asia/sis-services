// SIS admin + parent HTTP surface. Source of truth for sections,
// attendance, grades. Lives in sis-services (its own repo) since
// v0.2 after the admissions/SIS split; imports now reference the
// local auth_helper instead of admission-services::lead_handler.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::handlers::auth_helper::{require_admin, resolve_lead_id_from_bearer};
use crate::models::section_model::Section;
use crate::repositories::{sis_repository, user_repository};
use crate::utils::jwt::decode_verification_token;
use crate::utils::response::ApiResponse;
use crate::AppState;

// ---- Admin: Sections CRUD + assign ---------------------------------------

#[utoipa::path(
    post,
    path = "/api/leads/v1/admin/sis/sections",
    request_body = sis_repository::CreateSectionInput,
    responses(
        (status = 200, body = ApiResponse<Section>),
        (status = 400),
        (status = 403)
    ),
    security(("bearer_auth" = []))
)]
pub async fn admin_create_section_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<sis_repository::CreateSectionInput>,
) -> (StatusCode, Json<ApiResponse<Section>>) {
    let admin = match require_admin(&state, &headers).await {
        Ok(id) => id,
        Err((c, m)) => return (c, Json(ApiResponse::error(&m, c.as_u16() as i32))),
    };
    match sis_repository::create_section(&state.graph, &payload).await {
        Ok(s) => {
            let diff = format!(
                "{{\"school\":\"{}\",\"year\":\"{}\",\"ay\":\"{}\"}}",
                s.schoolId, s.yearGroup, s.academicYear
            );
            let _ = crate::repositories::audit_repository::emit(
                &state.graph, &admin, "section.created", "section", &s.sectionId, Some(&diff),
            ).await;
            (StatusCode::OK, Json(ApiResponse::success(s)))
        }
        Err(e) => {
            let msg = e.to_string();
            let code = if msg.contains("required") { StatusCode::BAD_REQUEST } else { StatusCode::INTERNAL_SERVER_ERROR };
            (code, Json(ApiResponse::error(&msg, code.as_u16() as i32)))
        }
    }
}

#[derive(Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct AdminListSectionsQuery {
    pub school: Option<String>,
    pub academic_year: Option<String>,
    pub year_group: Option<String>,
    pub status: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/leads/v1/admin/sis/sections",
    responses(
        (status = 200, body = ApiResponse<Vec<Section>>),
        (status = 403)
    ),
    security(("bearer_auth" = []))
)]
pub async fn admin_list_sections_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<AdminListSectionsQuery>,
) -> (StatusCode, Json<ApiResponse<Vec<Section>>>) {
    if let Err((c, m)) = require_admin(&state, &headers).await {
        return (c, Json(ApiResponse::error(&m, c.as_u16() as i32)));
    }
    let strip = |o: &Option<String>| o.as_ref().and_then(|s| if s.trim().is_empty() { None } else { Some(s.trim().to_string()) });
    let filters = sis_repository::ListSectionFilters {
        school: strip(&q.school),
        academic_year: strip(&q.academic_year),
        year_group: strip(&q.year_group),
        status: strip(&q.status),
    };
    match sis_repository::list_sections(&state.graph, &filters).await {
        Ok(v) => (StatusCode::OK, Json(ApiResponse::success(v))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(&e.to_string(), 500))),
    }
}

#[derive(Serialize, ToSchema)]
#[allow(non_snake_case)]
pub struct AdminSectionDetailResponse {
    pub section: Section,
    pub members: Vec<sis_repository::SectionMemberRow>,
}

#[utoipa::path(
    get,
    path = "/api/leads/v1/admin/sis/sections/{section_id}",
    responses(
        (status = 200, body = ApiResponse<AdminSectionDetailResponse>),
        (status = 403),
        (status = 404)
    ),
    security(("bearer_auth" = []))
)]
pub async fn admin_section_detail_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(section_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<AdminSectionDetailResponse>>) {
    if let Err((c, m)) = require_admin(&state, &headers).await {
        return (c, Json(ApiResponse::error(&m, c.as_u16() as i32)));
    }
    let section = match sis_repository::find_section_by_id(&state.graph, &section_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(ApiResponse::error("Section not found", 404))),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(&e.to_string(), 500))),
    };
    let members = sis_repository::list_section_members(&state.graph, &section_id).await.unwrap_or_default();
    (StatusCode::OK, Json(ApiResponse::success(AdminSectionDetailResponse { section, members })))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssignStudentsRequest {
    pub applicant_student_ids: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[allow(non_snake_case)]
pub struct AssignStudentsResponse {
    pub requested: i64,
    pub assigned: i64,
}

#[utoipa::path(
    post,
    path = "/api/leads/v1/admin/sis/sections/{section_id}/assign",
    request_body = AssignStudentsRequest,
    responses(
        (status = 200, body = ApiResponse<AssignStudentsResponse>),
        (status = 400),
        (status = 403)
    ),
    security(("bearer_auth" = []))
)]
pub async fn admin_assign_students_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(section_id): Path<String>,
    Json(payload): Json<AssignStudentsRequest>,
) -> (StatusCode, Json<ApiResponse<AssignStudentsResponse>>) {
    let admin = match require_admin(&state, &headers).await {
        Ok(id) => id,
        Err((c, m)) => return (c, Json(ApiResponse::error(&m, c.as_u16() as i32))),
    };
    if payload.applicant_student_ids.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("applicant_student_ids required", 400)));
    }
    if payload.applicant_student_ids.len() > 200 {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("max 200 students per assign call", 400)));
    }
    let requested = payload.applicant_student_ids.len() as i64;
    match sis_repository::assign_students(&state.graph, &section_id, &payload.applicant_student_ids).await {
        Ok(n) => {
            let diff = format!("{{\"requested\":{},\"assigned\":{}}}", requested, n);
            let _ = crate::repositories::audit_repository::emit(
                &state.graph, &admin, "section.assign", "section", &section_id, Some(&diff),
            ).await;
            (StatusCode::OK, Json(ApiResponse::success(AssignStudentsResponse { requested, assigned: n })))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(&e.to_string(), 500))),
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateHomeroomTeacherRequest {
    pub name: Option<String>,
    pub email: Option<String>,
}

pub async fn admin_update_homeroom_teacher_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(section_id): Path<String>,
    Json(payload): Json<UpdateHomeroomTeacherRequest>,
) -> (StatusCode, Json<ApiResponse<Section>>) {
    let admin = match require_admin(&state, &headers).await {
        Ok(id) => id,
        Err((c, m)) => return (c, Json(ApiResponse::error(&m, c.as_u16() as i32))),
    };
    let name = payload.name.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty());
    let email = payload.email.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty());
    if let Some(e) = email {
        // Cheapest valid-email guard — just enforce '@' + '.' presence.
        if !e.contains('@') || !e.contains('.') {
            return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("invalid email", 400)));
        }
    }
    match sis_repository::set_homeroom_teacher(&state.graph, &section_id, name, email).await {
        Ok(s) => {
            let diff = format!(
                "{{\"name\":\"{}\",\"email\":\"{}\"}}",
                name.unwrap_or(""), email.unwrap_or("")
            );
            let _ = crate::repositories::audit_repository::emit(
                &state.graph, &admin, "section.homeroom.set", "section", &section_id, Some(&diff),
            ).await;
            (StatusCode::OK, Json(ApiResponse::success(s)))
        }
        Err(e) => {
            let code = if matches!(e, crate::repositories::lead_repository::RepositoryError::NotFound) {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (code, Json(ApiResponse::error(&e.to_string(), code.as_u16() as i32)))
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSectionStatusRequest {
    pub status: String,
}

pub async fn admin_update_section_status_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(section_id): Path<String>,
    Json(payload): Json<UpdateSectionStatusRequest>,
) -> (StatusCode, Json<ApiResponse<Section>>) {
    if let Err((c, m)) = require_admin(&state, &headers).await {
        return (c, Json(ApiResponse::error(&m, c.as_u16() as i32)));
    }
    match sis_repository::set_section_status(&state.graph, &section_id, &payload.status).await {
        Ok(s) => (StatusCode::OK, Json(ApiResponse::success(s))),
        Err(e) => {
            let msg = e.to_string();
            let code = if msg.contains("invalid section status") { StatusCode::BAD_REQUEST }
                else if matches!(e, crate::repositories::lead_repository::RepositoryError::NotFound) { StatusCode::NOT_FOUND }
                else { StatusCode::INTERNAL_SERVER_ERROR };
            (code, Json(ApiResponse::error(&msg, code.as_u16() as i32)))
        }
    }
}

// ---- Admin: Attendance recording -----------------------------------------

#[derive(Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct AttendanceListQuery {
    pub date: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[allow(non_snake_case)]
pub struct AttendanceListResponse {
    pub sectionId: String,
    pub date: String,
    pub rows: Vec<sis_repository::AttendanceRosterRow>,
}

/// GET /admin/sis/sections/:id/attendance?date=YYYY-MM-DD
/// Defaults `date` to today in the server's timezone so an admin can
/// hit the page without typing the date.
pub async fn admin_attendance_roster_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(section_id): Path<String>,
    Query(q): Query<AttendanceListQuery>,
) -> (StatusCode, Json<ApiResponse<AttendanceListResponse>>) {
    if let Err((c, m)) = require_admin(&state, &headers).await {
        return (c, Json(ApiResponse::error(&m, c.as_u16() as i32)));
    }
    let date = q.date.as_deref().filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    // Shape guard — YYYY-MM-DD only; the query accepts string equality so
    // we don't want slop dates reaching cypher.
    if !is_valid_ymd(&date) {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("date must be YYYY-MM-DD", 400)));
    }
    match sis_repository::list_attendance_for_date(&state.graph, &section_id, &date).await {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::success(AttendanceListResponse {
            sectionId: section_id, date, rows,
        }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(&e.to_string(), 500))),
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttendanceEntryIn {
    pub applicant_student_id: String,
    pub status: String,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkAttendanceRequest {
    pub date: String,
    pub entries: Vec<AttendanceEntryIn>,
}

#[derive(Serialize, ToSchema)]
#[allow(non_snake_case)]
pub struct BulkAttendanceResponse {
    pub requested: i64,
    pub saved: i64,
    pub date: String,
}

pub async fn admin_attendance_upsert_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(section_id): Path<String>,
    Json(payload): Json<BulkAttendanceRequest>,
) -> (StatusCode, Json<ApiResponse<BulkAttendanceResponse>>) {
    let admin = match require_admin(&state, &headers).await {
        Ok(id) => id,
        Err((c, m)) => return (c, Json(ApiResponse::error(&m, c.as_u16() as i32))),
    };
    if !is_valid_ymd(&payload.date) {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("date must be YYYY-MM-DD", 400)));
    }
    if payload.entries.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("entries required", 400)));
    }
    if payload.entries.len() > 500 {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("max 500 entries per call", 400)));
    }
    let requested = payload.entries.len() as i64;
    let batch: Vec<sis_repository::BulkAttendanceEntry> = payload.entries.into_iter()
        .map(|e| sis_repository::BulkAttendanceEntry {
            applicant_student_id: e.applicant_student_id,
            status: e.status,
            notes: e.notes,
        })
        .collect();
    match sis_repository::upsert_attendance_batch(&state.graph, &section_id, &payload.date, &batch, &admin).await {
        Ok(n) => {
            let diff = format!("{{\"date\":\"{}\",\"saved\":{}}}", payload.date, n);
            let _ = crate::repositories::audit_repository::emit(
                &state.graph, &admin, "attendance.recorded", "section", &section_id, Some(&diff),
            ).await;
            (StatusCode::OK, Json(ApiResponse::success(BulkAttendanceResponse {
                requested, saved: n, date: payload.date,
            })))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(&e.to_string(), 500))),
    }
}

// ---- Admin: Grades -------------------------------------------------------

#[derive(Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct GradeListQuery {
    pub subject: Option<String>,
    pub term: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[allow(non_snake_case)]
pub struct GradeListResponse {
    pub sectionId: String,
    pub subject: String,
    pub term: String,
    pub rows: Vec<sis_repository::GradeRosterRow>,
}

pub async fn admin_grade_roster_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(section_id): Path<String>,
    Query(q): Query<GradeListQuery>,
) -> (StatusCode, Json<ApiResponse<GradeListResponse>>) {
    if let Err((c, m)) = require_admin(&state, &headers).await {
        return (c, Json(ApiResponse::error(&m, c.as_u16() as i32)));
    }
    let subject = q.subject.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("").to_string();
    let term = q.term.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("").to_string();
    if subject.is_empty() || term.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("subject + term required", 400)));
    }
    match sis_repository::list_grades_for_subject_term(&state.graph, &section_id, &subject, &term).await {
        Ok(rows) => (StatusCode::OK, Json(ApiResponse::success(GradeListResponse {
            sectionId: section_id, subject, term, rows,
        }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(&e.to_string(), 500))),
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GradeEntryIn {
    pub applicant_student_id: String,
    pub score: f64,
    pub max_score: f64,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BulkGradesRequest {
    pub subject: String,
    pub term: String,
    pub entries: Vec<GradeEntryIn>,
}

#[derive(Serialize, ToSchema)]
#[allow(non_snake_case)]
pub struct BulkGradesResponse {
    pub requested: i64,
    pub saved: i64,
}

pub async fn admin_grades_upsert_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(section_id): Path<String>,
    Json(payload): Json<BulkGradesRequest>,
) -> (StatusCode, Json<ApiResponse<BulkGradesResponse>>) {
    let admin = match require_admin(&state, &headers).await {
        Ok(id) => id,
        Err((c, m)) => return (c, Json(ApiResponse::error(&m, c.as_u16() as i32))),
    };
    if payload.subject.trim().is_empty() || payload.term.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("subject + term required", 400)));
    }
    if payload.entries.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("entries required", 400)));
    }
    if payload.entries.len() > 500 {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::error("max 500 entries per call", 400)));
    }
    let requested = payload.entries.len() as i64;
    let batch: Vec<sis_repository::BulkGradeEntry> = payload.entries.into_iter()
        .map(|e| sis_repository::BulkGradeEntry {
            applicant_student_id: e.applicant_student_id,
            score: e.score,
            max_score: e.max_score,
            notes: e.notes,
        })
        .collect();
    match sis_repository::upsert_grades_batch(&state.graph, &section_id, &payload.subject, &payload.term, &batch, &admin).await {
        Ok(n) => {
            let diff = format!(
                "{{\"subject\":\"{}\",\"term\":\"{}\",\"saved\":{}}}",
                payload.subject, payload.term, n
            );
            let _ = crate::repositories::audit_repository::emit(
                &state.graph, &admin, "grades.recorded", "section", &section_id, Some(&diff),
            ).await;
            (StatusCode::OK, Json(ApiResponse::success(BulkGradesResponse { requested, saved: n })))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(&e.to_string(), 500))),
    }
}

pub async fn parent_list_grades_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> (StatusCode, Json<ApiResponse<Vec<sis_repository::ParentGradeRow>>>) {
    let lead_id = match resolve_lead_id_from_bearer(&state, &headers).await {
        Ok(id) => id,
        Err((code, msg)) => return (code, Json(ApiResponse::error(&msg, code.as_u16() as i32))),
    };
    let lead_ids = user_repository::list_lead_ids_for_user_of_lead(&state.graph, &lead_id)
        .await
        .unwrap_or_else(|_| vec![lead_id.clone()]);
    match sis_repository::list_grades_for_parent(&state.graph, &lead_ids).await {
        Ok(v) => (StatusCode::OK, Json(ApiResponse::success(v))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(&e.to_string(), 500))),
    }
}

fn is_valid_ymd(s: &str) -> bool {
    if s.len() != 10 { return false; }
    let bytes = s.as_bytes();
    bytes[4] == b'-' && bytes[7] == b'-'
        && s[0..4].chars().all(|c| c.is_ascii_digit())
        && s[5..7].chars().all(|c| c.is_ascii_digit())
        && s[8..10].chars().all(|c| c.is_ascii_digit())
}

// ---- Parent: my child at school ------------------------------------------

#[derive(Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ParentAttendanceQuery {
    pub from: Option<String>,
    pub to: Option<String>,
}

pub async fn parent_list_attendance_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ParentAttendanceQuery>,
) -> (StatusCode, Json<ApiResponse<Vec<sis_repository::ParentAttendanceRow>>>) {
    let lead_id = match resolve_lead_id_from_bearer(&state, &headers).await {
        Ok(id) => id,
        Err((code, msg)) => return (code, Json(ApiResponse::error(&msg, code.as_u16() as i32))),
    };
    let lead_ids = user_repository::list_lead_ids_for_user_of_lead(&state.graph, &lead_id)
        .await
        .unwrap_or_else(|_| vec![lead_id.clone()]);
    // Default: last 14 days ending today.
    let today = chrono::Utc::now().date_naive();
    let from = q.from.filter(|s| is_valid_ymd(s)).unwrap_or_else(|| (today - chrono::Duration::days(14)).format("%Y-%m-%d").to_string());
    let to = q.to.filter(|s| is_valid_ymd(s)).unwrap_or_else(|| today.format("%Y-%m-%d").to_string());
    match sis_repository::list_attendance_for_parent(&state.graph, &lead_ids, &from, &to).await {
        Ok(v) => (StatusCode::OK, Json(ApiResponse::success(v))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(&e.to_string(), 500))),
    }
}

#[utoipa::path(
    get,
    path = "/api/leads/v1/me/sections",
    responses(
        (status = 200, body = ApiResponse<Vec<sis_repository::ParentSectionRow>>),
        (status = 401)
    ),
    security(("bearer_auth" = []))
)]
pub async fn parent_list_sections_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> (StatusCode, Json<ApiResponse<Vec<sis_repository::ParentSectionRow>>>) {
    let lead_id = match resolve_lead_id_from_bearer(&state, &headers).await {
        Ok(id) => id,
        Err((code, msg)) => return (code, Json(ApiResponse::error(&msg, code.as_u16() as i32))),
    };
    let lead_ids = user_repository::list_lead_ids_for_user_of_lead(&state.graph, &lead_id)
        .await
        .unwrap_or_else(|_| vec![lead_id.clone()]);
    match sis_repository::list_parent_sections(&state.graph, &lead_ids).await {
        Ok(v) => (StatusCode::OK, Json(ApiResponse::success(v))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::error(&e.to_string(), 500))),
    }
}
