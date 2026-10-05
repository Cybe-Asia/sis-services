use super::*;
use crate::school_portal::{
    auth::Teacher,
    model::{growth::*, PublishedRecord},
    repository as repo,
};
#[tokio::test]
#[ignore = "requires named disposable SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223"]
async fn heavy_growth_is_child_bound_latest_and_does_not_expand_home() {
    assert_eq!(
        std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap(),
        "127.0.0.1:3223"
    );
    let graph = Graph::new("127.0.0.1:3223", "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    repo::init(&graph).await.unwrap();
    let key = uuid::Uuid::new_v4().to_string();
    let email = format!("parent-{key}@example.test");
    graph.run(query("CREATE(t:StaffMember {id:$key,email:'growth-teacher@example.test',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['growth-school'],tenantIds:['growth-tenant'],test_key:$key}) CREATE(c:Section {section_id:$key,name:'Class',status:'active',school_id:'growth-school',tenant_id:'growth-tenant',homeroom_staff_member_id:$key,test_key:$key}) CREATE(u:User {id:$key,email:$email,role:'parent',test_key:$key})-[:HAS_APPLICATION]->(:Lead {email:$email,status:'verified',test_key:$key})-[:HAS_STUDENT]->(s:Student {studentId:$key,fullName:'Child',test_key:$key})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'growth-school',tenant_id:'growth-tenant',test_key:$key})-[:ENROLLED_IN]->(c) CREATE(s)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'growth-school',tenant_id:'growth-tenant',test_key:$key})-[:ENROLLED_IN]->(:Section {section_id:$other,name:'Second section',status:'active',school_id:'growth-school',tenant_id:'growth-tenant',test_key:$key})").param("key",key.clone()).param("email",email).param("other",format!("other-{key}"))).await.unwrap();
    let teacher = Teacher {
        active: true,
        staff_member_id: key.clone(),
        roles: vec!["teacher".into()],
        school_ids: vec!["growth-school".into()],
        tenant_ids: vec!["growth-tenant".into()],
    };
    let artifact = Artifact {
        title: "Actual project".into(),
        subject: "Science".into(),
        summary: "a".repeat(4000),
        format: "PDF".into(),
        score: None,
        maximum: None,
        award: None,
        document_id: None,
    };
    let report = GrowthReport {
        term: "Term 1".into(),
        academic: None,
        attendance: None,
        extracurricular: vec![],
        comparisons: vec![],
        analysis: None,
        teacher_note: None,
        portfolio: vec![artifact.clone(); 50],
    };
    let mut record = PublishedRecord::Growth {
        id: "old".into(),
        student_id: key.clone(),
        report: report.clone(),
    };
    assert!(record.valid());
    assert!(!repo::publish(
        &graph,
        &Teacher {
            staff_member_id: format!("foreign-{key}"),
            active: true,
            roles: teacher.roles.clone(),
            school_ids: teacher.school_ids.clone(),
            tenant_ids: teacher.tenant_ids.clone()
        },
        &key,
        &record
    )
    .await
    .unwrap());
    assert!(repo::publish(&graph, &teacher, &key, &record)
        .await
        .unwrap());
    record = PublishedRecord::Growth {
        id: "latest".into(),
        student_id: key.clone(),
        report: report.clone(),
    };
    assert!(repo::publish(&graph, &teacher, &key, &record)
        .await
        .unwrap());
    let value = repo::parent_growth(&graph, &key, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(value["content"]["id"], "latest");
    assert!(
        serde_json::to_vec(&json!({"responseCode":200,"data":value}))
            .unwrap()
            .len()
            < 524_288
    );
    assert!(
        repo::parent_snapshot(&graph, &key).await.unwrap()["records"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Valid heavy histories must not make the roster/attendance page huge,
    // or disclose full homeroom reports to a subject-only Teacher.
    let mut heavy = report.clone();
    for item in &mut heavy.portfolio {
        item.summary = format!("a{}", "\n".repeat(3999));
    }
    for index in 0..22 {
        let publication = PublishedRecord::Growth {
            id: format!("heavy-{index}"),
            student_id: key.clone(),
            report: heavy.clone(),
        };
        assert!(publication.valid());
        assert!(repo::publish(&graph, &teacher, &key, &publication)
            .await
            .unwrap());
    }
    let subject_id = format!("subject-{key}");
    graph.run(query("CREATE(t:StaffMember {id:$subject,email:'subject-teacher@example.test',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['growth-school'],tenantIds:['growth-tenant'],test_key:$key}) CREATE(:LearningTeachingGrant {key:$subject,staff_member_id:$subject,section_id:$key,school_id:'growth-school',tenant_id:'growth-tenant',status:'ACTIVE',test_key:$key})").param("subject",subject_id.clone()).param("key",key.clone())).await.unwrap();
    let subject_teacher = Teacher {
        active: true,
        staff_member_id: subject_id,
        roles: teacher.roles.clone(),
        school_ids: teacher.school_ids.clone(),
        tenant_ids: teacher.tenant_ids.clone(),
    };
    for member in [&teacher, &subject_teacher] {
        let classroom = repo::classroom(&graph, member, &key, "2026-10-01")
            .await
            .unwrap()
            .unwrap();
        assert!(classroom["records"].as_array().unwrap().is_empty());
        assert_eq!(classroom["recordWindow"]["hasMore"], false);
        assert!(serde_json::to_vec(&classroom).unwrap().len() < 524_288);
    }
    let payloads:Vec<String>=(0..101).map(|i|json!({"kind":"event","id":format!("window-{i}"),"title":"Class event","date":"2026-10-01","start":null,"end":null,"category":"school","location":"Campus"}).to_string()).collect();
    graph.run(query("UNWIND range(0,size($payloads)-1) AS i MATCH(c:Section {section_id:$key}) CREATE(r:SchoolPublishedRecord {key:$key+'|window-'+toString(i),kind:'event',payload:$payloads[i],created_at:datetime()+duration({seconds:i}),test_key:$key})-[:FOR_SECTION]->(c)").param("payloads",payloads).param("key",key.clone())).await.unwrap();
    let window = repo::classroom(&graph, &subject_teacher, &key, "2026-10-01")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(window["records"].as_array().unwrap().len(), 100);
    assert_eq!(window["recordWindow"]["hasMore"], true);
    assert_eq!(window["records"][0]["id"], "window-100");
    assert!(repo::parent_growth(&graph, &key, "foreign")
        .await
        .unwrap()
        .is_none());
    graph
        .run(
            query("MATCH(e:EnrolledStudent {test_key:$key}) SET e.status='inactive'")
                .param("key", key.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        repo::parent_growth(&graph, &key, &key).await.unwrap(),
        Some(serde_json::Value::Null)
    );
    let mut escaped = report;
    escaped.portfolio = vec![
        Artifact {
            summary: "\u{1}".repeat(4000),
            ..artifact
        };
        50
    ];
    assert!(!escaped.valid());
    graph
        .run(query("MATCH(n {test_key:$key}) DETACH DELETE n").param("key", key))
        .await
        .unwrap();
}
