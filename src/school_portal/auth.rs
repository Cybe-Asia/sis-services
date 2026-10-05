use super::{model::identifier, Failure};
use axum::http::{HeaderMap, StatusCode};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::Deserialize;

pub fn bearer(headers: &HeaderMap) -> Result<&str, Failure> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty() && v.len() <= 8192)
        .ok_or_else(|| super::failure(StatusCode::UNAUTHORIZED))
}
#[derive(Deserialize)]
struct ParentClaims {
    sub: String,
    exp: usize,
    scope: Option<String>,
}
pub fn parent(headers: &HeaderMap) -> Result<String, Failure> {
    let key =
        std::env::var("JWT_SECRET").map_err(|_| super::failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if key.len() < 32 {
        return Err(super::failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    let claims = decode::<ParentClaims>(
        bearer(headers)?,
        &DecodingKey::from_secret(key.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| super::failure(StatusCode::UNAUTHORIZED))?
    .claims;
    let _ = claims.exp;
    if claims.scope.is_some() || !identifier(&claims.sub) {
        return Err(super::failure(StatusCode::UNAUTHORIZED));
    }
    Ok(claims.sub)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Teacher {
    pub active: bool,
    pub staff_member_id: String,
    pub roles: Vec<String>,
    pub school_ids: Vec<String>,
    pub tenant_ids: Vec<String>,
}
impl Teacher {
    pub fn is_owner(&self) -> bool {
        self.roles.iter().any(|r| r == "owner")
    }
    pub fn role(&self) -> &'static str {
        if self.is_owner() {
            "owner"
        } else {
            "teacher"
        }
    }
    pub fn valid(&self) -> bool {
        self.active
            && identifier(&self.staff_member_id)
            && (self.is_owner() || self.roles.iter().any(|r| r == "teacher"))
            && [&self.school_ids, &self.tenant_ids]
                .iter()
                .all(|ids| ids.len() <= 64 && ids.iter().all(|id| identifier(id)))
            && ((self.is_owner() && self.school_ids.is_empty() && self.tenant_ids.is_empty())
                || (!self.school_ids.is_empty() && !self.tenant_ids.is_empty()))
    }
    pub fn select(
        mut self,
        selection: &super::staff_capability::Selection,
    ) -> Result<Self, Failure> {
        let selected = selection
            .pair()
            .map_err(|_| super::failure(StatusCode::BAD_REQUEST))?;
        if self.is_owner() && selected.is_none() {
            return Err(super::failure(StatusCode::BAD_REQUEST));
        }
        if let Some((school, tenant)) = selected {
            if !super::staff_capability::live_scope(
                self.is_owner(),
                &self.school_ids,
                &self.tenant_ids,
                school,
                tenant,
            ) {
                return Err(super::failure(StatusCode::FORBIDDEN));
            }
            self.school_ids = vec![school.into()];
            self.tenant_ids = vec![tenant.into()];
        }
        Ok(self)
    }
}
pub async fn teacher_in(
    headers: &HeaderMap,
    school: &str,
    tenant: &str,
) -> Result<Teacher, Failure> {
    teacher(headers)
        .await?
        .select(&super::staff_capability::Selection {
            school_id: Some(school.into()),
            tenant_id: Some(tenant.into()),
        })
}
pub async fn teacher(headers: &HeaderMap) -> Result<Teacher, Failure> {
    let token = bearer(headers)?;
    // Audience prefilter grants no authority: Auth checks signature, exact registration and live session.
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let audience = token
        .split('.')
        .nth(1)
        .and_then(|p| URL_SAFE_NO_PAD.decode(p).ok())
        .and_then(|p| serde_json::from_slice::<serde_json::Value>(&p).ok());
    if audience
        .as_ref()
        .and_then(|v| v.get("aud"))
        .and_then(|v| v.as_str())
        != Some("learning-api")
    {
        return Err(super::failure(StatusCode::UNAUTHORIZED));
    }
    let endpoint = std::env::var("AUTH_SCHOOL_MEMBERSHIP_URL")
        .map_err(|_| super::failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let url = reqwest::Url::parse(&endpoint)
        .map_err(|_| super::failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    let local_test =
        std::env::var("APP_ENV").as_deref() == Ok("test") && loopback && url.scheme() == "http";
    if (url.scheme() != "https" && !local_test)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/api/v1/auth-service/oauth/membership"
    {
        return Err(super::failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5));
    if let Ok(path) = std::env::var("AUTH_PORTAL_CA_FILE") {
        let metadata = std::fs::metadata(&path)
            .map_err(|_| super::failure(StatusCode::SERVICE_UNAVAILABLE))?;
        if !metadata.is_file() || metadata.len() > 65_536 {
            return Err(super::failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        let pem =
            std::fs::read(path).map_err(|_| super::failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let certificate = reqwest::Certificate::from_pem(&pem)
            .map_err(|_| super::failure(StatusCode::SERVICE_UNAVAILABLE))?;
        builder = builder.add_root_certificate(certificate);
    }
    let client = builder
        .build()
        .map_err(|_| super::failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut response = client
        .post(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| super::failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if response.status().as_u16() == 401 {
        return Err(super::failure(StatusCode::UNAUTHORIZED));
    }
    if response.status().as_u16() == 403 {
        return Err(super::failure(StatusCode::FORBIDDEN));
    }
    if !response.status().is_success() {
        return Err(super::failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| super::failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        if bytes.len() + chunk.len() > 16_384 {
            return Err(super::failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        bytes.extend_from_slice(&chunk);
    }
    let member: Teacher = serde_json::from_slice(&bytes)
        .map_err(|_| super::failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if !member.valid() {
        return Err(super::failure(StatusCode::FORBIDDEN));
    }
    Ok(member)
}
