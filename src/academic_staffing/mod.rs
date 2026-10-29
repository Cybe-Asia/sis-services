//! Subject plans per grade and teaching assignments (slice 2 of the teacher assignments spec,
//! `docs/TEACHING_ASSIGNMENTS.md`). Reads need `academic.staffing.read`, plan and weight writes
//! `academic.subject_plan.manage`, assignment commands `academic.staffing.manage`, all at the
//! school (ADR 0018 slice 6); school administrators and Owners hold them on day one. An
//! assignment grants its teacher Learning access to the class.
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, rejection::QueryRejection, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub mod model;
pub mod repository;
pub mod weights;

use crate::school_portal::model::identifier;
use repository::{
    refuse, unavailable, Failure, Session, Writer, STAFFING_MANAGE, STAFFING_READ,
    SUBJECT_PLAN_MANAGE,
};

fn invalid() -> Failure {
    refuse(StatusCode::BAD_REQUEST, "INVALID_INPUT", json!({}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Lookup {
    school_id: String,
    tenant_id: String,
    academic_year: String,
    section_id: Option<String>,
    staff_member_id: Option<String>,
}
impl Lookup {
    fn valid(&self) -> bool {
        identifier(&self.school_id)
            && identifier(&self.tenant_id)
            && model::key_part(&self.academic_year, 32)
            && self.section_id.as_deref().is_none_or(identifier)
            && self.staff_member_id.as_deref().is_none_or(identifier)
            && !(self.section_id.is_some() && self.staff_member_id.is_some())
    }
}

/// Session, the capability decision from the graph, the read, then the session again, as the
/// teacher list reads.
async fn guarded_read<F, Fut>(
    state: &AppState,
    headers: &HeaderMap,
    q: &Lookup,
    capability: &str,
    read: F,
) -> Result<Json<Value>, Failure>
where
    F: FnOnce(neo4rs::Graph) -> Fut,
    Fut: std::future::Future<Output = Result<Value, Failure>>,
{
    if !q.valid() {
        return Err(invalid());
    }
    let session =
        crate::learning_access::administrator_actor(headers, &q.tenant_id, &q.school_id).await?;
    let data = tokio::time::timeout(Duration::from_secs(8), async {
        if !crate::authorization::reads::permits(
            &mut crate::authorization::facts::Db::Graph(&state.graph),
            &q.tenant_id,
            &q.school_id,
            &session.0,
            capability,
        )
        .await?
        {
            return Err(refuse(StatusCode::FORBIDDEN, "FORBIDDEN", json!({})));
        }
        read(state.graph.clone()).await
    })
    .await
    .map_err(unavailable)??;
    if crate::learning_access::administrator_actor(headers, &q.tenant_id, &q.school_id).await?
        != session
    {
        return Err(refuse(StatusCode::FORBIDDEN, "FORBIDDEN", json!({})));
    }
    let mut data = data;
    data["schoolId"] = json!(q.school_id);
    data["tenantId"] = json!(q.tenant_id);
    data["academicYear"] = json!(q.academic_year);
    Ok(Json(json!({ "data": data })))
}

async fn plans(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<Lookup>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    if q.section_id.is_some() || q.staff_member_id.is_some() {
        return Err(invalid());
    }
    let (school, tenant, year) = (
        q.school_id.clone(),
        q.tenant_id.clone(),
        q.academic_year.clone(),
    );
    guarded_read(&state, &headers, &q, STAFFING_READ, |g| async move {
        repository::plans(&g, &school, &tenant, &year).await
    })
    .await
}

async fn assignments(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<Lookup>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    let (school, tenant, year) = (
        q.school_id.clone(),
        q.tenant_id.clone(),
        q.academic_year.clone(),
    );
    let (section, staff) = (
        q.section_id.clone().unwrap_or_default(),
        q.staff_member_id.clone().unwrap_or_default(),
    );
    guarded_read(&state, &headers, &q, STAFFING_READ, |g| async move {
        repository::overview(&g, &school, &tenant, &year, &section, &staff).await
    })
    .await
}

fn command_key(headers: &HeaderMap) -> Result<String, Failure> {
    headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .filter(|k| model::command_key(k))
        .map(str::to_string)
        .ok_or_else(invalid)
}

/// Writes run detached so a client disconnect cannot cancel a commit whose answer it missed;
/// a retry with the same `Idempotency-Key` gets the stored answer.
async fn write<T, F, Fut>(
    state: AppState,
    headers: HeaderMap,
    school: String,
    tenant: String,
    input: T,
    work: F,
) -> Result<Json<Value>, Failure>
where
    T: Send + Sync + 'static,
    F: FnOnce(neo4rs::Graph, HeaderMap, String, String, String, String, String, T) -> Fut
        + Send
        + 'static,
    Fut: std::future::Future<Output = Result<Value, Failure>> + Send,
{
    let command = command_key(&headers)?;
    let (actor, role) =
        crate::learning_access::administrator_actor(&headers, &tenant, &school).await?;
    let task = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(20),
            work(
                state.graph,
                headers,
                actor,
                role,
                school,
                tenant,
                command,
                input,
            ),
        )
        .await
        .map_err(unavailable)?
    });
    let data = task.await.map_err(unavailable)??;
    Ok(Json(json!({ "data": data })))
}

