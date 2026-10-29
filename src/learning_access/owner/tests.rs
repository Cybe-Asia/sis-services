use super::*;
#[tokio::test]
async fn setup_tokens_and_non_family_hs_tokens_have_no_learning_authority() {
    let previous_endpoint = std::env::var_os("AUTH_STUDENT_SESSION_URL");
    std::env::remove_var("AUTH_STUDENT_SESSION_URL");
    std::env::set_var("JWT_SECRET", "owner-contract-test-key-at-least-32-bytes");
    let scope = Scope {
        school_id: "school".into(),
        tenant_id: "tenant".into(),
        class_id: None,
        student_id: None,
        access: None,
    };
    for (role, scope_value, expected) in [
        ("parent", None, 200),
        ("student", None, 503),
        ("teacher", None, 403),
        ("parent", Some("password_setup"), 401),
        ("student", Some("eoi_setup"), 401),
    ] {
        let mut claims = json!({"sub":"user","role":role,"exp":4102444800u64});
        if let Some(value) = scope_value {
            claims["scope"] = json!(value);
        }
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(b"owner-contract-test-key-at-least-32-bytes"),
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        match actor(&headers, &scope).await {
            Ok(_) => assert_eq!(expected, 200),
            Err((status, _)) => assert_eq!(status.as_u16(), expected),
        }
    }
    if let Some(endpoint) = previous_endpoint {
        std::env::set_var("AUTH_STUDENT_SESSION_URL", endpoint);
    }
}
#[tokio::test]
#[ignore = "requires dedicated disposable SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223"]
async fn current_owner_projection_denies_foreign_family_and_revoked_class_access() {
    assert_eq!(
        std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap(),
        "127.0.0.1:3223"
    );
    let graph = neo4rs::Graph::new("127.0.0.1:3223", "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    for statement in include_str!("../../../migrations/0001_learning_access.cypher")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        graph.run(query(statement)).await.unwrap();
    }
    let key = uuid::Uuid::new_v4().to_string();
    graph.run(query("CREATE(sec:Section {section_id:$key,name:'Class',school_id:'owner-school',tenant_id:'owner-tenant',status:'active',homeroom_staff_member_id:$key,test_key:$key}) CREATE(:StaffMember {id:$key,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['owner-school'],tenantIds:['owner-tenant'],test_key:$key}) CREATE(u:User {id:$key,role:'parent',email:$email,test_key:$key})-[:HAS_APPLICATION]->(:Lead {email:$email,status:'verified',test_key:$key})-[:HAS_STUDENT]->(s:Student {studentId:$key,fullName:'Owned Student',test_key:$key})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'owner-school',tenant_id:'owner-tenant',test_key:$key})-[:ENROLLED_IN]->(sec) CREATE(:User {id:$student_user,role:'student',test_key:$key}) CREATE(:LearningStudentBinding {user_id:$student_user,student_id:$key,status:'ACTIVE',school_id:'owner-school',tenant_id:'owner-tenant',version:1,test_key:$key})").param("key",key.clone()).param("student_user",format!("student-{key}")).param("email",format!("parent-{key}@example.test"))).await.unwrap();
    let s = AppState {
        graph: graph.clone(),
        config: crate::config::config::AppConfig {
            server_port: 0,
            neo4j_uri: "127.0.0.1:3223".into(),
            neo4j_user: "neo4j".into(),
            neo4j_password: "parent-otp-fixture-password".into(),
            jwt_secret: "owner-contract-test-key-at-least-32-bytes".into(),
        },
        http_client: reqwest::Client::new(),
    };
    let mut scope = Scope {
        school_id: "owner-school".into(),
        tenant_id: "owner-tenant".into(),
        class_id: None,
        student_id: None,
        access: None,
    };
    for a in [
        Actor {
            id: key.clone(),
            role: "teacher".into(),
        },
        Actor {
            id: key.clone(),
            role: "parent".into(),
        },
        Actor {
            id: format!("student-{key}"),
            role: "student".into(),
        },
    ] {
        let rows = projection(&s, &a, &scope).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["students"][0]["id"], key);
        scope.tenant_id = "other-tenant".into();
        assert!(projection(&s, &a, &scope).await.unwrap().is_empty());
        scope.tenant_id = "owner-tenant".into();
    }
    assert!(projection(
        &s,
        &Actor {
            id: "foreign-parent".into(),
            role: "parent".into()
        },
        &scope
    )
    .await
    .unwrap()
    .is_empty());
    let parent = Actor {
        id: key.clone(),
        role: "parent".into(),
    };
    for statement in [
        "MATCH(l:Lead {test_key:$key}) SET l.status='revoked'",
        "MATCH(l:Lead {test_key:$key}) SET l.email='other@example.test'",
        "CREATE(:StaffMember {id:$staff,email:$email,test_key:$key})",
    ] {
        graph
            .run(
                query(statement)
                    .param("key", key.clone())
                    .param("staff", format!("directory-{key}"))
                    .param("email", format!("parent-{key}@example.test")),
            )
            .await
            .unwrap();
        assert!(projection(&s, &parent, &scope).await.unwrap().is_empty());
        graph
            .run(
                query("MATCH(l:Lead {test_key:$key}) SET l.status='verified',l.email=$email")
                    .param("key", key.clone())
                    .param("email", format!("parent-{key}@example.test")),
            )
            .await
            .unwrap();
    }
    graph
        .run(
            query("MATCH(staff:StaffMember {id:$staff,test_key:$key}) DELETE staff")
                .param("key", key.clone())
                .param("staff", format!("directory-{key}")),
        )
        .await
        .unwrap();
    assert_eq!(projection(&s, &parent, &scope).await.unwrap().len(), 1);
    graph.run(query("MATCH(sec:Section {test_key:$key}) REMOVE sec.homeroom_staff_member_id WITH sec MATCH(b:LearningStudentBinding {test_key:$key}) SET b.status='REVOKED'").param("key",key.clone())).await.unwrap();
    assert!(projection(
        &s,
        &Actor {
            id: key.clone(),
            role: "teacher".into()
        },
        &scope
    )
    .await
    .unwrap()
    .is_empty());
    assert!(projection(
        &s,
        &Actor {
            id: format!("student-{key}"),
            role: "student".into()
        },
        &scope
    )
    .await
    .unwrap()
    .is_empty());
    graph
        .run(query("MATCH(n {test_key:$key}) DETACH DELETE n").param("key", key))
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "requires dedicated disposable SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223"]
async fn homeroom_access_and_summary_belong_to_the_homeroom_teacher_or_an_owner() {
    use crate::school_portal::{auth::Teacher, model::homeroom::range, repository};
    assert_eq!(
        std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap(),
        "127.0.0.1:3223"
    );
    let graph = neo4rs::Graph::new("127.0.0.1:3223", "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    for statement in include_str!("../../../migrations/0001_learning_access.cypher")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        graph.run(query(statement)).await.unwrap();
    }
    let key = uuid::Uuid::new_v4().to_string();
    let (school, tenant, class) = (
        format!("sch-{key}"),
        format!("ten-{key}"),
        format!("sec-{key}"),
    );
    let (home, subject, owner) = (
        format!("home-{key}"),
        format!("subj-{key}"),
        format!("own-{key}"),
    );
    graph.run(query("CREATE(:School {school_id:$school,tenant_id:$tenant,test_key:$key}) CREATE(:StaffMember {id:$home,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],test_key:$key}) CREATE(:StaffMember {id:$subject,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],test_key:$key}) CREATE(:StaffMember {id:$owner,membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[],teamIds:[],test_key:$key}) CREATE(sec:Section {section_id:$class,name:'7A',year_group:'Year 7',school_id:$school,tenant_id:$tenant,status:'active',homeroom_staff_member_id:$home,test_key:$key}) CREATE(other:Section {section_id:$class+'-b',name:'7B',year_group:'Year 7',school_id:$school,tenant_id:$tenant,status:'active',test_key:$key}) CREATE(:LearningTeachingGrant {staff_member_id:$subject,section_id:$class,school_id:$school,tenant_id:$tenant,status:'ACTIVE',test_key:$key}) CREATE(a:Student {studentId:'aisha-'+$key,fullName:'Aisha',test_key:$key})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant,test_key:$key})-[:ENROLLED_IN]->(sec) CREATE(b:Student {studentId:'bima-'+$key,fullName:'Bima',test_key:$key})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant,test_key:$key})-[:ENROLLED_IN]->(sec) CREATE(c:Student {studentId:'citra-'+$key,fullName:'Citra',test_key:$key})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant,test_key:$key})-[:ENROLLED_IN]->(other) WITH a,b,c FOREACH(r IN [['aisha','2026-09-01','present'],['aisha','2026-09-02','late'],['aisha','2026-10-30','absent'],['bima','2026-09-01','excused'],['citra','2026-09-01','absent']] | CREATE(:AttendanceRecord {section_id:CASE r[0] WHEN 'citra' THEN $class+'-b' ELSE $class END,applicant_student_id:r[0]+'-'+$key,date:r[1],status:r[2],test_key:$key})) FOREACH(g IN [['aisha','Math','Semester 1',80],['aisha','Science','Semester 2',70],['bima','Math','Semester 1',60]] | CREATE(:GradeEntry {section_id:$class,applicant_student_id:g[0]+'-'+$key,subject:g[1],term:g[2],score:g[3],max_score:100.0,test_key:$key}))")
        .param("key",key.clone()).param("school",school.clone()).param("tenant",tenant.clone()).param("class",class.clone()).param("home",home.clone()).param("subject",subject.clone()).param("owner",owner.clone())).await.unwrap();
    let s = AppState {
        graph: graph.clone(),
        config: crate::config::config::AppConfig {
            server_port: 0,
            neo4j_uri: "127.0.0.1:3223".into(),
            neo4j_user: "neo4j".into(),
            neo4j_password: "parent-otp-fixture-password".into(),
            jwt_secret: "owner-contract-test-key-at-least-32-bytes".into(),
        },
        http_client: reqwest::Client::new(),
    };
    let scope = |access: &str| Scope {
        school_id: school.clone(),
        tenant_id: tenant.clone(),
        class_id: Some(class.clone()),
        student_id: None,
        access: Some(access.into()),
    };
    let actor = |id: &str, role: &str| Actor {
        id: id.into(),
        role: role.into(),
    };
    let status = |r: Result<(), Failure>| r.map_or_else(|(s, _)| s.as_u16(), |_| 200);
    assert_eq!(
        status(permitted(&s, &actor(&home, "teacher"), &scope("homeroom")).await),
        200
    );
    assert_eq!(
        status(permitted(&s, &actor(&home, "teacher"), &scope("teach")).await),
        200
    );
    assert_eq!(
        status(permitted(&s, &actor(&subject, "teacher"), &scope("teach")).await),
        200
    );
    assert_eq!(
        status(permitted(&s, &actor(&subject, "teacher"), &scope("homeroom")).await),
        403,
        "a teaching grant is not a homeroom"
    );
    assert_eq!(
        status(permitted(&s, &actor(&owner, "owner"), &scope("homeroom")).await),
        200
    );
    let mut foreign = scope("homeroom");
    foreign.class_id = Some(format!("{class}-b"));
    assert_eq!(
        status(permitted(&s, &actor(&home, "teacher"), &foreign).await),
        403
    );
    let mut with_student = scope("homeroom");
    with_student.student_id = Some(format!("aisha-{key}"));
    assert_eq!(
        status(permitted(&s, &actor(&home, "teacher"), &with_student).await),
        403
    );

    let member = |id: &str, roles: &[&str]| Teacher {
        active: true,
        staff_member_id: id.into(),
        roles: roles.iter().map(|r| r.to_string()).collect(),
        school_ids: vec![school.clone()],
        tenant_ids: vec![tenant.clone()],
    };
    let semester = range("2026-08-01", "2026-09-30").unwrap();
    let summary = repository::homeroom_summary(
        &graph,
        &member(&home, &["teacher"]),
        &class,
        semester,
        Some("Semester 1"),
    )
    .await
    .unwrap()
    .unwrap();
    let students = summary["students"].as_array().unwrap();
    assert_eq!(students.len(), 2, "another class's student is not listed");
    assert_eq!(students[0]["name"], "Aisha");
    assert_eq!(
        students[0]["attendance"],
        json!({"present":1,"late":1,"absent":0,"excused":0,"recordedDays":2}),
        "a record after the range is left out"
    );
    assert_eq!(
        students[0]["grades"],
        json!([{"subject":"Math","score":80.0,"maxScore":100.0}])
    );
    assert_eq!(students[1]["attendance"]["excused"], 1);
    assert_eq!(students[1]["grades"][0]["score"], 60.0);
    let no_term =
        repository::homeroom_summary(&graph, &member(&home, &["teacher"]), &class, semester, None)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(no_term["students"][0]["grades"], json!([]));
    assert!(
        repository::homeroom_summary(
            &graph,
            &member(&subject, &["teacher"]),
            &class,
            semester,
            None
        )
        .await
        .unwrap()
        .is_none(),
        "a subject-only grant gets nothing"
    );
    assert!(repository::homeroom_summary(
        &graph,
        &member(&owner, &["owner"]),
        &class,
        semester,
        None
    )
    .await
    .unwrap()
    .is_some());
    graph
        .run(query("MATCH(n {test_key:$key}) DETACH DELETE n").param("key", key))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires dedicated disposable SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223"]
async fn school_classes_carry_grade_year_and_the_callers_taught_subjects() {
    assert_eq!(
        std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap(),
        "127.0.0.1:3223"
    );
    let graph = neo4rs::Graph::new("127.0.0.1:3223", "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    for statement in include_str!("../../../migrations/0001_learning_access.cypher")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        graph.run(query(statement)).await.unwrap();
    }
    let key = uuid::Uuid::new_v4().to_string();
    let (school, tenant) = (format!("sch-{key}"), format!("ten-{key}"));
    let (teacher, owner, parentless) = (
        format!("t-{key}"),
        format!("own-{key}"),
        format!("other-{key}"),
    );
    graph.run(query("CREATE(:School {school_id:$school,tenant_id:$tenant,test_key:$key}) CREATE(:StaffMember {id:$teacher,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],test_key:$key}) CREATE(:StaffMember {id:$other,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['elsewhere'],tenantIds:[$tenant],test_key:$key}) CREATE(:StaffMember {id:$owner,membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[],teamIds:[],test_key:$key}) CREATE(:Section {section_id:'7a-'+$key,name:'7A',year_group:'Year 7',academic_year:'2026/2027',school_id:$school,tenant_id:$tenant,status:'active',test_key:$key}) CREATE(:Section {section_id:'7b-'+$key,name:'7B',year_group:'Year 7',academic_year:'2026/2027',school_id:$school,tenant_id:$tenant,status:'active',test_key:$key}) CREATE(:Section {section_id:'7c-'+$key,name:'7C',school_id:$school,tenant_id:$tenant,status:'archived',test_key:$key}) CREATE(:TeachingAssignment {staff_member_id:$teacher,section_id:'7b-'+$key,subject:'mathematics',academic_year:'2026/2027',school_id:$school,tenant_id:$tenant,status:'active',test_key:$key}) CREATE(:TeachingAssignment {staff_member_id:$teacher,section_id:'7a-'+$key,subject:'science',academic_year:'2025/2026',school_id:$school,tenant_id:$tenant,status:'active',test_key:$key}) CREATE(:TeachingAssignment {staff_member_id:$teacher,section_id:'7a-'+$key,subject:'physics',academic_year:'2026/2027',school_id:$school,tenant_id:$tenant,status:'ended',test_key:$key})")
        .param("key",key.clone()).param("school",school.clone()).param("tenant",tenant.clone()).param("teacher",teacher.clone()).param("owner",owner.clone()).param("other",parentless.clone())).await.unwrap();
    let s = AppState {
        graph: graph.clone(),
        config: crate::config::config::AppConfig {
            server_port: 0,
            neo4j_uri: "127.0.0.1:3223".into(),
            neo4j_user: "neo4j".into(),
            neo4j_password: "parent-otp-fixture-password".into(),
            jwt_secret: "owner-contract-test-key-at-least-32-bytes".into(),
        },
        http_client: reqwest::Client::new(),
    };
    let scope = Scope {
        school_id: school.clone(),
        tenant_id: tenant.clone(),
        class_id: None,
        student_id: None,
        access: None,
    };
    let actor = |id: &str, role: &str| Actor {
        id: id.into(),
        role: role.into(),
    };
    let classes = school_classes::read(&s, &actor(&teacher, "teacher"), &scope)
        .await
        .unwrap();
    assert_eq!(classes.len(), 2, "an archived class is left out");
    assert_eq!(
        classes[0],
        json!({"id":format!("7a-{key}"),"name":"7A","yearGroup":"Year 7","academicYear":"2026/2027","taughtSubjects":[]}),
        "an ended or other-year assignment is not taught"
    );
    assert_eq!(classes[1]["taughtSubjects"], json!(["mathematics"]));
    let owned = school_classes::read(&s, &actor(&owner, "owner"), &scope)
        .await
        .unwrap();
    assert_eq!(owned.len(), 2);
    for (id, role) in [
        (&parentless, "teacher"),
        (&teacher, "owner"),
        (&teacher, "student"),
    ] {
        assert_eq!(
            school_classes::read(&s, &actor(id, role), &scope)
                .await
                .unwrap_err()
                .0,
            StatusCode::FORBIDDEN
        );
    }
    graph
        .run(query("MATCH(n {test_key:$key}) DETACH DELETE n").param("key", key))
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "requires dedicated disposable SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223"]
async fn student_history_lists_enrolments_across_schools_to_callers_who_see_the_student() {
    assert_eq!(
        std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap(),
        "127.0.0.1:3223"
    );
    let graph = neo4rs::Graph::new("127.0.0.1:3223", "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    for statement in include_str!("../../../migrations/0001_learning_access.cypher")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        graph.run(query(statement)).await.unwrap();
    }
    let key = uuid::Uuid::new_v4().to_string();
    let (iiss, iihs, tenant) = (
        format!("iiss-{key}"),
        format!("iihs-{key}"),
        format!("ten-{key}"),
    );
    // A Student placed in IISS Year 7 in 2025, now in 8A there (that enrolment ended), then placed in IIHS 10A.
    graph.run(query("CREATE(:School {school_id:$iiss,tenant_id:$tenant,name:'IISS',test_key:$key}) CREATE(:School {school_id:$iihs,tenant_id:$tenant,name:'IIHS',test_key:$key}) CREATE(a8:Section {section_id:'8a-'+$key,name:'8A',year_group:'Year 8',academic_year:'2026/2027',school_id:$iiss,tenant_id:$tenant,status:'archived',test_key:$key}) CREATE(a10:Section {section_id:'10a-'+$key,name:'10A',year_group:'Year 10',academic_year:'2027/2028',school_id:$iihs,tenant_id:$tenant,status:'active',homeroom_staff_member_id:'home-'+$key,test_key:$key}) CREATE(:Section {section_id:'10b-'+$key,name:'10B',year_group:'Year 10',academic_year:'2027/2028',school_id:$iihs,tenant_id:$tenant,status:'active',homeroom_staff_member_id:'other-'+$key,test_key:$key}) CREATE(:StaffMember {id:'home-'+$key,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$iihs],tenantIds:[$tenant],test_key:$key}) CREATE(:StaffMember {id:'other-'+$key,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$iihs],tenantIds:[$tenant],test_key:$key}) CREATE(u:User {id:'parent-'+$key,role:'parent',email:$email,test_key:$key})-[:HAS_APPLICATION]->(:Lead {email:$email,status:'verified',test_key:$key})-[:HAS_STUDENT]->(s:Student {studentId:$key,fullName:'Moving Student',test_key:$key}) CREATE(s)-[:ENROLLED_AS]->(e1:EnrolledStudent {status:'inactive',school_id:$iiss,tenant_id:$tenant,entry_year:2025,year_group:'Year 7',graduation_year:2028,test_key:$key})-[:ENROLLED_IN]->(a8) CREATE(s)-[:ENROLLED_AS]->(e2:EnrolledStudent {status:'active',school_id:$iihs,tenant_id:$tenant,entry_year:2027,year_group:'Year 10',graduation_year:2030,test_key:$key})-[:ENROLLED_IN]->(a10) CREATE(s)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'foreign-'+$key,tenant_id:'foreign-'+$key,entry_year:2020,test_key:$key}) CREATE(:User {id:'student-'+$key,role:'student',test_key:$key}) CREATE(:LearningStudentBinding {user_id:'student-'+$key,student_id:$key,status:'ACTIVE',school_id:$iihs,tenant_id:$tenant,version:1,test_key:$key}) CREATE(other:Student {studentId:'peer-'+$key,fullName:'Peer',test_key:$key})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$iihs,tenant_id:$tenant,test_key:$key})-[:ENROLLED_IN]->(a10)")
        .param("key",key.clone()).param("iiss",iiss.clone()).param("iihs",iihs.clone()).param("tenant",tenant.clone()).param("email",format!("parent-{key}@example.test"))).await.unwrap();
    let s = AppState {
        graph: graph.clone(),
        config: crate::config::config::AppConfig {
            server_port: 0,
            neo4j_uri: "127.0.0.1:3223".into(),
            neo4j_user: "neo4j".into(),
            neo4j_password: "parent-otp-fixture-password".into(),
            jwt_secret: "owner-contract-test-key-at-least-32-bytes".into(),
        },
        http_client: reqwest::Client::new(),
    };
    let scope = Scope {
        school_id: iihs.clone(),
        tenant_id: tenant.clone(),
        class_id: None,
        student_id: None,
        access: None,
    };
    let actor = |id: String, role: &str| Actor {
        id,
        role: role.into(),
    };
    let expected = json!([
        {"schoolId":iiss,"schoolName":"IISS","status":"inactive","entryYear":2025,"entryYearGroup":"Year 7","graduationYear":2028,
         "class":{"id":format!("8a-{key}"),"name":"8A","yearGroup":"Year 8","academicYear":"2026/2027"}},
        {"schoolId":iihs,"schoolName":"IIHS","status":"active","entryYear":2027,"entryYearGroup":"Year 10","graduationYear":2030,
         "class":{"id":format!("10a-{key}"),"name":"10A","yearGroup":"Year 10","academicYear":"2027/2028"}},
    ]);
    for a in [
        actor(format!("student-{key}"), "student"),
        actor(format!("parent-{key}"), "parent"),
        actor(format!("home-{key}"), "teacher"),
    ] {
        let enrolments = student_history::read(&s, &a, &scope, &key).await.unwrap();
        assert_eq!(
            json!(enrolments),
            expected,
            "{}: both schools of the tenant, the other tenant left out",
            a.role
        );
    }
    for (a, student) in [
        (
            actor(format!("student-{key}"), "student"),
            format!("peer-{key}"),
        ),
        (actor(format!("other-{key}"), "teacher"), key.clone()),
        (
            actor(format!("parent-{key}"), "parent"),
            format!("peer-{key}"),
        ),
        (actor(format!("home-{key}"), "parent"), key.clone()),
    ] {
        assert_eq!(
            student_history::read(&s, &a, &scope, &student)
                .await
                .unwrap_err()
                .0,
            StatusCode::FORBIDDEN,
            "{} {student}",
            a.id
        );
    }
    // Pinned to IISS, where the Student no longer studies, the homeroom of IIHS sees nothing.
    let iiss_scope = Scope {
        school_id: iiss.clone(),
        ..scope
    };
    assert_eq!(
        student_history::read(
            &s,
            &actor(format!("home-{key}"), "teacher"),
            &iiss_scope,
            &key
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::FORBIDDEN
    );
    graph
        .run(query("MATCH(n {test_key:$key}) DETACH DELETE n").param("key", key))
        .await
        .unwrap();
}
