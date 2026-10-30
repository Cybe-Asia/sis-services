//! Capability-based access for school staff (ADR 0018, `docs/AUTHORIZATION.md`): the capability
//! catalogue, role templates, assignments and the decisions built on them and on today's
//! records. Any active staff portal session reaches these routes (a Principal may be a teacher in
//! Auth); what each may read or change is then decided from their own holdings. No service enforces
//! capabilities yet: their current role checks stay until each one moves.
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

pub mod commands;
pub mod facts;
pub mod model;
pub mod reads;

use crate::academic_staffing::repository::{refuse, unavailable, Failure};
use crate::school_portal::model::identifier;
use commands::{Session, Writer};
use reads::Reader;

fn invalid() -> Failure {
    refuse(StatusCode::BAD_REQUEST, "INVALID_INPUT", json!({}))
}

/// The caller's session, which must cover the tenant. Any active staff member passes; what they
/// may read is decided from their own holdings.
async fn reader(headers: &HeaderMap, tenant: &str) -> Result<Reader, Failure> {
    let (actor, role, schools, tenants) = crate::learning_access::any_staff_session(headers)
        .await?
        .staff_scope();
    if role == "owner" && schools.is_empty() && tenants.is_empty() {
        return Ok(Reader {
            actor,
            schools: None,
        });
    }
    if !tenants.iter().any(|t| t == tenant) {
        return Err(refuse(StatusCode::FORBIDDEN, "FORBIDDEN", json!({})));
    }
    Ok(Reader {
        actor,
        schools: Some(schools),
    })
}

