use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

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

/// `secret` is `AppConfig::jwt_secret`; a local fallback once disagreed with the config value.
pub fn decode_verification_token(token: &str, secret: &str) -> Result<String, String> {
    decode_parent_claims(token, secret).map(|c| c.sub)
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
    use jsonwebtoken::{encode, EncodingKey, Header};
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
