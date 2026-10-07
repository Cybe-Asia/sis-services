//! A placed child's family parent binds the Auth student account Auth has just prepared
//! to that child (the SIS step of parent LMS activation). SIS never creates accounts: it
//! only reads the pending Auth account for this enrollment. Revoked or conflicting
//! bindings stay a staff decision.
use super::*;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindRequest {
    pub student_id: String,
}
#[derive(Deserialize)]
struct Claims {
    sub: String,
    #[serde(default = "parent_role")]
    role: String,
    #[allow(dead_code)]
    exp: usize,
    scope: Option<String>,
}
fn parent_role() -> String {
    "parent".into()
}

fn enabled() -> bool {
    std::env::var("SIS_PARENT_LEARNING_BINDING_ENABLED").as_deref() == Ok("true")
}

/// A non-scoped HS256 parent access token; the caller never supplies an actor id.
fn parent(headers: &HeaderMap) -> Result<String, Failure> {
    let secret =
        std::env::var("JWT_SECRET").map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    parent_with(headers, &secret)
}
fn parent_with(headers: &HeaderMap, secret: &str) -> Result<String, Failure> {
    let token = crate::school_portal::auth::bearer(headers)?;
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
    if claims.scope.is_some() || !valid_id(&claims.sub) {
        return Err(failure(StatusCode::UNAUTHORIZED));
    }
    if claims.role != "parent" {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    Ok(claims.sub)
}

/// The child's single verified family parent (same rule as Auth eligibility), the child's
/// single active placement, and the pending/active Auth account for exactly that enrollment.
pub const FAMILY: &str = "MATCH (parent:User {id:$parent})-[:HAS_APPLICATION]->(lead:Lead)-[:HAS_STUDENT]->(s:Student {studentId:$student}) WHERE coalesce(parent.role,'parent')='parent' AND coalesce(parent.staffMemberId,'')='' AND all(role IN coalesce(parent.roles,[]) WHERE role='parent') AND NOT (parent)-[:STAFF_MEMBER]->() AND NOT (parent)-[:STAFF_PROFILE]->() AND coalesce(parent.email,'')<>'' AND toLower(parent.email)=toLower(lead.email) AND (lead.email_otp_verified_at IS NOT NULL OR coalesce(lead.otp_verified,false)=true OR EXISTS { MATCH (:ParentLoginChallenge {purpose:'parent_login',userId:parent.id,leadId:lead.lead_id,email:toLower(parent.email),channel:'email',isUsed:true,delivered:true}) }) WITH DISTINCT s WHERE COUNT { MATCH (other:User)-[:HAS_APPLICATION]->(:Lead)-[:HAS_STUDENT]->(s) WHERE coalesce(other.role,'parent')='parent' } = 1 MATCH (s)-[:ENROLLED_AS]->(e:EnrolledStudent {status:'active'})-[:ENROLLED_IN]->(sec:Section {status:'active'}) WHERE sec.school_id=e.school_id AND sec.tenant_id=e.tenant_id AND NOT EXISTS { MATCH (s)-[:ENROLLED_AS]->(other:EnrolledStudent {status:'active'}) WHERE other<>e } MATCH (account:AuthStudentAccount {enrolled_student_id:e.student_id}) WHERE account.student_id=s.studentId AND account.school_id=e.school_id AND account.tenant_id=e.tenant_id AND account.student_number=e.student_number AND account.status IN ['PENDING','ACTIVE'] MATCH (u:User {id:account.user_id,role:'student',authStudentAccount:true})";

/// Locks section then binding (the staff order), then creates the binding only when
/// neither the account nor the child has one; an identical active binding is a no-op.
pub const BIND: &str = " SET sec.learning_access_lock=coalesce(sec.learning_access_lock,0)+1 MERGE (lock:LearningBindingLock {key:'user|'+u.id}) ON CREATE SET lock.version=0 SET lock.version=lock.version+1 WITH s,e,u OPTIONAL MATCH (existing:LearningStudentBinding {user_id:u.id}) OPTIONAL MATCH (other:LearningStudentBinding {student_id:s.studentId}) WITH s,e,u,existing,other WHERE (existing IS NULL AND other IS NULL) OR (existing IS NOT NULL AND existing=other AND existing.status='ACTIVE' AND existing.school_id=e.school_id AND existing.tenant_id=e.tenant_id) CALL { WITH s,e,u,existing WITH s,e,u,existing WHERE existing IS NULL CREATE (b:LearningStudentBinding {user_id:u.id,version:1,student_id:s.studentId,school_id:e.school_id,tenant_id:e.tenant_id,status:'ACTIVE',updated_at:datetime()}) CREATE (:LearningAccessAudit {id:$audit,actor_id:$parent,actor_role:'parent',target_id:u.id,record_key:u.id,kind:'student_binding',student_id:s.studentId,school_id:e.school_id,tenant_id:e.tenant_id,version:1,status:'ACTIVE',created_at:datetime()}) } MATCH (b:LearningStudentBinding {user_id:u.id}) RETURN b.version AS version,b.status AS status";

pub async fn bind(
    graph: &neo4rs::Graph,
    parent: &str,
    student: &str,
) -> Result<(i64, String), Failure> {
    let unavailable = |_| failure(StatusCode::SERVICE_UNAVAILABLE);
    let params = |cypher: String| {
        query(&cypher)
            .param("parent", parent.to_string())
            .param("student", student.to_string())
            .param("audit", uuid::Uuid::new_v4().to_string())
    };
    let mut eligible = graph
        .execute(params(format!("{FAMILY} RETURN u.id AS user")))
        .await
        .map_err(unavailable)?;
    if eligible.next().await.map_err(unavailable)?.is_none() {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    if eligible.next().await.map_err(unavailable)?.is_some() {
        return Err(failure(StatusCode::CONFLICT));
    }
    drop(eligible);
    // One implicit transaction re-checks the family and writes binding and audit together.
    let mut rows = graph
        .execute(params(format!("{FAMILY}{BIND}")))
        .await
        .map_err(unavailable)?;
    let row = rows
        .next()
        .await
        .map_err(unavailable)?
        .ok_or_else(|| failure(StatusCode::CONFLICT))?;
    while rows.next().await.map_err(unavailable)?.is_some() {}
    Ok((
        row.get("version")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?,
        row.get("status")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?,
    ))
}

async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    input: Result<Json<BindRequest>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    if !enabled() {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    let parent = parent(&headers)?;
    let Json(input) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !valid_id(&input.student_id) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    ensure_schema(&state.graph).await?;
    let (version, status) = bind(&state.graph, &parent, &input.student_id).await?;
    Ok(Json(json!({"data":{"version":version,"status":status}})))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/leads/v1/me/learning/student-binding", post(handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};

    fn token(secret: &str, role: &str, scope: Option<&str>) -> String {
        let exp = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 300) as usize;
        encode(
            &Header::default(),
            &json!({"sub":"parent-user","email":"p@example.test","role":role,"exp":exp,"scope":scope}),
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    #[test]
    fn only_unscoped_parent_access_tokens_bind() {
        let secret = "x".repeat(32);
        let parent = |h: &HeaderMap| parent_with(h, &secret);
        let headers = |t: &str| {
            let mut h = HeaderMap::new();
            h.insert("authorization", format!("Bearer {t}").parse().unwrap());
            h
        };
        assert_eq!(
            parent(&headers(&token(&secret, "parent", None))).unwrap(),
            "parent-user"
        );
        assert_eq!(
            parent(&headers(&token(&secret, "student", None)))
                .unwrap_err()
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            parent(&headers(&token(&secret, "parent", Some("otp"))))
                .unwrap_err()
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            parent(&headers(&token(&"y".repeat(32), "parent", None)))
                .unwrap_err()
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert!(
            serde_json::from_value::<BindRequest>(json!({"studentId":"s","userId":"u"})).is_err()
        );
    }

    #[tokio::test]
    #[ignore = "requires disposable loopback LEARNING_TEST_NEO4J_* configuration"]
    async fn parent_binds_only_its_childs_pending_account_once() {
        let uri = std::env::var("LEARNING_TEST_NEO4J_URI").unwrap();
        assert!(uri.starts_with("bolt://127.0.0.1:"));
        let graph = neo4rs::Graph::new(
            uri,
            "neo4j",
            std::env::var("LEARNING_TEST_NEO4J_PASSWORD").unwrap(),
        )
        .await
        .unwrap();
        for statement in include_str!("../../migrations/0001_learning_access.cypher")
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            graph.run(query(statement)).await.unwrap();
        }
        let mark = uuid::Uuid::new_v4().simple().to_string();
        let parent = format!("parent-{mark}");
        let stranger = format!("stranger-{mark}");
        let user = format!("user-{mark}");
        graph.run(query("CREATE (:User {id:$stranger,role:'parent',email:'s-'+$mark+'@example.test',parent_binding_test:$mark}) CREATE (p:User {id:$parent,role:'parent',email:$mark+'@example.test',parent_binding_test:$mark})-[:HAS_APPLICATION]->(:Lead {lead_id:$mark,email:$mark+'@example.test',email_otp_verified_at:datetime(),parent_binding_test:$mark})-[:HAS_STUDENT]->(s:Student {studentId:$mark,parent_binding_test:$mark})-[:ENROLLED_AS]->(e:EnrolledStudent {student_id:$mark,student_number:'N-'+$mark,school_id:'school-test',tenant_id:'tenant-test',status:'active',parent_binding_test:$mark})-[:ENROLLED_IN]->(:Section {section_id:$mark,school_id:'school-test',tenant_id:'tenant-test',status:'active',parent_binding_test:$mark})").param("parent",parent.clone()).param("stranger",stranger.clone()).param("mark",mark.clone())).await.unwrap();
        // Auth has not prepared an account yet.
        assert_eq!(
            bind(&graph, &parent, &mark).await.unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        graph.run(query("CREATE (:AuthStudentAccount {enrolled_student_id:$mark,user_id:$user,student_id:$mark,school_id:'school-test',tenant_id:'tenant-test',student_number:'N-'+$mark,status:'PENDING',parent_binding_test:$mark}) CREATE (:User {id:$user,role:'student',authStudentAccount:true,parent_binding_test:$mark})").param("user",user.clone()).param("mark",mark.clone())).await.unwrap();
        assert_eq!(
            bind(&graph, &stranger, &mark).await.unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            bind(&graph, &parent, &mark).await.unwrap(),
            (1, "ACTIVE".to_string())
        );
        assert_eq!(
            bind(&graph, &parent, &mark).await.unwrap(),
            (1, "ACTIVE".to_string())
        );
        let mut audits = graph.execute(query("MATCH (a:LearningAccessAudit {target_id:$user,actor_role:'parent'}) RETURN count(a) AS count").param("user",user.clone())).await.unwrap();
        assert_eq!(
            audits
                .next()
                .await
                .unwrap()
                .unwrap()
                .get::<i64>("count")
                .unwrap(),
            1
        );
        drop(audits);
        // A revoked binding is a staff decision.
        graph
            .run(
                query("MATCH (b:LearningStudentBinding {user_id:$user}) SET b.status='REVOKED'")
                    .param("user", user.clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            bind(&graph, &parent, &mark).await.unwrap_err().0,
            StatusCode::CONFLICT
        );
        graph.run(query("MATCH (n) WHERE n.parent_binding_test=$mark OR (n:LearningStudentBinding AND n.user_id=$user) OR (n:LearningAccessAudit AND n.target_id=$user) OR (n:LearningBindingLock AND n.key='user|'+$user) DETACH DELETE n").param("mark",mark).param("user",user)).await.unwrap();
    }
}
