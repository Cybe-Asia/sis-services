use crate::{
    models::parent_request::{ParentRequest, ParentRequestInput},
    repositories::parent_request_repository as repo,
    utils::response::ApiResponse,
    AppState,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::Deserialize;
type Error = (StatusCode, Json<ApiResponse<String>>);
fn error(status: StatusCode) -> Error {
    (
        status,
        Json(ApiResponse::error(
            "Unable to process school request",
            status.as_u16() as i32,
        )),
    )
}
#[derive(Deserialize)]
struct Claims {
    sub: String,
    #[allow(dead_code)]
    exp: usize,
    scope: Option<String>,
}
fn subject(headers: &HeaderMap) -> Result<String, Error> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.len() <= 8192)
        .ok_or(error(StatusCode::UNAUTHORIZED))?;
    let key = std::env::var("JWT_SECRET").map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE))?;
    if key.is_empty() {
        return Err(error(StatusCode::SERVICE_UNAVAILABLE));
    }
    let claims = decode::<Claims>(
        token,
        &DecodingKey::from_secret(key.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| error(StatusCode::UNAUTHORIZED))?
    .claims;
    if claims.scope.is_some() || claims.sub.is_empty() || claims.sub.len() > 128 {
        return Err(error(StatusCode::UNAUTHORIZED));
    }
    Ok(claims.sub)
}
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<Vec<ParentRequest>>>, Error> {
    let sub = subject(&headers)?;
    repo::list(&state.graph, &sub)
        .await
        .map(|data| Json(ApiResponse::success(data)))
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE))
}
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ParentRequestInput>,
) -> Result<Json<ApiResponse<ParentRequest>>, Error> {
    let sub = subject(&headers)?;
    if !input.valid() {
        return Err(error(StatusCode::BAD_REQUEST));
    }
    match repo::create(&state.graph, &sub, &input).await {
        Ok(Some(data)) => Ok(Json(ApiResponse::success(data))),
        Ok(None) => Err(error(StatusCode::CONFLICT)),
        Err(_) => Err(error(StatusCode::SERVICE_UNAVAILABLE)),
    }
}
