//! ADR 0027 (Digital Schools workspace): who is shopping, or working, in the school Shop. shop-services
//! calls this with the user's own bearer, so it holds no SIS or Auth secret. A Student shops for
//! themself, a Parent for each eligible child with an active enrolment, and staff work in the schools
//! of their membership. Contract: `docs/AUTHORIZATION.md`, `GET /api/leads/v1/shop/context`.
use super::{
    auth, envelope, failure,
    repository::{PARENT, PARENT_ELIGIBLE},
    safety::OWNED,
    student_identity, Failure,
};
use crate::{
    authorization::{facts::Db, reads::permitted_schools},
    AppState,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use neo4rs::{query, Graph, Query};
use serde_json::{json, Value};
type Error = Box<dyn std::error::Error + Send + Sync>;

const MANAGE: &str = "shop.manage";
const OPERATE: &str = "shop.operate";
/// More children or schools than this for one caller is a broken data set, not a family or a staff member.
const MAX_ROWS: usize = 64;
/// An active enrolment of `s` with the id payment-services keeps the Student Credit wallet under.
const ENROLMENT: &str = "MATCH(s)-[:ENROLLED_AS]->(e:EnrolledStudent {status:'active'})-[:ENROLLED_IN]->(sec:Section {status:'active'}) \
     WHERE e.school_id=sec.school_id AND e.tenant_id=sec.tenant_id AND coalesce(e.school_id,'')<>'' \
       AND coalesce(e.tenant_id,'')<>'' AND coalesce(e.student_id,'')<>''";
const STUDENT_ROW: &str = "e.student_id AS student,coalesce(s.fullName,'') AS name,e.school_id AS school,e.tenant_id AS tenant";

/// Students with their schools, one school entry per school. Nobody shops as an admin or as staff here.
async fn shoppers(graph: &Graph, q: Query) -> Result<(Vec<Value>, Vec<Value>), Error> {
    let mut rows = graph.execute(q).await?;
    let (mut students, mut schools): (Vec<Value>, Vec<Value>) = (vec![], vec![]);
    while let Some(r) = rows.next().await? {
        let (student, school): (String, String) = (r.get("student")?, r.get("school")?);
        // shortcut: a Student active in two classes keeps the first, as in messaging members.
        if students.len() == MAX_ROWS || students.iter().any(|v| v["studentId"] == student) {
            continue;
        }
        if !schools.iter().any(|v| v["schoolId"] == school) {
            schools.push(json!({"schoolId": school, "tenantId": r.get::<String>("tenant")?, "shopAdmin": false, "shopStaff": false}));
        }
        students.push(
            json!({"studentId": student, "name": r.get::<String>("name")?, "schoolId": school}),
        );
    }
    Ok((students, schools))
}

/// `None` when the account is not exactly one Student with one active enrolment that payment knows.
pub(super) async fn student(graph: &Graph, actor: &str) -> Result<Option<Value>, Error> {
    let q = query(&format!(
        "{OWNED} WITH DISTINCT s,en AS e WHERE coalesce(e.student_id,'')<>'' RETURN {STUDENT_ROW} ORDER BY student LIMIT 2"
    ))
    .param("actor", actor);
    let (students, schools) = shoppers(graph, q).await?;
    if students.len() != 1 {
        return Ok(None);
    }
    Ok(Some(json!({
        "me": {"id": format!("student:{actor}"), "role": "student"},
        "schools": schools,
        "students": students,
    })))
}

/// `None` when the token names no single eligible Parent. A Parent without an enrolled child still
/// gets an answer, with nobody to shop for.
pub(super) async fn parent(graph: &Graph, sub: &str) -> Result<Option<Value>, Error> {
    let mut me = graph
        .execute(
            query(&format!(
                "{PARENT} WITH collect(DISTINCT u) AS owners WHERE size(owners)=1 RETURN owners[0].id AS id"
            ))
            .param("sub", sub),
        )
        .await?;
    let Some(me) = me.next().await? else {
        return Ok(None);
    };
    let id: String = me.get("id")?;
    // Each child is matched again from the one owner, so a child of another Lead of theirs counts too.
    let q = query(&format!(
        "MATCH(u:User {{id:$owner}})-[:HAS_APPLICATION]->(l:Lead)-[:HAS_STUDENT]->(s:Student) WHERE {PARENT_ELIGIBLE} \
         WITH DISTINCT s {ENROLMENT} RETURN DISTINCT {STUDENT_ROW} ORDER BY name,student LIMIT 200"
    ))
    .param("owner", id.clone());
    let (students, schools) = shoppers(graph, q).await?;
    Ok(Some(json!({
        "me": {"id": format!("parent:{id}"), "role": "parent"},
        "schools": schools,
        "students": students,
    })))
}

/// A staff member as Auth reports them on an Admin portal session; `role` is `owner`,
/// `school_admin` or `staff`.
pub(super) struct Staff {
    pub id: String,
    pub role: String,
    pub school_ids: Vec<String>,
    pub tenant_ids: Vec<String>,
}

/// The schools of the membership (every school for an unscoped Owner), each with what the caller may
/// do in its Shop. A school that is not one `School` node of its tenant is left out.
pub(super) async fn staff(graph: &Graph, member: &Staff) -> Result<Value, Failure> {
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let everywhere =
        member.role == "owner" && member.school_ids.is_empty() && member.tenant_ids.is_empty();
    let mut rows = graph
        .execute(
            query(
                "MATCH(sc:School) WHERE coalesce(sc.school_id,'')<>'' AND coalesce(sc.tenant_id,'')<>'' \
                   AND ($everywhere OR (sc.school_id IN $schools AND sc.tenant_id IN $tenants)) \
                 RETURN DISTINCT sc.school_id AS school,sc.tenant_id AS tenant ORDER BY tenant,school LIMIT 65",
            )
            .param("everywhere", everywhere)
            .param("schools", member.school_ids.clone())
            .param("tenants", member.tenant_ids.clone()),
        )
        .await
        .map_err(|_| unavailable())?;
    let mut places: Vec<(String, String)> = vec![];
    while let Some(r) = rows.next().await.map_err(|_| unavailable())? {
        if places.len() < MAX_ROWS {
            places.push((
                r.get("tenant").map_err(|_| unavailable())?,
                r.get("school").map_err(|_| unavailable())?,
            ));
        }
    }
    let mut tenants: Vec<String> = places.iter().map(|(tenant, _)| tenant.clone()).collect();
    tenants.dedup();
    let mut schools = vec![];
    // One capability read per tenant and code; a membership spans very few tenants.
    for tenant in tenants {
        let here: Vec<String> = places
            .iter()
            .filter(|(t, _)| *t == tenant)
            .map(|(_, s)| s.clone())
            .collect();
        let mut db = Db::Graph(graph);
        let manage = permitted_schools(&mut db, &tenant, &here, &member.id, MANAGE).await?;
        let operate = permitted_schools(&mut db, &tenant, &here, &member.id, OPERATE).await?;
        for school in here {
            // An Owner and a school administrator manage the Shop by role, as Auth reports it for this
            // session; everyone else needs the capability.
            let admin = matches!(member.role.as_str(), "owner" | "school_admin")
                || manage.contains(&school);
            let works = admin || operate.contains(&school);
            schools.push(json!({"schoolId": school, "tenantId": tenant, "shopAdmin": admin, "shopStaff": works}));
        }
    }
    Ok(json!({
        "me": {"id": format!("staff:{}", member.id), "role": "staff"},
        "schools": schools,
        "students": [],
    }))
}

/// The `role` claim, unverified: it only chooses which verifier reads the token.
fn claimed_role(headers: &HeaderMap) -> Option<String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let payload = auth::bearer(headers).ok()?.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    claims.get("role")?.as_str().map(str::to_owned)
}