async fn save_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<model::PlanChange>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(change) = body.map_err(|_| invalid())?;
    if !change.valid() {
        return Err(invalid());
    }
    let (school, tenant) = (change.school_id.clone(), change.tenant_id.clone());
    write(
        state,
        headers,
        school,
        tenant,
        change,
        |g, h, actor, role, school, tenant, command, change| async move {
            let w = Writer {
                session: Session::Headers(&h),
                actor: &actor,
                role: &role,
                school: &school,
                tenant: &tenant,
                command: &command,
                capability: SUBJECT_PLAN_MANAGE,
            };
            repository::save_plan(&g, &w, &change).await
        },
    )
    .await
}

async fn copy_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<model::PlanCopy>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(copy) = body.map_err(|_| invalid())?;
    if !copy.valid() {
        return Err(invalid());
    }
    let (school, tenant) = (copy.school_id.clone(), copy.tenant_id.clone());
    write(
        state,
        headers,
        school,
        tenant,
        copy,
        |g, h, actor, role, school, tenant, command, copy| async move {
            let w = Writer {
                session: Session::Headers(&h),
                actor: &actor,
                role: &role,
                school: &school,
                tenant: &tenant,
                command: &command,
                capability: SUBJECT_PLAN_MANAGE,
            };
            repository::copy_plan(&g, &w, &copy).await
        },
    )
    .await
}

async fn commands(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<model::Commands>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(batch) = body.map_err(|_| invalid())?;
    if !batch.valid() {
        return Err(invalid());
    }
    let (school, tenant) = (batch.school_id.clone(), batch.tenant_id.clone());
    write(
        state,
        headers,
        school,
        tenant,
        batch,
        |g, h, actor, role, school, tenant, command, batch| async move {
            let w = Writer {
                session: Session::Headers(&h),
                actor: &actor,
                role: &role,
                school: &school,
                tenant: &tenant,
                command: &command,
                capability: STAFFING_MANAGE,
            };
            repository::run_commands(&g, &w, &batch).await
        },
    )
    .await
}

async fn grade_weights(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<Lookup>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    if q.section_id.is_some() || q.staff_member_id.is_some() {
        return Err(invalid());
    }
    let (school, tenant, year) = (
        q.school_id.clone(),
        q.tenant_id.clone(),
        q.academic_year.clone(),
    );
    guarded_read(&state, &headers, &q, STAFFING_READ, |g| async move {
        repository::weights::list(&g, &school, &tenant, &year).await
    })
    .await
}

