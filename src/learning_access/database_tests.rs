use super::*;
#[tokio::test]
#[ignore = "requires disposable loopback LEARNING_TEST_NEO4J_* configuration"]
async fn grants_binding_cas_revocation_and_atomic_audit() {
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
    ensure_schema(&graph).await.unwrap();
    let key = uuid::Uuid::new_v4().to_string();
    let actor = format!("admin-{key}");
    let teacher = format!("teacher-{key}");
    let section = format!("class-{key}");
    let user = format!("user-{key}");
    let pupil = format!("pupil-{key}");
    graph.run(query("CREATE (:StaffMember {id:$actor,membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['school-test'],tenantIds:['tenant-test']}) CREATE (:StaffMember {id:$teacher,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school-test'],tenantIds:['tenant-test']}) CREATE (sec:Section {section_id:$section,name:'Class Test',status:'active',school_id:'school-test',tenant_id:'tenant-test'}) CREATE (:User {id:$user,role:'student'}) CREATE (:Student {studentId:$student,fullName:'Student Test'})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'school-test',tenant_id:'tenant-test'})-[:ENROLLED_IN]->(sec)").param("actor",actor.clone()).param("teacher",teacher.clone()).param("section",section.clone()).param("user",user.clone()).param("student",pupil.clone())).await.unwrap();
    async fn change(
        graph: &neo4rs::Graph,
        actor: &str,
        section: &str,
        target: &str,
        pupil: &str,
        version: i64,
        active: bool,
        student: bool,
        school: &str,
    ) -> Result<Option<i64>, neo4rs::Error> {
        let cypher = format!(
            "{SCOPE} {}",
            if student {
                STUDENT_QUERY
            } else {
                TEACHER_QUERY
            }
        );
        let mut rows = graph
            .execute(
                query(&cypher)
                    .param("actor", actor)
                    .param("section", section)
                    .param("target", target)
                    .param("student", pupil)
                    .param("school", school)
                    .param("tenant", "tenant-test")
                    .param("key", format!("{target}|{section}"))
                    .param("version", version)
                    .param("active", active)
                    .param("status", if active { "ACTIVE" } else { "REVOKED" })
                    .param("audit", uuid::Uuid::new_v4().to_string()),
            )
            .await?;
        let version = rows.next().await?.map(|r| r.get::<i64>("version").unwrap());
        while rows.next().await?.is_some() {}
        Ok(version)
    }
    assert_eq!(
        change(
            &graph,
            &actor,
            &section,
            &teacher,
            "",
            0,
            true,
            false,
            "school-other"
        )
        .await
        .unwrap(),
        None
    );
    // Revocation cannot reserve an identity, even for an administrator's own section.
    assert_eq!(
        change(
            &graph,
            &actor,
            &section,
            &user,
            &pupil,
            0,
            false,
            true,
            "school-test"
        )
        .await
        .unwrap(),
        None
    );
    graph
        .run(
            query("CREATE (:Student {studentId:$foreign})")
                .param("foreign", format!("foreign-{key}")),
        )
        .await
        .unwrap();
    assert_eq!(
        change(
            &graph,
            &actor,
            &section,
            &user,
            &format!("foreign-{key}"),
            0,
            false,
            true,
            "school-test"
        )
        .await
        .unwrap(),
        None
    );
    let (a, c) = tokio::join!(
        change(
            &graph,
            &actor,
            &section,
            &teacher,
            "",
            0,
            true,
            false,
            "school-test"
        ),
        change(
            &graph,
            &actor,
            &section,
            &teacher,
            "",
            0,
            true,
            false,
            "school-test"
        )
    );
    assert_eq!(
        [a.unwrap(), c.unwrap()]
            .iter()
            .filter(|r| **r == Some(1))
            .count(),
        1
    );
    assert_eq!(
        change(
            &graph,
            &actor,
            &section,
            &teacher,
            "",
            0,
            true,
            false,
            "school-test"
        )
        .await
        .unwrap(),
        None
    );
    assert_eq!(
        change(
            &graph,
            &actor,
            &section,
            &teacher,
            "",
            1,
            false,
            false,
            "school-test"
        )
        .await
        .unwrap(),
        Some(2)
    );
    assert_eq!(
        change(
            &graph,
            &actor,
            &section,
            &user,
            &pupil,
            0,
            true,
            true,
            "school-test"
        )
        .await
        .unwrap(),
        Some(1)
    );
    assert_eq!(
        change(
            &graph,
            &actor,
            &section,
            &user,
            "another-student",
            1,
            true,
            true,
            "school-test"
        )
        .await
        .unwrap(),
        None
    );
    assert_eq!(
        change(
            &graph,
            &actor,
            &section,
            &user,
            &pupil,
            1,
            false,
            true,
            "school-test"
        )
        .await
        .unwrap(),
        Some(2)
    );
    async fn records(
        graph: &neo4rs::Graph,
        actor: &str,
        section: &str,
        school: &str,
        after: &str,
        student: bool,
    ) -> Vec<(String, i64, String)> {
        let mut stream = graph
            .execute(
                query(&read_query(student))
                    .param("actor", actor)
                    .param("section", section)
                    .param("school", school)
                    .param("tenant", "tenant-test")
                    .param("after", after),
            )
            .await
            .unwrap();
        let mut rows = Vec::new();
        while let Some(row) = stream.next().await.unwrap() {
            rows.push((
                row.get("target").unwrap(),
                row.get("version").unwrap(),
                row.get("status").unwrap(),
            ));
        }
        rows
    }
    // The exact GET projection shares the canonical scope and cursor checks.
    assert_eq!(
        records(&graph, &actor, &section, "school-test", "", false).await,
        vec![(teacher.clone(), 2, "REVOKED".into())]
    );
    assert_eq!(
        records(&graph, &actor, &section, "school-test", "", true).await,
        vec![(user.clone(), 2, "REVOKED".into())]
    );
    assert!(records(&graph, &actor, &section, "school-other", "", false)
        .await
        .is_empty());
    assert!(
        records(&graph, &actor, "another-section", "school-test", "", true)
            .await
            .is_empty()
    );
    assert!(
        records(&graph, &actor, &section, "school-test", &teacher, false)
            .await
            .is_empty()
    );
    async fn context_count(graph: &neo4rs::Graph, actor: &str, school: &str) -> usize {
        let mut rows = graph
            .execute(
                query(context::CONTEXT_QUERY)
                    .param("actor", actor)
                    .param("schools", vec![school])
                    .param("tenants", vec!["tenant-test"]),
            )
            .await
            .unwrap();
        let mut count = 0;
        while let Some(row) = rows.next().await.unwrap() {
            assert_eq!(row.get::<String>("name").unwrap(), "Class Test");
            count += 1;
        }
        count
    }
    assert_eq!(context_count(&graph, &actor, "school-test").await, 1);
    assert_eq!(context_count(&graph, &actor, "school-other").await, 0);
    let mut held = graph.start_txn().await.unwrap();
    held.run(query("MATCH (s:Section {section_id:$section}) SET s.learning_access_lock=coalesce(s.learning_access_lock,0)+1").param("section",section.clone())).await.unwrap();
    let g = graph.clone();
    let a = actor.clone();
    let sec = section.clone();
    let t = teacher.clone();
    let waiting = tokio::spawn(async move {
        change(&g, &a, &sec, &t, "", 2, true, false, "school-test")
            .await
            .unwrap()
    });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(!waiting.is_finished());
    graph
        .run(
            query("MATCH (a:StaffMember {id:$actor}) SET a.membershipStatus='REVOKED'")
                .param("actor", actor.clone()),
        )
        .await
        .unwrap();
    held.commit().await.unwrap();
    assert_eq!(waiting.await.unwrap(), None);
    assert_eq!(context_count(&graph, &actor, "school-test").await, 0);
    assert!(records(&graph, &actor, &section, "school-test", "", false)
        .await
        .is_empty());
    let mut rows = graph
        .execute(
            query("MATCH (a:LearningAccessAudit {actor_id:$actor}) RETURN count(a) AS n")
                .param("actor", actor.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>("n").unwrap(),
        4
    );
}
