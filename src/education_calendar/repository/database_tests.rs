use super::*;
#[tokio::test]
#[ignore = "requires disposable loopback SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223"]
async fn cas_lifecycle_audience_and_current_membership() {
    assert_eq!(
        std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap(),
        "127.0.0.1:3223"
    );
    let graph = Graph::new("127.0.0.1:3223", "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    init(&graph).await.unwrap();
    let key = uuid::Uuid::new_v4().to_string();
    graph.run(query("CREATE(:StaffMember {id:$key,membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:[$key],tenantIds:[$key],test_key:$key}) CREATE(:Section {section_id:$key,school_id:$key,tenant_id:$key,academic_year:'2026/2027',status:'active',test_key:$key}) CREATE(:Section {section_id:$other,school_id:$key,tenant_id:$key,academic_year:'2026/2027',status:'active',test_key:$key}) CREATE(:Section {section_id:$foreign,school_id:'other-school',tenant_id:$key,academic_year:'2026/2027',status:'active',test_key:$key})").param("key",key.clone()).param("other",format!("other-{key}")).param("foreign",format!("foreign-{key}"))).await.unwrap();
    let mut v = Change {
        entry: Entry {
            id: key.clone(),
            school_id: key.clone(),
            tenant_id: key.clone(),
            academic_year: "2026/2027".into(),
            kind: "holiday".into(),
            title: "Isolated holiday".into(),
            description: "".into(),
            start_date: "2026-12-24".into(),
            end_date: "2027-01-03".into(),
            audience: "sections".into(),
            section_ids: vec![key.clone()],
        },
        expected_version: 0,
        action: "save_draft".into(),
    };
    assert_eq!(change(&graph, &key, &v).await.unwrap(), Some(1));
    assert_eq!(change(&graph, &key, &v).await.unwrap(), None);
    v.expected_version = 1;
    v.entry.title = "Revised draft".into();
    let (a, b) = tokio::join!(change(&graph, &key, &v), change(&graph, &key, &v));
    let outcomes = [a.unwrap(), b.unwrap()];
    assert_eq!(outcomes.iter().filter(|v| **v == Some(2)).count(), 1);
    async fn read(graph: &Graph, id: &str) -> Value {
        published(graph,query(&format!("MATCH(sec:Section {{section_id:$id,status:'active'}}) {} RETURN r.payload AS payload,r.version AS version ORDER BY r.id LIMIT 101",PUBLISHED)).param("id",id)).await.unwrap()
    }
    assert!(read(&graph, &key).await["items"]
        .as_array()
        .unwrap()
        .is_empty());
    v.expected_version = 2;
    v.action = "publish".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), Some(3));
    graph.run(query("MATCH(sec:Section {section_id:$key}) CREATE(p:User {id:$parent,email:$email,role:'parent',test_key:$key})-[:HAS_APPLICATION]->(l:Lead {lead_id:$parent,email:$email,status:'verified',test_key:$key})-[:HAS_STUDENT]->(s:Student {studentId:$parent,fullName:'Fixture child',test_key:$key})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$key,tenant_id:$key,test_key:$key})-[:ENROLLED_IN]->(sec) CREATE(t:StaffMember {id:$teacher,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$key],tenantIds:[$key],test_key:$key}) SET sec.homeroom_staff_member_id=t.id").param("key",key.clone()).param("parent",format!("parent-{key}")).param("teacher",format!("teacher-{key}")).param("email",format!("parent-{key}@example.test"))).await.unwrap();
    let snapshot =
        crate::school_portal::repository::parent_snapshot(&graph, &format!("parent-{key}"))
            .await
            .unwrap();
    assert_eq!(
        snapshot["educationCalendar"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let member = crate::school_portal::auth::Teacher {
        active: true,
        staff_member_id: format!("teacher-{key}"),
        roles: vec!["teacher".into()],
        school_ids: vec![key.clone()],
        tenant_ids: vec![key.clone()],
    };
    let classroom =
        crate::school_portal::repository::classroom(&graph, &member, &key, "2026-12-25")
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        classroom["educationCalendar"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        read(&graph, &key).await["items"][0]["entry"]["endDate"],
        "2027-01-03"
    );
    assert!(read(&graph, &format!("other-{key}")).await["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(read(&graph, &format!("foreign-{key}")).await["items"]
        .as_array()
        .unwrap()
        .is_empty());
    v.expected_version = 3;
    v.action = "save_draft".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), None);
    v.action = "archive".into();
    v.entry.description = "tampered".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), None);
    v.entry.description = "".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), Some(4));
    assert!(read(&graph, &key).await["items"]
        .as_array()
        .unwrap()
        .is_empty());
    v.entry.id = format!("second-{key}");
    v.expected_version = 0;
    v.action = "save_draft".into();
    v.entry.audience = "school".into();
    v.entry.section_ids.clear();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), Some(1));
    v.expected_version = 1;
    v.action = "publish".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), Some(2));
    assert_eq!(
        read(&graph, &format!("other-{key}")).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    graph
        .run(
            query("MATCH(sec:Section {section_id:$other}) SET sec.academic_year='2027/2028'")
                .param("other", format!("other-{key}")),
        )
        .await
        .unwrap();
    assert!(read(&graph, &format!("other-{key}")).await["items"]
        .as_array()
        .unwrap()
        .is_empty());
    graph
        .run(
            query("MATCH(a:StaffMember {id:$key}) SET a.membershipStatus='REVOKED'")
                .param("key", key.clone()),
        )
        .await
        .unwrap();
    v.expected_version = 2;
    v.action = "archive".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), None);
    let mut audit = graph
        .execute(
            query("MATCH(a:EducationCalendarAudit {school_id:$key}) RETURN count(a) AS n")
                .param("key", key.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        audit
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>("n")
            .unwrap(),
        6
    );
    // Only records owned by this unique fixture are removed.
    graph.run(query("MATCH(n) WHERE n.test_key=$key OR (n:EducationCalendarEntry AND n.school_id=$key) OR (n:EducationCalendarAudit AND n.school_id=$key) OR (n:EducationCalendarLock AND n.key STARTS WITH $prefix) DETACH DELETE n").param("key",key.clone()).param("prefix",format!("{key}|{key}"))).await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable loopback SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223"]
async fn cross_year_creation_archive_and_bounded_range_pages() {
    assert_eq!(
        std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap(),
        "127.0.0.1:3223"
    );
    let graph = Graph::new("127.0.0.1:3223", "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    init(&graph).await.unwrap();
    let key = uuid::Uuid::new_v4().to_string();
    graph.run(query("CREATE(:StaffMember {id:$key,membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:[$key],tenantIds:[$key],test_key:$key}) CREATE(:Section {section_id:$key,name:'Current class',school_id:$key,tenant_id:$key,academic_year:'2026/2027',status:'active',test_key:$key}) CREATE(:Section {section_id:$other,name:'Future class',school_id:$key,tenant_id:$key,academic_year:'2027/2028',status:'active',test_key:$key})").param("key",key.clone()).param("other",format!("other-{key}"))).await.unwrap();
    let entry = Entry {
        id: key.clone(),
        school_id: key.clone(),
        tenant_id: key.clone(),
        academic_year: "2026/2027".into(),
        kind: "holiday".into(),
        title: "Holiday".into(),
        description: "".into(),
        start_date: "2026-12-24".into(),
        end_date: "2027-01-03".into(),
        audience: "sections".into(),
        section_ids: vec![key.clone()],
    };
    let first = Change {
        entry: entry.clone(),
        action: "save_draft".into(),
        expected_version: 0,
    };
    let mut future = entry.clone();
    future.academic_year = "2027/2028".into();
    future.section_ids = vec![format!("other-{key}")];
    let second = Change {
        entry: future,
        action: "save_draft".into(),
        expected_version: 0,
    };
    let (a, b) = tokio::join!(change(&graph, &key, &first), change(&graph, &key, &second));
    assert_eq!(
        [a.unwrap(), b.unwrap()]
            .iter()
            .filter(|v| v.is_some())
            .count(),
        1
    );
    let mut rows=graph.execute(query("MATCH(r:EducationCalendarEntry {school_id:$key}) RETURN r.payload AS payload,r.academic_year AS year,r.version AS version").param("key",key.clone())).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let canonical: Entry = serde_json::from_str(&row.get::<String>("payload").unwrap()).unwrap();
    assert_eq!(canonical.academic_year, row.get::<String>("year").unwrap());
    assert_eq!(row.get::<i64>("version").unwrap(), 1);
    assert_eq!(
        change(
            &graph,
            &key,
            &Change {
                entry: canonical.clone(),
                action: "publish".into(),
                expected_version: 1
            }
        )
        .await
        .unwrap(),
        Some(2)
    );
    graph
        .run(
            query("MATCH(c:Section {school_id:$key}) SET c.status='archived'")
                .param("key", key.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        context(&graph, &key, &[key.clone()], &[key.clone()], "")
            .await
            .unwrap()["classes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        change(
            &graph,
            &key,
            &Change {
                entry: canonical,
                action: "archive".into(),
                expected_version: 2
            }
        )
        .await
        .unwrap(),
        Some(3)
    );
    graph
        .run(
            query("MATCH(c:Section {section_id:$key}) SET c.status='active'")
                .param("key", key.clone()),
        )
        .await
        .unwrap();
    graph.run(query("UNWIND range(0,204) AS i CREATE(:Section {section_id:$key+'-'+toString(i),name:'Historical class',school_id:$key,tenant_id:$key,academic_year:'2020/2021',status:'archived',test_key:$key})").param("key",key.clone())).await.unwrap();
    let first_context = context(&graph, &key, &[key.clone()], &[key.clone()], "")
        .await
        .unwrap();
    assert_eq!(first_context["classes"].as_array().unwrap().len(), 200);
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let after = String::from_utf8(
        URL_SAFE_NO_PAD
            .decode(first_context["next"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    let second_context = context(&graph, &key, &[key.clone()], &[key.clone()], &after)
        .await
        .unwrap();
    assert_eq!(second_context["classes"].as_array().unwrap().len(), 7);
    assert!(second_context["next"].is_null());
    // Large, valid publications exceed row-only limits, but bounded pages remain reachable.
    let ids = (0..200)
        .map(|i| format!("{i:03}-{}", "s".repeat(124)))
        .collect::<Vec<_>>();
    for i in 0..110 {
        let mut e = entry.clone();
        e.id = format!("date-{i:03}");
        e.section_ids = ids.clone();
        e.section_ids[0] = key.clone();
        e.start_date = if i == 109 { "2027-02-10" } else { "2026-10-01" }.into();
        e.end_date = e.start_date.clone();
        graph.run(query("CREATE(:EducationCalendarEntry {key:$record,id:$id,school_id:$key,tenant_id:$key,academic_year:'2026/2027',audience:'sections',section_ids:$sections,payload:$payload,status:'published',version:1,start_date:$start,end_date:$start})").param("record",format!("{key}|{key}|{}",e.id)).param("id",e.id.clone()).param("key",key.clone()).param("sections",e.section_ids.clone()).param("payload",serde_json::to_string(&e).unwrap()).param("start",e.start_date)).await.unwrap();
    }
    let q = Scope {
        tenant_id: key.clone(),
        school_id: key.clone(),
        academic_year: "2026/2027".into(),
        after: None,
        id: None,
    };
    let page = list(&graph, &key, &q).await.unwrap();
    assert!(serde_json::to_vec(&page).unwrap().len() < 1_500_000);
    assert!(page["next"].is_string());
    let mut w = crate::education_calendar::window::Window {
        from: "2026-10-01".into(),
        to: "2026-10-31".into(),
        after: None,
        student_id: None,
    };
    let mut count = 0;
    let mut seen = std::collections::HashSet::new();
    loop {
        let calendar_query = query(&format!(
            "MATCH(sec:Section {{section_id:$key,status:'active'}}) {PUBLISHED} {}",
            w.clause()
        ))
        .param("key", key.clone())
        .param("from", w.from.clone())
        .param("to", w.to.clone())
        .param("after", w.cursor().unwrap());
        let page = crate::education_calendar::window::page(&graph, calendar_query)
            .await
            .unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() < 400_000);
        for item in page["items"].as_array().unwrap() {
            assert!(seen.insert(item["entry"]["id"].as_str().unwrap().to_string()));
        }
        count += page["items"].as_array().unwrap().len();
        w.after = page["next"].as_str().map(str::to_string);
        if w.after.is_none() {
            break;
        }
    }
    assert_eq!(count, 109);
    graph.run(query("MATCH(r:EducationCalendarEntry {school_id:$key,status:'published'}) SET r.audience='school',r.section_ids=[]").param("key",key.clone())).await.unwrap();
    w.from = "2027-02-01".into();
    w.to = "2027-02-28".into();
    let calendar_query = query(&format!(
        "MATCH(sec:Section {{section_id:$key,status:'active'}}) {PUBLISHED} {}",
        w.clause()
    ))
    .param("key", key.clone())
    .param("from", w.from.clone())
    .param("to", w.to.clone())
    .param("after", "");
    let page = crate::education_calendar::window::page(&graph, calendar_query)
        .await
        .unwrap();
    assert_eq!(page["items"][0]["entry"]["id"], "date-109");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let outside = query(&format!(
        "MATCH(sec:Section {{section_id:$key,status:'active'}}) {PUBLISHED} {}",
        w.clause()
    ))
    .param("key", key.clone())
    .param("from", "2026-10-01")
    .param("to", "2026-10-31")
    .param("after", format!("{key}|{key}|date-099"));
    let remaining = crate::education_calendar::window::page(&graph, outside)
        .await
        .unwrap();
    assert_eq!(remaining["items"].as_array().unwrap().len(), 9);
    graph.run(query("MATCH(n) WHERE n.test_key=$key OR (n:EducationCalendarEntry AND n.school_id=$key) OR (n:EducationCalendarAudit AND n.school_id=$key) OR (n:EducationCalendarLock AND n.key STARTS WITH $prefix) DETACH DELETE n").param("key",key.clone()).param("prefix",format!("{key}|{key}"))).await.unwrap();
}
