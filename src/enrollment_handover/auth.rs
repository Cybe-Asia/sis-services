//! Auth's existing staff-api-v2 credential family, with live directory scope in Cypher.
use super::model::identifier;
use axum::http::HeaderMap;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct Actor {
    pub subject: String,
    pub staff: String,
    pub expires: i64,
}
#[derive(Deserialize)]
struct Claims {
    sub: String,
    staff_member_id: String,
    scope: String,
    iat: u64,
    exp: u64,
}
#[derive(Debug)]
pub enum Error {
    Unavailable,
    Unauthorized,
}
pub fn actor(headers: &HeaderMap, parent_key: &str) -> Result<Actor, Error> {
    let key = std::env::var("STAFF_DOWNSTREAM_JWT_SECRET").map_err(|_| Error::Unavailable)?;
    let issuer = std::env::var("STAFF_DOWNSTREAM_JWT_ISSUER").map_err(|_| Error::Unavailable)?;
    if key.len() < 32
        || key == parent_key
        || !issuer.starts_with("https://")
        || issuer.contains(['?', '#'])
    {
        return Err(Error::Unavailable);
    }
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty() && v.len() <= 8192)
        .ok_or(Error::Unauthorized)?;
    verify(token, &key, &issuer)
}
fn verify(token: &str, key: &str, issuer: &str) -> Result<Actor, Error> {
    if decode_header(token)
        .map_err(|_| Error::Unauthorized)?
        .typ
        .as_deref()
        != Some("staff-api+jwt")
    {
        return Err(Error::Unauthorized);
    }
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_audience(&["digital-school-admin-api"]);
    validation.set_issuer(&[issuer]);
    validation.leeway = 0;
    let c = decode::<Claims>(
        token,
        &DecodingKey::from_secret(key.as_bytes()),
        &validation,
    )
    .map_err(|_| Error::Unauthorized)?
    .claims;
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    if !identifier(&c.sub)
        || !identifier(&c.staff_member_id)
        || c.scope != "staff_downstream"
        || c.iat > now
        || c.exp <= c.iat
        || c.exp - c.iat > 900
    {
        return Err(Error::Unauthorized);
    }
    Ok(Actor {
        subject: c.sub,
        staff: c.staff_member_id,
        expires: i64::try_from(c.exp).map_err(|_| Error::Unauthorized)?,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn staff_type_signature_scope_and_lifetime_are_required() {
        let key = "synthetic-handover-staff-key-at-least-32-bytes";
        let issuer = "https://auth.example.test/staff-api";
        let now = chrono::Utc::now().timestamp();
        let good = serde_json::json!({"sub":"user","staff_member_id":"staff","iss":issuer,"aud":"digital-school-admin-api","scope":"staff_downstream","iat":now,"exp":now+300});
        let mut header = jsonwebtoken::Header::default();
        header.typ = Some("staff-api+jwt".into());
        let issue = |v: &serde_json::Value, h: &jsonwebtoken::Header| {
            jsonwebtoken::encode(
                h,
                v,
                &jsonwebtoken::EncodingKey::from_secret(key.as_bytes()),
            )
            .unwrap()
        };
        assert!(verify(&issue(&good, &header), key, issuer).is_ok());
        assert!(verify(&issue(&good, &jsonwebtoken::Header::default()), key, issuer).is_err());
        for (field, value) in [
            ("scope", serde_json::json!("parent")),
            ("exp", serde_json::json!(now - 1)),
            ("exp", serde_json::json!(now + 901)),
            ("aud", serde_json::json!("learning-api")),
            ("staff_member_id", serde_json::json!("")),
        ] {
            let mut bad = good.clone();
            bad[field] = value;
            assert!(verify(&issue(&bad, &header), key, issuer).is_err());
        }
    }
}
