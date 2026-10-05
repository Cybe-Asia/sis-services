//! SIS owns the current enrollment/family projection. Payment consumes the same
//! read-only Cypher contract in its shared-graph transactions to avoid a stale
//! HTTP authorization check. No admissions lifecycle state grants billing access.
use crate::AppState;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub const CURRENT_FAMILY: &str = include_str!("current_family.cypher");
type Failure = (StatusCode, Json<Value>);
fn fail(status: StatusCode) -> Failure {
    (
        status,
        Json(
            json!({"error":{"code":if status==StatusCode::FORBIDDEN {"BILLING_ACCESS_DENIED"} else {"BILLING_OWNER_UNAVAILABLE"}}}),
        ),
    )
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selection {
    student_id: String,
}
async fn current_child(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(input): Query<Selection>,
) -> Result<Json<Value>, Failure> {
    let subject = crate::school_portal::auth::parent(&headers)?;
    if !crate::school_portal::model::identifier(&input.student_id) {
        return Err(fail(StatusCode::BAD_REQUEST));
    }
    let mut rows=state.graph.execute(neo4rs::query(&format!("{CURRENT_FAMILY} RETURN u.id AS payer,s.studentId AS student,e.student_id AS enrolled,e.school_id AS school,e.tenant_id AS tenant LIMIT 2")).param("payer",subject).param("student",input.student_id)).await.map_err(|_|fail(StatusCode::SERVICE_UNAVAILABLE))?;
    let row = rows
        .next()
        .await
        .map_err(|_| fail(StatusCode::SERVICE_UNAVAILABLE))?
        .ok_or_else(|| fail(StatusCode::FORBIDDEN))?;
    if rows
        .next()
        .await
        .map_err(|_| fail(StatusCode::SERVICE_UNAVAILABLE))?
        .is_some()
    {
        return Err(fail(StatusCode::FORBIDDEN));
    }
    let field = |k| {
        row.get::<String>(k)
            .map_err(|_| fail(StatusCode::SERVICE_UNAVAILABLE))
    };
    Ok(Json(
        json!({"data":{"contractVersion":1,"payerUserId":field("payer")?,"studentId":field("student")?,"enrolledStudentId":field("enrolled")?,"schoolId":field("school")?,"tenantId":field("tenant")?,"historicalAccessPolicy":"unconfigured"}}),
    ))
}
pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/sis-service/billing/current-child",
        get(current_child),
    )
}
