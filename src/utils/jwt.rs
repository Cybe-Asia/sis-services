use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    // For magic-link tokens this is a lead_id (`LEAD-*`).
    // For auth-service login tokens it is a User.id UUID.
    // Callers that need to distinguish inspect the prefix themselves.
    pub sub: String,
    pub exp: usize,
    // Optional — auth-service login tokens embed the parent email so
    // admission-service can authorise an admin without having to do
    // a graph lookup first. Magic-link tokens omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

pub fn generate_verification_token(id: &str) -> Result<String, String> {
    let secret = env::var("JWT_SECRET").unwrap_or_else(|_| "secret".to_string());
    let expiration = Utc::now()
        .checked_add_signed(Duration::hours(24))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = Claims {
        sub: id.to_owned(),
        exp: expiration,
        email: None,
        scope: None,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_ref()),
    )
    .map_err(|e| e.to_string())
}

pub fn decode_verification_token(token: &str) -> Result<String, String> {
    decode_full_claims(token).map(|c| c.sub)
}

/// Returns the embedded `email` claim, if any. Auth-service login
/// tokens carry it; magic-link tokens do not.
pub fn decode_verification_token_email(token: &str) -> Result<String, String> {
    let claims = decode_full_claims(token)?;
    claims
        .email
        .ok_or_else(|| "token has no email claim".to_string())
}

fn decode_full_claims(token: &str) -> Result<Claims, String> {
    let secret = env::var("JWT_SECRET").unwrap_or_else(|_| "secret".to_string());

    decode_parent_claims(token, &secret)
}
fn decode_parent_claims(token: &str, secret: &str) -> Result<Claims, String> {
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_ref()),
        &Validation::default(),
    )
    .map_err(|e| e.to_string())?;

    if token_data.claims.scope.is_some() {
        return Err("token scope mismatch".into());
    }
    Ok(token_data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parent_records_reject_setup_proofs() {
        let secret = "test-sis-scope";
        for scope in [None, Some("eoi_setup"), Some("password_setup")] {
            let claims = Claims {
                sub: "parent-id".into(),
                exp: usize::MAX / 2,
                email: Some("parent@example.test".into()),
                scope: scope.map(str::to_string),
            };
            let token = encode(
                &Header::default(),
                &claims,
                &EncodingKey::from_secret(secret.as_bytes()),
            )
            .unwrap();
            assert_eq!(
                decode_parent_claims(&token, secret).is_ok(),
                scope.is_none()
            );
        }
    }
}
