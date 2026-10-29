//! Read-only SIS projection for Learning. The caller never supplies an actor ID.
use super::*;
use axum::routing::get;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use std::collections::{BTreeMap, BTreeSet};
mod school_classes;
mod student_profile;
mod student_school;

pub(crate) const TEACHER: &str = "MATCH (p:StaffMember {id:$principal,membershipStatus:'ACTIVE'}) WHERE NOT 'owner' IN coalesce(p.roles,[]) AND 'teacher' IN coalesce(p.roles,[]) AND $school IN coalesce(p.schoolIds,[]) AND $tenant IN coalesce(p.tenantIds,[]) MATCH (c:Section {school_id:$school,tenant_id:$tenant,status:'active'}) WHERE c.homeroom_staff_member_id=p.id OR EXISTS { MATCH (:LearningTeachingGrant {staff_member_id:p.id,section_id:c.section_id,status:'ACTIVE',school_id:$school,tenant_id:$tenant}) } OPTIONAL MATCH (s:Student)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant})-[:ENROLLED_IN]->(c)";
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
/// The current answer for one access kind. `homeroom` (ADR 0015) is narrower than `teach`: a
/// Teacher qualifies only as the class's homeroom Staff Member, never through a teaching grant;
/// an Owner qualifies by the same live scope as for `teach`.
async fn permitted(s: &AppState, a: &Actor, q: &Scope) -> Result<(), Failure> {
    let compatible = matches!(
        (a.role.as_str(), q.access.as_deref()),
        ("teacher", Some("teach" | "homeroom"))
            | ("owner", Some("teach" | "homeroom"))
            | ("student", Some("learn"))
            | ("parent", Some("parent"))
    );
    if !compatible || matches!(a.role.as_str(), "teacher" | "owner") != q.student_id.is_none() {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let classes = projection(s, a, q).await?;
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
    if a.role == "teacher" && q.access.as_deref() == Some("homeroom") {
        let mut rows = s.graph.execute(query("MATCH (c:Section {section_id:$class,school_id:$school,tenant_id:$tenant,status:'active'}) WHERE c.homeroom_staff_member_id=$principal RETURN count(c) AS n")
            .param("class", q.class_id.clone().unwrap_or_default()).param("school", q.school_id.clone()).param("tenant", q.tenant_id.clone()).param("principal", a.id.clone()))
            .await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let n: i64 = rows
            .next()
            .await
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
            .and_then(|r| r.get("n").ok())
            .ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        if n != 1 {
            return Err(failure(StatusCode::FORBIDDEN));
        }
    }
    Ok(())
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
    permitted(&s, &a, &q).await?;
    let mut data = json!({"contractVersion":1,"databaseId":identity(&s.graph).await?,"actor":{"id":a.id,"role":a.role},"schoolId":q.school_id,"tenantId":q.tenant_id,"allowed":true});
    // ADR 0018: what the staff member may do in the class, for Learning to move to
    // capabilities. Informational until it does; `allowed` above is unchanged.
    if matches!(a.role.as_str(), "teacher" | "owner") {
        let class = q.class_id.clone().unwrap_or_default();
        let capabilities = crate::authorization::reads::learning_capabilities(
            &s.graph,
            &q.tenant_id,
            &q.school_id,
            &class,
            &a.id,
        )
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        data["capabilities"] = json!(capabilities);
    }
    Ok(Json(json!({ "data": data })))
}
/// ADR 0018 slice 5: the caller's `learning.*` capabilities inside the pinned school, by scope, so
/// Learning can check school and subject roles (curriculum, plans, review). The caller is
/// authenticated and pinned exactly as for `authorize`; Students and Parents hold none here.
async fn capabilities(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<Scope>,
) -> Result<axum::response::Response, Failure> {
    use axum::response::IntoResponse;
    if !valid_id(&q.school_id)
        || !valid_id(&q.tenant_id)
        || q.class_id.is_some()
        || q.student_id.is_some()
        || q.access.is_some()
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let a = actor(&h, &q).await?;
    if !matches!(a.role.as_str(), "teacher" | "owner") {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    super::ensure_schema(&s.graph).await?;
    let (capabilities, version) =
        crate::authorization::reads::learning_scopes(&s.graph, &q.tenant_id, &q.school_id, &a.id)
            .await
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
            .ok_or_else(|| failure(StatusCode::FORBIDDEN))?;
    let mut response = Json(json!({"data":{"contractVersion":1,"capabilities":capabilities,"authorizationVersion":version,"cacheSeconds":crate::authorization::model::CACHE_SECONDS}})).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RosterQuery {
    school_id: String,
    tenant_id: String,
    subject: Option<String>,
    grade: Option<String>,
}
/// ADR 0018 remainder: the active classes and students of the pinned school for which the caller
/// holds `learning.mastery.read`, so Learning can show Mastery to a curriculum coordinator or
/// subject lead who teaches no class. Authenticated and pinned as `capabilities`; the answer is
/// always about the caller. `subject` keeps classes where that subject is covered, `grade` keeps
/// one grade ("Year 7" or "7").
async fn mastery_roster(
    State(s): State<AppState>,
    h: HeaderMap,
    q: Query<RosterQuery>,
) -> Result<axum::response::Response, Failure> {
    use axum::response::IntoResponse;
    let Query(q) = q;
    let grade_ok = |g: &str| {
        crate::academic_staffing::model::key_part(g, 64)
            && !g.contains('#')
            && crate::authorization::model::grade_number(g).is_some()
    };
    if !valid_id(&q.school_id)
        || !valid_id(&q.tenant_id)
        || !q
            .subject
            .as_deref()
            .is_none_or(crate::academic_staffing::model::subject_code)
        || !q.grade.as_deref().is_none_or(grade_ok)
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let scope = Scope {
        school_id: q.school_id.clone(),
        tenant_id: q.tenant_id.clone(),
        class_id: None,
        student_id: None,
        access: None,
    };
    let a = actor(&h, &scope).await?;
    if !matches!(a.role.as_str(), "teacher" | "owner") {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    super::ensure_schema(&s.graph).await?;
    let (classes, version) = crate::authorization::reads::mastery_roster(
        &s.graph,
        &q.tenant_id,
        &q.school_id,
        &a.id,
        q.subject.as_deref(),
        q.grade.as_deref(),
    )
    .await
    .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    .ok_or_else(|| failure(StatusCode::FORBIDDEN))?;
    let mut response = Json(json!({"data":{"contractVersion":1,"schoolId":q.school_id,"tenantId":q.tenant_id,"classes":classes,"authorizationVersion":version,"cacheSeconds":crate::authorization::model::CACHE_SECONDS}})).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/learning/owner/student/calendar",
            get(student_school::calendar),
        )
        .route("/api/v1/sis-service/learning/owner/context", get(context))
        .route(
            "/api/v1/sis-service/learning/owner/classes",
            get(school_classes::handler),
        )
        .route(
            "/api/v1/sis-service/learning/owner/authorize",
            get(authorize),
        )
        .route(
            "/api/v1/sis-service/learning/owner/capabilities",
            get(capabilities),
        )
        .route(
            "/api/v1/sis-service/learning/owner/mastery-roster",
            get(mastery_roster),
        )
        .route("/api/v1/sis-service/learning/owner/health", get(health))
        .route(
            "/api/v1/sis-service/learning/owner/student/school",
            get(student_school::handler),
        )
        .route(
            "/api/v1/sis-service/learning/owner/student/profile",
            get(student_profile::handler),
        )
}
#[cfg(test)]
mod tests;