/// Session, the read, then the session again, as the other Admin reads do.
async fn guarded<F, Fut>(headers: &HeaderMap, tenant: &str, read: F) -> Result<Json<Value>, Failure>
where
    F: FnOnce(Reader) -> Fut,
    Fut: std::future::Future<Output = Result<Value, Failure>>,
{
    if !identifier(tenant) {
        return Err(invalid());
    }
    let first = reader(headers, tenant).await?;
    let actor = first.actor.clone();
    let data = tokio::time::timeout(Duration::from_secs(8), read(first))
        .await
        .map_err(unavailable)??;
    if reader(headers, tenant).await?.actor != actor {
        return Err(refuse(StatusCode::FORBIDDEN, "FORBIDDEN", json!({})));
    }
    Ok(Json(json!({ "data": data })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TenantQuery {
    tenant_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TemplateQuery {
    tenant_id: String,
    code: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StaffQuery {
    tenant_id: String,
    staff_member_id: String,
}

async fn capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<TenantQuery>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    let tenant = q.tenant_id.clone();
    guarded(&headers, &q.tenant_id, |r| async move {
        reads::view(
            &state.graph,
            &r,
            &tenant,
            std::slice::from_ref(&r.actor),
            &[],
        )
        .await?;
        Ok(reads::catalogue())
    })
    .await
}

async fn templates(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<TenantQuery>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    let tenant = q.tenant_id.clone();
    guarded(&headers, &q.tenant_id, |r| async move {
        reads::templates(&state.graph, &r, &tenant).await
    })
    .await
}

async fn template_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<TemplateQuery>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    if !model::template_code(&q.code) {
        return Err(invalid());
    }
    let (tenant, code) = (q.tenant_id.clone(), q.code.clone());
    guarded(&headers, &q.tenant_id, |r| async move {
        reads::history(&state.graph, &r, &tenant, &code).await
    })
    .await
}

async fn template_impact(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<TemplateQuery>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    if !model::template_code(&q.code) {
        return Err(invalid());
    }
    let (tenant, code) = (q.tenant_id.clone(), q.code.clone());
    guarded(&headers, &q.tenant_id, |r| async move {
        reads::impact(&state.graph, &r, &tenant, &code).await
    })
    .await
}

async fn assignments(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<reads::Filter>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(f) = q.map_err(|_| invalid())?;
    let ids = [&f.school_id, &f.staff_member_id, &f.section_id];
    if !ids.iter().all(|v| v.as_deref().is_none_or(identifier))
        || !f.template_code.as_deref().is_none_or(model::template_code)
        || !f
            .subject
            .as_deref()
            .is_none_or(crate::academic_staffing::model::subject_code)
        || !f
            .status
            .as_deref()
            .is_none_or(|s| ["active", "revoked", "all"].contains(&s))
    {
        return Err(invalid());
    }
    let tenant = f.tenant_id.clone();
    guarded(&headers, &tenant, |r| async move {
        reads::assignments(&state.graph, &r, &f).await
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpiringQuery {
    tenant_id: String,
    days: Option<u32>,
    school_id: Option<String>,
}

async fn expiring(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<ExpiringQuery>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    let days = q.days.unwrap_or(30);
    if !(1..=366).contains(&days) || !q.school_id.as_deref().is_none_or(identifier) {
        return Err(invalid());
    }
    let tenant = q.tenant_id.clone();
    guarded(&headers, &q.tenant_id, |r| async move {
        reads::expiring(&state.graph, &r, &tenant, days, q.school_id.as_deref()).await
    })
    .await
}

async fn effective(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<StaffQuery>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    if !identifier(&q.staff_member_id) {
        return Err(invalid());
    }
    let (tenant, staff) = (q.tenant_id.clone(), q.staff_member_id.clone());
    guarded(&headers, &q.tenant_id, |r| async move {
        if staff == r.actor {
            reads::own(&state.graph, &r, &tenant).await
        } else {
            reads::effective(&state.graph, &r, &tenant, &staff).await
        }
    })
    .await
}

async fn check(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<model::CheckRequest>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(request) = body.map_err(|_| invalid())?;
    if !request.valid() {
        return Err(invalid());
    }
    let tenant = request.tenant_id.clone();
    guarded(&headers, &tenant, |r| async move {
        reads::check(&state.graph, &r, &request).await
    })
    .await
}

async fn audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<TenantQuery>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    let tenant = q.tenant_id.clone();
    guarded(&headers, &q.tenant_id, |r| async move {
        reads::audit(&state.graph, &r, &tenant).await
    })
    .await
}

fn command_key(headers: &HeaderMap) -> Result<String, Failure> {
    headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .filter(|k| crate::academic_staffing::model::command_key(k))
        .map(str::to_string)
        .ok_or_else(invalid)
}

/// Writes run detached so a client disconnect cannot cancel a commit whose answer it missed;
/// a retry with the same `Idempotency-Key` gets the stored answer.
async fn write<T, F, Fut>(
    state: AppState,
    headers: HeaderMap,
    tenant: String,
    input: T,
    work: F,
) -> Result<Json<Value>, Failure>
where
    T: Send + Sync + 'static,
    F: FnOnce(neo4rs::Graph, HeaderMap, String, String, String, String, T) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<Value, Failure>> + Send,
{
    let command = command_key(&headers)?;
    let (actor, role) = commands::live_session(&headers, &tenant).await?;
    let task = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(20),
            work(state.graph, headers, actor, role, tenant, command, input),
        )
        .await
        .map_err(unavailable)?
    });
    let data = task.await.map_err(unavailable)??;
    Ok(Json(json!({ "data": data })))
}

async fn template_commands(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<model::TemplateCommands>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(batch) = body.map_err(|_| invalid())?;
    if !batch.valid() {
        return Err(invalid());
    }
    let tenant = batch.tenant_id.clone();
    write(
        state,
        headers,
        tenant,
        batch,
        |g, h, actor, role, tenant, command, batch| async move {
            let w = Writer {
                session: Session::Headers(&h),
                actor: &actor,
                role: &role,
                tenant: &tenant,
                command: &command,
            };
            commands::templates(&g, &w, &batch).await
        },
    )
    .await
}

async fn assignment_commands(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<model::AssignmentCommands>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(batch) = body.map_err(|_| invalid())?;
    if !batch.valid() {
        return Err(invalid());
    }
    let tenant = batch.tenant_id.clone();
    write(
        state,
        headers,
        tenant,
        batch,
        |g, h, actor, role, tenant, command, batch| async move {
            let w = Writer {
                session: Session::Headers(&h),
                actor: &actor,
                role: &role,
                tenant: &tenant,
                command: &command,
            };
            commands::assignments(&g, &w, &batch).await
        },
    )
    .await
}

/// Constraints and indexes; also checks the catalogue and seed templates, so a bad file stops
/// start-up instead of a request.
pub async fn init(graph: &neo4rs::Graph) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    model::load()?;
    for statement in include_str!("../../migrations/0007_authorization.cypher")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        graph.run(neo4rs::query(statement)).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HeldQuery {
    capabilities: String,
}
/// `GET /held?capabilities=a,b` for Admission and Payment (ADR 0018 slice 6): the caller is the
/// Staff Member named by Auth's `staff-api+jwt` (the credential those services receive from
/// Admin), and the answer covers only them.
async fn held(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<HeldQuery>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| invalid())?;
    let codes: Vec<String> = q.capabilities.split(',').map(str::to_string).collect();
    let r = model::reference();
    let unique: std::collections::BTreeSet<&String> = codes.iter().collect();
    if codes.len() > 20
        || unique.len() != codes.len()
        || codes.iter().any(|c| r.capability(c).is_none())
    {
        return Err(invalid());
    }
    let actor = crate::enrollment_handover::auth::actor(&headers, &state.config.jwt_secret)
        .map_err(|e| match e {
            crate::enrollment_handover::auth::Error::Unavailable => refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                "DEPENDENCY_UNAVAILABLE",
                json!({}),
            ),
            crate::enrollment_handover::auth::Error::Unauthorized => {
                refuse(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", json!({}))
            }
        })?;
    let data = tokio::time::timeout(
        Duration::from_secs(8),
        reads::held(&state.graph, &actor.staff, &codes),
    )
    .await
    .map_err(unavailable)??;
    Ok(Json(json!({ "data": data })))
}

pub fn router() -> Router<AppState> {
    const BASE: &str = "/api/v1/sis-service/authorization";
    Router::new()
        .route(&format!("{BASE}/capabilities"), get(capabilities))
        .route(&format!("{BASE}/templates"), get(templates))
        .route(&format!("{BASE}/templates/history"), get(template_history))
        .route(&format!("{BASE}/templates/impact"), get(template_impact))
        .route(
            &format!("{BASE}/templates/commands"),
            post(template_commands),
        )
        .route(&format!("{BASE}/assignments"), get(assignments))
        .route(&format!("{BASE}/assignments/expiring"), get(expiring))
        .route(
            &format!("{BASE}/assignments/commands"),
            post(assignment_commands),
        )
        .route(&format!("{BASE}/check"), post(check))
        .route(&format!("{BASE}/effective"), get(effective))
        .route(&format!("{BASE}/held"), get(held))
        .route(&format!("{BASE}/audit"), get(audit))
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
