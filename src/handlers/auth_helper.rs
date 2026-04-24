// Shared auth helpers. Functionally identical to
// admission-services::handlers::lead_handler::{require_admin,
// resolve_lead_id_from_bearer} — duplicated (not shared) because
// the two services may evolve auth independently (e.g. SIS adopts
// teacher-role auth before admissions does).
//
// Both services verify JWTs issued by auth-services using the
// shared JWT_SECRET env var. admin privilege is controlled by the
// same ADMIN_EMAILS env allowlist, which must be set identically
// on both services' pods.

use axum::http::{HeaderMap, StatusCode};

use crate::repositories::lead_repository;
use crate::utils::jwt::{decode_verification_token, decode_verification_token_email};
use crate::AppState;

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

/// Bearer auth + admin-email allowlist. Returns the admin's own
/// lead_id (empty string for admins with no Lead yet) on success;
/// 403 if the session's email isn't in ADMIN_EMAILS.
///
/// `ADMIN_EMAILS` is a comma-separated env var. Empty/unset = "no
/// admins configured" — every admin call returns 403. That's the
/// safe default for a fresh cluster.
pub async fn require_admin(
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

    let (lead_id, email) = if sub.starts_with("LEAD-") {
        let email = lead_repository::find_email_by_lead_id(&state.graph, &sub)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or((
                StatusCode::UNAUTHORIZED,
                "Lead not found for session".to_string(),
            ))?;
        (sub, email)
    } else {
        // UUID sub from auth-services login token. Email comes from
        // the JWT claims directly so admin access doesn't require
        // the admin to have a Lead at all — some admins will never
        // submit an EOI.
        let email = decode_verification_token_email(&token)
            .map_err(|_| (StatusCode::UNAUTHORIZED, "Token missing email claim".to_string()))?;
        let lead_id =
            match lead_repository::resolve_primary_lead_for_user(&state.graph, &sub).await {
                Ok(Some(lid)) => lid,
                _ => String::new(),
            };
        (lead_id, email)
    };

    let allowed = std::env::var("ADMIN_EMAILS").unwrap_or_default();
    let requester_email = email.to_lowercase();
    let is_admin = allowed
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .any(|e| !e.is_empty() && e == requester_email);

    if !is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            "Admin access required".to_string(),
        ));
    }
    Ok(lead_id)
}

fn bearer_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
