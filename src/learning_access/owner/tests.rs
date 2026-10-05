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
