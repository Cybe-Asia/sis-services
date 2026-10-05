mod contract;
mod learning;
mod repository;
use super::{failure, model::identifier, Failure};
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
pub use repository::init;
use serde_json::Value;
pub async fn import(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(section): Path<String>,
    body: Result<Json<contract::Import>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(input) = body.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    let command = headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .filter(|s| identifier(s))
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST))?
        .to_string();
    if !identifier(&section) || !input.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    // Disconnects must not cancel a database commit with an uncertain acknowledgement.
    let task = tokio::spawn(async move {
        tokio::time::timeout(
            std::time::Duration::from_secs(25),
            repository::import(s.graph, headers, section, input, command),
        )
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    });
    let data = task
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))??;
    Ok(super::envelope(data))
}
#[cfg(test)]
mod tests;
