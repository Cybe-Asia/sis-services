//! Explicit synthetic browser fixture on the dedicated graph; never a school-data seed.
use super::{auth::Actor, model::*, repository::*};
use neo4rs::{query, Graph};
#[tokio::test]
#[ignore = "manual EXTRACURRICULAR_BROWSER_FIXTURE=dedicated-3223 only"]
async fn seed_owned_browser_fixture() {
    assert_eq!(
        std::env::var("EXTRACURRICULAR_BROWSER_FIXTURE").unwrap(),
        "dedicated-3223"
    );
    let g = Graph::new(
        "127.0.0.1:3223",
        &std::env::var("NEO4J_USER").unwrap(),
        &std::env::var("NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    init(&g).await.unwrap();
    let mark = std::env::var("EXTRA_BROWSER_MARK").unwrap();
    assert!(mark.starts_with("extra-browser-"));
    let parent = std::env::var("EXTRA_PARENT_ID").unwrap();
    let learner = std::env::var("EXTRA_STUDENT_USER_ID").unwrap();
    let coach = std::env::var("EXTRA_COACH_ID").unwrap();
    let child = format!("{mark}-child");
    let section = format!("{mark}-class");
    let admin = format!("{mark}-admin");
    let mut prior = g
        .execute(
            query("MATCH(b:LearningStudentBinding {user_id:$user}) RETURN count(b) AS count")
                .param("user", learner.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        prior
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>("count")
            .unwrap(),
        0,
        "preserve existing student binding"
    );
    g.run(query("MERGE(p:User {id:$parent}) ON CREATE SET p.role='parent',p.email=$email,p.test_key=$mark WITH p WHERE p.role='parent' CREATE(p)-[:HAS_APPLICATION]->(:Lead {lead_id:$mark,email:p.email,status:'verified',test_key:$mark})-[:HAS_STUDENT]->(s:Student {studentId:$child,fullName:'Synthetic extracurricular acceptance',test_key:$mark})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'SCH-IIHS',tenant_id:'TENANT-TEST',test_key:$mark})-[:ENROLLED_IN]->(:Section {section_id:$section,name:'Synthetic activity class',status:'active',academic_year:'2026/2027',school_id:'SCH-IIHS',tenant_id:'TENANT-TEST',test_key:$mark}) MERGE(u:User {id:$user}) ON CREATE SET u.role='student',u.test_key=$mark WITH p,u,s WHERE u.role='student' CREATE(:LearningStudentBinding {user_id:$user,student_id:$child,status:'ACTIVE',school_id:'SCH-IIHS',tenant_id:'TENANT-TEST',test_key:$mark}) MERGE(t:StaffMember {id:$coach}) ON CREATE SET t.membershipStatus='ACTIVE',t.roles=['teacher'],t.schoolIds=['SCH-IIHS'],t.tenantIds=['TENANT-TEST'],t.fullName='Synthetic coach (Auth verified)',t.test_key=$mark CREATE(:StaffMember {id:$admin,membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['SCH-IIHS'],tenantIds:['TENANT-TEST'],test_key:$mark})").param("parent",parent).param("email",format!("{mark}@example.test")).param("user",learner).param("coach",coach.clone()).param("admin",admin.clone()).param("child",child).param("section",section.clone()).param("mark",mark.clone())).await.unwrap();
    let text = |s: &str| Text {
        en: s.into(),
        id: s.into(),
    };
    let a = Activity {
        presentation: None,
        id: mark.clone(),
        title: Text {
            en: "Robotics · synthetic acceptance".into(),
            id: "Robotika · acceptance sintetis".into(),
        },
        summary: Text {
            en: "Local service fixture, not an official school activity.".into(),
            id: "Fixture layanan lokal, bukan kegiatan resmi sekolah.".into(),
        },
        category: "stem".into(),
        image: Some("/assets/after-school-activity/robotics-ai-builders.jpg".into()),
        coach_id: coach,
        section_ids: vec![section],
        capacity: 2,
        parent_consent: true,
        published: true,
        meetings: vec![Meeting {
            id: format!("{mark}-meeting"),
            date: "2026-10-08".into(),
            start: "15:00".into(),
            end: "16:00".into(),
            title: Text {
                en: "Sensor workshop".into(),
                id: "Workshop sensor".into(),
            },
            location: text("Lab"),
        }],
        criteria: vec![Criterion {
            id: format!("{mark}-criterion"),
            title: Text {
                en: "Collaboration".into(),
                id: "Kolaborasi".into(),
            },
            levels: vec![
                Text {
                    en: "Developing".into(),
                    id: "Berkembang".into(),
                },
                Text {
                    en: "Secure".into(),
                    id: "Mantap".into(),
                },
            ],
        }],
    };
    let v = Change {
        scope: Scope {
            school_id: "SCH-IIHS".into(),
            tenant_id: "TENANT-TEST".into(),
            academic_year: "2026/2027".into(),
            student_id: None,
            after: None,
        },
        activity_id: mark,
        revision: 0,
        request_id: "fixture-catalog".into(),
        command: Command::Save { activity: a },
    };
    assert!(change(
        &g,
        &Actor {
            id: admin,
            role: "admin".into()
        },
        &v
    )
    .await
    .unwrap()
    .is_some());
}
#[tokio::test]
#[ignore = "manual dedicated fixture asset correction only"]
async fn correct_owned_browser_asset() {
    assert_eq!(
        std::env::var("EXTRACURRICULAR_BROWSER_FIXTURE").unwrap(),
        "dedicated-3223"
    );
    let mark = std::env::var("EXTRA_BROWSER_MARK").unwrap();
    assert!(mark.starts_with("extra-browser-"));
    let g = Graph::new(
        "127.0.0.1:3223",
        &std::env::var("NEO4J_USER").unwrap(),
        &std::env::var("NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    let scope = Scope {
        school_id: "SCH-IIHS".into(),
        tenant_id: "TENANT-TEST".into(),
        academic_year: "2026/2027".into(),
        student_id: None,
        after: None,
    };
    let key = activity_key(&scope, &mark);
    let mut rows=g.execute(query("MATCH(a:ExtraCurricularActivity {key:$key,id:$id}) RETURN a.payload AS payload,a.revision AS revision").param("key",key).param("id",mark.clone())).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let mut a: Activity = serde_json::from_str(&row.get::<String>("payload").unwrap()).unwrap();
    a.image = Some("/assets/after-school-activity/robotics-ai-builders.jpg".into());
    let v = Change {
        scope,
        activity_id: mark.clone(),
        revision: row.get::<i64>("revision").unwrap() as u32,
        request_id: "fixture-correct-asset".into(),
        command: Command::Save { activity: a },
    };
    assert!(change(
        &g,
        &Actor {
            id: format!("{mark}-admin"),
            role: "admin".into()
        },
        &v
    )
    .await
    .unwrap()
    .is_some());
}
#[tokio::test]
#[ignore = "manual owned synthetic fixture identity alignment only"]
async fn align_owned_browser_child() {
    assert_eq!(
        std::env::var("EXTRACURRICULAR_BROWSER_FIXTURE").unwrap(),
        "dedicated-3223"
    );
    let mark = std::env::var("EXTRA_BROWSER_MARK").unwrap();
    assert!(mark.starts_with("extra-browser-"));
    let child = std::env::var("EXTRA_BROWSER_CHILD_ID").unwrap();
    assert!(crate::school_portal::model::identifier(&child));
    let g = Graph::new(
        "127.0.0.1:3223",
        &std::env::var("NEO4J_USER").unwrap(),
        &std::env::var("NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    let mut r = g
        .execute(
            query("MATCH(s:Student {studentId:$child}) RETURN count(s) AS count")
                .param("child", child.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        r.next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>("count")
            .unwrap(),
        0,
        "preserve existing student record"
    );
    g.run(query("MATCH(s:Student {test_key:$mark}), (b:LearningStudentBinding {test_key:$mark}) SET s.studentId=$child,b.student_id=$child").param("mark",mark).param("child",child)).await.unwrap();
}

#[tokio::test]
#[ignore = "manual cleanup of this task's dedicated graph fixture only"]
async fn clean_owned_browser_fixture() {
    assert_eq!(
        std::env::var("EXTRACURRICULAR_BROWSER_FIXTURE").unwrap(),
        "dedicated-3223"
    );
    let mark = std::env::var("EXTRA_BROWSER_MARK").unwrap();
    assert!(mark.starts_with("extra-browser-") && crate::school_portal::model::identifier(&mark));
    let g = Graph::new(
        "127.0.0.1:3223",
        &std::env::var("NEO4J_USER").unwrap(),
        &std::env::var("NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    let prefix = format!("TENANT-TEST|SCH-IIHS|2026/2027|{mark}");
    g.run(query("MATCH(n) WHERE n.test_key=$mark OR n.key=$prefix OR n.key STARTS WITH $children OR n.activity_key=$prefix DETACH DELETE n").param("mark",mark.clone()).param("prefix",prefix.clone()).param("children",format!("{prefix}|"))).await.unwrap();
    g.run(
        query("MATCH(n:ExtraCurricularOperation) WHERE n.request CONTAINS $mark DETACH DELETE n")
            .param("mark", mark.clone()),
    )
    .await
    .unwrap();
    let mut r=g.execute(query("MATCH(n) WHERE n.test_key=$mark OR n.activity_key=$prefix OR n.key=$prefix OR n.key STARTS WITH $children RETURN count(n) AS count").param("mark",mark).param("prefix",prefix.clone()).param("children",format!("{prefix}|"))).await.unwrap();
    assert_eq!(
        r.next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>("count")
            .unwrap(),
        0
    );
}

#[tokio::test]
#[ignore = "manual exact activity cleanup after primary browser acceptance only"]
async fn clean_owned_primary_acceptance() {
    assert_eq!(
        std::env::var("EXTRACURRICULAR_PRIMARY_CLEANUP").unwrap(),
        "owned-3213"
    );
    let marker = std::env::var("EXTRA_PRIMARY_MARK").unwrap();
    let id = std::env::var("EXTRA_PRIMARY_ACTIVITY_ID").unwrap();
    assert!(
        marker.starts_with("extra-primary-20261002-")
            && crate::school_portal::model::identifier(&marker)
    );
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    let graph = Graph::new(
        "127.0.0.1:3213",
        &std::env::var("NEO4J_USER").unwrap(),
        &std::env::var("NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    let key = format!("TENANT-TEST|SCH-IIHS|2026/2027|{id}");
    let mut tx = graph.start_txn().await.unwrap();
    let mut activity = tx
        .execute(
            query("MATCH(a:ExtraCurricularActivity {key:$key,id:$id}) RETURN a.payload AS payload")
                .param("key", key.clone())
                .param("id", id.clone()),
        )
        .await
        .unwrap();
    let row = activity
        .next(tx.handle())
        .await
        .unwrap()
        .expect("exact browser-created activity required");
    assert!(activity.next(tx.handle()).await.unwrap().is_none());
    let payload: Activity = serde_json::from_str(&row.get::<String>("payload").unwrap()).unwrap();
    assert_eq!(payload.id, id);
    assert_eq!(payload.title.en, format!("Robotics · {marker}"));
    assert!(!payload.parent_consent);
    let predicate = "(n:ExtraCurricularActivity OR n:ExtraCurricularLock OR n:ExtraCurricularEnrollment OR n:ExtraCurricularOperation OR n:ExtraCurricularAttendance OR n:ExtraCurricularOutcome OR n:ExtraCurricularAudit OR n:ExtraCurricularMeetingRecord) AND (n.key=$key OR n.key STARTS WITH $children OR n.activity_key=$key OR ((n:ExtraCurricularOperation OR n:ExtraCurricularAudit) AND n.key CONTAINS $operation))";
    let scoped = |cypher: String| {
        query(&cypher)
            .param("key", key.clone())
            .param("children", format!("{key}|"))
            .param("operation", format!("|{key}|"))
    };
    let mut count = tx
        .execute(scoped(format!(
            "MATCH(n) WHERE {predicate} RETURN count(n) AS count"
        )))
        .await
        .unwrap();
    let before = count
        .next(tx.handle())
        .await
        .unwrap()
        .unwrap()
        .get::<i64>("count")
        .unwrap();
    assert!(
        (1..=80).contains(&before),
        "bounded exact owner records only"
    );
    tx.run(scoped(format!(
        "MATCH(n) WHERE {predicate} DETACH DELETE n"
    )))
    .await
    .unwrap();
    let mut count = tx
        .execute(scoped(format!(
            "MATCH(n) WHERE {predicate} RETURN count(n) AS count"
        )))
        .await
        .unwrap();
    assert_eq!(
        count
            .next(tx.handle())
            .await
            .unwrap()
            .unwrap()
            .get::<i64>("count")
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
    println!("Removed {before} exact activity-owned acceptance records; zero remaining.");
}
