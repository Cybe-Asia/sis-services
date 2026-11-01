use super::{
    super::{auth, failure, Failure},
    contract::{Export, Import},
};
use crate::config::config::AppConfig;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use std::sync::{Mutex, OnceLock};

// The private Learning trust anchor belongs only to this owner transport.
// Keep default public roots and hostname verification on both TLS backends.
fn client_builder(ca: Option<&[u8]>) -> Result<reqwest::ClientBuilder, Failure> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(5));
    if let Some(pem) = ca {
        for certificate in private_roots(pem)? {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder)
}
fn private_roots(pem: &[u8]) -> Result<Vec<reqwest::Certificate>, Failure> {
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    if pem.len() > 262_144 {
        return Err(unavailable());
    }
    let mut remaining = std::str::from_utf8(pem).map_err(|_| unavailable())?.trim();
    let mut roots = Vec::new();
    // Accept certificate PEM blocks only: no keys, ignored garbage or partial
    // bundles. A configured invalid file must never fall back to default trust.
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    while !remaining.is_empty() {
        if roots.len() == 32 || !remaining.starts_with(BEGIN) {
            return Err(unavailable());
        }
        let end = remaining.find(END).ok_or_else(unavailable)? + END.len();
        let mut parsed = reqwest::Certificate::from_pem_bundle(remaining[..end].as_bytes())
            .map_err(|_| unavailable())?;
        if parsed.len() != 1 {
            return Err(unavailable());
        }
        roots.push(parsed.remove(0));
        remaining = remaining[end..].trim();
    }
    if roots.is_empty() {
        return Err(unavailable());
    }
    Ok(roots)
}
/// One pooled client per trust anchor, as in `auth::auth_client`: config read the CA file at
/// startup, and the TLS roots are loaded when the client is first built, not on every grade read.
// shortcut: a rotated anchor applies after a restart; re-read on a timer if rotation must apply live.
fn client(ca: Option<&[u8]>) -> Result<reqwest::Client, Failure> {
    type Entry = (Option<Vec<u8>>, reqwest::Client);
    static CLIENTS: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let mut clients = CLIENTS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| unavailable())?;
    if let Some((_, client)) = clients.iter().find(|(pem, _)| pem.as_deref() == ca) {
        return Ok(client.clone());
    }
    let client = client_builder(ca)?.build().map_err(|_| unavailable())?;
    if clients.len() >= 8 {
        clients.clear();
    }
    clients.push((ca.map(<[u8]>::to_vec), client.clone()));
    Ok(client)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    data: Export,
}
pub async fn read(
    config: &AppConfig,
    headers: &HeaderMap,
    input: &Import,
) -> Result<Export, Failure> {
    let origin = config
        .learning_service_origin
        .as_deref()
        .ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut url =
        reqwest::Url::parse(origin).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let local_test = config.local_test
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
        && url.scheme() == "http";
    if (url.scheme() != "https" && !local_test)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    url.path_segments_mut()
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .clear()
        .extend([
            "api",
            "v1",
            "learning",
            "grade-exports",
            "courses",
            &input.course_id,
            "items",
            &input.assessment_id,
            "attempts",
            &input.attempt_id,
        ]);
    let client = client(config.learning_service_ca.as_deref())?;
    let mut response = client
        .get(url)
        .bearer_auth(auth::bearer(headers)?)
        .send()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if !response.status().is_success() {
        return Err(failure(match response.status().as_u16() {
            401 => StatusCode::UNAUTHORIZED,
            403 | 404 => StatusCode::FORBIDDEN,
            409 => StatusCode::CONFLICT,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        }));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        if bytes.len() + chunk.len() > 8192 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice::<Envelope>(&bytes)
        .map(|v| v.data)
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))
}

#[cfg(test)]
mod tls_tests;
