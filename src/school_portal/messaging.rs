//! ADR 0024: who takes part in each homeroom ↔ parent thread. messaging-services calls this with the
//! user's own bearer, so it holds no SIS or Auth secret. A thread's members are the Student's current
//! homeroom teacher and every eligible Parent, by the same rules as the Parent sections read and
//! `school-requests`. Contract: digital-school `docs/messaging/homeroom-messaging-slice1-spec.md` §6.
use super::{
    auth, envelope, failure,
    repository::{teacher_query, ENROLLED, PARENT, PARENT_ELIGIBLE, PARENT_SECTION},
    Failure,
};
use crate::AppState;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use neo4rs::{query, Graph, Query};
use serde_json::{json, Value};
type Error = Box<dyn std::error::Error + Send + Sync>;

/// Given `s` and its class `sec`, and with no `u` or `l` in scope: the homeroom and the guardians.
fn members_tail() -> String {
    format!(
        "OPTIONAL MATCH(h:StaffMember {{id:sec.homeroom_staff_member_id,membershipStatus:'ACTIVE'}}) \
         WHERE 'teacher' IN coalesce(h.roles,[]) AND sec.school_id IN coalesce(h.schoolIds,[]) AND sec.tenant_id IN coalesce(h.tenantIds,[]) \
         CALL {{ WITH s MATCH(u:User)-[:HAS_APPLICATION]->(l:Lead)-[:HAS_STUDENT]->(s) WHERE {PARENT_ELIGIBLE} \
           WITH u,collect(l.parent_name) AS names ORDER BY u.id \
           RETURN collect('parent:'+u.id) AS guardian_ids,collect(coalesce(u.fullName,names[0],'')) AS guardian_names }} \
         RETURN DISTINCT s.studentId AS student,coalesce(s.fullName,'') AS name,sec.school_id AS school,sec.tenant_id AS tenant,sec.section_id AS section, \
           CASE WHEN h IS NULL THEN null ELSE 'staff:'+h.id END AS homeroom_id,coalesce(h.displayName,h.fullName,'') AS homeroom_name, \
           guardian_ids,guardian_names ORDER BY name,student LIMIT 500"
    )
}

async fn students(graph: &Graph, q: Query) -> Result<Vec<Value>, Error> {
    let mut rows = graph.execute(q).await?;
    let mut out: Vec<Value> = vec![];
    while let Some(r) = rows.next().await? {
        let student: String = r.get("student")?;
        // shortcut: a Student active in two classes keeps the first; split threads per class if that becomes real.
        if out.iter().any(|v| v["studentId"] == student) {
            continue;
        }
        let ids: Vec<String> = r.get("guardian_ids")?;
        let names: Vec<String> = r.get("guardian_names")?;
        let homeroom = r.get::<String>("homeroom_id").ok().map(
            |id| json!({"id": id, "name": r.get::<String>("homeroom_name").unwrap_or_default()}),
        );
        out.push(json!({
            "studentId": student,
            "studentName": r.get::<String>("name")?,
            "schoolId": r.get::<String>("school")?,
            "tenantId": r.get::<String>("tenant")?,
            "sectionId": r.get::<String>("section")?,
            "homeroom": homeroom,
            "guardians": ids.into_iter().zip(names).map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
        }));
    }
    Ok(out)
}

/// `None` when the token names no single eligible Parent. A magic-link `LEAD-…` subject resolves to
/// its User, so `me.id` equals the id in `guardians`.
pub(super) async fn parent(graph: &Graph, sub: &str) -> Result<Option<Value>, Error> {
    let mut me = graph
        .execute(query(&format!("{PARENT} WITH collect(DISTINCT u) AS owners WHERE size(owners)=1 WITH owners[0] AS u MATCH(u)-[:HAS_APPLICATION]->(l:Lead) WITH u,collect(l.parent_name) AS names RETURN u.id AS id,coalesce(u.fullName,names[0],'') AS name")).param("sub", sub))
        .await?;
    let Some(me) = me.next().await? else {
        return Ok(None);
    };
    let q = query(&format!("{PARENT} WITH collect(DISTINCT u) AS owners,collect(DISTINCT s) AS kids WHERE size(owners)=1 UNWIND kids AS s WITH s {PARENT_SECTION} WITH DISTINCT s,sec {}", members_tail())).param("sub", sub);
    Ok(Some(json!({
        "me": {"id": format!("parent:{}", me.get::<String>("id")?), "role": "parent", "name": me.get::<String>("name")?},
        "students": students(graph, q).await?,
    })))
}

/// Only the classes the caller is homeroom of: a subject grant or Owner management is not membership.
pub(super) async fn teacher(graph: &Graph, member: &auth::Teacher) -> Result<Value, Error> {
    let mut me = graph
        .execute(
            query("MATCH(t:StaffMember {id:$actor}) RETURN coalesce(t.displayName,t.fullName,'') AS name")
                .param("actor", member.staff_member_id.clone()),
        )
        .await?;
    let name = match me.next().await? {
        Some(r) => r.get::<String>("name")?,
        None => String::new(),
    };
    let body = format!(
        "WITH t,sec WHERE homeroom {ENROLLED} WITH DISTINCT s,sec {}",
        members_tail()
    );
    Ok(json!({
        "me": {"id": format!("staff:{}", member.staff_member_id), "role": "teacher", "name": name},
        "students": students(graph, teacher_query(member, &body, None, false, true)).await?,
    }))
}

