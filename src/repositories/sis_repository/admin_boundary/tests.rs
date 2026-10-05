//! Marker-only isolated graph evidence, never a genuine SSO acceptance claim.
use super::*;
use crate::{handlers::sis_handler as h, repositories::sis_repository as r, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};

#[test]
fn batch_identifiers_are_bounded_unique_and_literal() {
    assert!(ids_valid(&["student-1".into()], 500));
    for ids in [
        vec![],
        vec!["".into()],
        vec!["student'".into()],
        vec!["same".into(), "same".into()],
        vec!["x".repeat(129)],
    ] {
        assert!(!ids_valid(&ids, 500));
    }
    assert!(!ids_valid(&vec!["student".into(); 501], 500));
}
struct Fixture {
    tag: String,
    school: String,
    other: String,
    tenant: String,
    section: String,
    next: String,
    foreign: String,
    child: String,
    foreign_child: String,
    owner: AdminActor,
    scoped: AdminActor,
    teacher: AdminActor,
    finance: AdminActor,
}
impl Fixture {
    fn new() -> Self {
        let tag = format!("academic-test-{}", uuid::Uuid::new_v4());
        let actor = |role: &str| AdminActor {
            subject: format!("{tag}-{role}-user"),
            staff: format!("{tag}-{role}-staff"),
            expires: chrono::Utc::now().timestamp() + 600,
        };
        Self {
            school: format!("{tag}-school"),
            other: format!("{tag}-other-school"),
            tenant: format!("{tag}-tenant"),
            section: format!("{tag}-section"),
            next: format!("{tag}-next-section"),
            foreign: format!("{tag}-foreign-section"),
            child: format!("{tag}-child"),
            foreign_child: format!("{tag}-foreign-child"),
            owner: actor("owner"),
            scoped: actor("admin"),
            teacher: actor("teacher"),
            finance: actor("finance"),
            tag,
        }
    }
    fn params(&self, q: neo4rs::Query) -> neo4rs::Query {
        q.param("tag", self.tag.clone())
            .param("school", self.school.clone())
            .param("other", self.other.clone())
            .param("tenant", self.tenant.clone())
            .param("section", self.section.clone())
            .param("next", self.next.clone())
            .param("foreign", self.foreign.clone())
            .param("child", self.child.clone())
            .param("foreign_child", self.foreign_child.clone())
    }
    fn input(&self) -> r::CreateSectionInput {
        r::CreateSectionInput {
            school_id: self.school.clone(),
            tenant_id: Some(self.tenant.clone()),
            name: "Synthetic academic section".into(),
            year_group: "Grade 7".into(),
            academic_year: "2026/2027".into(),
        }
    }
    fn headers(&self, a: &AdminActor, typ: &str, scope: &str, key: &str) -> HeaderMap {
        let mut header = jsonwebtoken::Header::default();
        header.typ = Some(typ.into());
        let token=jsonwebtoken::encode(&header,&serde_json::json!({"sub":a.subject,"staff_member_id":a.staff,"scope":scope,"iss":"https://auth.academic.example.test/staff-api","aud":"digital-school-admin-api","iat":chrono::Utc::now().timestamp(),"exp":a.expires,"roles":["owner","teacher"]}),&jsonwebtoken::EncodingKey::from_secret(key.as_bytes())).unwrap();
        let mut out = HeaderMap::new();
        out.insert("authorization", format!("Bearer {token}").parse().unwrap());
        out
    }
}
async fn count(g: &Graph, f: &Fixture, cy: &str) -> i64 {
    g.execute(f.params(query(cy)))
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get("n")
        .unwrap()
}
fn denied<T>(result: Result<T, RepositoryError>) {
    assert!(matches!(result,Err(RepositoryError::DbError(ref code)) if code=="FORBIDDEN"));
}
async fn run(g: Graph, f: Fixture) {
    for (a, role, global) in [
        (&f.owner, "owner", true),
        (&f.scoped, "school_admin", false),
        (&f.teacher, "teacher", false),
        (&f.finance, "finance_admin", false),
    ] {
        g.run(f.params(query("CREATE(u:User {id:$subject,staffMemberId:$actor,sis_academic_test:$tag})-[:STAFF_MEMBER]->(:StaffMember {id:$actor,membershipStatus:'ACTIVE',roles:[$role],schoolIds:$schools,tenantIds:$tenants,teamIds:[],sis_academic_test:$tag})")).param("subject",a.subject.clone()).param("actor",a.staff.clone()).param("role",role).param("schools",if global {vec![]} else {vec![f.school.clone()]}).param("tenants",if global {vec![]} else {vec![f.tenant.clone()]})).await.unwrap();
    }
    g.run(f.params(query("CREATE(:School {school_id:$school,tenant_id:$tenant,sis_academic_test:$tag}),(:School {school_id:$other,tenant_id:$tenant,sis_academic_test:$tag}) WITH 1 AS ready UNWIND [$section,$next,$foreign] AS id CREATE(sec:Section {section_id:id,name:'Synthetic section',school_id:CASE WHEN id=$foreign THEN $other ELSE $school END,tenant_id:$tenant,status:'active',year_group:'Grade 7',academic_year:'2026/2027',sis_academic_test:$tag})"))).await.unwrap();
    g.run(f.params(query("UNWIND [$child,$foreign_child] AS id MATCH(sec:Section {section_id:CASE WHEN id=$child THEN $section ELSE $foreign END}) CREATE(s:Student {studentId:id,fullName:'Synthetic child',sis_academic_test:$tag})-[:ENROLLED_AS]->(e:EnrolledStudent {student_id:id+'-permanent',applicant_student_id:id,student_number:id+'-number',status:'active',school_id:sec.school_id,tenant_id:sec.tenant_id,academic_year:sec.academic_year,year_group:sec.year_group,sis_academic_test:$tag})-[:ENROLLED_IN]->(sec)"))).await.unwrap();
    let key = "synthetic-academic-staff-key-at-least-32-bytes";
    std::env::set_var("STAFF_DOWNSTREAM_JWT_SECRET", key);
    std::env::set_var(
        "STAFF_DOWNSTREAM_JWT_ISSUER",
        "https://auth.academic.example.test/staff-api",
    );
    let state = AppState {
        graph: g.clone(),
        config: crate::config::config::AppConfig {
            server_port: 0,
            neo4j_uri: "bolt://127.0.0.1:3385".into(),
            neo4j_user: String::new(),
            neo4j_password: String::new(),
            jwt_secret: "synthetic-parent-key-at-least-32-bytes".into(),
        },
        http_client: reqwest::Client::new(),
    };
    let good = |a: &AdminActor| f.headers(a, "staff-api+jwt", "staff_downstream", key);
    // Actual handlers: typed staff credential succeeds; legacy/session/type/scope/key fail.
    let (code, _) =
        h::admin_create_section_handler(State(state.clone()), good(&f.owner), Json(f.input()))
            .await;
    assert_eq!(code, StatusCode::OK);
    for headers in [
        HeaderMap::new(),
        f.headers(&f.owner, "JWT", "staff_downstream", key),
        f.headers(&f.owner, "staff-api+jwt", "parent", key),
        f.headers(&f.owner, "staff-api+jwt", "staff_downstream", "wrong-key"),
        f.headers(&f.owner, "portal-session-v1", "staff_session", key),
    ] {
        assert_eq!(
            h::admin_list_sections_handler(
                State(state.clone()),
                headers,
                Query(h::AdminListSectionsQuery::default())
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    for a in [&f.teacher, &f.finance] {
        assert_eq!(
            h::admin_list_sections_handler(
                State(state.clone()),
                good(a),
                Query(h::AdminListSectionsQuery::default())
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    let list = r::list_sections(&g, &f.scoped, &r::ListSectionFilters::default())
        .await
        .unwrap();
    assert!(list.iter().all(|s| s.schoolId == f.school));
    assert_eq!(list.len(), 3);
    denied(
        r::list_sections(
            &g,
            &f.scoped,
            &r::ListSectionFilters {
                school: Some(f.other.clone()),
                ..Default::default()
            },
        )
        .await,
    );
    denied(r::find_section_by_id(&g, &f.scoped, &f.foreign).await);
    let (code, Json(body)) = h::admin_section_detail_handler(
        State(state.clone()),
        good(&f.scoped),
        Path(f.section.clone()),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body.data.unwrap().members.len(), 1);
    assert_eq!(
        h::admin_update_homeroom_teacher_handler(
            State(state.clone()),
            good(&f.owner),
            Path(f.section.clone()),
            Json(h::UpdateHomeroomTeacherRequest {
                name: Some("Synthetic display only".into()),
                email: Some("teacher@example.test".into())
            })
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        h::admin_update_section_status_handler(
            State(state.clone()),
            good(&f.owner),
            Path(f.section.clone()),
            Json(h::UpdateSectionStatusRequest {
                status: "active".into()
            })
        )
        .await
        .0,
        StatusCode::OK
    );
    let att = |id: &str| r::BulkAttendanceEntry {
        applicant_student_id: id.into(),
        status: "present".into(),
        notes: None,
    };
    let grade = |id: &str| r::BulkGradeEntry {
        applicant_student_id: id.into(),
        score: 8.,
        max_score: 10.,
        notes: None,
    };
    let before = count(
        &g,
        &f,
        "MATCH(a:SISAcademicAudit {school_id:$school}) RETURN count(a) AS n",
    )
    .await;
    // Mixed valid + foreign batches fail atomically, preserving the foreign edge.
    denied(
        r::assign_students(
            &g,
            &f.owner,
            &f.next,
            &[f.child.clone(), f.foreign_child.clone()],
        )
        .await,
    );
    denied(
        r::upsert_attendance_batch(
            &g,
            &f.owner,
            &f.section,
            "2026-10-05",
            &[att(&f.child), att(&f.foreign_child)],
        )
        .await,
    );
    denied(
        r::upsert_grades_batch(
            &g,
            &f.owner,
            &f.section,
            "Mathematics",
            "Term 1",
            &[grade(&f.child), grade(&f.foreign_child)],
        )
        .await,
    );
    assert_eq!(
        count(
            &g,
            &f,
            "MATCH(a:SISAcademicAudit {school_id:$school}) RETURN count(a) AS n"
        )
        .await,
        before
    );
    assert_eq!(count(&g,&f,"MATCH(:Student {studentId:$foreign_child})-[:ENROLLED_AS]->(:EnrolledStudent)-[:ENROLLED_IN]->(:Section {section_id:$foreign}) RETURN count(*) AS n").await,1);
    assert_eq!(
        count(
            &g,
            &f,
            "MATCH(r:AttendanceRecord {section_id:$section}) RETURN count(r) AS n"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &g,
            &f,
            "MATCH(g:GradeEntry {section_id:$section}) RETURN count(g) AS n"
        )
        .await,
        0
    );
    // Same-scope move is supported; Owner retains its distinct principal.
    assert_eq!(
        h::admin_assign_students_handler(
            State(state.clone()),
            good(&f.owner),
            Path(f.next.clone()),
            Json(h::AssignStudentsRequest {
                applicant_student_ids: vec![f.child.clone()]
            })
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        h::admin_attendance_upsert_handler(
            State(state.clone()),
            good(&f.owner),
            Path(f.next.clone()),
            Json(h::BulkAttendanceRequest {
                date: "2026-10-05".into(),
                entries: vec![h::AttendanceEntryIn {
                    applicant_student_id: f.child.clone(),
                    status: "present".into(),
                    notes: None
                }]
            })
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        h::admin_grades_upsert_handler(
            State(state.clone()),
            good(&f.owner),
            Path(f.next.clone()),
            Json(h::BulkGradesRequest {
                subject: "Mathematics".into(),
                term: "Term 1".into(),
                entries: vec![h::GradeEntryIn {
                    applicant_student_id: f.child.clone(),
                    score: 8.,
                    max_score: 10.,
                    notes: None
                }]
            })
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        h::admin_attendance_roster_handler(
            State(state.clone()),
            good(&f.scoped),
            Path(f.next.clone()),
            Query(h::AttendanceListQuery {
                date: Some("2026-10-05".into())
            })
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        h::admin_grade_roster_handler(
            State(state.clone()),
            good(&f.scoped),
            Path(f.next.clone()),
            Query(h::GradeListQuery {
                subject: Some("Mathematics".into()),
                term: Some("Term 1".into())
            })
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        h::admin_attendance_roster_handler(
            State(state.clone()),
            good(&f.owner),
            Path(f.next.clone()),
            Query(h::AttendanceListQuery {
                date: Some("2026-02-31".into())
            })
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let roster = r::list_attendance_for_date(&g, &f.scoped, &f.next, "2026-10-05")
        .await
        .unwrap();
    assert_eq!(
        roster[0].recordedBy.as_deref(),
        Some(f.owner.staff.as_str())
    );
    assert_eq!(count(&g,&f,"MATCH(a:SISAcademicAudit {school_id:$school}) WHERE a.actor_role='owner' AND a.actor_staff_member_id ENDS WITH '-owner-staff' AND a.actor_user_id ENDS WITH '-owner-user' RETURN count(a) AS n").await,6);
    assert_eq!(count(&g,&f,"MATCH(a:StaffMember {sis_academic_test:$tag}) WHERE a.id ENDS WITH '-owner-staff' AND 'teacher' IN a.roles RETURN count(a) AS n").await,0);
    // Same school with a foreign tenant is still forbidden.
    let mut wrong_tenant = f.input();
    wrong_tenant.tenant_id = Some(format!("{}-foreign", f.tenant));
    denied(r::create_section(&g, &f.scoped, &wrong_tenant).await);
    g.run(f.params(query(
        "MATCH(e:EnrolledStudent {applicant_student_id:$child}) SET e.tenant_id=$tenant+'-foreign'",
    )))
    .await
    .unwrap();
    assert!(r::list_section_members(&g, &f.scoped, &f.next)
        .await
        .unwrap()
        .is_empty());
    denied(
        r::upsert_attendance_batch(&g, &f.scoped, &f.next, "2026-10-05", &[att(&f.child)]).await,
    );
    g.run(f.params(query(
        "MATCH(e:EnrolledStudent {applicant_student_id:$child}) SET e.tenant_id=$tenant",
    )))
    .await
    .unwrap();
    let mut bad_att = att(&f.child);
    bad_att.status = "invalid".into();
    assert!(
        matches!(r::upsert_attendance_batch(&g,&f.owner,&f.next,"2026-10-05",&[bad_att]).await,Err(RepositoryError::DbError(ref code)) if code=="INVALID_INPUT")
    );
    let mut bad_grade = grade(&f.child);
    bad_grade.score = 11.;
    assert!(
        matches!(r::upsert_grades_batch(&g,&f.owner,&f.next,"Mathematics","Term 1",&[bad_grade]).await,Err(RepositoryError::DbError(ref code)) if code=="INVALID_INPUT")
    );
    // Corrupt/partial/team/revoked directory scope cannot fall back to email/JWT roles.
    for (mutation, restore) in [
        (
            "SET a.membershipStatus='INACTIVE'",
            "SET a.membershipStatus='ACTIVE'",
        ),
        ("SET a.roles=['teacher']", "SET a.roles=['school_admin']"),
        ("SET a.tenantIds=[]", "SET a.tenantIds=[$tenant]"),
        ("SET a.teamIds=['foreign-team']", "SET a.teamIds=[]"),
        ("SET u.staffMemberId='mismatch'", "SET u.staffMemberId=a.id"),
    ] {
        g.run(
            f.params(
                query(&format!(
                    "MATCH(u:User {{id:$subject}})-[:STAFF_MEMBER]->(a) {mutation}"
                ))
                .param("subject", f.scoped.subject.clone()),
            ),
        )
        .await
        .unwrap();
        denied(r::list_sections(&g, &f.scoped, &r::ListSectionFilters::default()).await);
        g.run(
            f.params(
                query(&format!(
                    "MATCH(u:User {{id:$subject}})-[:STAFF_MEMBER]->(a) {restore}"
                ))
                .param("subject", f.scoped.subject.clone()),
            ),
        )
        .await
        .unwrap();
    }
    // Duplicate canonical relationship fails closed even when token roles claim Owner.
    g.run(query("MATCH(u:User {id:$subject})-[:STAFF_MEMBER]->(a) CREATE(u)-[:STAFF_MEMBER {academic_duplicate:true}]->(a)").param("subject",f.scoped.subject.clone())).await.unwrap();
    denied(r::list_sections(&g, &f.scoped, &r::ListSectionFilters::default()).await);
    g.run(query("MATCH(u:User {id:$subject})-[edge:STAFF_MEMBER {academic_duplicate:true}]->() DELETE edge").param("subject",f.scoped.subject.clone())).await.unwrap();
    // Revocation while waiting on the school mutex cannot write or audit.
    let before_revoke = count(
        &g,
        &f,
        "MATCH(a:SISAcademicAudit {school_id:$school}) RETURN count(a) AS n",
    )
    .await;
    let mut school_lock = g.start_txn().await.unwrap();
    school_lock
        .run(
            query("MATCH(school:School {school_id:$school}) SET school.school_id=school.school_id")
                .param("school", f.school.clone()),
        )
        .await
        .unwrap();
    let (g2, actor, id) = (g.clone(), f.scoped.clone(), f.next.clone());
    let revoked =
        tokio::spawn(async move { r::set_section_status(&g2, &actor, &id, "archived").await });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(!revoked.is_finished());
    g.run(
        query("MATCH(a:StaffMember {id:$actor}) SET a.membershipStatus='INACTIVE'")
            .param("actor", f.scoped.staff.clone()),
    )
    .await
    .unwrap();
    school_lock.commit().await.unwrap();
    denied(revoked.await.unwrap());
    assert_eq!(
        count(
            &g,
            &f,
            "MATCH(a:SISAcademicAudit {school_id:$school}) RETURN count(a) AS n"
        )
        .await,
        before_revoke
    );
    g.run(
        query("MATCH(a:StaffMember {id:$actor}) SET a.membershipStatus='ACTIVE'")
            .param("actor", f.scoped.staff.clone()),
    )
    .await
    .unwrap();
    // A foreign enrollment edge cannot expose a child through otherwise scoped section.
    g.run(f.params(query("MATCH(e:EnrolledStudent {applicant_student_id:$foreign_child}),(sec:Section {section_id:$next}) CREATE(e)-[:ENROLLED_IN]->(sec)"))).await.unwrap();
    assert_eq!(
        r::list_section_members(&g, &f.scoped, &f.next)
            .await
            .unwrap()
            .len(),
        1
    );
    // A Section whose scope changes while queued cannot use the old authority.
    let before_scope = count(
        &g,
        &f,
        "MATCH(a:SISAcademicAudit {school_id:$school}) RETURN count(a) AS n",
    )
    .await;
    let mut scope_lock = g.start_txn().await.unwrap();
    scope_lock
        .run(
            query("MATCH(sec:Section {section_id:$id}) SET sec.section_id=sec.section_id")
                .param("id", f.next.clone()),
        )
        .await
        .unwrap();
    let (g2, actor, id) = (g.clone(), f.scoped.clone(), f.next.clone());
    let moved =
        tokio::spawn(async move { r::set_section_status(&g2, &actor, &id, "archived").await });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(!moved.is_finished());
    scope_lock
        .run(
            query("MATCH(sec:Section {section_id:$id}) SET sec.school_id=$other")
                .param("id", f.next.clone())
                .param("other", f.other.clone()),
        )
        .await
        .unwrap();
    scope_lock.commit().await.unwrap();
    denied(moved.await.unwrap());
    assert_eq!(
        count(
            &g,
            &f,
            "MATCH(a:SISAcademicAudit {school_id:$school}) RETURN count(a) AS n"
        )
        .await,
        before_scope
    );
    g.run(
        query("MATCH(sec:Section {section_id:$id}) SET sec.school_id=$school")
            .param("id", f.next.clone())
            .param("school", f.school.clone()),
    )
    .await
    .unwrap();
    // Credential/directory revocation while queued on Section is checked after waiting.
    let before = count(
        &g,
        &f,
        "MATCH(a:SISAcademicAudit {school_id:$school}) RETURN count(a) AS n",
    )
    .await;
    let mut lock = g.start_txn().await.unwrap();
    lock.run(
        query("MATCH(sec:Section {section_id:$id}) SET sec.section_id=sec.section_id")
            .param("id", f.next.clone()),
    )
    .await
    .unwrap();
    let (g2, actor, id) = (g.clone(), f.scoped.clone(), f.next.clone());
    let pending =
        tokio::spawn(async move { r::set_section_status(&g2, &actor, &id, "archived").await });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(!pending.is_finished());
    // This simulates expiry of a queued request without updating real identities:
    // use a short expiry with another request after the first safely completes.
    lock.commit().await.unwrap();
    assert!(pending.await.unwrap().is_ok());
    r::set_section_status(&g, &f.owner, &f.next, "active")
        .await
        .unwrap();
    let mut lock = g.start_txn().await.unwrap();
    lock.run(
        query("MATCH(sec:Section {section_id:$id}) SET sec.section_id=sec.section_id")
            .param("id", f.next.clone()),
    )
    .await
    .unwrap();
    let (g2, mut actor, id) = (g.clone(), f.scoped.clone(), f.next.clone());
    actor.expires = chrono::Utc::now().timestamp() + 2;
    let expired =
        tokio::spawn(async move { r::set_section_status(&g2, &actor, &id, "archived").await });
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert!(!expired.is_finished());
    lock.commit().await.unwrap();
    denied(expired.await.unwrap());
    assert_eq!(
        count(
            &g,
            &f,
            "MATCH(a:SISAcademicAudit {school_id:$school}) RETURN count(a) AS n"
        )
        .await,
        before + 2
    );
    assert_eq!(
        r::find_section_by_id(&g, &f.owner, &f.next)
            .await
            .unwrap()
            .unwrap()
            .status,
        "active"
    );
}
#[tokio::test]
#[ignore = "explicit dedicated isolated3385 required; synthetic protocol evidence only"]
async fn typed_admin_scope_atomic_batches_and_queued_expiry() {
    let uri = std::env::var("SIS_ACADEMIC_TEST_NEO4J_URI").unwrap();
    assert_eq!(uri, "bolt://127.0.0.1:3385");
    let g = Graph::new(
        &uri,
        &std::env::var("SIS_ACADEMIC_TEST_NEO4J_USER").unwrap(),
        &std::env::var("SIS_ACADEMIC_TEST_NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    let f = Fixture::new();
    let cleanup = Fixture {
        tag: f.tag.clone(),
        school: f.school.clone(),
        other: f.other.clone(),
        tenant: f.tenant.clone(),
        section: f.section.clone(),
        next: f.next.clone(),
        foreign: f.foreign.clone(),
        child: f.child.clone(),
        foreign_child: f.foreign_child.clone(),
        owner: f.owner.clone(),
        scoped: f.scoped.clone(),
        teacher: f.teacher.clone(),
        finance: f.finance.clone(),
    };
    let g2 = g.clone();
    let outcome = tokio::spawn(async move { run(g2, f).await }).await;
    g.run(cleanup.params(query("MATCH(n) WHERE n.sis_academic_test=$tag OR ((n:Section OR n:SISAcademicAudit) AND n.school_id IN [$school,$other]) OR ((n:AttendanceRecord OR n:GradeEntry) AND n.section_id IN [$section,$next,$foreign]) DETACH DELETE n"))).await.unwrap();
    outcome.unwrap();
}
