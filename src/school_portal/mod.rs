//! School-owned classroom operations. Staff Auth and canonical class assignment are both required.
pub mod assessment_grades;
pub(crate) mod auth;
pub mod model;
pub mod repository;
pub(crate) mod staff_capability;
pub(crate) mod student_identity;
mod student_preferences;
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
pub type Failure = (StatusCode, Json<Value>);
fn failure(status: StatusCode) -> Failure {
    (
        status,
        Json(
            json!({"error":{"code":match status {StatusCode::UNAUTHORIZED=>"UNAUTHORIZED",StatusCode::FORBIDDEN=>"FORBIDDEN",StatusCode::BAD_REQUEST=>"INVALID_INPUT",StatusCode::CONFLICT=>"REVISION_CONFLICT",_=>"DEPENDENCY_UNAVAILABLE"}}}),
        ),
    )
}
fn envelope(data: Value) -> Json<Value> {
    Json(json!({"responseCode":200,"data":data}))
}
fn saved(
    result: Result<bool, Box<dyn std::error::Error + Send + Sync>>,
) -> Result<Json<Value>, Failure> {
    match result {
        Ok(true) => Ok(envelope(json!({"saved":true}))),
        Ok(false) => Err(failure(StatusCode::CONFLICT)),
        Err(error) => match error.downcast::<staff_capability::SessionFailure>() {
            Ok(error) => Err(error.0),
            Err(_) => Err(failure(StatusCode::SERVICE_UNAVAILABLE)),
        },
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Day {
    date: Option<String>,
    #[serde(alias = "schoolId")]
    school_id: Option<String>,
    #[serde(alias = "tenantId")]
    tenant_id: Option<String>,
}
async fn context(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(scope): Query<staff_capability::Selection>,
) -> Result<Json<Value>, Failure> {
    let m = auth::teacher(&h).await?.select(&scope)?;
    repository::context(&s.graph, &m)
        .await
        .map(envelope)
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))
}
async fn classroom(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(section): Path<String>,
    Query(q): Query<Day>,
) -> Result<Json<Value>, Failure> {
    let scope = staff_capability::Selection {
        school_id: q.school_id,
        tenant_id: q.tenant_id,
    };
    let m = auth::teacher(&h).await?.select(&scope)?;
    let date = q.date.unwrap_or_else(|| model::school_today().to_string());
    if !model::identifier(&section) || !model::date(&date) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    match repository::classroom(&s.graph, &m, &section, &date).await {
        Ok(Some(v)) => Ok(envelope(v)),
        Ok(None) => Err(failure(StatusCode::FORBIDDEN)),
        Err(_) => Err(failure(StatusCode::SERVICE_UNAVAILABLE)),
    }
}
async fn attendance(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(section): Path<String>,
    Query(scope): Query<staff_capability::Selection>,
    input: Result<Json<model::RollCall>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let m = auth::teacher(&h).await?.select(&scope)?;
    let Json(v) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !model::identifier(&section) || !v.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    saved(repository::attendance_live(&s.graph, &m, &section, &v, &h).await)
}
async fn publish(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(section): Path<String>,
    Query(scope): Query<staff_capability::Selection>,
    input: Result<Json<model::PublishedRecord>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let m = auth::teacher(&h).await?.select(&scope)?;
    let Json(v) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !model::identifier(&section) || !v.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    saved(repository::publish_live(&s.graph, &m, &section, &v, &h).await)
}
async fn review(
    State(s): State<AppState>,
    h: HeaderMap,
    Path((section, id)): Path<(String, String)>,
    Query(scope): Query<staff_capability::Selection>,
    input: Result<Json<model::Review>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let m = auth::teacher(&h).await?.select(&scope)?;
    let Json(v) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !model::identifier(&section) || !model::identifier(&id) || !v.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    saved(repository::review_live(&s.graph, &m, &section, &id, &v, &h).await)
}
async fn parent(State(s): State<AppState>, h: HeaderMap) -> Result<Json<Value>, Failure> {
    let sub = auth::parent(&h)?;
    repository::parent_snapshot(&s.graph, &sub)
        .await
        .map(envelope)
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))
}
async fn parent_growth(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(student): Path<String>,
) -> Result<Json<Value>, Failure> {
    let sub = auth::parent(&h)?;
    if !model::identifier(&student) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    match repository::parent_growth(&s.graph, &sub, &student).await {
        Ok(Some(v)) => Ok(envelope(v)),
        Ok(None) => Err(failure(StatusCode::FORBIDDEN)),
        Err(_) => Err(failure(StatusCode::SERVICE_UNAVAILABLE)),
    }
}
async fn preferences(
    State(s): State<AppState>,
    h: HeaderMap,
    input: Result<Json<model::Preferences>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let sub = auth::parent(&h)?;
    let Json(v) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !v.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    saved(repository::preferences(&s.graph, &sub, &v).await)
}
async fn rsvp(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    input: Result<Json<model::Rsvp>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let sub = auth::parent(&h)?;
    let Json(v) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !model::identifier(&id) || !v.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    saved(repository::rsvp(&s.graph, &sub, &id, &v).await)
}
async fn read(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let sub = auth::parent(&h)?;
    if !model::identifier(&id) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    saved(repository::mark_read(&s.graph, &sub, &id).await)
}
async fn parent_calendar(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(w): Query<crate::education_calendar::window::Window>,
) -> Result<Json<Value>, Failure> {
    let sub = auth::parent(&h)?;
    if !w.valid() || w.student_id.is_none() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    match repository::parent_calendar(&s.graph, &sub, &w).await {
        Ok(Some(v)) => Ok(envelope(v)),
        Ok(None) => Err(failure(StatusCode::FORBIDDEN)),
        Err(_) => Err(failure(StatusCode::SERVICE_UNAVAILABLE)),
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TeacherCalendar {
    from: String,
    to: String,
    after: Option<String>,
    student_id: Option<String>,
    #[serde(alias = "school_id")]
    school_id: Option<String>,
    #[serde(alias = "tenant_id")]
    tenant_id: Option<String>,
}
async fn teacher_calendar(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(section): Path<String>,
    Query(q): Query<TeacherCalendar>,
) -> Result<Json<Value>, Failure> {
    let scope = staff_capability::Selection {
        school_id: q.school_id,
        tenant_id: q.tenant_id,
    };
    let member = auth::teacher(&h).await?.select(&scope)?;
    let w = crate::education_calendar::window::Window {
        from: q.from,
        to: q.to,
        after: q.after,
        student_id: q.student_id,
    };
    if !w.valid() || w.student_id.is_some() || !model::identifier(&section) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    match repository::teacher_calendar(&s.graph, &member, &section, &w).await {
        Ok(Some(v)) => Ok(envelope(v)),
        Ok(None) => Err(failure(StatusCode::FORBIDDEN)),
        Err(_) => Err(failure(StatusCode::SERVICE_UNAVAILABLE)),
    }
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/learning/owner/student/preferences",
            get(student_preferences::read).post(student_preferences::write),
        )
        .route("/api/leads/v1/me/school/calendar", get(parent_calendar))
        .route(
            "/api/leads/v1/teacher/school/classes/:section/calendar",
            get(teacher_calendar),
        )
        .route("/api/leads/v1/teacher/school/context", get(context))
        .route(
            "/api/leads/v1/teacher/school/classes/:section",
            get(classroom),
        )
        .route(
            "/api/leads/v1/teacher/school/classes/:section/attendance",
            post(attendance),
        )
        .route(
            "/api/leads/v1/teacher/school/classes/:section/records",
            post(publish),
        )
        .route(
            "/api/leads/v1/teacher/school/classes/:section/requests/:id/review",
            post(review),
        )
        .route(
            "/api/leads/v1/teacher/school/classes/:section/assessment-grades",
            post(assessment_grades::import),
        )
        .route("/api/leads/v1/me/school", get(parent))
        .route(
            "/api/leads/v1/me/school/growth/:student",
            get(parent_growth),
        )
        .route("/api/leads/v1/me/school/preferences", post(preferences))
        .route("/api/leads/v1/me/school/events/:id/rsvp", post(rsvp))
        .route("/api/leads/v1/me/school/notices/:id/read", post(read))
        .layer(axum::extract::DefaultBodyLimit::max(524_288))
        .layer(axum::middleware::map_response(
            |mut r: axum::response::Response| async move {
                r.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                r
            },
        ))
}
#[cfg(test)]
mod database_tests;
