use super::{
    contract::{Export, Import},
    repository,
};
use axum::{
    http::{HeaderMap, HeaderValue, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
fn input() -> Import {
    Import {
        course_id: "course".into(),
        assessment_id: "assessment".into(),
        attempt_id: "attempt".into(),
        attempt_revision: 5,
        term: "Term 1".into(),
    }
}
fn export() -> Export {
    Export {
        schema_version: 1,
        school_id: "school".into(),
        tenant_id: "tenant".into(),
        class_id: "class".into(),
        course_id: "course".into(),
        course_revision: 3,
        assessment_id: "assessment".into(),
        assessment_revision: 3,
        attempt_id: "attempt".into(),
        attempt_revision: 5,
        student_id: "student".into(),
        author_id: "teacher".into(),
        subject: "mathematics".into(),
        score_minor: 4800,
        maximum_minor: 5000,
        submitted_at: 1000,
        results_available_at: None,
    }
}
#[test]
fn strict_owner_input_and_export_binding() {
    let body = json!({"courseId":"course","assessmentId":"assessment","attemptId":"attempt","attemptRevision":5,"term":"Term 1"});
    for field in [
        "score",
        "maximum",
        "studentId",
        "subject",
        "actorId",
        "source",
    ] {
        let mut bad = body.clone();
        bad[field] = json!("untrusted");
        assert!(serde_json::from_value::<Import>(bad).is_err());
    }
    assert!(input().valid());
    assert!(export().matches(&input(), "class", "teacher"));
    let mut e = export();
    e.attempt_revision = 4;
    assert!(!e.matches(&input(), "class", "teacher"));
    let mut e = export();
    e.score_minor = 5001;
    assert!(!e.matches(&input(), "class", "teacher"));
    let mut e = export();
    e.school_id = "school|other".into();
    assert!(!e.matches(&input(), "class", "teacher"));
    assert!(!export().matches(&input(), "foreign", "teacher"));
    assert!(!export().matches(&input(), "class", "other"));
    let mut raw = serde_json::to_value(export()).unwrap();
    raw["answers"] = json!([]);
    assert!(serde_json::from_value::<Export>(raw).is_err());
}
#[tokio::test]
#[ignore = "requires dedicated disposable graph on 3223; never clears unrelated records"]
async fn isolated_import_replay_conflict_scope_and_parent_publication() {
    use neo4rs::{query, Graph};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let uri = std::env::var("GRADE_IMPORT_TEST_BOLT").unwrap();
    assert_eq!(uri, "127.0.0.1:3223");
    let graph = Graph::new(
        uri,
        "neo4j",
        std::env::var("GRADE_IMPORT_TEST_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    repository::init(&graph).await.unwrap();
    // Clean only remnants carrying this module's own explicit test marker.
    graph.run(query("MATCH(marker) WHERE marker.grade_test_run STARTS WITH 'grade-' WITH collect(DISTINCT marker.grade_test_run) AS runs MATCH(n) WHERE n.grade_test_run IN runs OR any(run IN runs WHERE ((n:SchoolAssessmentGradeImport OR n:SchoolAssessmentGradeReceipt OR n:SchoolPublishedRecord) AND n.key STARTS WITH run+'|') OR (n:SchoolPortalAudit AND n.school_id=run)) DETACH DELETE n")).await.unwrap();
    let run = format!("grade-{}", uuid::Uuid::new_v4());
    let mut e = export();
    e.school_id = run.clone();
    e.tenant_id = run.clone();
    e.class_id = run.clone();
    e.student_id = run.clone();
    e.author_id = run.clone();
    let export_for_server = e.clone();
    let active = Arc::new(AtomicBool::new(true));
    let auth_active = active.clone();
    let teacher = run.clone();
    let router=Router::new().route("/api/v1/auth-service/oauth/membership",post(move||{let active=auth_active.clone();let teacher=teacher.clone();async move {if !active.load(Ordering::SeqCst){return (StatusCode::FORBIDDEN,Json(json!({})));}(StatusCode::OK,Json(json!({"active":true,"staffMemberId":teacher,"roles":["teacher"],"schoolIds":[teacher],"tenantIds":[teacher]})))}}))
        .route("/api/v1/learning/grade-exports/courses/:c/items/:a/attempts/:t",get(move||{let e=export_for_server.clone();async move{Json(json!({"data":e}))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    std::env::set_var("APP_ENV", "test");
    std::env::set_var(
        "AUTH_SCHOOL_MEMBERSHIP_URL",
        format!("http://127.0.0.1:{port}/api/v1/auth-service/oauth/membership"),
    );
    std::env::set_var(
        "LEARNING_SERVICE_ORIGIN",
        format!("http://127.0.0.1:{port}"),
    );
    // Audience-only transport stub; authority is the test membership endpoint.
    // This is a mock boundary test, never an Auth token or runtime acceptance credential.
    use base64::Engine;
    let payload =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"aud":"learning-api"}"#);
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer test.{payload}.stub")).unwrap(),
    );
    graph.run(query("CREATE(t:StaffMember {id:$run,email:$email,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$run],tenantIds:[$run],grade_test_run:$run}) CREATE(sec:Section {section_id:$run,name:'Test class',year_group:'7',school_id:$run,tenant_id:$run,status:'active',homeroom_staff_member_id:$run,grade_test_run:$run}) CREATE(u:User {id:$run,email:$parent,role:'parent',grade_test_run:$run})-[:HAS_APPLICATION]->(l:Lead {lead_id:$run,email:$parent,status:'verified',grade_test_run:$run})-[:HAS_STUDENT]->(s:Student {studentId:$run,fullName:'Isolated child',grade_test_run:$run})-[:ENROLLED_AS]->(en:EnrolledStudent {status:'active',school_id:$run,tenant_id:$run,grade_test_run:$run})-[:ENROLLED_IN]->(sec) CREATE(g:GradeEntry {entry_id:$run,section_id:$run,applicant_student_id:$run,subject:'mathematics',term:'Term 1',score:17.0,max_score:20.0,grade_test_run:$run}) CREATE(s)-[:HAS_GRADE]->(g)").param("run",run.clone()).param("email",format!("{run}@teacher.test")).param("parent",format!("{run}@parent.test"))).await.unwrap();
    let first = repository::import(
        graph.clone(),
        headers.clone(),
        run.clone(),
        input(),
        "command".into(),
    )
    .await
    .unwrap();
    let (a, b) = tokio::join!(
        repository::import(
            graph.clone(),
            headers.clone(),
            run.clone(),
            input(),
            "command".into()
        ),
        repository::import(
            graph.clone(),
            headers.clone(),
            run.clone(),
            input(),
            "command-2".into()
        )
    );
    assert_eq!(a.unwrap(), first);
    assert_eq!(b.unwrap(), first);
    let mut changed = input();
    changed.term = "Term 2".into();
    assert_eq!(
        repository::import(
            graph.clone(),
            headers.clone(),
            run.clone(),
            changed,
            "command".into()
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    let mut changed = input();
    changed.term = "Term 2".into();
    assert_eq!(
        repository::import(
            graph.clone(),
            headers.clone(),
            run.clone(),
            changed,
            "command-3".into()
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    // Grades and the independently owned calendar share the same Parent snapshot.
    // Exercise their composed read so a calendar query error cannot hide grades.
    let calendar = json!({
        "id": "grade-test-holiday",
        "schoolId": run,
        "tenantId": run,
        "academicYear": "2026/2027",
        "kind": "holiday",
        "title": "Isolated holiday",
        "description": "",
        "startDate": "2026-12-24",
        "endDate": "2026-12-25",
        "audience": "sections",
        "sectionIds": [run]
    });
    graph.run(query("MATCH(sec:Section {section_id:$run}) SET sec.academic_year='2026/2027' CREATE(:EducationCalendarEntry {key:$key,id:'grade-test-holiday',school_id:$run,tenant_id:$run,academic_year:'2026/2027',status:'published',version:3,audience:'sections',section_ids:[$run],start_date:'2026-12-24',end_date:'2026-12-25',payload:$payload,grade_test_run:$run})").param("run",run.clone()).param("key",format!("{run}|calendar")).param("payload",calendar.to_string())).await.unwrap();
    let snapshot = crate::school_portal::repository::parent_snapshot(&graph, &run)
        .await
        .unwrap();
    assert_eq!(snapshot["records"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["records"][0]["studentId"], run);
    assert_eq!(
        snapshot["records"][0]["content"]["id"],
        first["grade"]["id"]
    );
    assert_eq!(snapshot["records"][0]["content"]["score"], 48.0);
    assert_eq!(
        snapshot["records"][0]["content"]["source"]["attempt_revision"],
        5
    );
    assert_eq!(
        snapshot["educationCalendar"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(snapshot["educationCalendar"]["items"][0]["entry"], calendar);
    let mut published=graph.execute(query(&format!("{} WITH DISTINCT u,s {} MATCH(r:SchoolPublishedRecord)-[:FOR_SECTION]->(sec) WHERE r.kind='grade' AND r.student_id=s.studentId RETURN DISTINCT r.payload AS payload",crate::school_portal::repository::PARENT,crate::school_portal::repository::PARENT_SECTION)).param("sub",run.clone())).await.unwrap();
    let grade: serde_json::Value = serde_json::from_str(
        &published
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<String>("payload")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(grade["score"], 48.0);
    assert_eq!(grade["source"]["attempt_revision"], 5);
    assert!(published.next().await.unwrap().is_none());
    let mut manual = graph
        .execute(
            query("MATCH(g:GradeEntry {entry_id:$run}) RETURN g.score AS score")
                .param("run", run.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        manual
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<f64>("score")
            .unwrap(),
        17.0
    );
    graph
        .run(
            query("MATCH(e:EnrolledStudent {grade_test_run:$run}) SET e.status='inactive'")
                .param("run", run.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        repository::import(
            graph.clone(),
            headers.clone(),
            run.clone(),
            input(),
            "command".into()
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::FORBIDDEN
    );
    graph
        .run(
            query("MATCH(e:EnrolledStudent {grade_test_run:$run}) SET e.status='active'")
                .param("run", run.clone()),
        )
        .await
        .unwrap();
    active.store(false, Ordering::SeqCst);
    assert_eq!(
        repository::import(
            graph.clone(),
            headers,
            run.clone(),
            input(),
            "command".into()
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::FORBIDDEN
    );
    graph.run(query("MATCH(n) WHERE n.grade_test_run=$run OR (n:SchoolAssessmentGradeImport AND n.key STARTS WITH $prefix) OR (n:SchoolAssessmentGradeReceipt AND n.key STARTS WITH $prefix) OR (n:SchoolPublishedRecord AND n.key STARTS WITH $prefix) OR (n:SchoolPortalAudit AND n.school_id=$run) DETACH DELETE n").param("run",run.clone()).param("prefix",format!("{run}|"))).await.unwrap();
    server.abort();
}

#[test]
fn owner_import_preserves_actual_author_without_bypassing_export_integrity() {
    let mut e = export();
    let original_author = e.author_id.clone();
    assert!(e.matches_for(&input(), "class", "real-owner", true));
    assert_eq!(e.author_id, original_author);
    assert!(!e.matches_for(&input(), "class", "real-owner", false));
    e.maximum_minor = 0;
    assert!(!e.matches_for(&input(), "class", "real-owner", true));
}
