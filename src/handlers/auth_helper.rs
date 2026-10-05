//! Parent compatibility and the typed Staff v2 boundary for academic Admin APIs.
pub(crate) use crate::enrollment_handover::auth::Actor as AdminActor;
use crate::repositories::lead_repository;
use crate::utils::jwt::decode_verification_token;
use crate::AppState;
use axum::http::{HeaderMap, StatusCode};

/// Resolve a Lead id from the `Authorization: Bearer …` header.
/// Handles both JWT subject formats:
///   - `sub = "LEAD-…"` — issued by magic-link emails
///   - `sub = <UUID>`    — issued by auth-services password login
///
/// For UUID subs, walks (User)-[:HAS_APPLICATION]->(Lead) to pick
/// the newest Lead. Returns 401 on missing/invalid token, 404 when
/// a password-login user has no Lead yet.
pub async fn resolve_lead_id_from_bearer(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, String)> {
    let Some(token) = bearer_from(headers) else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Missing or invalid Authorization header".to_string(),
        ));
    };
    let sub = decode_verification_token(&token)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid or expired token".to_string()))?;

    if sub.starts_with("LEAD-") {
        return Ok(sub);
    }

    match lead_repository::resolve_primary_lead_for_user(&state.graph, &sub).await {
        Ok(Some(lid)) => Ok(lid),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            "No lead found for this user".to_string(),
        )),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// Only Auth's typed downstream staff credential is accepted. Email, Lead and
/// token role claims do not grant academic authority; each repository operation
/// rechecks the live canonical directory under its transaction locks.
pub async fn require_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<AdminActor, (StatusCode, String)> {
    let actor = crate::enrollment_handover::auth::actor(headers, &state.config.jwt_secret)
        .map_err(|e| match e {
            crate::enrollment_handover::auth::Error::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Staff authentication unavailable".into(),
            ),
            crate::enrollment_handover::auth::Error::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "Invalid or expired staff credential".into(),
            ),
        })?;
    crate::repositories::sis_repository::check_admin(&state.graph, &actor)
        .await
        .map_err(|e| match e {
            lead_repository::RepositoryError::DbError(ref code) if code == "FORBIDDEN" => (
                StatusCode::FORBIDDEN,
                "Academic admin access required".into(),
            ),
            _ => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Academic service unavailable".into(),
            ),
        })?;
    Ok(actor)
}

fn bearer_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
