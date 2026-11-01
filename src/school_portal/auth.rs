use super::{model::identifier, Failure};
use crate::config::config::AppConfig;
use axum::http::{HeaderMap, StatusCode};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::Deserialize;
use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};

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
pub fn parent(config: &AppConfig, headers: &HeaderMap) -> Result<String, Failure> {
    let key = &config.jwt_secret;
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
    config: &AppConfig,
    headers: &HeaderMap,
    school: &str,
    tenant: &str,
) -> Result<Teacher, Failure> {
    teacher(config, headers)
        .await?
        .select(&super::staff_capability::Selection {
            school_id: Some(school.into()),
            tenant_id: Some(tenant.into()),
        })
}
/// One pooled client per trust anchor for calls to Auth. A client built per request loaded the
/// TLS roots and opened a new TLS connection each time: about 100 ms of SIS CPU and a full
/// handshake on Auth's 250m TLS proxy for every Learning read, so concurrent reads queued into
/// 503s (QA B5, 2026-10-23). Keyed by the `AUTH_PORTAL_CA_FILE` anchor, which config read at
/// startup, so no file is read on the request path.
// shortcut: a rotated anchor applies after a restart; re-read on a timer if rotation must apply live.
pub(crate) fn auth_client(config: &AppConfig, no_proxy: bool) -> Result<reqwest::Client, Failure> {
    type Entry = (Option<Vec<u8>>, bool, reqwest::Client);
    static CLIENTS: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    let unavailable = || super::failure(StatusCode::SERVICE_UNAVAILABLE);
    let anchor = config.auth_portal_ca.as_deref();
    let mut clients = CLIENTS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| unavailable())?;
    if let Some((_, _, client)) = clients
        .iter()
        .find(|(pem, proxy, _)| pem.as_deref() == anchor && *proxy == no_proxy)
    {
        return Ok(client.clone());
    }
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        // Shorter than nginx's 75 s keepalive_timeout on Auth's TLS proxy, so a pooled
        // connection is never one the proxy has already closed.
        .pool_idle_timeout(Duration::from_secs(30));
    if no_proxy {
        builder = builder.no_proxy();
    }
    if let Some(pem) = anchor {
        if pem.len() > 65_536 {
            return Err(unavailable());
        }
        builder = builder
            .add_root_certificate(reqwest::Certificate::from_pem(pem).map_err(|_| unavailable())?);
    }
    let client = builder.build().map_err(|_| unavailable())?;
    // Old anchors after rotations never accumulate.
    if clients.len() >= 8 {
        clients.clear();
    }
    clients.push((anchor.map(<[u8]>::to_vec), no_proxy, client.clone()));
    Ok(client)
}
pub async fn teacher(config: &AppConfig, headers: &HeaderMap) -> Result<Teacher, Failure> {
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
    let endpoint = config
        .auth_school_membership_url
        .as_deref()
        .ok_or_else(|| super::failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let url = reqwest::Url::parse(endpoint)
        .map_err(|_| super::failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    let local_test = config.local_test && loopback && url.scheme() == "http";
    if (url.scheme() != "https" && !local_test)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/api/v1/auth-service/oauth/membership"
    {
        return Err(super::failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    let client = auth_client(config, false)?;
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
#[cfg(test)]
mod client_tests {
    use super::{auth_client, AppConfig};
    use std::{
        io::{BufRead, BufReader, Write},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    /// Answers every request with 200 on a kept-alive connection and counts connections.
    fn server() -> (String, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let connections = Arc::new(AtomicUsize::new(0));
        let count = connections.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                count.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        if line == "\r\n" {
                            let reply = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok";
                            if stream.write_all(reply.as_bytes()).is_err() {
                                return;
                            }
                        }
                    }
                });
            }
        });
        (origin, connections)
    }

    #[tokio::test]
    async fn auth_calls_share_one_pooled_connection() {
        let (origin, connections) = server();
        for _ in 0..3 {
            let client = auth_client(&AppConfig::default(), true).unwrap();
            let response = client
                .post(format!("{origin}/session"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.text().await.unwrap(), "ok");
        }
        assert_eq!(connections.load(Ordering::SeqCst), 1);
    }
}
