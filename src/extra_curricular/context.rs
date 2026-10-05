use super::{auth, failure, model::Scope, Failure};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    student_id: Option<String>,
    #[serde(alias = "school_id")]
    school_id: Option<String>,
    #[serde(alias = "tenant_id")]
    tenant_id: Option<String>,
}
pub async fn context(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(role): Path<String>,
    Query(q): Query<Selection>,
) -> Result<Json<Value>, Failure> {
    let (a, live_schools, live_tenants) = if role == "admin" {
        let (id, actor_role, schools, tenants) = crate::learning_access::calendar_actor(
            &h,
            &crate::school_portal::staff_capability::Selection {
                school_id: q.school_id.clone(),
                tenant_id: q.tenant_id.clone(),
            },
        )
        .await?;
        (
            auth::Actor {
                id,
                role: actor_role,
            },
            schools,
            tenants,
        )
    } else if role == "teacher" {
        let m = crate::school_portal::auth::teacher(&h).await?.select(
            &crate::school_portal::staff_capability::Selection {
                school_id: q.school_id.clone(),
                tenant_id: q.tenant_id.clone(),
            },
        )?;
        let actor_role = m.role().to_string();
        (
            auth::Actor {
                id: m.staff_member_id,
                role: actor_role,
            },
            m.school_ids,
            m.tenant_ids,
        )
    } else {
        if q.school_id.is_some() || q.tenant_id.is_some() {
            return Err(failure(StatusCode::BAD_REQUEST));
        }
        let scope = Scope {
            school_id: "context".into(),
            tenant_id: "context".into(),
            academic_year: "context".into(),
            student_id: q.student_id.clone(),
            after: None,
        };
        if !scope.valid() {
            return Err(failure(StatusCode::BAD_REQUEST));
        }
        (auth::actor(&h, &role, &scope).await?, vec![], vec![])
    };
    if role != "parent" && q.student_id.is_some() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let data = super::application::context(
        &super::repository::GraphContext(&s.graph),
        &a,
        q.student_id,
        live_schools,
        live_tenants,
    )
    .await
    .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    .ok_or_else(|| failure(StatusCode::FORBIDDEN))?;
    Ok(Json(json!({"data":data})))
}
