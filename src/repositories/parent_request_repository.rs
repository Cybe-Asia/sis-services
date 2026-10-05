use crate::models::parent_request::{
    ParentRequest, ParentRequestInput, RequestKind, PENDING_REVIEW,
};
use neo4rs::{query, Graph, Row};
type RepoError = Box<dyn std::error::Error + Send + Sync>;
// Explicit ownership only; no email-only repair or caller-selected Parent identity.
const ELIGIBLE:&str="coalesce(u.role,'parent')='parent' AND coalesce(u.staffMemberId,'')='' AND all(role IN coalesce(u.roles,[]) WHERE role='parent') AND all(role IN coalesce(u.marketingRoles,[]) WHERE role='parent') AND NOT (u)-[:STAFF_PROFILE]->() AND NOT EXISTS { MATCH (staff:StaffMember) WHERE toLower(staff.email)=toLower(u.email) } AND l.status IN ['verified','paid'] AND toLower(u.email)=toLower(l.email)";
fn row(r: Row) -> Result<ParentRequest, RepoError> {
    Ok(ParentRequest {
        id: r.get("id")?,
        student_id: r.get("studentId")?,
        kind: r.get("kind")?,
        date: r.get("date")?,
        time: r.get::<String>("time").ok().filter(|s| !s.is_empty()),
        teacher_email: r
            .get::<String>("teacherEmail")
            .ok()
            .filter(|s| !s.is_empty()),
        reason: r.get::<String>("reason").ok().filter(|s| !s.is_empty()),
        note: r.get("note")?,
        status: r.get("status")?,
        created_at: r.get("createdAt")?,
        review_note: r.get::<String>("reviewNote").ok(),
    })
}
const PROJECTION:&str="r.id AS id,s.studentId AS studentId,r.kind AS kind,r.date AS date,r.time AS time,r.teacherEmail AS teacherEmail,r.reason AS reason,r.note AS note,r.status AS status,toString(r.createdAt) AS createdAt,r.review_note AS reviewNote";
pub async fn list(graph: &Graph, sub: &str) -> Result<Vec<ParentRequest>, RepoError> {
    let q=format!("MATCH (u:User)-[:HAS_APPLICATION]->(l:Lead)-[:HAS_STUDENT]->(s:Student)<-[:FOR_STUDENT]-(r:ParentSchoolRequest)<-[:CREATED_SCHOOL_REQUEST]-(u) WHERE (u.id=$sub OR l.lead_id=$sub) AND {ELIGIBLE} RETURN DISTINCT {PROJECTION} ORDER BY createdAt DESC LIMIT 100");
    let mut rows = graph.execute(query(&q).param("sub", sub)).await?;
    let mut out = vec![];
    while let Some(r) = rows.next().await? {
        out.push(row(r)?);
    }
    Ok(out)
}
pub async fn create(
    graph: &Graph,
    sub: &str,
    input: &ParentRequestInput,
) -> Result<Option<ParentRequest>, RepoError> {
    // Idempotency is per owning User and locked on that User before read/create.
    let q=format!("MATCH (u:User)-[:HAS_APPLICATION]->(l:Lead)-[:HAS_STUDENT]->(s:Student {{studentId:$studentId}})-[:ENROLLED_AS]->(enrolled:EnrolledStudent)-[:ENROLLED_IN]->(section:Section) WHERE (u.id=$sub OR l.lead_id=$sub) WITH DISTINCT u,l,s,section,enrolled SET u.parentRequestLock=coalesce(u.parentRequestLock,0)+1 WITH u,l,s,section,enrolled WHERE {ELIGIBLE} AND enrolled.status='active' AND section.status='active' AND enrolled.school_id=section.school_id AND enrolled.tenant_id=section.tenant_id AND coalesce(section.school_id,'')<>'' AND coalesce(section.tenant_id,'')<>'' AND ($kind='leave' OR EXISTS {{ MATCH(homeroom:StaffMember {{id:section.homeroom_staff_member_id,membershipStatus:'ACTIVE'}}) WHERE 'teacher' IN coalesce(homeroom.roles,[]) AND section.school_id IN coalesce(homeroom.schoolIds,[]) AND section.tenant_id IN coalesce(homeroom.tenantIds,[]) AND toLower(homeroom.email)=toLower($teacherEmail) }}) WITH DISTINCT u,s,section MERGE (u)-[:CREATED_SCHOOL_REQUEST]->(r:ParentSchoolRequest {{id:$id}}) ON CREATE SET r.public_id=randomUUID(),r.school_id=section.school_id,r.tenant_id=section.tenant_id,r.section_id=section.section_id,r.kind=$kind,r.date=$date,r.time=$time,r.teacherEmail=$teacherEmail,r.reason=$reason,r.note=$note,r.status=$pending,r.createdAt=datetime() WITH u,s,r WHERE r.kind=$kind AND r.date=$date AND coalesce(r.time,'')=$time AND coalesce(r.teacherEmail,'')=$teacherEmail AND coalesce(r.reason,'')=$reason AND r.note=$note AND (NOT (r)-[:FOR_STUDENT]->() OR (r)-[:FOR_STUDENT]->(s)) MERGE (r)-[:FOR_STUDENT]->(s) RETURN {PROJECTION}");
    let q = query(&q)
        .param("sub", sub)
        .param("studentId", input.student_id.as_str())
        .param("id", input.idempotency_key.as_str())
        .param("kind", input.kind.as_str())
        .param("date", input.date.as_str())
        .param("time", input.time.as_deref().unwrap_or(""))
        .param("teacherEmail", input.teacher_email.as_deref().unwrap_or(""))
        .param("reason", input.reason.as_deref().unwrap_or(""))
        .param("note", input.note.trim())
        .param("pending", PENDING_REVIEW);
    graph.execute(q).await?.next().await?.map(row).transpose()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "explicit local SCHOOL_WORKFLOW_TEST_BOLT; cleans only its own fixture"]
    async fn canonical_homeroom_contacts_and_appointments() {
        let uri = std::env::var("SCHOOL_WORKFLOW_TEST_BOLT").unwrap();
        assert_eq!(uri, "127.0.0.1:3213");
        let graph = Graph::new(
            uri,
            &std::env::var("NEO4J_USER").unwrap(),
            &std::env::var("NEO4J_PASSWORD").unwrap(),
        )
        .await
        .unwrap();
        let mark = format!("school-request-check-{}", uuid::Uuid::new_v4());
        graph.run(query("CREATE(u:User {id:$mark,email:$parent,role:'parent',workflow_test:$mark})-[:HAS_APPLICATION]->(l:Lead {lead_id:$mark,email:$parent,status:'verified',workflow_test:$mark})-[:HAS_STUDENT]->(:Student {studentId:$mark,fullName:'Isolated request check',workflow_test:$mark})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$mark,tenant_id:$mark,workflow_test:$mark})-[:ENROLLED_IN]->(:Section {section_id:$mark,name:'Isolated class',status:'active',school_id:$mark,tenant_id:$mark,homeroom_staff_member_id:$mark,homeroom_teacher_email:'legacy@example.test',workflow_test:$mark}) CREATE(:StaffMember {id:$mark,email:$teacher,fullName:'Current homeroom',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$mark],tenantIds:[$mark],workflow_test:$mark})").param("mark",mark.clone()).param("parent",format!("{mark}@parent.test")).param("teacher",format!("{mark}@teacher.test"))).await.unwrap();
        let sections =
            super::super::sis_repository::list_parent_sections(&graph, &[mark.clone()], &mark)
                .await
                .unwrap();
        assert_eq!(
            sections[0].homeroomTeacherEmail,
            Some(format!("{mark}@teacher.test"))
        );
        let mut input = ParentRequestInput {
            idempotency_key: uuid::Uuid::new_v4().to_string(),
            student_id: mark.clone(),
            kind: RequestKind::Appointment,
            date: crate::school_portal::model::school_today().to_string(),
            time: Some("10:00".into()),
            teacher_email: Some("legacy@example.test".into()),
            reason: None,
            note: mark.clone(),
        };
        assert!(create(&graph, &mark, &input).await.unwrap().is_none());
        input.teacher_email = Some(format!("{mark}@teacher.test"));
        assert!(create(&graph, &mark, &input).await.unwrap().is_some());
        graph
            .run(
                query("MATCH(u:User {workflow_test:$mark}) SET u.role='student'")
                    .param("mark", mark.clone()),
            )
            .await
            .unwrap();
        assert!(
            super::super::sis_repository::list_parent_sections(&graph, &[mark.clone()], &mark)
                .await
                .unwrap()
                .is_empty()
        );
        graph
            .run(
                query("MATCH(u:User {workflow_test:$mark}) SET u.role='parent'")
                    .param("mark", mark.clone()),
            )
            .await
            .unwrap();
        assert!(super::super::sis_repository::list_parent_sections(
            &graph,
            &[mark.clone()],
            "foreign"
        )
        .await
        .unwrap()
        .is_empty());
        input.idempotency_key = uuid::Uuid::new_v4().to_string();
        for mutation in ["MATCH(s:Section {workflow_test:$mark}) REMOVE s.homeroom_staff_member_id","MATCH(s:Section {workflow_test:$mark}) SET s.homeroom_staff_member_id=$mark WITH s MATCH(t:StaffMember {workflow_test:$mark}) SET t.membershipStatus='INACTIVE'","MATCH(t:StaffMember {workflow_test:$mark}) SET t.membershipStatus='ACTIVE',t.tenantIds=['foreign']"]{
            graph.run(query(mutation).param("mark",mark.clone())).await.unwrap();
            assert!(create(&graph,&mark,&input).await.unwrap().is_none());
            let sections=super::super::sis_repository::list_parent_sections(&graph,&[mark.clone()], &mark).await.unwrap();
            assert!(sections[0].homeroomTeacherEmail.is_none());
        }
        graph.run(query("MATCH(r:ParentSchoolRequest)-[:FOR_STUDENT]->(:Student {workflow_test:$mark}) DETACH DELETE r").param("mark",mark.clone())).await.unwrap();
        graph
            .run(
                query("MATCH(n {workflow_test:$mark}) DETACH DELETE n").param("mark", mark.clone()),
            )
            .await
            .unwrap();
    }
    #[tokio::test]
    #[ignore = "requires disposable PARENT_REQUEST_TEST_BOLT graph"]
    async fn durable_owned_requests() {
        let uri = std::env::var("PARENT_REQUEST_TEST_BOLT").unwrap();
        assert!(uri.starts_with("127.0.0.1:"));
        let graph = Graph::new(uri, "neo4j", "parent-otp-fixture-password")
            .await
            .unwrap();
        graph.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
        graph.run(query("CREATE (u:User {id:'parent-test',email:'parent@example.test',role:'parent'})-[:HAS_APPLICATION]->(l:Lead {lead_id:'LEAD-PARENT',email:'parent@example.test',status:'verified'})-[:HAS_STUDENT]->(s:Student {studentId:'child-test'})-[:ENROLLED_AS]->(enrolled:EnrolledStudent {status:'active',school_id:'school-test',tenant_id:'tenant-test'})-[:ENROLLED_IN]->(:Section {section_id:'section-test',school_id:'school-test',tenant_id:'tenant-test',status:'active',homeroom_staff_member_id:'teacher-test'}) CREATE(:StaffMember {id:'teacher-test',email:'teacher@example.test',fullName:'Current Teacher',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school-test'],tenantIds:['tenant-test']})")).await.unwrap();
        let mut input = ParentRequestInput {
            idempotency_key: uuid::Uuid::new_v4().to_string(),
            student_id: "child-test".into(),
            kind: RequestKind::Leave,
            date: chrono::Utc::now().date_naive().to_string(),
            time: None,
            teacher_email: None,
            reason: Some("sick".into()),
            note: "test note".into(),
        };
        assert!(create(&graph, "foreign", &input).await.unwrap().is_none());
        let r = create(&graph, "parent-test", &input)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.status, PENDING_REVIEW);
        assert_eq!(
            create(&graph, "parent-test", &input)
                .await
                .unwrap()
                .unwrap()
                .id,
            r.id
        );
        assert_eq!(list(&graph, "parent-test").await.unwrap().len(), 1);
        input.note = "changed".into();
        assert!(create(&graph, "parent-test", &input)
            .await
            .unwrap()
            .is_none());
        input.idempotency_key = uuid::Uuid::new_v4().to_string();
        input.student_id = "foreign".into();
        assert!(create(&graph, "parent-test", &input)
            .await
            .unwrap()
            .is_none());
        input.student_id = "child-test".into();
        input.kind = RequestKind::Appointment;
        input.reason = None;
        input.teacher_email = Some("foreign@example.test".into());
        input.time = Some("10:00".into());
        assert!(create(&graph, "parent-test", &input)
            .await
            .unwrap()
            .is_none());
        input.teacher_email = Some("teacher@example.test".into());
        assert!(create(&graph, "parent-test", &input)
            .await
            .unwrap()
            .is_some());
        for mutation in ["MATCH(e:EnrolledStudent) SET e.status='inactive'","MATCH(e:EnrolledStudent) SET e.status='active',e.school_id='foreign'","MATCH(e:EnrolledStudent) SET e.school_id='school-test' WITH e MATCH(s:Section) SET s.status='inactive'"] {
            graph.run(query(mutation)).await.unwrap();
            input.idempotency_key=uuid::Uuid::new_v4().to_string();
            assert!(create(&graph,"parent-test",&input).await.unwrap().is_none());
        }
        graph
            .run(query("MATCH(s:Section) SET s.status='active'"))
            .await
            .unwrap();
        graph
            .run(query("MATCH(u:User) SET u.role='teacher'"))
            .await
            .unwrap();
        assert!(list(&graph, "parent-test").await.unwrap().is_empty());
        input.idempotency_key = uuid::Uuid::new_v4().to_string();
        assert!(create(&graph, "parent-test", &input)
            .await
            .unwrap()
            .is_none());
        graph.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
    }
}
