//! School-owned extracurricular operations; additive graph records with no demo seed.
mod application;
mod auth;
mod context;
pub mod model;
mod policy;
mod ports;
pub mod repository;
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
type Failure = (StatusCode, Json<Value>);
fn failure(s: StatusCode) -> Failure {
    (
        s,
        Json(
            json!({"error":{"code":match s{StatusCode::BAD_REQUEST=>"INVALID_INPUT",StatusCode::UNAUTHORIZED=>"UNAUTHORIZED",StatusCode::FORBIDDEN=>"FORBIDDEN",StatusCode::CONFLICT=>"REVISION_CONFLICT",_=>"DEPENDENCY_UNAVAILABLE"}}}),
        ),
    )
}
async fn list(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(role): Path<String>,
    Query(scope): Query<model::Scope>,
) -> Result<Json<Value>, Failure> {
    if !scope.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let a = auth::actor(&h, &role, &scope).await?;
    match repository::list(&s.graph, &a, &scope).await {
        Ok(Some(data)) => Ok(Json(json!({"data":data}))),
        Ok(None) => Err(failure(StatusCode::FORBIDDEN)),
        Err(_) => Err(failure(StatusCode::SERVICE_UNAVAILABLE)),
    }
}
async fn change(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(role): Path<String>,
    body: Result<Json<model::Change>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(v) = body.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !v.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let a = auth::actor(&h, &role, &v.scope).await?;
    match repository::change_live(&s.graph, &a, &v, &h, &role).await {
        Ok(Some(data)) => Ok(Json(json!({"data":data}))),
        Ok(None) => Err(failure(StatusCode::CONFLICT)),
        Err(error) => {
            match error.downcast::<crate::school_portal::staff_capability::SessionFailure>() {
                Ok(error) => Err(error.0),
                Err(_) => Err(failure(StatusCode::SERVICE_UNAVAILABLE)),
            }
        }
    }
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/activities/context/:role",
            get(context::context),
        )
        .route(
            "/api/v1/sis-service/activities/:role",
            get(list).post(change),
        )
        .layer(axum::extract::DefaultBodyLimit::max(65536))
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
mod tests;

#[cfg(test)]
mod browser_fixture;
