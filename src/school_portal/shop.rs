//! ADR 0027 (Digital Schools workspace): who is shopping, or working, in the school Shop. shop-services
//! calls this with the user's own bearer, so it holds no SIS or Auth secret. A Student shops for
//! themself, a Parent for each eligible child with an active enrolment, and staff work in the schools
//! of their membership. Contract: `docs/AUTHORIZATION.md`, `GET /api/leads/v1/shop/context`.
use super::{
    auth, envelope, failure,
    model::identifier,
    repository::{PARENT, PARENT_ELIGIBLE},
    safety::OWNED,
    student_identity, Failure,
};
use crate::{
    authorization::{
        facts::{self, Db},
        model::{decide, Scope},
        reads::permitted_schools,
    },
    AppState,
};
use axum::{
    extract::{Query as Params, State},
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
/// The staff a Shop admin may assign to a stall; a school with more is a broken data set.
const MAX_WORKERS: usize = 200;
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

/// The caller of a Shop staff app token, with the role Auth reports for this session.
async fn shop_member(
    config: &crate::config::config::AppConfig,
    h: &HeaderMap,
) -> Result<Staff, Failure> {
    let member = auth::shop_staff(config, h).await?;
    let role = if member.is_owner() {
        "owner"
    } else if member.roles.iter().any(|r| r == "school_admin") {
        "school_admin"
    } else {
        "staff"
    };
    Ok(Staff {
        id: member.staff_member_id,
        role: role.into(),
        school_ids: member.school_ids,
        tenant_ids: member.tenant_ids,
    })
}

/// The staff who may work the Shop of `school`: everyone holding `shop.operate` or `shop.manage`
/// there, through an explicit assignment, a tenant-wide one, or derivation (Owner), sorted by name.
/// One tenant load and one email read, whatever the number of staff.
pub(super) async fn workers(
    graph: &Graph,
    tenant: &str,
    school: &str,
) -> Result<Vec<Value>, Failure> {
    let mut db = Db::Graph(graph);
    let facts = facts::load(&mut db, tenant, &[], &[]).await?;
    let scope = Scope::school(school);
    let mut people: Vec<(&str, &str)> = facts
        .staff
        .iter()
        .filter(|p| {
            let holdings = facts.holdings(&p.id);
            [OPERATE, MANAGE]
                .iter()
                .any(|code| decide(&facts, &holdings, code, &scope).is_some())
        })
        .map(|p| (p.name.as_str(), p.id.as_str()))
        .collect();
    people.sort_by_cached_key(|(name, id)| (name.to_lowercase(), *id));
    people.truncate(MAX_WORKERS);
    let ids: Vec<String> = people.iter().map(|(_, id)| (*id).to_owned()).collect();
    // `holdings` gives nothing to a duplicated node, so each id is one row here.
    let emails = db
        .rows(
            query("MATCH(s:StaffMember) WHERE s.id IN $ids RETURN s.id AS id,coalesce(s.email,'') AS email")
                .param("ids", ids),
            MAX_WORKERS,
        )
        .await?;
    let mut by_id = std::collections::HashMap::new();
    for r in &emails {
        by_id.insert(
            facts::get::<String>(r, "id")?,
            facts::get::<String>(r, "email")?,
        );
    }
    Ok(people
        .into_iter()
        .map(|(name, id)| {
            json!({"id": format!("staff:{id}"), "name": name, "email": by_id.get(id).cloned().unwrap_or_default()})
        })
        .collect())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SchoolParam {
    school_id: String,
}

/// `GET /api/leads/v1/shop/staff?schoolId=`: for a Shop admin of that school on a Shop staff app
/// token, the staff they may assign to its stalls.
pub(super) async fn staff_list(
    State(s): State<AppState>,
    h: HeaderMap,
    Params(p): Params<SchoolParam>,
) -> Result<Json<Value>, Failure> {
    if !identifier(&p.school_id) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    if auth::claimed_audience(&h).as_deref() != Some(auth::SHOP_AUDIENCE) {
        return Err(failure(StatusCode::UNAUTHORIZED));
    }
    let me = staff(&s.graph, &shop_member(&s.config, &h).await?).await?;
    // The same Shop admin rule as the context, for this school only.
    let tenant = me["schools"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| v["schoolId"] == p.school_id.as_str() && v["shopAdmin"] == true)
        .and_then(|v| v["tenantId"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| failure(StatusCode::FORBIDDEN))?;
    Ok(envelope(json!(
        workers(&s.graph, &tenant, &p.school_id).await?
    )))
}

/// A Shop note for the parents of one enrolled student (shop-services' notification worker, spec §7.1).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ParentNotice {
    /// The Shop's event key; each parent gets one note per key, so a retry writes nothing new.
    key: String,
    school_id: String,
    /// The enrolled student id, as payment-services and the Shop know the child.
    student_id: String,
    title: String,
    /// `{child}` becomes the child's name.
    body: String,
}
const MAX_KEY: usize = 256;
const MAX_TITLE: usize = 100;
const MAX_BODY: usize = 1000;

impl ParentNotice {
    fn valid(&self) -> bool {
        let key = (1..=MAX_KEY).contains(&self.key.len())
            && self
                .key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b));
        let text = |t: &str, max: usize| {
            let n = t.trim().chars().count();
            (1..=max).contains(&n) && !t.chars().any(|c| c.is_control() && c != '\n')
        };
        key && identifier(&self.school_id)
            && identifier(&self.student_id)
            && text(&self.title, MAX_TITLE)
            && text(&self.body, MAX_BODY)
    }
}

