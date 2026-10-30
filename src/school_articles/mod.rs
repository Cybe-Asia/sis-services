//! Articles and webinars the school publishes for families. Staff holding `school.content.manage`
//! at the school (its administrators and Owners on day one) write them;
//! the school portal lets a Parent read the published ones and counts who opened them.
pub mod cover;
pub mod model;
pub mod repository;
use crate::AppState;
use axum::{
    body::Bytes,
    extract::{
        rejection::{BytesRejection, JsonRejection},
        Path, Query, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
type Failure = (StatusCode, Json<Value>);
fn failure(status: StatusCode) -> Failure {
    (
        status,
        Json(
            json!({"error":{"code":match status {StatusCode::BAD_REQUEST=>"INVALID_INPUT",StatusCode::FORBIDDEN=>"FORBIDDEN",StatusCode::UNAUTHORIZED=>"UNAUTHORIZED",StatusCode::CONFLICT=>"REVISION_CONFLICT",StatusCode::NOT_FOUND=>"NOT_FOUND",StatusCode::PAYLOAD_TOO_LARGE=>"TOO_LARGE",_=>"DEPENDENCY_UNAVAILABLE"}}}),
        ),
    )
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub tenant_id: String,
    pub school_id: String,
    pub after: Option<String>,
    pub id: Option<String>,
}
/// `school.content.manage` at the school (ADR 0018 slice 6): today's administrators and Owners hold
/// it, and so does anyone the school assigned it. The graph statement checks the caller's node again.
const CONTENT_MANAGE: &str = "school.content.manage";
async fn manager(
    s: &AppState,
    h: &HeaderMap,
    tenant: &String,
    school: &String,
) -> Result<String, Failure> {
    let (actor, _) = crate::learning_access::school_staff_actor(h, tenant, school).await?;
    if crate::authorization::reads::permits(
        &mut crate::authorization::facts::Db::Graph(&s.graph),
        tenant,
        school,
        &actor,
        CONTENT_MANAGE,
    )
    .await
    .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        Ok(actor)
    } else {
        Err(failure(StatusCode::FORBIDDEN))
    }
}
fn capable(id: &str) -> repository::Actor<'_> {
    repository::Actor { id, capable: true }
}
async fn list(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<Scope>,
) -> Result<Json<Value>, Failure> {
    use crate::school_portal::model::identifier;
    if !identifier(&q.school_id)
        || !identifier(&q.tenant_id)
        || q.id.as_deref().is_some_and(|id| !identifier(id))
        || q.after.as_deref().is_some_and(|a| !identifier(a))
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let actor = manager(&s, &h, &q.tenant_id, &q.school_id).await?;
    repository::list(&s.graph, capable(&actor), &q)
        .await
        .map(|data| Json(json!({"data":data})))
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))
}
async fn change(
    State(s): State<AppState>,
    h: HeaderMap,
    j: Result<Json<model::Change>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(v) = j.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !v.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let actor = manager(&s, &h, &v.entry.tenant_id, &v.entry.school_id).await?;
    match repository::change(&s.graph, capable(&actor), &v).await {
        Ok(Some(version)) => Ok(Json(json!({"data":{"version":version}}))),
        Ok(None) => Err(failure(StatusCode::CONFLICT)),
        Err(_) => Err(failure(StatusCode::SERVICE_UNAVAILABLE)),
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CoverScope {
    tenant_id: String,
    school_id: String,
    cover_id: Option<String>,
}
impl CoverScope {
    fn valid(&self) -> bool {
        use crate::school_portal::model::identifier;
        identifier(&self.tenant_id)
            && identifier(&self.school_id)
            && self.cover_id.as_deref().is_none_or(identifier)
    }
}
/// A stored cover is always the JPEG this service wrote; the browser may keep it for the session.
fn photograph(bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg")),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=3600"),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        bytes,
    )
        .into_response()
}
fn found(result: Result<Option<Vec<u8>>, repository::Error>) -> Result<Response, Failure> {
    match result {
        Ok(Some(bytes)) => Ok(photograph(bytes)),
        Ok(None) => Err(failure(StatusCode::NOT_FOUND)),
        Err(_) => Err(failure(StatusCode::SERVICE_UNAVAILABLE)),
    }
}
/// The request body is the picture itself (JPEG, PNG or WebP). It is stored re-encoded, never as sent.
async fn upload_cover(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<CoverScope>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Value>, Failure> {
    let kind = h
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !q.valid() || q.cover_id.is_some() || !cover::UPLOAD_TYPES.contains(&kind) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let source = body.map_err(|_| failure(StatusCode::PAYLOAD_TOO_LARGE))?;
    let actor = manager(&s, &h, &q.tenant_id, &q.school_id).await?;
    let store = cover::store().ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let stored = tokio::task::spawn_blocking(move || cover::normalize(&source))
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST))?;
    match repository::add_cover(
        &s.graph,
        store,
        capable(&actor),
        &q.tenant_id,
        &q.school_id,
        stored,
    )
    .await
    {
        Ok(Some(id)) => Ok(Json(json!({"data":{"coverId":id}}))),
        Ok(None) => Err(failure(StatusCode::FORBIDDEN)),
        Err(_) => Err(failure(StatusCode::SERVICE_UNAVAILABLE)),
    }
}
async fn admin_cover(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<CoverScope>,
) -> Result<Response, Failure> {
    let Some(id) = q.cover_id.as_deref().filter(|_| q.valid()) else {
        return Err(failure(StatusCode::BAD_REQUEST));
    };
    let actor = manager(&s, &h, &q.tenant_id, &q.school_id).await?;
    let store = cover::store().ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    found(
        repository::admin_cover(
            &s.graph,
            store,
            capable(&actor),
            &q.tenant_id,
            &q.school_id,
            id,
        )
        .await,
    )
}
/// The cover of a published article, for a Parent whose child's class received the article.
async fn parent_cover(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Failure> {
    let sub = crate::school_portal::auth::parent(&h)?;
    if !crate::school_portal::model::identifier(&id) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let store = cover::store().ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    found(crate::school_portal::repository::article_cover(&s.graph, store, &sub, &id).await)
}
pub fn router() -> Router<AppState> {
    let records = Router::new()
        .route("/api/v1/sis-service/school-articles", get(list).post(change))
        .layer(axum::extract::DefaultBodyLimit::max(65_536))
        .layer(axum::middleware::map_response(
            |mut r: axum::response::Response| async move {
                r.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                r
            },
        ));
    // Photographs answer with their own cache rule; an error among them is never kept.
    let covers = Router::new()
        .route(
            "/api/v1/sis-service/school-articles/cover",
            get(admin_cover).post(upload_cover),
        )
        .route(
            "/api/leads/v1/me/school/articles/:id/cover",
            get(parent_cover),
        )
        .layer(axum::extract::DefaultBodyLimit::max(cover::UPLOAD_MAX_BYTES))
        .layer(axum::middleware::map_response(
            |mut r: axum::response::Response| async move {
                if !r.status().is_success() {
                    r.headers_mut().insert(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static("no-store"),
                    );
                }
                r
            },
        ));
    records.merge(covers)
}
