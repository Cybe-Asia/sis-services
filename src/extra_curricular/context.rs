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
/// ADR 0018 slice 7: role `manager` over the schools (one tenant) where a staff member who is not an
/// administrator holds `academic.extracurricular.manage`; `FORBIDDEN` when there is none.
async fn manager_scope(
    s: &AppState,
    h: &HeaderMap,
    selection: &crate::school_portal::staff_capability::Selection,
) -> Result<(auth::Actor, Vec<String>, Vec<String>), Failure> {
    let (actor, role, mut schools, mut tenants) =
        crate::learning_access::any_staff_session(&s.config, h)
            .await?
            .staff_scope();
    if role != "staff" {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    if let Some((school, tenant)) = selection
        .pair()
        .map_err(|_| failure(StatusCode::BAD_REQUEST))?
    {
        if !(schools.iter().any(|x| x == school) && tenants.iter().any(|x| x == tenant)) {
            return Err(failure(StatusCode::FORBIDDEN));
        }
        (schools, tenants) = (vec![school.into()], vec![tenant.into()]);
    }
    let [tenant] = tenants.as_slice() else {
        return Err(failure(StatusCode::FORBIDDEN));
    };
    let held = crate::authorization::reads::permitted_schools(
        &mut crate::authorization::facts::Db::Graph(&s.graph),
        tenant,
        &schools,
        &actor,
        auth::EXTRACURRICULAR_MANAGE,
    )
    .await
    .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if held.is_empty() {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    Ok((
        auth::Actor {
            id: actor,
            role: "manager".into(),
        },
        held,
        vec![tenant.clone()],
    ))
}
pub async fn context(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(role): Path<String>,
    Query(q): Query<Selection>,
) -> Result<Json<Value>, Failure> {
    let selection = crate::school_portal::staff_capability::Selection {
        school_id: q.school_id.clone(),
        tenant_id: q.tenant_id.clone(),
    };
    let (a, live_schools, live_tenants) = if role == "admin" {
        match crate::learning_access::calendar_actor(&s.config, &h, &selection).await {
            Ok((id, actor_role, schools, tenants)) => (
                auth::Actor {
                    id,
                    role: actor_role,
                },
                schools,
                tenants,
            ),
            // Not an administrator: the schools where the caller manages extracurriculars.
            Err(e) if e.0 == StatusCode::FORBIDDEN => manager_scope(&s, &h, &selection).await?,
            Err(e) => return Err(e),
        }
    } else if role == "teacher" {
        let m = crate::school_portal::auth::teacher(&s.config, &h)
            .await?
            .select(&crate::school_portal::staff_capability::Selection {
                school_id: q.school_id.clone(),
                tenant_id: q.tenant_id.clone(),
            })?;
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
        (
            auth::actor(&s.config, &h, &role, &scope).await?,
            vec![],
            vec![],
        )
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