/// Writes the note on one Lead of each eligible parent of the student (the Parent board reads every Lead of the
/// user, so one is enough) and answers the child's SIS id with each parent's push recipient. A student with no
/// active enrolment of that id in that school has nobody to tell.
pub(super) async fn tell_parents(graph: &Graph, n: &ParentNotice) -> Result<Value, Error> {
    let q = query(&format!(
        "MATCH(s:Student)-[:ENROLLED_AS]->(e:EnrolledStudent {{student_id:$student,school_id:$school}}) \
         WHERE e.status='active' \
         WITH s,head(collect(e)) AS e \
         CALL {{ WITH s,e MATCH(u:User)-[:HAS_APPLICATION]->(l:Lead)-[:HAS_STUDENT]->(s) WHERE {PARENT_ELIGIBLE} \
           WITH s,e,u,l ORDER BY l.lead_id WITH s,e,u,head(collect(l)) AS lead \
           MERGE(lead)-[:HAS_MESSAGE]->(m:Message {{shop_key:$key}}) \
           ON CREATE SET m.message_id='MSG-'+randomUUID(),m.tenant_id=e.tenant_id,m.title=$title, \
             m.body=replace($body,'{{child}}',coalesce(s.fullName,'')),m.category='shop',m.is_read=false, \
             m.created_at=datetime(),m.read_at=null \
           RETURN collect(DISTINCT 'parent:'+u.id) AS recipients }} \
         RETURN s.studentId AS student,recipients LIMIT 1"
    ))
    .param("student", n.student_id.clone())
    .param("school", n.school_id.clone())
    .param("key", n.key.clone())
    .param("title", n.title.trim())
    .param("body", n.body.trim());
    let mut rows = graph.execute(q).await?;
    let Some(r) = rows.next().await? else {
        return Ok(json!({"studentId": "", "recipients": []}));
    };
    let mut recipients: Vec<String> = r.get("recipients").unwrap_or_default();
    recipients.sort();
    recipients.truncate(MAX_ROWS);
    Ok(json!({"studentId": r.get::<String>("student")?, "recipients": recipients}))
}