pub(super) async fn members(
    State(s): State<AppState>,
    h: HeaderMap,
) -> Result<Json<Value>, Failure> {
    let unavailable = |_: Error| failure(StatusCode::SERVICE_UNAVAILABLE);
    // A Parent token is checked locally; anything it rejects as unauthorised may be a staff token for Auth.
    let data = match auth::parent(&s.config, &h) {
        Ok(sub) => parent(&s.graph, &sub)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| failure(StatusCode::FORBIDDEN))?,
        Err((StatusCode::UNAUTHORIZED, _)) => {
            let member = auth::teacher(&s.config, &h).await?;
            teacher(&s.graph, &member).await.map_err(unavailable)?
        }
        Err(e) => return Err(e),
    };
    Ok(envelope(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires disposable MESSAGING_MEMBERS_TEST_BOLT graph"]
    async fn homeroom_and_guardians_only() {
        let uri = std::env::var("MESSAGING_MEMBERS_TEST_BOLT").unwrap();
        assert!(uri.starts_with("127.0.0.1:"));
        let g = Graph::new(uri, "neo4j", "parent-otp-fixture-password")
            .await
            .unwrap();
        g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
        g.run(query("CREATE(t:StaffMember {id:'teacher',email:'teacher@example.test',fullName:'Bu Rina',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) \
            CREATE(other:StaffMember {id:'other',email:'other@example.test',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) \
            CREATE(sec:Section {section_id:'class',name:'7A',school_id:'school',tenant_id:'tenant',status:'active',homeroom_staff_member_id:'teacher'}) \
            CREATE(:LearningTeachingGrant {staff_member_id:'other',section_id:'class',school_id:'school',tenant_id:'tenant',status:'ACTIVE'}) \
            CREATE(s:Student {studentId:'child',fullName:'Aisha'})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'school',tenant_id:'tenant'})-[:ENROLLED_IN]->(sec) \
            CREATE(:User {id:'mother',email:'mother@example.test',role:'parent'})-[:HAS_APPLICATION]->(:Lead {lead_id:'LEAD-MOTHER',email:'mother@example.test',status:'verified',parent_name:'Ibu Aisha'})-[:HAS_STUDENT]->(s) \
            CREATE(:User {id:'father',email:'father@example.test',role:'parent',fullName:'Pak Budi'})-[:HAS_APPLICATION]->(:Lead {lead_id:'LEAD-FATHER',email:'father@example.test',status:'paid'})-[:HAS_STUDENT]->(s) \
            CREATE(:User {id:'pending',email:'pending@example.test',role:'parent'})-[:HAS_APPLICATION]->(:Lead {lead_id:'LEAD-PENDING',email:'pending@example.test',status:'new'})-[:HAS_STUDENT]->(s) \
            CREATE(:User {id:'stranger',email:'stranger@example.test',role:'parent'})-[:HAS_APPLICATION]->(:Lead {lead_id:'LEAD-STRANGER',email:'stranger@example.test',status:'verified'})-[:HAS_STUDENT]->(:Student {studentId:'foreign',fullName:'Other'})")).await.unwrap();
        let staff = |id: &str| auth::Teacher {
            active: true,
            staff_member_id: id.into(),
            roles: vec!["teacher".into()],
            school_ids: vec!["school".into()],
            tenant_ids: vec!["tenant".into()],
        };

        // The magic-link subject resolves to the User id that also appears among the guardians.
        let mother = parent(&g, "LEAD-MOTHER").await.unwrap().unwrap();
        assert_eq!(
            mother["me"],
            json!({"id":"parent:mother","role":"parent","name":"Ibu Aisha"})
        );
        let child = &mother["students"][0];
        assert_eq!(child["studentId"], "child");
        assert_eq!(
            child["homeroom"],
            json!({"id":"staff:teacher","name":"Bu Rina"})
        );
        assert_eq!(
            child["guardians"],
            json!([{"id":"parent:father","name":"Pak Budi"},{"id":"parent:mother","name":"Ibu Aisha"}])
        );
        // An unverified Lead is no Parent; a Student without a class has no thread.
        assert!(parent(&g, "pending").await.unwrap().is_none());
        assert_eq!(
            parent(&g, "stranger").await.unwrap().unwrap()["students"],
            json!([])
        );

        let homeroom = teacher(&g, &staff("teacher")).await.unwrap();
        assert_eq!(
            homeroom["me"],
            json!({"id":"staff:teacher","role":"teacher","name":"Bu Rina"})
        );
        assert_eq!(homeroom["students"], mother["students"]);
        // A subject grant is not membership.
        assert_eq!(
            teacher(&g, &staff("other")).await.unwrap()["students"],
            json!([])
        );

        // A new homeroom takes over at once; an inactive one has none.
        g.run(query(
            "MATCH(s:Section) SET s.homeroom_staff_member_id='other'",
        ))
        .await
        .unwrap();
        assert_eq!(
            teacher(&g, &staff("teacher")).await.unwrap()["students"],
            json!([])
        );
        assert_eq!(
            teacher(&g, &staff("other")).await.unwrap()["students"][0]["studentId"],
            "child"
        );
        g.run(query(
            "MATCH(t:StaffMember {id:'other'}) SET t.membershipStatus='INACTIVE'",
        ))
        .await
        .unwrap();
        assert_eq!(
            parent(&g, "mother").await.unwrap().unwrap()["students"][0]["homeroom"],
            Value::Null
        );
        g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
    }
}
