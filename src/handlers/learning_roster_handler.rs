//! Minimal SIS roster contract consumed by LMS; excludes parent contact data.
use crate::{repositories::sis_repository, AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
#[derive(Deserialize)]
struct Claims {
    sub: String,
    operation: String,
    exp: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Member {
    enrolled_student_id: String,
    full_name: String,
}
pub async fn members(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(section): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let secret = std::env::var("LMS_ADMISSIONS_SECRET")
        .ok()
        .filter(|s| s.len() >= 32)
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    validate_token(token, &secret, &section)?;
    let section_record = sis_repository::find_section_by_id(&state.graph, &section)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if section_record.tenantId != state.config.tenant_id {
        return Err(StatusCode::FORBIDDEN);
    }
    if section_record.status != "active" {
        return Err(StatusCode::CONFLICT);
    }
    let members: Vec<Member> = sis_repository::list_section_members(&state.graph, &section)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .into_iter()
        .map(|r| Member {
            enrolled_student_id: r.enrolledStudentId,
            full_name: r.fullName,
        })
        .collect();
    Ok(Json(serde_json::json!({"data":{"members":members}})))
}

fn validate_token(token: &str, secret: &str, section: &str) -> Result<(), StatusCode> {
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.set_audience(&["sis-roster"]);
    validation.leeway = 0;
    let c = jsonwebtoken::decode::<Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?
    .claims;
    if c.sub != section
        || c.operation != "roster"
        || c.exp > chrono::Utc::now().timestamp().max(0) as u64 + 120
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roster_token_cannot_switch_section_or_purpose() {
        let key = "fixture-roster-key-with-at-least-32-characters";
        let mut claims = serde_json::json!({"sub":"section-a","aud":"sis-roster","operation":"roster","exp":chrono::Utc::now().timestamp()+60});
        let sign = |v: &serde_json::Value| {
            jsonwebtoken::encode(
                &jsonwebtoken::Header::default(),
                v,
                &jsonwebtoken::EncodingKey::from_secret(key.as_bytes()),
            )
            .unwrap()
        };
        assert!(validate_token(&sign(&claims), key, "section-a").is_ok());
        assert!(validate_token(&sign(&claims), key, "section-b").is_err());
        assert!(validate_token(&sign(&claims), "wrong-key", "section-a").is_err());
        for (field, value) in [
            ("aud", serde_json::json!("lms-onboarding")),
            ("operation", serde_json::json!("readiness")),
            (
                "exp",
                serde_json::json!(chrono::Utc::now().timestamp() + 3600),
            ),
            (
                "exp",
                serde_json::json!(chrono::Utc::now().timestamp() - 60),
            ),
        ] {
            let original = claims[field].clone();
            claims[field] = value;
            assert!(validate_token(&sign(&claims), key, "section-a").is_err());
            claims[field] = original;
        }
    }
}
