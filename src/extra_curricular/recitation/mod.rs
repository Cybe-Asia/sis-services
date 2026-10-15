//! Recitation submissions of a tahfiz activity (digital-school ADR 0009). After the AI check the
//! Student screen sends the take and the verdict here; one submission per verse, the newest
//! replacing the previous. The coach lists the roster's submissions, plays a clip and grades it
//! against the activity's criteria. Clips live in the shared object store and are removed after
//! the retention period; the graph keeps the grade history.
pub mod model;
pub mod repository;
use crate::extra_curricular::auth;
use crate::AppState;
use axum::{
    body::Body,
    extract::{rejection::JsonRejection, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use repository::Outcome;
use serde_json::{json, Value};
type Failure = (StatusCode, Json<Value>);
fn failure(s: StatusCode) -> Failure {
    (
        s,
        Json(
            json!({"error":{"code":match s{StatusCode::BAD_REQUEST=>"INVALID_INPUT",StatusCode::UNAUTHORIZED=>"UNAUTHORIZED",StatusCode::FORBIDDEN=>"FORBIDDEN",StatusCode::CONFLICT=>"REVISION_CONFLICT",StatusCode::NOT_FOUND=>"NOT_FOUND",StatusCode::PAYLOAD_TOO_LARGE=>"TOO_LARGE",_=>"DEPENDENCY_UNAVAILABLE"}}}),
        ),
    )
}
fn store() -> Result<&'static crate::school_articles::cover::Store, Failure> {
    crate::school_articles::cover::store().ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))
}
fn done(outcome: Result<Outcome, crate::extra_curricular::repository::Error>) -> Result<Json<Value>, Failure> {
    match outcome {
        Ok(Outcome::Done(data)) => Ok(Json(json!({"data":data}))),
        Ok(Outcome::Forbidden) => Err(failure(StatusCode::FORBIDDEN)),
        Ok(Outcome::Conflict) => Err(failure(StatusCode::CONFLICT)),
        Ok(Outcome::Missing) => Err(failure(StatusCode::NOT_FOUND)),
        Err(error) => {
            tracing::warn!(%error, "recitation request failed");
            Err(failure(StatusCode::SERVICE_UNAVAILABLE))
        }
    }
}
async fn list(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(role): Path<String>,
    Query(q): Query<model::Query>,
) -> Result<Json<Value>, Failure> {
    if !matches!(role.as_str(), "student" | "teacher") || !q.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let actor = auth::actor(&h, &role, &q.scope()).await?;
    match repository::list(&s.graph, &actor, &q).await {
        Ok(Some(data)) => Ok(Json(json!({"data":data}))),
        Ok(None) => Err(failure(StatusCode::FORBIDDEN)),
        Err(error) => {
            tracing::warn!(%error, "recitation list failed");
            Err(failure(StatusCode::SERVICE_UNAVAILABLE))
        }
    }
}
/// The take and its verdict, base64 in JSON as the AI check receives it. The clip is checked to
/// begin like the type it claims before anything is stored.
async fn submit(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(role): Path<String>,
    body: Result<Json<model::Submission>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    if role != "student" {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let Json(v) = body.map_err(|rejection| match rejection {
        JsonRejection::BytesRejection(_) => failure(StatusCode::PAYLOAD_TOO_LARGE),
        _ => failure(StatusCode::BAD_REQUEST),
    })?;
    if !v.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let audio = STANDARD
        .decode(&v.audio_base64)
        .map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if audio.len() > model::MAX_AUDIO_BYTES || !model::looks_like(&v.mime, &audio) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let actor = auth::actor(&h, &role, &v.scope).await?;
    let store = store()?;
    done(repository::submit(&s.graph, store, &actor, &v, audio).await)
}
async fn audio(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(role): Path<String>,
    Query(q): Query<model::Query>,
) -> Result<Response, Failure> {
    if !matches!(role.as_str(), "student" | "teacher") || !q.valid() || q.verse.is_none() || q.student_id.is_none() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let actor = auth::actor(&h, &role, &q.scope()).await?;
    let store = store()?;
    let clip = done(repository::audio(&s.graph, store, &actor, &q).await)?.0;
    let mime = clip["data"]["mime"].as_str().unwrap_or("application/octet-stream").to_string();
    let bytes: Vec<u8> = clip["data"]["bytes"]
        .as_array()
        .map(|a| a.iter().filter_map(|b| b.as_u64()).map(|b| b as u8).collect())
        .unwrap_or_default();
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("application/octet-stream"))),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (header::CONTENT_DISPOSITION, HeaderValue::from_static("inline")),
        ],
        Body::from(bytes),
    )
        .into_response())
}
async fn review(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(role): Path<String>,
    body: Result<Json<model::Review>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    if role != "teacher" {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let Json(v) = body.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !v.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let actor = auth::actor(&h, &role, &v.scope).await?;
    done(repository::review(&s.graph, &actor, &v).await)
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/activities/recitations/:role",
            get(list).put(submit),
        )
        .route(
            "/api/v1/sis-service/activities/recitations/:role/audio",
            get(audio),
        )
        .route(
            "/api/v1/sis-service/activities/recitations/:role/review",
            axum::routing::post(review),
        )
        .layer(axum::extract::DefaultBodyLimit::max(model::BODY_LIMIT))
        .layer(axum::middleware::map_response(
            |mut r: Response| async move {
                r.headers_mut().insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("no-store"),
                );
                r
            },
        ))
}
/// Every six hours, clips past their retention date are removed. Without an object store there is
/// nothing to remove.
pub fn start_retention(graph: neo4rs::Graph) {
    let Some(store) = crate::school_articles::cover::store() else {
        return;
    };
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(6 * 60 * 60));
        loop {
            tick.tick().await;
            match repository::sweep(&graph, store).await {
                Ok(0) => {}
                Ok(removed) => tracing::info!(removed, "recitation clips past retention removed"),
                Err(error) => tracing::warn!(%error, "recitation retention sweep failed"),
            }
        }
    });
}
#[cfg(test)]
mod tests;