/// `POST /api/leads/v1/internal/shop/parent-notices`: for shop-services only, with `X-Internal-Token`.
pub(super) async fn parent_notices(
    State(s): State<AppState>,
    h: HeaderMap,
    body: Result<Json<ParentNotice>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Value>, Failure> {
    auth::internal_token(s.config.shop_internal_token.as_deref(), &h)?;
    let Ok(Json(notice)) = body else {
        return Err(failure(StatusCode::BAD_REQUEST));
    };
    if !notice.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let told = tell_parents(&s.graph, &notice)
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    Ok(envelope(told))
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
            staff(&s.graph, &shop_member(&s.config, &h).await?).await?
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

        // Stall staff to choose from: explicit `shop.operate` here, a tenant-wide grant, an Owner by
        // derivation; not a grant for another school, nor staff without one. Sorted by name.
        g.run(query("CREATE(:AuthorizationRoleTemplate {tenant_id:'tenant',code:'canteen',name_id:'Kantin',name_en:'Canteen',version:1,capabilities:['shop.operate'],allowed_scopes:['SCHOOL','TENANT'],status:'active',updated_at:datetime()}) \
            CREATE(:AuthorizationAssignment {id:'a-cook',tenant_id:'tenant',staff_member_id:'cook',template_code:'canteen',scope_type:'SCHOOL',school_id:'school',status:'active',version:1,updated_at:datetime()}) \
            CREATE(:StaffMember {id:'baker',displayName:'Ani',email:'baker@example.test',membershipStatus:'ACTIVE',roles:['staff'],schoolIds:['other'],tenantIds:['tenant']}) \
            CREATE(:AuthorizationAssignment {id:'a-baker',tenant_id:'tenant',staff_member_id:'baker',template_code:'canteen',scope_type:'SCHOOL',school_id:'other',status:'active',version:1,updated_at:datetime()}) \
            CREATE(:StaffMember {id:'clerk',displayName:'Zaki',email:'clerk@example.test',membershipStatus:'ACTIVE',roles:['staff'],schoolIds:['school','other'],tenantIds:['tenant']}) \
            CREATE(:AuthorizationAssignment {id:'a-clerk',tenant_id:'tenant',staff_member_id:'clerk',template_code:'canteen',scope_type:'TENANT',status:'active',version:1,updated_at:datetime()}) \
            CREATE(:StaffMember {id:'head',displayName:'Bu Rina',email:'head@example.test',membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[]}) \
            CREATE(:StaffMember {id:'guard',displayName:'Budi',email:'guard@example.test',membershipStatus:'ACTIVE',roles:['staff'],schoolIds:['school'],tenantIds:['tenant']})")).await.unwrap();
        let cook = staff(&g, &member("cook", "staff", true)).await.unwrap();
        assert_eq!(
            cook["schools"],
            json!([{"schoolId": "school", "tenantId": "tenant", "shopAdmin": false, "shopStaff": true}])
        );
        assert_eq!(
            workers(&g, "tenant", "school").await.unwrap(),
            vec![
                json!({"id": "staff:head", "name": "Bu Rina", "email": "head@example.test"}),
                json!({"id": "staff:cook", "name": "cook", "email": "cook@example.test"}),
                json!({"id": "staff:clerk", "name": "Zaki", "email": "clerk@example.test"}),
            ]
        );

        // Shop notes: one per eligible parent and event key, on one of their Leads, with the child's name.
        g.run(query("MATCH(s:Student {studentId:'child'}) \
            CREATE(:User {id:'father',email:'father@example.test',role:'parent'}) \
              -[:HAS_APPLICATION]->(:Lead {lead_id:'LEAD-FATHER',email:'father@example.test',status:'paid'}) \
              -[:HAS_STUDENT]->(s)")).await.unwrap();
        let notice = |key: &str, school: &str| ParentNotice {
            key: key.into(),
            school_id: school.into(),
            student_id: "ENR-1".into(),
            title: "Permintaan isi saldo".into(),
            body: "{child} meminta isi saldo Rp 20.000.".into(),
        };
        let told = json!({"studentId": "child", "recipients": ["parent:father", "parent:mother"]});
        for _ in 0..2 {
            let first = notice("top-up-requested:ENR-1:1", "school");
            assert!(first.valid());
            assert_eq!(tell_parents(&g, &first).await.unwrap(), told);
        }
        assert_eq!(
            tell_parents(&g, &notice("ticket-forfeited:t1", "other"))
                .await
                .unwrap(),
            json!({"studentId": "", "recipients": []})
        );
        let mut notes = g
            .execute(query("MATCH(u:User)-[:HAS_APPLICATION]->(:Lead)-[:HAS_MESSAGE]->(m:Message {category:'shop'}) \
                RETURN u.id AS parent,m.body AS body,m.message_id STARTS WITH 'MSG-' AS id ORDER BY parent"))
            .await
            .unwrap();
        let mut seen = vec![];
        while let Some(r) = notes.next().await.unwrap() {
            seen.push((
                r.get::<String>("parent").unwrap(),
                r.get::<String>("body").unwrap(),
                r.get::<bool>("id").unwrap(),
            ));
        }
        let body = "Aisha meminta isi saldo Rp 20.000.".to_string();
        assert_eq!(
            seen,
            vec![
                ("father".to_string(), body.clone(), true),
                ("mother".to_string(), body, true)
            ]
        );
    }

    #[test]
    fn parent_notices_need_the_shop_token_and_plain_input() {
        let mut h = HeaderMap::new();
        h.insert(
            "x-internal-token",
            "0123456789abcdef0123456789abcdef".parse().unwrap(),
        );
        assert_eq!(
            auth::internal_token(None, &h).unwrap_err().0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        let other = Some("fedcba9876543210fedcba9876543210");
        assert_eq!(
            auth::internal_token(other, &h).unwrap_err().0,
            StatusCode::UNAUTHORIZED
        );
        assert!(auth::internal_token(Some("0123456789abcdef0123456789abcdef"), &h).is_ok());
        let notice = |key: &str, title: &str| ParentNotice {
            key: key.into(),
            school_id: "school".into(),
            student_id: "ENR-1".into(),
            title: title.into(),
            body: "{child}".into(),
        };
        assert!(notice("ticket-ready:1", "Pesanan").valid());
        assert!(!notice("bad key", "Pesanan").valid());
        assert!(!notice(&"k".repeat(257), "Pesanan").valid());
        assert!(!notice("k", " ").valid());
        assert!(!notice("k", &"t".repeat(101)).valid());
    }
}