async fn save_grade_weights(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<weights::Change>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(change) = body.map_err(|_| invalid())?;
    if !change.valid() {
        return Err(invalid());
    }
    let (school, tenant) = (change.school_id.clone(), change.tenant_id.clone());
    write(
        state,
        headers,
        school,
        tenant,
        change,
        |g, h, actor, role, school, tenant, command, change| async move {
            let w = Writer {
                session: Session::Headers(&h),
                actor: &actor,
                role: &role,
                school: &school,
                tenant: &tenant,
                command: &command,
                capability: SUBJECT_PLAN_MANAGE,
            };
            repository::weights::save(&g, &w, &change).await
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WeightsFor {
    school_id: String,
    tenant_id: String,
    section_id: String,
    subject: String,
    term: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlansFor {
    school_id: String,
    tenant_id: String,
    academic_year: String,
}
/// The subject plans Learning's subject catalogue reads (ADR 0017 slice 6): the same answer as the
/// Admin read, for a Learning bearer. Teachers of the school read them as well as its administrators
/// and Owners, as grade weights; the Admin route takes Admin portal sessions only, which Learning
/// never holds. Nobody writes here.
async fn learning_subject_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<PlansFor>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    if !(identifier(&q.school_id)
        && identifier(&q.tenant_id)
        && model::key_part(&q.academic_year, 32))
    {
        return Err(invalid());
    }
    match crate::school_portal::auth::teacher_in(&headers, &q.school_id, &q.tenant_id).await {
        Ok(m) if m.school_ids.contains(&q.school_id) && m.tenant_ids.contains(&q.tenant_id) => {}
        _ => {
            crate::learning_access::administrator_actor(&headers, &q.tenant_id, &q.school_id)
                .await?;
        }
    }
    let mut data = tokio::time::timeout(
        Duration::from_secs(8),
        repository::plans(&state.graph, &q.school_id, &q.tenant_id, &q.academic_year),
    )
    .await
    .map_err(unavailable)??;
    data["schoolId"] = json!(q.school_id);
    data["tenantId"] = json!(q.tenant_id);
    data["academicYear"] = json!(q.academic_year);
    Ok(Json(json!({ "data": data })))
}

/// The weights Learning applies to one class, subject and term. Teachers of the school read them
/// as well as its administrators and Owners; nobody writes here.
async fn learning_grade_weights(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<WeightsFor>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    if !(identifier(&q.school_id)
        && identifier(&q.tenant_id)
        && identifier(&q.section_id)
        && model::subject_code(&q.subject)
        && weights::term(&q.term))
    {
        return Err(invalid());
    }
    let actor =
        match crate::school_portal::auth::teacher_in(&headers, &q.school_id, &q.tenant_id).await {
            Ok(m) if m.school_ids.contains(&q.school_id) && m.tenant_ids.contains(&q.tenant_id) => {
                m.staff_member_id
            }
            _ => {
                crate::learning_access::administrator_actor(&headers, &q.tenant_id, &q.school_id)
                    .await?
                    .0
            }
        };
    let data = tokio::time::timeout(
        Duration::from_secs(8),
        repository::weights::for_section(
            &state.graph,
            &actor,
            &q.school_id,
            &q.tenant_id,
            &q.section_id,
            &q.subject,
            &q.term,
        ),
    )
    .await
    .map_err(unavailable)??;
    Ok(Json(json!({ "data": data })))
}

pub use repository::init;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/academic/subject-plans",
            get(plans).put(save_plan),
        )
        .route(
            "/api/v1/sis-service/academic/subject-plans/copy",
            post(copy_plan),
        )
        .route(
            "/api/v1/sis-service/academic/teaching-assignments",
            get(assignments),
        )
        .route(
            "/api/v1/sis-service/academic/teaching-assignments/commands",
            post(commands),
        )
        .route(
            "/api/v1/sis-service/academic/grade-weights",
            get(grade_weights).put(save_grade_weights),
        )
        .route(
            "/api/v1/sis-service/learning/grade-weights",
            get(learning_grade_weights),
        )
        .route(
            "/api/v1/sis-service/learning/subject-plans",
            get(learning_subject_plans),
        )
        .layer(axum::extract::DefaultBodyLimit::max(32 * 1024))
        .layer(axum::middleware::map_response(
            |mut r: axum::response::Response| async move {
                r.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                r
            },
        ))
}

#[cfg(test)]
mod database_tests;
#[cfg(test)]
mod tests;
