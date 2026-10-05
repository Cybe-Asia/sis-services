//! SIS owns school education dates. Classroom events and learner work retain their owners.
pub mod model;
pub mod repository;
pub mod window;
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, Query, State},
    http::{HeaderMap, StatusCode},
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
            json!({"error":{"code":match status {StatusCode::BAD_REQUEST=>"INVALID_INPUT",StatusCode::FORBIDDEN=>"FORBIDDEN",StatusCode::UNAUTHORIZED=>"UNAUTHORIZED",StatusCode::CONFLICT=>"REVISION_CONFLICT",_=>"DEPENDENCY_UNAVAILABLE"}}}),
        ),
    )
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub tenant_id: String,
    pub school_id: String,
    pub academic_year: String,
    pub after: Option<String>,
    pub id: Option<String>,
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ContextPage {
    after: Option<String>,
    #[serde(alias = "schoolId")]
    school_id: Option<String>,
    #[serde(alias = "tenantId")]
    tenant_id: Option<String>,
}
async fn context(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<ContextPage>,
) -> Result<Json<Value>, Failure> {
    // Introspection plus current graph membership, without returning pupil identities.
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let after = match q.after {
        None => String::new(),
        Some(v) if v.len() <= 768 => String::from_utf8(
            URL_SAFE_NO_PAD
                .decode(v)
                .map_err(|_| failure(StatusCode::BAD_REQUEST))?,
        )
        .map_err(|_| failure(StatusCode::BAD_REQUEST))?,
        _ => return Err(failure(StatusCode::BAD_REQUEST)),
    };
    let member = crate::learning_access::calendar_membership_for(
        &h,
        &crate::school_portal::staff_capability::Selection {
            school_id: q.school_id,
            tenant_id: q.tenant_id,
        },
    )
    .await?;
    repository::context(&s.graph, &member.0, &member.1, &member.2, &after)
        .await
        .map(|data| Json(json!({"data":data})))
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))
}
async fn list(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<Scope>,
) -> Result<Json<Value>, Failure> {
    if q.id
        .as_deref()
        .is_some_and(|id| !crate::school_portal::model::identifier(id))
        || !crate::school_portal::model::identifier(&q.school_id)
        || !crate::school_portal::model::identifier(&q.tenant_id)
        || q.academic_year.is_empty()
        || q.academic_year.len() > 32
        || q.after
            .as_deref()
            .is_some_and(|a| !crate::school_portal::model::identifier(a))
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let actor = crate::learning_access::administrator(&h, &q.tenant_id, &q.school_id).await?;
    repository::list(&s.graph, &actor, &q)
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
    let (actor, role) =
        crate::learning_access::administrator_actor(&h, &v.entry.tenant_id, &v.entry.school_id)
            .await?;
    match repository::change_live(&s.graph, &actor, &role, &v, &h).await? {
        Some(version) => Ok(Json(json!({"data":{"version":version}}))),
        None => Err(failure(StatusCode::CONFLICT)),
    }
}
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/education-calendar/context",
            get(context),
        )
        .route(
            "/api/v1/sis-service/education-calendar",
            get(list).post(change),
        )
}
