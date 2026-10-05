//! Read-only SIS projection for Learning. The caller never supplies an actor ID.
use super::*;
use axum::routing::get;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use std::collections::{BTreeMap, BTreeSet};
mod student_school;

const TEACHER: &str = "MATCH (p:StaffMember {id:$principal,membershipStatus:'ACTIVE'}) WHERE NOT 'owner' IN coalesce(p.roles,[]) AND 'teacher' IN coalesce(p.roles,[]) AND $school IN coalesce(p.schoolIds,[]) AND $tenant IN coalesce(p.tenantIds,[]) MATCH (c:Section {school_id:$school,tenant_id:$tenant,status:'active'}) WHERE c.homeroom_staff_member_id=p.id OR EXISTS { MATCH (:LearningTeachingGrant {staff_member_id:p.id,section_id:c.section_id,status:'ACTIVE',school_id:$school,tenant_id:$tenant}) } OPTIONAL MATCH (s:Student)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant})-[:ENROLLED_IN]->(c)";
const STUDENT: &str = "MATCH (p:User {id:$principal,role:'student'}) MATCH (b:LearningStudentBinding {user_id:p.id,status:'ACTIVE',school_id:$school,tenant_id:$tenant}) MATCH (s:Student {studentId:b.student_id})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant})-[:ENROLLED_IN]->(c:Section {school_id:$school,tenant_id:$tenant,status:'active'})";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    school_id: String,
    tenant_id: String,
    class_id: Option<String>,
    student_id: Option<String>,
    access: Option<String>,
}
#[derive(Deserialize)]
struct Claims {
    sub: String,
    role: String,
    exp: usize,
    scope: Option<String>,
}
struct Actor {
    id: String,
    role: String,
}
async fn actor(headers: &HeaderMap, scope: &Scope) -> Result<Actor, Failure> {
    let token = crate::school_portal::auth::bearer(headers)?;
    match decode_header(token)
        .map_err(|_| failure(StatusCode::UNAUTHORIZED))?
        .alg
    {
        Algorithm::HS256 => {
            let secret = std::env::var("JWT_SECRET")
                .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
            if secret.len() < 32 {
                return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
            }
            let mut validation = Validation::new(Algorithm::HS256);
            validation.leeway = 0;
            let claims = decode::<Claims>(
                token,
                &DecodingKey::from_secret(secret.as_bytes()),
                &validation,
            )
            .map_err(|_| failure(StatusCode::UNAUTHORIZED))?
            .claims;
            let _ = claims.exp;
            if claims.scope.is_some() || !valid_id(&claims.sub) {
                return Err(failure(StatusCode::UNAUTHORIZED));
            }
            if !matches!(claims.role.as_str(), "parent" | "student") {
                return Err(failure(StatusCode::FORBIDDEN));
            }
            if claims.role == "student" {
                return Ok(Actor {
                    id: crate::school_portal::student_identity::actor(headers).await?,
                    role: claims.role,
                });
            }
            Ok(Actor {
                id: claims.sub,
                role: claims.role,
            })
        }
        Algorithm::RS256 => {
            let member =
                crate::school_portal::auth::teacher_in(headers, &scope.school_id, &scope.tenant_id)
                    .await?;
            if !member.school_ids.contains(&scope.school_id)
                || !member.tenant_ids.contains(&scope.tenant_id)
            {
                return Err(failure(StatusCode::FORBIDDEN));
            }
            let role = member.role().into();
            Ok(Actor {
                id: member.staff_member_id,
                role,
            })
        }
        _ => Err(failure(StatusCode::UNAUTHORIZED)),
    }
}
pub(super) async fn class_actor(
    state: &AppState,
    headers: &HeaderMap,
    school: &str,
    tenant: &str,
    class: &str,
) -> Result<(String, String), Failure> {
    let scope = Scope {
        school_id: school.into(),
        tenant_id: tenant.into(),
        class_id: Some(class.into()),
        student_id: None,
        access: None,
    };
    let a = actor(headers, &scope).await?;
    if !matches!(a.role.as_str(), "teacher" | "owner" | "student")
        || !projection(state, &a, &scope)
            .await?
            .iter()
            .any(|c| c["id"].as_str() == Some(class))
    {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    Ok((a.id, a.role))
}
async fn identity(graph: &neo4rs::Graph) -> Result<String, Failure> {
    let mut rows = graph
        .execute(query("CALL db.info() YIELD id RETURN id"))
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    rows.next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .and_then(|r| r.get::<String>("id").ok())
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))
}
// Operational identity only, like the existing SIS health endpoint. No accounts,
// topology, database credentials or school records appear in this response.
async fn health(State(s): State<AppState>) -> Result<Json<Value>, Failure> {
    super::ensure_schema(&s.graph).await?;
    Ok(Json(
        json!({"data":{"contractVersion":1,"databaseId":identity(&s.graph).await?}}),
    ))
}
async fn projection(s: &AppState, a: &Actor, scope: &Scope) -> Result<Vec<Value>, Failure> {
    super::ensure_schema(&s.graph).await?;
    let parent_query=format!("{} AND u.id=$principal WITH u,s MATCH (s)-[:ENROLLED_AS]->(:EnrolledStudent {{status:'active',school_id:$school,tenant_id:$tenant}})-[:ENROLLED_IN]->(c:Section {{school_id:$school,tenant_id:$tenant,status:'active'}})",crate::school_portal::repository::PARENT);
    let owner_scope = crate::school_portal::staff_capability::owner("p", "$school", "$tenant");
    let owner_query=format!("MATCH(p:StaffMember {{id:$principal,membershipStatus:'ACTIVE'}}) WHERE {owner_scope} MATCH(c:Section {{school_id:$school,tenant_id:$tenant,status:'active'}}) OPTIONAL MATCH(s:Student)-[:ENROLLED_AS]->(:EnrolledStudent {{status:'active',school_id:$school,tenant_id:$tenant}})-[:ENROLLED_IN]->(c)");
    let base = match a.role.as_str() {
        "owner" => owner_query.as_str(),
        "teacher" => TEACHER,
        "student" => STUDENT,
        "parent" => &parent_query,
        _ => return Err(failure(StatusCode::FORBIDDEN)),
    };
    let mut rows = s.graph.execute(query(&format!("{base} RETURN DISTINCT c.section_id AS class_id,c.name AS class_name,s.studentId AS student_id,s.fullName AS student_name ORDER BY class_id,student_id LIMIT 1001"))
        .param("principal",a.id.clone()).param("sub",a.id.clone()).param("school",scope.school_id.clone()).param("tenant",scope.tenant_id.clone()))
        .await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut classes = BTreeMap::<String, Value>::new();
    let mut student_ids = BTreeSet::new();
    let mut count = 0;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        count += 1;
        if count > 1000 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        let id: String = row
            .get("class_id")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let name: String = row
            .get("class_name")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        if !valid_id(&id) || name.is_empty() || name.len() > 256 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        let class = classes
            .entry(id.clone())
            .or_insert_with(|| json!({"id":id,"name":name,"students":[]}));
        if let Some(id) = row
            .get::<Option<String>>("student_id")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        {
            let name: String = row
                .get("student_name")
                .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
            if !valid_id(&id) || name.is_empty() || name.len() > 256 {
                return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
            }
            student_ids.insert(id.clone());
            class["students"]
                .as_array_mut()
                .ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))?
                .push(json!({"id":id,"name":name}));
        }
    }
    if a.role == "student" && student_ids.len() > 1 {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    Ok(classes.into_values().collect())
}
async fn context(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<Scope>,
) -> Result<Json<Value>, Failure> {
    if !valid_id(&q.school_id)
        || !valid_id(&q.tenant_id)
        || q.class_id.is_some()
        || q.student_id.is_some()
        || q.access.is_some()
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let a = actor(&h, &q).await?;
    let classes = projection(&s, &a, &q).await?;
    Ok(Json(
        json!({"data":{"contractVersion":1,"databaseId":identity(&s.graph).await?,"actor":{"id":a.id,"role":a.role},"schoolId":q.school_id,"tenantId":q.tenant_id,"classes":classes}}),
    ))
}
async fn authorize(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<Scope>,
) -> Result<Json<Value>, Failure> {
    if !valid_id(&q.school_id)
        || !valid_id(&q.tenant_id)
        || !q.class_id.as_deref().is_some_and(valid_id)
        || q.student_id.as_deref().is_some_and(|v| !valid_id(v))
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let a = actor(&h, &q).await?;
    let compatible = matches!(
        (a.role.as_str(), q.access.as_deref()),
        ("teacher", Some("teach"))
            | ("owner", Some("teach"))
            | ("student", Some("learn"))
            | ("parent", Some("parent"))
    );
    if !compatible || matches!(a.role.as_str(), "teacher" | "owner") != q.student_id.is_none() {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let classes = projection(&s, &a, &q).await?;
    let allowed = classes.iter().any(|c| {
        c["id"].as_str() == q.class_id.as_deref()
            && (matches!(a.role.as_str(), "teacher" | "owner")
                || c["students"].as_array().is_some_and(|ss| {
                    ss.iter()
                        .any(|s| s["id"].as_str() == q.student_id.as_deref())
                }))
    });
    if !allowed {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    Ok(Json(
        json!({"data":{"contractVersion":1,"databaseId":identity(&s.graph).await?,"actor":{"id":a.id,"role":a.role},"schoolId":q.school_id,"tenantId":q.tenant_id,"allowed":true}}),
    ))
}
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/learning/owner/student/calendar",
            get(student_school::calendar),
        )
        .route("/api/v1/sis-service/learning/owner/context", get(context))
        .route(
            "/api/v1/sis-service/learning/owner/authorize",
            get(authorize),
        )
        .route("/api/v1/sis-service/learning/owner/health", get(health))
        .route(
            "/api/v1/sis-service/learning/owner/student/school",
            get(student_school::handler),
        )
}
#[cfg(test)]
mod tests;
