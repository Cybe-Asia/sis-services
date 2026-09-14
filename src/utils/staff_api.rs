use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::Deserialize;

pub const STAFF_API_AUDIENCE: &str = "digital-school-admin-api";
pub const STAFF_API_PURPOSE: &str = "staff_downstream";

#[derive(Debug, Deserialize)]
pub struct StaffApiClaims {
    pub sub: String,
    pub staff_member_id: String,
    pub scope: String,
    pub iat: u64,
    pub exp: u64,
}

pub fn decode_staff_api_claims(
    token: &str,
    secret: &str,
    issuer: &str,
) -> Result<StaffApiClaims, String> {
    let header = jsonwebtoken::decode_header(token).map_err(|_| "Invalid staff token")?;
    if header.typ.as_deref() != Some("staff-api+jwt")
        || secret.len() < 32
        || !issuer.starts_with("https://")
    {
        return Err("Invalid staff credential configuration or type".into());
    }
    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.set_audience(&[STAFF_API_AUDIENCE]);
    validation.set_issuer(&[issuer]);
    validation.leeway = 0;
    let claims = decode::<StaffApiClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|_| "Invalid staff token")?
    .claims;
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    if claims.scope != STAFF_API_PURPOSE
        || claims.sub.is_empty()
        || claims.staff_member_id.is_empty()
        || claims.iat > now
        || claims.exp <= claims.iat
        || claims.exp - claims.iat > 900
    {
        return Err("Invalid staff token claims".into());
    }
    Ok(claims)
}
