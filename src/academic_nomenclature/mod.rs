//! School nomenclature (`docs/ACADEMIC_NOMENCLATURE.md`): one list of bilingual labels per kind
//! and school, read by Admin, Teacher, Student/Parent and the report card. Staff of the school
//! read; saves need `academic.nomenclature.manage` at the school (ADR 0018), which school
//! administrators and Owners hold on day one. Codes never change, only labels.
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, rejection::QueryRejection, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub mod model;
pub mod repository;

use crate::academic_staffing::repository::{refuse, unavailable, Failure, Session, Writer};
use crate::school_portal::model::identifier;
use repository::NOMENCLATURE_MANAGE;

fn invalid() -> Failure {
    refuse(StatusCode::BAD_REQUEST, "INVALID_INPUT", json!({}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Lookup {
    school_id: String,
    tenant_id: String,
    /// Admin's editor also reads archived rows; everyone else reads active rows only.
    #[serde(default)]
    include_archived: bool,
}

/// Who reads: an administrator or Owner of the school, else a teacher of it. The administrator's
/// refusal is the answer when neither session fits.
async fn reader(
    headers: &HeaderMap,
    tenant: &str,
    school: &str,
) -> Result<(String, String), Failure> {
    match crate::learning_access::administrator_actor(
        headers,
        &tenant.to_string(),
        &school.to_string(),
    )
    .await
    {
        Ok(actor) => Ok(actor),
        Err(refused) => match crate::school_portal::auth::teacher_in(headers, school, tenant).await
        {
            Ok(m)
                if m.active
                    && m.school_ids.iter().any(|s| s == school)
                    && m.tenant_ids.iter().any(|t| t == tenant) =>
            {
                Ok((m.staff_member_id, "teacher".into()))
            }
            _ => Err(refused),
        },
    }
}

/// Session, the read (seeding missing kinds), then the session again. Active-only reads may be
/// cached for five minutes and revalidated with `If-None-Match`.
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<Lookup>, QueryRejection>,
) -> Result<Response, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    if !(identifier(&q.school_id) && identifier(&q.tenant_id)) {
        return Err(invalid());
    }
    let (actor, role) = reader(&headers, &q.tenant_id, &q.school_id).await?;
    let known = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);
    let (tag, data, manage) = tokio::time::timeout(Duration::from_secs(8), async {
        let (tag, data) = repository::read(
            &state.graph,
            &q.tenant_id,
            &q.school_id,
            &actor,
            &role,
            q.include_archived,
            known.as_deref(),
        )
        .await?;
        let manage = match data {
            Some(_) => {
                crate::authorization::reads::permits(
                    &mut crate::authorization::facts::Db::Graph(&state.graph),
                    &q.tenant_id,
                    &q.school_id,
                    &actor,
                    NOMENCLATURE_MANAGE,
                )
                .await?
            }
            None => false,
        };
        Ok::<_, Failure>((tag, data, manage))
    })
    .await
    .map_err(unavailable)??;
    if reader(&headers, &q.tenant_id, &q.school_id).await?.0 != actor {
        return Err(refuse(StatusCode::FORBIDDEN, "FORBIDDEN", json!({})));
    }
    let cache = if q.include_archived {
        "no-store".to_string()
    } else {
        format!("private, max-age={}", model::CACHE_SECONDS)
    };
    let mut response = match data {
        None => StatusCode::NOT_MODIFIED.into_response(),
        Some(mut data) => {
            data["contractVersion"] = json!(model::CONTRACT_VERSION);
            data["schoolId"] = json!(q.school_id);
            data["tenantId"] = json!(q.tenant_id);
            data["cacheSeconds"] = json!(model::CACHE_SECONDS);
            data["canManage"] = json!(manage);
            data["etag"] = json!(tag);
            Json(json!({ "data": data })).into_response()
        }
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_str(&cache).map_err(unavailable)?,
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&tag).map_err(unavailable)?,
    );
    headers.insert(header::VARY, HeaderValue::from_static("Authorization"));
    Ok(response)
}

/// Saves run detached so a client disconnect cannot cancel a commit whose answer it missed; a
/// retry with the same `Idempotency-Key` gets the stored answer.
async fn save(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    headers: HeaderMap,
    body: Result<Json<model::Change>, JsonRejection>,
) -> Result<Response, Failure> {
    let Json(change) = body.map_err(|_| invalid())?;
    if !model::kind(&kind) || !change.valid(&kind) {
        return Err(invalid());
    }
    let command = headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .filter(|k| crate::academic_staffing::model::command_key(k))
        .map(str::to_string)
        .ok_or_else(invalid)?;
    let (actor, role) =
        crate::learning_access::administrator_actor(&headers, &change.tenant_id, &change.school_id)
            .await?;
    let task = tokio::spawn(async move {
        let w = Writer {
            session: Session::Headers(&headers),
            actor: &actor,
            role: &role,
            school: &change.school_id,
            tenant: &change.tenant_id,
            command: &command,
            capability: NOMENCLATURE_MANAGE,
        };
        tokio::time::timeout(
            Duration::from_secs(20),
            repository::save(&state.graph, &w, &kind, &change),
        )
        .await
        .map_err(unavailable)?
    });
    let data: Value = task.await.map_err(unavailable)??;
    let mut response = Json(json!({ "data": data })).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub use repository::init;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sis-service/academic/nomenclature", get(list))
        .route("/api/v1/sis-service/academic/nomenclature/:kind", put(save))
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}

#[cfg(test)]
mod database_tests;
#[cfg(test)]
mod tests;
