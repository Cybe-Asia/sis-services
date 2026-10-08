//! Draft schedules and current publication share one resource concurrency boundary.
pub(crate) mod calendar_guard;
pub(crate) mod model;
pub(crate) mod reads;
pub(crate) mod repository;
mod validation;
pub(crate) mod weekly;
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
type Failure = crate::school_portal::Failure;
pub(super) fn failure(status: StatusCode) -> Failure {
    (
        status,
        Json(
            json!({"error":{"code":match status {StatusCode::BAD_REQUEST=>"INVALID_INPUT",StatusCode::UNAUTHORIZED=>"UNAUTHORIZED",StatusCode::FORBIDDEN=>"FORBIDDEN",StatusCode::CONFLICT=>"REVISION_CONFLICT",_=>"DEPENDENCY_UNAVAILABLE"}}}),
        ),
    )
}
pub(super) fn invalid(issues: Vec<&'static str>) -> Failure {
    (
        StatusCode::CONFLICT,
        Json(
            json!({"error":{"code":"TIMETABLE_INVALID","issues":issues.into_iter().map(|code|json!({"code":code})).collect::<Vec<_>>()}}),
        ),
    )
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(scope): Query<model::Scope>,
) -> Result<Json<Value>, Failure> {
    if !scope.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (actor, role) =
        super::administrator_actor(&headers, &scope.tenant_id, &scope.school_id).await?;
    let data = reads::list(&state.graph, &actor, &role, &scope).await?;
    if super::administrator_actor(&headers, &scope.tenant_id, &scope.school_id).await?
        != (actor, role)
    {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    Ok(Json(json!({"responseCode":200,"data":data})))
}
async fn change(
    State(state): State<AppState>,
    headers: HeaderMap,
    input: Result<Json<model::Change>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(mut input) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    input.meeting.room_id = input.meeting.room_id.to_ascii_lowercase();
    if !input.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (actor, role) =
        super::administrator_actor(&headers, &input.meeting.tenant_id, &input.meeting.school_id)
            .await?;
    repository::change(
        &state.graph,
        &actor,
        &role,
        &headers,
        repository::Family::Portal,
        &input,
    )
    .await
    .map(|data| Json(json!({"responseCode":200,"data":data})))
}
async fn learning_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(scope): Query<model::Scope>,
) -> Result<Json<Value>, Failure> {
    if !scope.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (actor, role) = super::owner::class_actor(
        &state,
        &headers,
        &scope.school_id,
        &scope.tenant_id,
        &scope.class_id,
    )
    .await?;
    if !matches!(role.as_str(), "teacher" | "owner") {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let mut data = reads::list(&state.graph, &actor, &role, &scope).await?;
    if super::owner::class_actor(
        &state,
        &headers,
        &scope.school_id,
        &scope.tenant_id,
        &scope.class_id,
    )
    .await?
        != (actor.clone(), role.clone())
    {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    data["actor"] = json!({"id":actor,"role":role});
    data["schoolId"] = json!(scope.school_id);
    data["tenantId"] = json!(scope.tenant_id);
    data["classId"] = json!(scope.class_id);
    Ok(Json(json!({"responseCode":200,"data":data})))
}
async fn learning_change(
    State(state): State<AppState>,
    headers: HeaderMap,
    input: Result<Json<model::Change>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(mut input) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    input.meeting.room_id = input.meeting.room_id.to_ascii_lowercase();
    if !input.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let m = &input.meeting;
    let (actor, role) =
        super::owner::class_actor(&state, &headers, &m.school_id, &m.tenant_id, &m.class_id)
            .await?;
    if !matches!(role.as_str(), "teacher" | "owner") {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    repository::change(
        &state.graph,
        &actor,
        &role,
        &headers,
        repository::Family::Learning,
        &input,
    )
    .await
    .map(|data| Json(json!({"responseCode":200,"data":data})))
}
async fn learning_weekly(
    State(state): State<AppState>,
    headers: HeaderMap,
    input: Result<Json<weekly::Weekly>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(input) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !input.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (actor, role) = super::owner::class_actor(
        &state,
        &headers,
        &input.school_id,
        &input.tenant_id,
        &input.class_id,
    )
    .await?;
    if !matches!(role.as_str(), "teacher" | "owner") {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    weekly::apply(
        &state.graph,
        &actor,
        &role,
        &headers,
        repository::Family::Learning,
        &input,
    )
    .await
    .map(|data| Json(json!({"responseCode":200,"data":data})))
}
async fn portal_weekly(
    State(state): State<AppState>,
    headers: HeaderMap,
    input: Result<Json<weekly::Weekly>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(input) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !input.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (actor, role) =
        super::administrator_actor(&headers, &input.tenant_id, &input.school_id).await?;
    weekly::apply(
        &state.graph,
        &actor,
        &role,
        &headers,
        repository::Family::Portal,
        &input,
    )
    .await
    .map(|data| Json(json!({"responseCode":200,"data":data})))
}
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sis-service/timetable/context", get(reads::context))
        .route("/api/v1/sis-service/timetable", get(list).post(change))
        .route(
            "/api/v1/sis-service/learning/timetable",
            get(learning_list).post(learning_change),
        )
        .route(
            "/api/v1/sis-service/learning/timetable/context",
            get(reads::learning_context),
        )
        .route(
            "/api/v1/sis-service/learning/class-meetings/teachers",
            get(reads::learning_context),
        )
        .layer(axum::extract::DefaultBodyLimit::max(16_384))
        // Weekly slots plus per-course material order exceed the single-meeting body limit.
        .merge(
            Router::new()
                .route(
                    "/api/v1/sis-service/learning/timetable/weekly",
                    axum::routing::post(learning_weekly),
                )
                .route(
                    "/api/v1/sis-service/timetable/weekly",
                    axum::routing::post(portal_weekly),
                )
                .layer(axum::extract::DefaultBodyLimit::max(131_072)),
        )
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
