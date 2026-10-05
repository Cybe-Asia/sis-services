//! School defaults owned by SIS, protected by live Auth and current graph scope.
use super::*;
use axum::{
    extract::{rejection::QueryRejection, DefaultBodyLimit},
    routing::get,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PolicyScope {
    school_id: String,
    tenant_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    school_id: String,
    tenant_id: String,
    expected_version: u32,
    opening_offset_minutes: u32,
    teacher_override_allowed: bool,
}
const ADMIN: &str="MATCH (actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WHERE ((NOT 'owner' IN coalesce(actor.roles,[]) AND 'school_admin' IN coalesce(actor.roles,[]) AND $school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[])) OR ('owner' IN coalesce(actor.roles,[]) AND size(coalesce(actor.teamIds,[]))=0 AND ((size(coalesce(actor.schoolIds,[]))=0 AND size(coalesce(actor.tenantIds,[]))=0) OR ($school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[]))) AND NOT EXISTS {MATCH(other:StaffMember {id:actor.id}) WHERE other<>actor} AND EXISTS {MATCH(:School {school_id:$school,tenant_id:$tenant})} AND NOT EXISTS {MATCH(one:School {school_id:$school,tenant_id:$tenant}),(two:School {school_id:$school,tenant_id:$tenant}) WHERE one<>two}))";
const READER: &str="MATCH (actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WHERE ((NOT 'owner' IN coalesce(actor.roles,[]) AND $role IN coalesce(actor.roles,[]) AND $school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[])) OR ('owner' IN coalesce(actor.roles,[]) AND size(coalesce(actor.teamIds,[]))=0 AND ((size(coalesce(actor.schoolIds,[]))=0 AND size(coalesce(actor.tenantIds,[]))=0) OR ($school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[]))) AND NOT EXISTS {MATCH(other:StaffMember {id:actor.id}) WHERE other<>actor} AND EXISTS {MATCH(:School {school_id:$school,tenant_id:$tenant})} AND NOT EXISTS {MATCH(one:School {school_id:$school,tenant_id:$tenant}),(two:School {school_id:$school,tenant_id:$tenant}) WHERE one<>two}))";
fn key(s: &PolicyScope) -> String {
    format!(
        "{}:{}{}:{}",
        s.school_id.len(),
        s.school_id,
        s.tenant_id.len(),
        s.tenant_id
    )
}
fn valid(s: &PolicyScope) -> bool {
    valid_id(&s.school_id) && valid_id(&s.tenant_id)
}
pub(super) async fn migrate(
    graph: &neo4rs::Graph,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    graph.run(query("CREATE CONSTRAINT learning_prework_policy_key IF NOT EXISTS FOR (p:LearningPreworkPolicy) REQUIRE p.key IS UNIQUE")).await?;
    Ok(())
}
fn view(row: neo4rs::Row, s: &PolicyScope) -> Result<Value, Failure> {
    let version: i64 = row
        .get("version")
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let offset: i64 = row
        .get("offset")
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let allowed: bool = row
        .get("allowed")
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if !(0..i32::MAX as i64).contains(&version) || !(1..=120).contains(&offset) {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    Ok(
        json!({"schoolId":s.school_id,"tenantId":s.tenant_id,"version":version,"openingOffsetMinutes":offset,"teacherOverrideAllowed":allowed,"closingRule":"class_start"}),
    )
}
async fn read(
    State(state): State<AppState>,
    q: Result<Query<PolicyScope>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Json<Value>, Failure> {
    let Query(scope) = q.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !valid(&scope) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (actor, role) =
        match crate::school_portal::auth::teacher_in(&headers, &scope.school_id, &scope.tenant_id)
            .await
        {
            Ok(m)
                if m.school_ids.contains(&scope.school_id)
                    && m.tenant_ids.contains(&scope.tenant_id) =>
            {
                let role = m.role();
                (m.staff_member_id, role)
            }
            _ => (
                administrator(&headers, &scope.tenant_id, &scope.school_id).await?,
                "school_admin",
            ),
        };
    let mut rows=state.graph.execute(query(&format!("{READER} OPTIONAL MATCH (p:LearningPreworkPolicy {{key:$key}}) RETURN coalesce(p.version,0) AS version,coalesce(p.opening_offset_minutes,10) AS offset,coalesce(p.teacher_override_allowed,false) AS allowed")).param("actor",actor).param("role",role).param("school",scope.school_id.clone()).param("tenant",scope.tenant_id.clone()).param("key",key(&scope))).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let row = rows
        .next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .ok_or_else(|| failure(StatusCode::FORBIDDEN))?;
    Ok(Json(json!({"data":view(row,&scope)?})))
}
async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Change>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(change) = body.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    let scope = PolicyScope {
        school_id: change.school_id,
        tenant_id: change.tenant_id,
    };
    if !valid(&scope)
        || change.expected_version >= i32::MAX as u32
        || !(1..=120).contains(&change.opening_offset_minutes)
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (actor, actor_role) =
        administrator_actor(&headers, &scope.tenant_id, &scope.school_id).await?;
    // Unique policy node serializes CAS and audit in the same implicit transaction.
    let q=query(&format!("{ADMIN} MERGE (p:LearningPreworkPolicy {{key:$key}}) ON CREATE SET p.version=0,p.school_id=$school,p.tenant_id=$tenant SET p.lock=coalesce(p.lock,0)+1 WITH p {ADMIN} WITH actor,p WHERE p.version=$version AND p.school_id=$school AND p.tenant_id=$tenant SET p.version=p.version+1,p.opening_offset_minutes=$offset,p.teacher_override_allowed=$allowed,p.updated_at=timestamp() CREATE (:LearningPreworkPolicyAudit {{id:$audit,actor_id:actor.id,actor_role:CASE WHEN 'owner' IN coalesce(actor.roles,[]) THEN 'owner' ELSE 'school_admin' END,school_id:$school,tenant_id:$tenant,version:p.version,opening_offset_minutes:$offset,teacher_override_allowed:$allowed,created_at:timestamp()}}) RETURN p.version AS version,p.opening_offset_minutes AS offset,p.teacher_override_allowed AS allowed")).param("actor",actor.clone()).param("school",scope.school_id.clone()).param("tenant",scope.tenant_id.clone()).param("key",key(&scope)).param("version",i64::from(change.expected_version)).param("offset",i64::from(change.opening_offset_minutes)).param("allowed",change.teacher_override_allowed).param("audit",uuid::Uuid::new_v4().to_string());
    let row=if actor_role=="owner"{
        let lock=query(&format!("{ADMIN} MERGE(p:LearningPreworkPolicy {{key:$key}}) ON CREATE SET p.version=0,p.school_id=$school,p.tenant_id=$tenant SET p.lock=coalesce(p.lock,0)+1 RETURN p.key AS key")).param("actor",actor.clone()).param("school",scope.school_id.clone()).param("tenant",scope.tenant_id.clone()).param("key",key(&scope));
        crate::school_portal::staff_capability::live_write(&state.graph,crate::school_portal::staff_capability::OwnerSession{headers:&headers,actor:&actor,school:&scope.school_id,tenant:&scope.tenant_id,learning:false},lock,q).await?
    }else{
        let mut rows=state.graph.execute(q).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let row=rows.next().await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
        while rows.next().await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?.is_some(){}
        row
    }.ok_or_else(||failure(StatusCode::CONFLICT))?;
    Ok(Json(json!({"data":view(row,&scope)?})))
}
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/learning/prework-policy",
            get(read).post(update),
        )
        .layer(DefaultBodyLimit::max(8192))
}