pub(super) async fn context(
    State(s): State<AppState>,
    h: HeaderMap,
) -> Result<Json<Value>, Failure> {
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let forbidden = || failure(StatusCode::FORBIDDEN);
    // A Student token also verifies as a Parent token (same key, no role check there), so it goes first.
    if claimed_role(&h).as_deref() == Some("student") {
        let actor = student_identity::actor(&s.config, &h).await?;
        let data = student(&s.graph, &actor).await.map_err(|_| unavailable())?;
        return Ok(envelope(data.ok_or_else(forbidden)?));
    }
    let data = match auth::parent(&s.config, &h) {
        Ok(sub) => parent(&s.graph, &sub)
            .await
            .map_err(|_| unavailable())?
            .ok_or_else(forbidden)?,
        // Anything the Parent check rejects as unauthorised may be a staff session for Auth.
        // A Shop staff app token (Staff SSO module token) is checked with Auth's membership read.
        Err((StatusCode::UNAUTHORIZED, _))
            if auth::claimed_audience(&h).as_deref() == Some(auth::SHOP_AUDIENCE) =>
        {
            let member = auth::shop_staff(&s.config, &h).await?;
            let role = if member.is_owner() {
                "owner"
            } else if member.roles.iter().any(|r| r == "school_admin") {
                "school_admin"
            } else {
                "staff"
            };
            let who = Staff {
                id: member.staff_member_id,
                role: role.into(),
                school_ids: member.school_ids,
                tenant_ids: member.tenant_ids,
            };
            staff(&s.graph, &who).await?
        }
        Err((StatusCode::UNAUTHORIZED, _)) => {
            let (id, role, school_ids, tenant_ids) =
                crate::learning_access::any_staff_session(&s.config, &h)
                    .await?
                    .staff_scope();
            staff(
                &s.graph,
                &Staff {
                    id,
                    role,
                    school_ids,
                    tenant_ids,
                },
            )
            .await?
        }
        Err(e) => return Err(e),
    };
    Ok(envelope(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires disposable SHOP_CONTEXT_TEST_BOLT graph"]
    async fn shop_context_by_caller() {
        let uri = std::env::var("SHOP_CONTEXT_TEST_BOLT").unwrap();
        assert!(uri.starts_with("127.0.0.1:"));
        let g = Graph::new(uri, "neo4j", "parent-otp-fixture-password")
            .await
            .unwrap();
        g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
        crate::authorization::init(&g).await.unwrap();
        g.run(query("CREATE(:School {school_id:'school',tenant_id:'tenant',name:'IISS'}) CREATE(:School {school_id:'other',tenant_id:'tenant',name:'IIHS'}) \
            CREATE(sec:Section {section_id:'class',name:'7A',school_id:'school',tenant_id:'tenant',status:'active'}) \
            CREATE(s:Student {studentId:'child',fullName:'Aisha'})-[:ENROLLED_AS]->(:EnrolledStudent {student_id:'ENR-1',status:'active',school_id:'school',tenant_id:'tenant'})-[:ENROLLED_IN]->(sec) \
            CREATE(b:Student {studentId:'brother',fullName:'Bima'})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'school',tenant_id:'tenant'})-[:ENROLLED_IN]->(sec) \
            CREATE(g:Student {studentId:'graduate',fullName:'Citra'})-[:ENROLLED_AS]->(:EnrolledStudent {student_id:'ENR-3',status:'withdrawn',school_id:'school',tenant_id:'tenant'})-[:ENROLLED_IN]->(sec) \
            CREATE(m:User {id:'mother',email:'mother@example.test',role:'parent'})-[:HAS_APPLICATION]->(l:Lead {lead_id:'LEAD-MOTHER',email:'mother@example.test',status:'verified'}) \
            CREATE(l)-[:HAS_STUDENT]->(s) CREATE(l)-[:HAS_STUDENT]->(b) CREATE(l)-[:HAS_STUDENT]->(g) \
            CREATE(:User {id:'pending',email:'pending@example.test',role:'parent'})-[:HAS_APPLICATION]->(:Lead {lead_id:'LEAD-PENDING',email:'pending@example.test',status:'new'})-[:HAS_STUDENT]->(s) \
            CREATE(:User {id:'childless',email:'childless@example.test',role:'parent'})-[:HAS_APPLICATION]->(:Lead {lead_id:'LEAD-NONE',email:'childless@example.test',status:'verified'})-[:HAS_STUDENT]->(:Student {studentId:'applicant',fullName:'Dewi'}) \
            CREATE(:User {id:'aisha',email:'aisha@example.test',role:'student',roles:['student']}) \
            CREATE(:LearningStudentBinding {user_id:'aisha',student_id:'child',school_id:'school',tenant_id:'tenant',status:'ACTIVE'}) \
            CREATE(:User {id:'bima',email:'bima@example.test',role:'student',roles:['student']}) \
            CREATE(:LearningStudentBinding {user_id:'bima',student_id:'brother',school_id:'school',tenant_id:'tenant',status:'ACTIVE'}) \
            CREATE(:StaffMember {id:'cook',email:'cook@example.test',membershipStatus:'ACTIVE',roles:['staff'],schoolIds:['school'],tenantIds:['tenant']})")).await.unwrap();

        // A Student: themself, under the id payment knows; without that id there is nobody to shop as.
        let me = student(&g, "aisha").await.unwrap().unwrap();
        assert_eq!(me["me"], json!({"id": "student:aisha", "role": "student"}));
        assert_eq!(
            me["students"],
            json!([{"studentId": "ENR-1", "name": "Aisha", "schoolId": "school"}])
        );
        assert_eq!(
            me["schools"],
            json!([{"schoolId": "school", "tenantId": "tenant", "shopAdmin": false, "shopStaff": false}])
        );
        assert!(student(&g, "bima").await.unwrap().is_none());
        assert!(student(&g, "mother").await.unwrap().is_none());

        // A Parent: eligible children with an active enrolment payment knows, by User id or Lead id.
        for sub in ["mother", "LEAD-MOTHER"] {
            let family = parent(&g, sub).await.unwrap().unwrap();
            assert_eq!(
                family["me"],
                json!({"id": "parent:mother", "role": "parent"})
            );
            assert_eq!(
                family["students"],
                json!([{"studentId": "ENR-1", "name": "Aisha", "schoolId": "school"}])
            );
            assert_eq!(family["schools"].as_array().unwrap().len(), 1);
        }
        let childless = parent(&g, "childless").await.unwrap().unwrap();
        assert_eq!(
            (childless["students"].clone(), childless["schools"].clone()),
            (json!([]), json!([]))
        );
        assert!(parent(&g, "pending").await.unwrap().is_none());
        assert!(parent(&g, "aisha").await.unwrap().is_none());

        // Staff: an unscoped Owner manages every school, a school administrator theirs, plain staff nothing yet.
        let member = |id: &str, role: &str, scoped: bool| Staff {
            id: id.into(),
            role: role.into(),
            school_ids: if scoped {
                vec!["school".into()]
            } else {
                vec![]
            },
            tenant_ids: if scoped {
                vec!["tenant".into()]
            } else {
                vec![]
            },
        };
        let owner = staff(&g, &member("owner", "owner", false)).await.unwrap();
        assert_eq!(owner["me"], json!({"id": "staff:owner", "role": "staff"}));
        assert_eq!(
            owner["schools"],
            json!([
                {"schoolId": "other", "tenantId": "tenant", "shopAdmin": true, "shopStaff": true},
                {"schoolId": "school", "tenantId": "tenant", "shopAdmin": true, "shopStaff": true},
            ])
        );
        assert_eq!(owner["students"], json!([]));
        let admin = staff(&g, &member("admin", "school_admin", true))
            .await
            .unwrap();
        assert_eq!(
            admin["schools"],
            json!([{"schoolId": "school", "tenantId": "tenant", "shopAdmin": true, "shopStaff": true}])
        );
        let cook = staff(&g, &member("cook", "staff", true)).await.unwrap();
        assert_eq!(
            cook["schools"],
            json!([{"schoolId": "school", "tenantId": "tenant", "shopAdmin": false, "shopStaff": false}])
        );
    }
}
