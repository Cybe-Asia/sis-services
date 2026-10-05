//! Explicit Admission -> permanent SIS enrollment. No payment or identity mutation.
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
pub(crate) mod auth;
mod billing;
mod context;
mod model;
mod repository;
#[cfg(test)]
mod tests;
type Failure = (StatusCode, Json<Value>);
fn failure(s: StatusCode) -> Failure {
    (
        s,
        Json(
            json!({"error":{"code":match s {StatusCode::BAD_REQUEST=>"INVALID_HANDOVER",StatusCode::UNAUTHORIZED=>"UNAUTHORIZED",StatusCode::FORBIDDEN=>"HANDOVER_FORBIDDEN",StatusCode::CONFLICT=>"HANDOVER_CONFLICT",_=>"HANDOVER_UNAVAILABLE"}}}),
        ),
    )
}
async fn handover(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<model::Command>, JsonRejection>,
) -> Result<(HeaderMap, Json<Value>), Failure> {
    let actor = auth::actor(&headers, &state.config.jwt_secret).map_err(|e| {
        failure(match e {
            auth::Error::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            auth::Error::Unauthorized => StatusCode::UNAUTHORIZED,
        })
    })?;
    let Json(command) = body.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !command.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let receipt = repository::execute(&state.graph, &actor, &command)
        .await
        .map_err(|e| {
            failure(match e {
                repository::Error::Denied => StatusCode::FORBIDDEN,
                repository::Error::Conflict => StatusCode::CONFLICT,
                repository::Error::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            })
        })?;
    let mut headers = HeaderMap::new();
    headers.insert("cache-control", "no-store".parse().unwrap());
    Ok((headers, Json(json!({"data":receipt}))))
}
fn no_store(data: Value) -> (HeaderMap, Json<Value>) {
    let mut headers = HeaderMap::new();
    headers.insert("cache-control", "no-store".parse().unwrap());
    (headers, Json(json!({"data":data})))
}
async fn context(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<Value>), Failure> {
    let actor = auth::actor(&headers, &state.config.jwt_secret).map_err(|e| {
        failure(match e {
            auth::Error::Unauthorized => StatusCode::UNAUTHORIZED,
            auth::Error::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        })
    })?;
    context::context(&state.graph, &actor)
        .await
        .map(no_store)
        .map_err(|e| {
            failure(match e {
                repository::Error::Denied => StatusCode::FORBIDDEN,
                _ => StatusCode::SERVICE_UNAVAILABLE,
            })
        })
}
async fn billing_families(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<Value>), Failure> {
    let actor = auth::actor(&headers, &state.config.jwt_secret).map_err(|e| {
        failure(match e {
            auth::Error::Unauthorized => StatusCode::UNAUTHORIZED,
            auth::Error::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        })
    })?;
    billing::families(&state.graph, &actor)
        .await
        .map(no_store)
        .map_err(|e| {
            failure(match e {
                repository::Error::Denied => StatusCode::FORBIDDEN,
                _ => StatusCode::SERVICE_UNAVAILABLE,
            })
        })
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/admin/enrollments/handover/context",
            get(context),
        )
        .route(
            "/api/v1/sis-service/billing/admin/current-families",
            get(billing_families),
        )
        .route(
            "/api/v1/sis-service/admin/enrollments/handover",
            post(handover),
        )
}

pub async fn migrate(graph: &neo4rs::Graph) -> Result<(), neo4rs::Error> {
    repository::migrate(graph).await
}
