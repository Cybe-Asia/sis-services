use super::{
    super::{auth, failure, Failure},
    contract::{Export, Import},
};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    data: Export,
}
pub async fn read(headers: &HeaderMap, input: &Import) -> Result<Export, Failure> {
    let origin = std::env::var("LEARNING_SERVICE_ORIGIN")
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut url =
        reqwest::Url::parse(&origin).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let local_test = std::env::var("APP_ENV").as_deref() == Ok("test")
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
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
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
