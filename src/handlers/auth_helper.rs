// Shared auth helpers. Functionally identical to
// admission-services::handlers::lead_handler::{require_admin,
// resolve_lead_id_from_bearer} — duplicated (not shared) because
// the two services may evolve auth independently (e.g. SIS adopts
// teacher-role auth before admissions does).
//
// Both services verify JWTs issued by auth-services using the shared
// JWT_SECRET env var. SIS privilege comes from active canonical staff roles.

use axum::http::{HeaderMap, StatusCode};
use neo4rs::query;

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
    let sub = decode_verification_token(&token).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            "Invalid or expired token".to_string(),
        )
    })?;

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

/// Bearer auth + active canonical SIS administrator role. Returns the
/// administrator's own lead_id (empty when no Lead exists) on success.
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
    let sub = decode_verification_token(&token).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            "Invalid or expired token".to_string(),
        )
    })?;

    let (lead_id, email) = if sub.starts_with("LEAD-") {
        let email = lead_repository::find_email_by_lead_id(&state.graph, &sub)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or((
                StatusCode::UNAUTHORIZED,
                "Lead not found for session".to_string(),
            ))?;
        (sub.clone(), email)
    } else {
        // UUID sub from auth-services login token. Email comes from
        // the JWT claims directly so admin access doesn't require
        // the admin to have a Lead at all — some admins will never
        // submit an EOI.
        let email = decode_verification_token_email(&token).map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "Token missing email claim".to_string(),
            )
        })?;
        let lead_id = match lead_repository::resolve_primary_lead_for_user(&state.graph, &sub).await
        {
            Ok(Some(lid)) => lid,
            _ => String::new(),
        };
        (lead_id, email)
    };

    let roles = active_staff_roles(&state.graph, &sub).await?;
    let is_admin = sis_admin_roles_allowed(&roles)
        || nonprod_admin_email_fallback_enabled() && is_admin_email(&email);

    if !is_admin {
        return Err((StatusCode::FORBIDDEN, "Admin access required".to_string()));
    }
    Ok(lead_id)
}

fn sis_admin_roles_allowed(roles: &[String]) -> bool {
    // SIS repositories do not yet accept canonical school scope on every
    // mutation. Until that contract lands, only the global owner may use the
    // unscoped legacy admin surface; school_admin fails closed instead of
    // silently becoming cross-school.
    roles.iter().any(|role| role == "owner")
}

async fn active_staff_roles(
    graph: &neo4rs::Graph,
    subject: &str,
) -> Result<Vec<String>, (StatusCode, String)> {
    let mut result = graph
        .execute(
            query(
                "MATCH (u:User {id:$subject}) \
                 OPTIONAL MATCH (u)-[:STAFF_MEMBER]->(member:StaffMember) \
                 OPTIONAL MATCH (u)-[:STAFF_PROFILE]->(profile:StaffProfile) \
                 RETURN CASE \
                   WHEN member IS NOT NULL THEN CASE WHEN member.membershipStatus='ACTIVE' THEN coalesce(member.roles,[]) ELSE [] END \
                   WHEN profile IS NOT NULL THEN CASE WHEN toLower(profile.status)='active' THEN coalesce(profile.roles,u.roles,[]) ELSE [] END \
                   WHEN toLower(coalesce(u.staffStatus,''))='active' THEN coalesce(u.roles,[]) \
                   ELSE [] END AS roles LIMIT 1",
            )
            .param("subject", subject.to_string()),
        )
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Staff authorization unavailable".to_string(),
            )
        })?;
    Ok(result
        .next()
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Staff authorization unavailable".to_string(),
            )
        })?
        .and_then(|row| row.get::<Vec<String>>("roles").ok())
        .unwrap_or_default())
}

fn nonprod_admin_email_fallback_enabled() -> bool {
    let enabled = std::env::var("SIS_NONPROD_ADMIN_EMAIL_FALLBACK_ENABLED")
        .ok()
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or(false);
    enabled
        && matches!(
            std::env::var("APP_ENV")
                .unwrap_or_else(|_| "local".to_string())
                .as_str(),
            "local" | "dev" | "test"
        )
}

fn is_admin_email(email: &str) -> bool {
    let requester = email.trim().to_ascii_lowercase();
    std::env::var("ADMIN_EMAILS")
        .unwrap_or_default()
        .split(',')
        .map(|value| value.trim().to_ascii_lowercase())
        .any(|allowed| !allowed.is_empty() && allowed == requester)
}

fn bearer_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::sis_admin_roles_allowed;

    fn roles(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn sis_administration_requires_an_explicit_role() {
        assert!(sis_admin_roles_allowed(&roles(&["owner"])));
        assert!(!sis_admin_roles_allowed(&roles(&["school_admin"])));
        assert!(!sis_admin_roles_allowed(&roles(&["teacher"])));
        assert!(!sis_admin_roles_allowed(&roles(&["admissions_admin"])));
        assert!(!sis_admin_roles_allowed(&roles(&[])));
    }
}
