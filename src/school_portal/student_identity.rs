//! Auth owns Student credential lifecycle; SIS still owns current classroom rights.
use super::{auth::bearer, failure, model::identifier, Failure};
use axum::http::{HeaderMap, StatusCode};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;

const SESSION_PATH: &str = "/api/v1/auth-service/student/session";
const MAX_RESPONSE: usize = 4096;

struct SignedStudent {
    id: String,
    version: Option<i64>,
}

fn signed(token: &str, secret: &str) -> Result<SignedStudent, Failure> {
    if secret.len() < 32 {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    let claims = decode::<Value>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|_| failure(StatusCode::UNAUTHORIZED))?
    .claims;
    let id = claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|id| identifier(id))
        .ok_or_else(|| failure(StatusCode::UNAUTHORIZED))?;
    if claims.get("exp").and_then(Value::as_u64).is_none() || claims.get("scope").is_some() {
        return Err(failure(StatusCode::UNAUTHORIZED));
    }
    if claims.get("role").and_then(Value::as_str) != Some("student") {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let version = match claims.get("credentialVersion") {
        None => None,
        Some(value) => Some(
            value
                .as_i64()
                .filter(|v| *v > 0)
                .ok_or_else(|| failure(StatusCode::UNAUTHORIZED))?,
        ),
    };
    Ok(SignedStudent {
        id: id.into(),
        version,
    })
}

fn endpoint(raw: &str) -> Result<reqwest::Url, Failure> {
    let url = reqwest::Url::parse(raw).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != SESSION_PATH
    {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    Ok(url)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Session {
    active: bool,
    user_id: String,
    role: String,
    managed: bool,
    credential_version: Option<i64>,
}

fn current(claims: &SignedStudent, bytes: &[u8]) -> Result<(), Failure> {
    // The field is required even for legacy accounts, where Auth sends explicit null.
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if value.get("credentialVersion").is_none() {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    let session: Session =
        serde_json::from_value(value).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if !session.active || session.user_id != claims.id || session.role != "student" {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let valid = if session.managed {
        session.credential_version.is_some_and(|v| v > 0)
            && session.credential_version == claims.version
    } else {
        session.credential_version.is_none() && claims.version.is_none()
    };
    if !valid {
        return Err(failure(StatusCode::UNAUTHORIZED));
    }
    Ok(())
}

async fn verify(
    token: &str,
    claims: SignedStudent,
    url: reqwest::Url,
    client: reqwest::Client,
) -> Result<String, Failure> {
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let mut response = client
        .post(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| unavailable())?;
    match response.status().as_u16() {
        200 => {}
        401 => return Err(failure(StatusCode::UNAUTHORIZED)),
        403 => return Err(failure(StatusCode::FORBIDDEN)),
        _ => return Err(unavailable()),
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len() + chunk.len() > MAX_RESPONSE {
            return Err(unavailable());
        }
        bytes.extend_from_slice(&chunk);
    }
    current(&claims, &bytes)?;
    Ok(claims.id)
}

pub(crate) async fn actor(headers: &HeaderMap) -> Result<String, Failure> {
    let token = bearer(headers)?;
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let secret = std::env::var("JWT_SECRET").map_err(|_| unavailable())?;
    let claims = signed(token, &secret)?;
    // Separate from Staff membership/portal URLs. Missing configuration never falls back.
    let url = endpoint(&std::env::var("AUTH_STUDENT_SESSION_URL").map_err(|_| unavailable())?)?;
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5));
    if matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        builder = builder.no_proxy();
    }
    if let Ok(path) = std::env::var("AUTH_PORTAL_CA_FILE") {
        let pem = std::fs::read(path).map_err(|_| unavailable())?;
        if pem.len() > 65_536 {
            return Err(unavailable());
        }
        let ca = reqwest::Certificate::from_pem(&pem).map_err(|_| unavailable())?;
        builder = builder.add_root_certificate(ca);
    }
    verify(
        token,
        claims,
        url,
        builder.build().map_err(|_| unavailable())?,
    )
    .await
}

#[cfg(test)]
mod tests;
