use super::*;
use crate::school_articles::cover::Store;
use crate::school_portal::repository::{article_cover, article_view, parent_snapshot};
#[tokio::test]
#[ignore = "requires disposable loopback SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223"]
async fn lifecycle_audience_and_one_view_per_family() {
    assert_eq!(
        std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap(),
        "127.0.0.1:3223"
    );
    let graph = Graph::new("127.0.0.1:3223", "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    init(&graph).await.unwrap();
    let key = uuid::Uuid::new_v4().to_string();
    let other = format!("other-{key}");
    graph.run(query("CREATE(:StaffMember {id:$key,membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:[$key],tenantIds:[$key],test_key:$key}) CREATE(:StaffMember {id:$teacher,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$key],tenantIds:[$key],test_key:$key}) CREATE(:Section {section_id:$key,name:'Year 7A',year_group:'Year 7',school_id:$key,tenant_id:$key,academic_year:'2026/2027',status:'active',test_key:$key}) CREATE(:Section {section_id:$other,name:'Year 8A',year_group:'Year 8',school_id:$key,tenant_id:$key,academic_year:'2026/2027',status:'active',test_key:$key}) CREATE(:Section {section_id:$foreign,school_id:'other-school',tenant_id:$key,academic_year:'2026/2027',status:'active',test_key:$key})").param("key",key.clone()).param("teacher",format!("teacher-{key}")).param("other",other.clone()).param("foreign",format!("foreign-{key}"))).await.unwrap();
    for (parent, section) in [
        (format!("parent-{key}"), key.clone()),
        (format!("neighbour-{key}"), other.clone()),
    ] {
        graph.run(query("MATCH(sec:Section {section_id:$section}) CREATE(p:User {id:$parent,email:$email,role:'parent',test_key:$key})-[:HAS_APPLICATION]->(l:Lead {lead_id:$parent,email:$email,status:'verified',test_key:$key})-[:HAS_STUDENT]->(s:Student {studentId:$parent,fullName:'Fixture child',test_key:$key})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$key,tenant_id:$key,test_key:$key})-[:ENROLLED_IN]->(sec)").param("key",key.clone()).param("section",section).param("email",format!("{parent}@example.test")).param("parent",parent)).await.unwrap();
    }
    let parent = format!("parent-{key}");
    let neighbour = format!("neighbour-{key}");
    async fn read(graph: &Graph, sub: &str) -> Vec<Value> {
        parent_snapshot(graph, sub).await.unwrap()["articles"]
            .as_array()
            .unwrap()
            .clone()
    }
    let mut v = Change {
        entry: Article {
            id: key.clone(),
            school_id: key.clone(),
            tenant_id: key.clone(),
            kind: "webinar".into(),
            category: "parenting".into(),
            title: "Calm before the examination".into(),
            body: "How families can help.".into(),
            audience: "sections".into(),
            section_ids: vec![key.clone()],
            date: Some("2026-11-02".into()),
            start: Some("19:00".into()),
            end: Some("20:00".into()),
            join_url: Some("https://meet.example.org/calm".into()),
            cover: None,
        },
        expected_version: 0,
        action: "save_draft".into(),
    };
    // A cover is kept for the school by its administrator only, and an article may name only a
    // cover of its own school.
    let store = Store::Memory(Default::default());
    let photograph = vec![0xff, 0xd8, 0xff, 0xd9];
    assert!(add_cover(&graph, &store, &format!("teacher-{key}"), &key, &key, photograph.clone())
        .await
        .unwrap()
        .is_none());
    let cover = add_cover(&graph, &store, &key, &key, &key, photograph.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        admin_cover(&graph, &store, &key, &key, &key, &cover).await.unwrap(),
        Some(photograph.clone())
    );
    assert!(admin_cover(&graph, &store, &format!("teacher-{key}"), &key, &key, &cover)
        .await
        .unwrap()
        .is_none());
    assert!(admin_cover(&graph, &store, &key, &key, "other-school", &cover)
        .await
        .unwrap()
        .is_none());
    v.entry.cover = Some("never-uploaded".into());
    assert_eq!(change(&graph, &key, &v).await.unwrap(), None);
    v.entry.cover = Some(cover.clone());
    // Only a school administrator of this school writes; a teacher of the same school does not.
    assert_eq!(
        change(&graph, &format!("teacher-{key}"), &v).await.unwrap(),
        None
    );
    assert_eq!(change(&graph, &key, &v).await.unwrap(), Some(1));
    assert_eq!(change(&graph, &key, &v).await.unwrap(), None);
    assert!(read(&graph, &parent).await.is_empty());
    // A class of another school is not an audience.
    v.expected_version = 1;
    v.entry.section_ids = vec![format!("foreign-{key}")];
    assert_eq!(change(&graph, &key, &v).await.unwrap(), None);
    v.entry.section_ids = vec![key.clone()];
    // Publishing takes the draft as it was saved, not a text sent with the request.
    v.action = "publish".into();
    v.entry.title = "Tampered".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), None);
    v.entry.title = "Calm before the examination".into();
    let (a, b) = tokio::join!(change(&graph, &key, &v), change(&graph, &key, &v));
    let outcomes = [a.unwrap(), b.unwrap()];
    assert_eq!(outcomes.iter().filter(|v| **v == Some(2)).count(), 1);
    assert_eq!(outcomes.iter().filter(|v| v.is_none()).count(), 1);

    let seen = read(&graph, &parent).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["content"]["joinUrl"], "https://meet.example.org/calm");
    assert_eq!(seen[0]["content"]["start"], "19:00");
    assert!(seen[0]["content"].get("sectionIds").is_none());
    assert!(seen[0]["content"].get("schoolId").is_none());
    assert_eq!(seen[0]["views"], 0);
    assert_eq!(seen[0]["read"], false);
    // The family that received the article sees its cover; the neighbouring class does not, and
    // an unpublished article shows none to anyone.
    assert_eq!(seen[0]["content"]["cover"], cover.as_str());
    assert_eq!(
        article_cover(&graph, &store, &parent, &key).await.unwrap(),
        Some(photograph.clone())
    );
    assert!(article_cover(&graph, &store, &neighbour, &key)
        .await
        .unwrap()
        .is_none());
    assert!(seen[0]["publishedAt"].as_str().is_some_and(|s| !s.is_empty()));
    // The neighbouring class did not receive it: nothing to read and nothing to count.
    assert!(read(&graph, &neighbour).await.is_empty());
    assert!(!article_view(&graph, &neighbour, &key).await.unwrap());
    assert!(article_view(&graph, &parent, &key).await.unwrap());
    assert!(article_view(&graph, &parent, &key).await.unwrap());
    let seen = read(&graph, &parent).await;
    assert_eq!(seen[0]["views"], 1);
    assert_eq!(seen[0]["read"], true);

    // A published article is not edited in place.
    v.expected_version = 2;
    v.action = "save_draft".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), None);
    v.action = "unpublish".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), Some(3));
    assert!(read(&graph, &parent).await.is_empty());
    assert!(!article_view(&graph, &parent, &key).await.unwrap());
    assert!(article_cover(&graph, &store, &parent, &key)
        .await
        .unwrap()
        .is_none());
    v.expected_version = 3;
    v.action = "save_draft".into();
    v.entry.audience = "school".into();
    v.entry.section_ids.clear();
    v.entry.join_url = None;
    v.entry.cover = None;
    assert_eq!(change(&graph, &key, &v).await.unwrap(), Some(4));
    v.expected_version = 4;
    v.action = "publish".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), Some(5));
    // Published again for the whole school: the first family's view stands, the neighbour now reads it.
    let seen = read(&graph, &parent).await;
    assert_eq!(seen[0]["read"], true);
    assert_eq!(seen[0]["content"]["joinUrl"], Value::Null);
    assert_eq!(seen[0]["content"]["cover"], Value::Null);
    assert!(article_cover(&graph, &store, &parent, &key)
        .await
        .unwrap()
        .is_none());
    let seen = read(&graph, &neighbour).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["read"], false);
    assert_eq!(seen[0]["views"], 1);
    assert!(article_view(&graph, &neighbour, &key).await.unwrap());

    // In-app webinars reach the staff who may host them: here the webinar has no outside address
    // and its day is far ahead, so the school's teacher of a class sees it; a teacher of no class
    // of this school sees none.
    let teacher = |id: String| crate::school_portal::auth::Teacher {
        active: true,
        staff_member_id: id,
        roles: vec!["teacher".into()],
        school_ids: vec![key.clone()],
        tenant_ids: vec![key.clone()],
    };
    graph.run(query("MATCH(sec:Section {section_id:$key}),(t:StaffMember {id:$teacher}) SET sec.homeroom_staff_member_id=t.id").param("key",key.clone()).param("teacher",format!("teacher-{key}"))).await.unwrap();
    let hosted = crate::school_portal::repository::context(&graph, &teacher(format!("teacher-{key}")))
        .await
        .unwrap();
    assert_eq!(hosted["webinars"].as_array().unwrap().len(), 1);
    assert_eq!(hosted["webinars"][0]["id"], key.as_str());
    assert_eq!(hosted["webinars"][0]["start"], "19:00");
    let stranger = crate::school_portal::repository::context(&graph, &teacher(format!("stranger-{key}")))
        .await
        .unwrap();
    assert!(stranger["webinars"].as_array().unwrap().is_empty());

    let scope = Scope {
        tenant_id: key.clone(),
        school_id: key.clone(),
        after: None,
        id: None,
    };
    let page = list(&graph, &key, &scope).await.unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["status"], "published");
    assert_eq!(page["items"][0]["version"], 5);
    assert_eq!(page["items"][0]["views"], 2);
    assert_eq!(page["items"][0]["entry"]["audience"], "school");
    assert!(page["items"][0]["publishedAt"].is_string());
    assert!(list(&graph, &format!("teacher-{key}"), &scope).await.unwrap()["items"]
        .as_array()
        .unwrap()
        .is_empty());

    v.expected_version = 5;
    v.action = "archive".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), Some(6));
    assert!(read(&graph, &parent).await.is_empty());
    assert_eq!(
        list(&graph, &key, &scope).await.unwrap()["items"][0]["status"],
        "archived"
    );
    v.expected_version = 6;
    v.action = "save_draft".into();
    assert_eq!(change(&graph, &key, &v).await.unwrap(), None);
    let mut audit=graph.execute(query("MATCH(a:SchoolArticleAudit {school_id:$key}) RETURN count(a) AS writes,collect(DISTINCT a.actor_role) AS roles").param("key",key.clone())).await.unwrap();
    let row = audit.next().await.unwrap().unwrap();
    assert_eq!(row.get::<i64>("writes").unwrap(), 6);
    assert_eq!(row.get::<Vec<String>>("roles").unwrap(), ["school_admin"]);
    drop(audit);
    graph.run(query("MATCH(n) WHERE n.test_key=$key OR (n:SchoolArticle AND n.school_id=$key) OR (n:SchoolArticleAudit AND n.school_id=$key) OR (n:SchoolArticleLock AND n.key STARTS WITH $prefix) OR (n:SchoolArticleView AND n.article_key STARTS WITH $prefix) OR (n:SchoolArticleCover AND n.school_id=$key) DETACH DELETE n").param("key",key.clone()).param("prefix",format!("{key}|{key}"))).await.unwrap();
}
