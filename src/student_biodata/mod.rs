//! Admin record of the Student biodata and guardians the Student Profile reads
//! (`docs/STUDENT_BIODATA.md`). School administrators and owners of the Student's school only.
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, rejection::QueryRejection, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::time::Duration;

pub mod model;
pub mod repository;

use crate::school_portal::model::identifier;
use model::{Change, Lookup};
use repository::Saved;

type Failure = (StatusCode, Json<Value>);
fn coded(status: StatusCode, code: &str) -> Failure {
    (status, Json(json!({"error":{"code":code}})))
}
pub(crate) fn failure(status: StatusCode) -> Failure {
    coded(
        status,
        match status {
            StatusCode::BAD_REQUEST => "INVALID_INPUT",
            StatusCode::FORBIDDEN => "FORBIDDEN",
            StatusCode::UNAUTHORIZED => "UNAUTHORIZED",
            StatusCode::NOT_FOUND => "NOT_FOUND",
            StatusCode::CONFLICT => "REVISION_CONFLICT",
            _ => "DEPENDENCY_UNAVAILABLE",
        },
    )
}
async fn deadline<T>(work: impl std::future::Future<Output = Result<T, Failure>>) -> Result<T, Failure> {
    tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
}

async fn read(
    State(s): State<AppState>,
    h: HeaderMap,
    q: Result<Query<Lookup>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !identifier(&q.student_id) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    // The session is verified before any lookup; staff of another school see the same 404 as
    // for an unknown Student, so they cannot probe who is enrolled elsewhere.
    let session = crate::learning_access::staff_session(&h).await?;
    let not_found = || failure(StatusCode::NOT_FOUND);
    let scope = deadline(repository::enrolment(&s.graph, &q.student_id))
        .await?
        .ok_or_else(not_found)?;
    let (actor, _) = session
        .administrator(&scope.tenant_id, &scope.school_id)
        .map_err(|e| if e.0 == StatusCode::FORBIDDEN { not_found() } else { e })?;
    deadline(repository::read(&s.graph, &actor, &scope))
        .await?
        .map(|data| Json(json!({"data":data})))
        .ok_or_else(not_found)
}

async fn change(
    State(s): State<AppState>,
    h: HeaderMap,
    j: Result<Json<Change>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(c) = j.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    let clean = c.clean().ok_or_else(|| failure(StatusCode::BAD_REQUEST))?;
    let scope = c.scope();
    let actor = crate::learning_access::administrator(&h, &scope.tenant_id, &scope.school_id).await?;
    match deadline(repository::save(&s.graph, &actor, &clean, &scope, c.expected_version)).await? {
        Saved::Version(version) => Ok(Json(json!({"data":{"version":version}}))),
        Saved::Conflict => Err(failure(StatusCode::CONFLICT)),
        Saved::NisnInUse => Err(coded(StatusCode::CONFLICT, "NISN_IN_USE")),
        Saved::NotFound => Err(failure(StatusCode::NOT_FOUND)),
        Saved::Invalid => Err(failure(StatusCode::BAD_REQUEST)),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sis-service/admin/student-biodata", get(read).post(change))
        .layer(axum::extract::DefaultBodyLimit::max(8192))
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

pub use repository::init;

#[cfg(test)]
mod tests;
