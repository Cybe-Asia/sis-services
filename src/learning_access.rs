//! SIS-owned, governed references to Auth identity and Admission enrolment.
//! Grants never create accounts, change roles, or infer identity from contact data.
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, Query, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use neo4rs::query;
use serde::Deserialize;
use serde_json::{json, Value};

type Failure = (StatusCode, Json<Value>);
fn failure(status: StatusCode) -> Failure {
    (
        status,
        Json(
            json!({"error":{"code":match status { StatusCode::FORBIDDEN=>"FORBIDDEN", StatusCode::UNAUTHORIZED=>"UNAUTHORIZED", StatusCode::BAD_REQUEST=>"INVALID_INPUT", StatusCode::CONFLICT=>"REVISION_OR_BINDING_CONFLICT", _=>"DEPENDENCY_UNAVAILABLE"}}}),
        ),
    )
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantRequest {
    pub tenant_id: String,
    pub school_id: String,
    pub section_id: String,
    pub target_id: String,
    pub student_id: Option<String>,
    pub expected_version: u32,
    pub active: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Membership {
    active: bool,
    staff_member_id: Option<String>,
    roles: Option<Vec<String>>,
    tenant_ids: Option<Vec<String>>,
    school_ids: Option<Vec<String>>,
}
async fn membership(headers: &HeaderMap) -> Result<Membership, Failure> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|s| !s.is_empty() && s.len() <= 8192)
        .ok_or_else(|| failure(StatusCode::UNAUTHORIZED))?;
    let endpoint = std::env::var("AUTH_PORTAL_INTROSPECTION_URL")
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let url =
        reqwest::Url::parse(&endpoint).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/api/v1/auth-service/oauth/portal-session/introspect"
    {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none());
    if let Ok(path) = std::env::var("AUTH_PORTAL_CA_FILE") {
        let pem = std::fs::read(path).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let certificate = reqwest::Certificate::from_pem(&pem)
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        builder = builder.add_root_certificate(certificate);
    }
    let client = builder
        .build()
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let response = client
        .post(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(failure(StatusCode::UNAUTHORIZED));
    }
    if !response.status().is_success() {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    let member = read_membership(response).await?;
    if !member.active
        || !member.roles.as_ref().is_some_and(|roles| {
            roles
                .iter()
                .any(|r| matches!(r.as_str(), "school_admin" | "owner"))
        })
        || !member
            .staff_member_id
            .as_ref()
            .is_some_and(|id| valid_id(id))
    {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    for ids in [&member.tenant_ids, &member.school_ids] {
        if !ids
            .as_ref()
            .is_some_and(|values| values.len() <= 64 && values.iter().all(|id| valid_id(id)))
        {
            return Err(failure(StatusCode::FORBIDDEN));
        }
    }
    let owner = member
        .roles
        .as_ref()
        .is_some_and(|roles| roles.iter().any(|r| r == "owner"));
    let schools = member.school_ids.as_ref().unwrap();
    let tenants = member.tenant_ids.as_ref().unwrap();
    if !(owner && schools.is_empty() && tenants.is_empty())
        && (schools.is_empty() || tenants.is_empty())
    {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    Ok(member)
}
pub(crate) async fn calendar_membership_for(
    headers: &HeaderMap,
    selection: &crate::school_portal::staff_capability::Selection,
) -> Result<(String, Vec<String>, Vec<String>), Failure> {
    let (id, _, schools, tenants) = calendar_actor(headers, selection).await?;
    Ok((id, schools, tenants))
}
pub(crate) async fn calendar_actor(
    headers: &HeaderMap,
    selection: &crate::school_portal::staff_capability::Selection,
) -> Result<(String, String, Vec<String>, Vec<String>), Failure> {
    let m = membership(headers).await?;
    let owner = m
        .roles
        .as_ref()
        .is_some_and(|rs| rs.iter().any(|r| r == "owner"));
    let mut schools = m.school_ids.unwrap_or_default();
    let mut tenants = m.tenant_ids.unwrap_or_default();
    let selected = selection
        .pair()
        .map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if owner && selected.is_none() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    if let Some((school, tenant)) = selected {
        if !crate::school_portal::staff_capability::live_scope(
            owner, &schools, &tenants, school, tenant,
        ) {
            return Err(failure(StatusCode::FORBIDDEN));
        }
        schools = vec![school.into()];
        tenants = vec![tenant.into()];
    }
    Ok((
        m.staff_member_id.unwrap_or_default(),
        if owner { "owner" } else { "admin" }.into(),
        schools,
        tenants,
    ))
}
pub(crate) async fn administrator(
    headers: &HeaderMap,
    tenant_id: &String,
    school_id: &String,
) -> Result<String, Failure> {
    Ok(administrator_actor(headers, tenant_id, school_id).await?.0)
}
pub(crate) async fn administrator_actor(
    headers: &HeaderMap,
    tenant_id: &String,
    school_id: &String,
) -> Result<(String, String), Failure> {
    let member = membership(headers).await?;
    let owner = member
        .roles
        .as_ref()
        .is_some_and(|rs| rs.iter().any(|r| r == "owner"));
    if !crate::school_portal::staff_capability::live_scope(
        owner,
        member.school_ids.as_ref().unwrap(),
        member.tenant_ids.as_ref().unwrap(),
        school_id,
        tenant_id,
    ) {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    Ok((
        member
            .staff_member_id
            .ok_or_else(|| failure(StatusCode::FORBIDDEN))?,
        if owner { "owner" } else { "school_admin" }.into(),
    ))
}
mod class_meetings;
mod context;
mod owner;
mod prework_policy;
pub(crate) mod timetable;

const SCOPE: &str = "MATCH (sec:Section {section_id:$section,school_id:$school,tenant_id:$tenant}) SET sec.learning_access_lock=coalesce(sec.learning_access_lock,0)+1 WITH sec MATCH (actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WHERE ((NOT 'owner' IN coalesce(actor.roles,[]) AND 'school_admin' IN coalesce(actor.roles,[]) AND $school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[])) OR ('owner' IN coalesce(actor.roles,[]) AND size(coalesce(actor.teamIds,[]))=0 AND ((size(coalesce(actor.schoolIds,[]))=0 AND size(coalesce(actor.tenantIds,[]))=0) OR ($school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[]))) AND NOT EXISTS {MATCH(other:StaffMember {id:actor.id}) WHERE other<>actor} AND EXISTS {MATCH(:School {school_id:$school,tenant_id:$tenant})} AND NOT EXISTS {MATCH(one:School {school_id:$school,tenant_id:$tenant}),(two:School {school_id:$school,tenant_id:$tenant}) WHERE one<>two})) WITH actor,sec";
pub const TEACHER_QUERY:&str="MATCH (teacher:StaffMember {id:$target}) WHERE $active=false OR (teacher.membershipStatus='ACTIVE' AND 'teacher' IN coalesce(teacher.roles,[]) AND $school IN coalesce(teacher.schoolIds,[]) AND $tenant IN coalesce(teacher.tenantIds,[]) AND sec.status='active') OPTIONAL MATCH (existing:LearningTeachingGrant {key:$key}) WITH actor,sec,existing WHERE (existing IS NULL AND $version=0 AND $active=true) OR (existing.version=$version AND existing.staff_member_id=$target AND existing.section_id=$section AND existing.school_id=$school AND existing.tenant_id=$tenant) MERGE (grant:LearningTeachingGrant {key:$key}) ON CREATE SET grant.version=0,grant.staff_member_id=$target,grant.section_id=$section,grant.school_id=$school,grant.tenant_id=$tenant SET grant.version=grant.version+1,grant.status=$status,grant.updated_at=datetime() CREATE (:LearningAccessAudit {id:$audit,actor_id:$actor,actor_role:CASE WHEN 'owner' IN coalesce(actor.roles,[]) THEN 'owner' ELSE 'school_admin' END,target_id:$target,record_key:$key,kind:'teaching',school_id:$school,tenant_id:$tenant,version:grant.version,status:$status,created_at:datetime()}) RETURN grant.version AS version";
pub const STUDENT_QUERY:&str="MATCH (u:User {id:$target}) MERGE (lock:LearningBindingLock {key:'user|'+$target}) ON CREATE SET lock.version=0 SET lock.version=lock.version+1 WITH sec,u MATCH (actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WHERE ((NOT 'owner' IN coalesce(actor.roles,[]) AND 'school_admin' IN coalesce(actor.roles,[]) AND $school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[])) OR ('owner' IN coalesce(actor.roles,[]) AND size(coalesce(actor.teamIds,[]))=0 AND ((size(coalesce(actor.schoolIds,[]))=0 AND size(coalesce(actor.tenantIds,[]))=0) OR ($school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[]))) AND NOT EXISTS {MATCH(other:StaffMember {id:actor.id}) WHERE other<>actor} AND EXISTS {MATCH(:School {school_id:$school,tenant_id:$tenant})} AND NOT EXISTS {MATCH(one:School {school_id:$school,tenant_id:$tenant}),(two:School {school_id:$school,tenant_id:$tenant}) WHERE one<>two})) WITH actor,sec,u MATCH (student:Student {studentId:$student}) WHERE $active=false OR (u.role='student' AND sec.status='active' AND EXISTS { MATCH (student)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',tenant_id:$tenant,school_id:$school})-[:ENROLLED_IN]->(sec) }) OPTIONAL MATCH (existing:LearningStudentBinding {user_id:$target}) OPTIONAL MATCH (other:LearningStudentBinding {student_id:$student}) WITH actor,sec,u,existing,other WHERE ((existing IS NULL AND $version=0 AND $active=true) OR (existing.version=$version AND existing.student_id=$student AND existing.school_id=$school AND existing.tenant_id=$tenant)) AND (other IS NULL OR other.user_id=$target) MERGE (grant:LearningStudentBinding {user_id:$target}) ON CREATE SET grant.version=0,grant.student_id=$student,grant.school_id=$school,grant.tenant_id=$tenant SET grant.version=grant.version+1,grant.status=$status,grant.updated_at=datetime() CREATE (:LearningAccessAudit {id:$audit,actor_id:$actor,actor_role:CASE WHEN 'owner' IN coalesce(actor.roles,[]) THEN 'owner' ELSE 'school_admin' END,target_id:$target,record_key:$target,kind:'student_binding',student_id:$student,school_id:$school,tenant_id:$tenant,version:grant.version,status:$status,created_at:datetime()}) RETURN grant.version AS version";
async fn ensure_schema(graph: &neo4rs::Graph) -> Result<(), Failure> {
    let mut schema = graph.execute(query("SHOW CONSTRAINTS YIELD name WHERE name IN ['learning_teaching_key','learning_student_user','learning_student_identity','learning_access_audit','learning_binding_lock'] RETURN count(*) AS count")).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let count = schema
        .next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .and_then(|r| r.get::<i64>("count").ok());
    if count != Some(5) {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    Ok(())
}
pub async fn migrate(
    graph: &neo4rs::Graph,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    prework_policy::migrate(graph).await?;
    class_meetings::migrate(graph).await?;
    for statement in include_str!("../migrations/0001_learning_access.cypher")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        graph.run(query(statement)).await?;
    }
    ensure_schema(graph)
        .await
        .map_err(|_| "Learning access schema verification failed")?;
    Ok(())
}
async fn update(
    state: AppState,
    headers: HeaderMap,
    input: Result<Json<GrantRequest>, JsonRejection>,
    student: bool,
) -> Result<Json<Value>, Failure> {
    let Json(input) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if ![
        &input.tenant_id,
        &input.school_id,
        &input.section_id,
        &input.target_id,
    ]
    .iter()
    .all(|s| valid_id(s))
        || input.expected_version >= i32::MAX as u32
        || (student && !input.student_id.as_deref().is_some_and(valid_id))
        || (!student && input.student_id.is_some())
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (actor, actor_role) =
        administrator_actor(&headers, &input.tenant_id, &input.school_id).await?;
    ensure_schema(&state.graph).await?;
    // One implicit transaction commits state and immutable evidence together. CAS protects unknown HTTP outcomes.
    let q = query(&format!(
        "{SCOPE} {}",
        if student {
            STUDENT_QUERY
        } else {
            TEACHER_QUERY
        }
    ))
    .param("actor", actor.clone())
    .param("tenant", input.tenant_id.clone())
    .param("school", input.school_id.clone())
    .param("section", input.section_id.clone())
    .param("target", input.target_id.clone())
    .param("student", input.student_id.clone().unwrap_or_default())
    .param("key", format!("{}|{}", input.target_id, input.section_id))
    .param("version", i64::from(input.expected_version))
    .param("active", input.active)
    .param("status", if input.active { "ACTIVE" } else { "REVOKED" })
    .param("audit", uuid::Uuid::new_v4().to_string());
    let row = if actor_role=="owner" {
        let binding_lock=if student {"MATCH (u:User {id:$target}) MERGE (lock:LearningBindingLock {key:'user|'+$target}) ON CREATE SET lock.version=0 SET lock.version=lock.version+1 WITH sec"} else {"WITH sec"};
        let lock=query(&format!("{SCOPE} {binding_lock} RETURN sec.section_id AS id")).param("actor",actor.clone()).param("school",input.school_id.clone()).param("tenant",input.tenant_id.clone()).param("section",input.section_id.clone()).param("target",input.target_id.clone());
        crate::school_portal::staff_capability::live_write(&state.graph,crate::school_portal::staff_capability::OwnerSession{headers:&headers,actor:&actor,school:&input.school_id,tenant:&input.tenant_id,learning:false},lock,q).await?
    } else {
        let mut rows=state.graph.execute(q).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let row=rows.next().await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
        while rows.next().await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?.is_some(){}
        row
    }.ok_or_else(||failure(StatusCode::CONFLICT))?;
    let version: i64 = row
        .get("version")
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    Ok(Json(json!({"data":{"version":version}})))
}
async fn teacher(
    State(s): State<AppState>,
    h: HeaderMap,
    j: Result<Json<GrantRequest>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    update(s, h, j, false).await
}
async fn student(
    State(s): State<AppState>,
    h: HeaderMap,
    j: Result<Json<GrantRequest>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    update(s, h, j, true).await
}
async fn read_membership(mut response: reqwest::Response) -> Result<Membership, Failure> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        if body.len() + chunk.len() > 16 * 1024 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))
}
const READ_SCOPE: &str = "MATCH (actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WHERE ((NOT 'owner' IN coalesce(actor.roles,[]) AND 'school_admin' IN coalesce(actor.roles,[]) AND $school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[])) OR ('owner' IN coalesce(actor.roles,[]) AND size(coalesce(actor.teamIds,[]))=0 AND ((size(coalesce(actor.schoolIds,[]))=0 AND size(coalesce(actor.tenantIds,[]))=0) OR ($school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[]))) AND NOT EXISTS {MATCH(other:StaffMember {id:actor.id}) WHERE other<>actor} AND EXISTS {MATCH(:School {school_id:$school,tenant_id:$tenant})} AND NOT EXISTS {MATCH(one:School {school_id:$school,tenant_id:$tenant}),(two:School {school_id:$school,tenant_id:$tenant}) WHERE one<>two})) MATCH (sec:Section {section_id:$section,school_id:$school,tenant_id:$tenant})";
const READ_STUDENTS: &str = "MATCH (s:Student)-[:ENROLLED_AS]->(:EnrolledStudent {school_id:$school,tenant_id:$tenant})-[:ENROLLED_IN]->(sec) MATCH (g:LearningStudentBinding {student_id:s.studentId,school_id:$school,tenant_id:$tenant}) WITH DISTINCT g WHERE g.user_id>$after RETURN g.user_id AS target,g.student_id AS student,g.version AS version,g.status AS status ORDER BY target LIMIT 201";
const READ_TEACHERS: &str = "MATCH (g:LearningTeachingGrant {section_id:sec.section_id,school_id:$school,tenant_id:$tenant}) WITH g WHERE g.staff_member_id>$after RETURN g.staff_member_id AS target,null AS student,g.version AS version,g.status AS status ORDER BY target LIMIT 201";
fn read_query(student: bool) -> String {
    format!(
        "{READ_SCOPE} {}",
        if student {
            READ_STUDENTS
        } else {
            READ_TEACHERS
        }
    )
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScopeQuery {
    tenant_id: String,
    school_id: String,
    section_id: String,
    after: Option<String>,
}
async fn list(
    state: AppState,
    headers: HeaderMap,
    input: ScopeQuery,
    student: bool,
) -> Result<Json<Value>, Failure> {
    if ![&input.tenant_id, &input.school_id, &input.section_id]
        .iter()
        .all(|s| valid_id(s))
        || input.after.as_deref().is_some_and(|s| !valid_id(s))
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let actor = administrator(&headers, &input.tenant_id, &input.school_id).await?;
    ensure_schema(&state.graph).await?;
    let mut rows = state
        .graph
        .execute(
            query(&read_query(student))
                .param("actor", actor)
                .param("tenant", input.tenant_id)
                .param("school", input.school_id)
                .param("section", input.section_id)
                .param("after", input.after.unwrap_or_default()),
        )
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut items = vec![];
    while let Some(row) = rows
        .next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        let target: String = row
            .get("target")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let student: Option<String> = row
            .get("student")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let version: i64 = row
            .get("version")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let status: String = row
            .get("status")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        items
            .push(json!({"targetId":target,"studentId":student,"version":version,"status":status}));
    }
    let next = if items.len() > 200 {
        items.truncate(200);
        items.last().and_then(|v| v.get("targetId")).cloned()
    } else {
        None
    };
    Ok(Json(json!({"data":{"items":items,"next":next}})))
}
async fn teachers(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<Value>, Failure> {
    list(s, h, q, false).await
}
async fn students(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<Value>, Failure> {
    list(s, h, q, true).await
}
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(prework_policy::router())
        .merge(class_meetings::router())
        .merge(timetable::router())
        .merge(owner::router())
        .route(
            "/api/v1/sis-service/learning/context",
            axum::routing::get(context::context),
        )
        .route(
            "/api/v1/sis-service/learning/teaching-grants",
            post(teacher).get(teachers),
        )
        .route(
            "/api/v1/sis-service/learning/student-bindings",
            post(student).get(students),
        )
        .layer(axum::extract::DefaultBodyLimit::max(4096))
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
mod tests {
    use super::*;
    #[tokio::test]
    async fn introspection_body_is_bounded_before_decoding() {
        async fn response(body: String) -> (reqwest::Response, tokio::task::JoinHandle<()>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let app = Router::new().route("/", axum::routing::get(move || async move { body }));
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            (
                reqwest::get(format!("http://{addr}/")).await.unwrap(),
                server,
            )
        }
        let (large, server) = response("x".repeat(16 * 1024 + 1)).await;
        assert!(read_membership(large).await.is_err());
        server.abort();
        let (valid, server) = response(r#"{"active":false}"#.to_owned()).await;
        assert!(!read_membership(valid).await.unwrap().active);
        server.abort();
    }
    #[test]
    fn identifiers_cannot_change_query_keys() {
        assert!(valid_id("SEC-123"));
        assert!(!valid_id("a|b"));
        assert!(!valid_id(""));
        assert!(!valid_id("../x"));
    }
}

#[cfg(test)]
mod database_tests;
