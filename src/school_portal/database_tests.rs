use super::{auth::Teacher, model::*, repository as repo};
use neo4rs::{query, Graph};
#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn teacher_parent_school_journey_and_scope() {
    let uri = std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap();
    assert_eq!(uri, "127.0.0.1:3223");
    let g = Graph::new(uri, "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
    repo::init(&g).await.unwrap();
    g.run(query("CREATE(t:StaffMember {id:'teacher',email:'teacher@example.test',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(other:StaffMember {id:'other',email:'other@example.test',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(sec:Section {section_id:'class',name:'7A',year_group:'Year 7',school_id:'school',tenant_id:'tenant',status:'active',homeroom_staff_member_id:'teacher',homeroom_teacher_email:'teacher@example.test'}) CREATE(:LearningTeachingGrant {staff_member_id:'other',section_id:'class',school_id:'school',tenant_id:'tenant',status:'ACTIVE'}) CREATE(u:User {id:'parent',email:'parent@example.test',role:'parent'})-[:HAS_APPLICATION]->(l:Lead {lead_id:'LEAD-PARENT',email:'parent@example.test',status:'verified'})-[:HAS_STUDENT]->(s:Student {studentId:'child',fullName:'Actual Child'})-[:ENROLLED_AS]->(e:EnrolledStudent {status:'active',school_id:'school',tenant_id:'tenant'})-[:ENROLLED_IN]->(sec) CREATE(u)-[:CREATED_SCHOOL_REQUEST]->(r:ParentSchoolRequest {id:'request',public_id:'request-public',section_id:'class',school_id:'school',tenant_id:'tenant',kind:'leave',date:'2026-09-30',reason:'sick',note:'School report',status:'pending_review',createdAt:datetime()})-[:FOR_STUDENT]->(s)")).await.unwrap();
    let teacher = Teacher {
        active: true,
        staff_member_id: "teacher".into(),
        roles: vec!["teacher".into()],
        school_ids: vec!["school".into()],
        tenant_ids: vec!["tenant".into()],
    };
    let other = Teacher {
        staff_member_id: "other".into(),
        ..Teacher {
            active: true,
            staff_member_id: String::new(),
            roles: vec!["teacher".into()],
            school_ids: vec!["school".into()],
            tenant_ids: vec!["tenant".into()],
        }
    };
    assert_eq!(
        repo::context(&g, &teacher).await.unwrap()["classes"][0]["homeroom"],
        true
    );
    assert_eq!(
        repo::context(&g, &other).await.unwrap()["classes"][0]["homeroom"],
        false
    );
    let classroom = repo::classroom(&g, &teacher, "class", &school_today().to_string())
        .await.unwrap().unwrap();
    assert_eq!(classroom["requests"][0]["reason"], "sick");
    assert!(repo::classroom(&g, &other, "class", &school_today().to_string())
        .await.unwrap().unwrap()["requests"].as_array().unwrap().is_empty());
    let mut call = RollCall {
        date: school_today().to_string(),
        entries: vec![AttendanceInput {
            student_id: "child".into(),
            status: "present".into(),
            arrived_at: Some("07:10".into()),
            dismissed_at: Some("14:00".into()),
            note: String::new(),
        }],
    };
    assert!(!repo::attendance(&g, &other, "class", &call).await.unwrap());
    assert!(repo::attendance(&g, &teacher, "class", &call)
        .await
        .unwrap());
    call.entries.push(AttendanceInput {
        student_id: "foreign".into(),
        status: "absent".into(),
        arrived_at: None,
        dismissed_at: None,
        note: String::new(),
    });
    assert!(!repo::attendance(&g, &teacher, "class", &call)
        .await
        .unwrap());
    assert_eq!(
        repo::classroom(&g, &teacher, "class", &call.date)
            .await
            .unwrap()
            .unwrap()["students"][0]["status"],
        "present"
    );
    let records = [
        PublishedRecord::Event {
            id: "event".into(),
            title: "Parent meeting".into(),
            date: school_today().to_string(),
            start: Some("10:00".into()),
            end: Some("11:00".into()),
            location: "School".into(),
            category: "meeting".into(),
            rsvp: true,
        },
        PublishedRecord::Project {
            id: "project".into(),
            student_id: "child".into(),
            title: "Water project".into(),
            subject: "Science".into(),
            summary: "Published progress".into(),
            state: "in_progress".into(),
        },
        PublishedRecord::Report {
            id: "report".into(),
            student_id: "child".into(),
            term: "Term1".into(),
            gpa: 3.5,
            maximum: 4.0,
        },
        PublishedRecord::Grade {
            id: "grade".into(),
            student_id: "child".into(),
            subject: "Math".into(),
            term: "Term1".into(),
            score: 85.0,
            maximum: 100.0,
        },
    ];
    for r in &records {
        assert!(r.valid());
        assert!(repo::publish(&g, &teacher, "class", r).await.unwrap());
        assert!(repo::publish(&g, &teacher, "class", r).await.unwrap());
    }
    let mut snapshot = repo::parent_snapshot(&g, "parent").await.unwrap();
    assert_eq!(snapshot["records"].as_array().unwrap().len(), 4);
    assert_eq!(snapshot["attendance"][0]["arrivedAt"], "07:10");
    assert!(repo::parent_snapshot(&g, "foreign").await.is_err());
    assert!(repo::rsvp(
        &g,
        "parent",
        "event",
        &Rsvp {
            student_id: "child".into(),
            response: "attending".into()
        }
    )
    .await
    .unwrap());
    assert!(!repo::rsvp(
        &g,
        "parent",
        "event",
        &Rsvp {
            student_id: "foreign".into(),
            response: "attending".into()
        }
    )
    .await
    .unwrap());
    let prefs = Preferences {
        version: 0,
        locale: "id".into(),
        theme: "dark".into(),
        notifications: true,
    };
    assert!(repo::preferences(&g, "parent", &prefs).await.unwrap());
    assert!(!repo::preferences(&g, "parent", &prefs).await.unwrap());
    let review = Review {
        expected_status: "pending_review".into(),
        status: "approved".into(),
        note: "Acknowledged".into(),
    };
    assert!(
        !repo::review(&g, &other, "class", "request-public", &review)
            .await
            .unwrap()
    );
    assert!(
        repo::review(&g, &teacher, "class", "request-public", &review)
            .await
            .unwrap()
    );
    assert!(
        repo::review(&g, &teacher, "class", "request-public", &review)
            .await
            .unwrap()
    );
    assert!(!repo::review(
        &g,
        &teacher,
        "class",
        "request-public",
        &Review {
            status: "declined".into(),
            ..review
        }
    )
    .await
    .unwrap());
    snapshot = repo::parent_snapshot(&g, "parent").await.unwrap();
    assert_eq!(snapshot["messages"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["preferences"]["version"], 1);
    assert!(
        repo::mark_read(&g, "parent", "school-request:request-public")
            .await
            .unwrap()
    );
    assert!(
        !repo::mark_read(&g, "foreign", "school-request:request-public")
            .await
            .unwrap()
    );

    // Shared caller retry keys never identify another parent's request or notice.
    g.run(query("CREATE(u:User {id:'parent-two',email:'two@example.test',role:'parent'})-[:HAS_APPLICATION]->(:Lead {email:'two@example.test',status:'verified'})-[:HAS_STUDENT]->(s:Student {studentId:'child-two',fullName:'Second Child'})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'school',tenant_id:'tenant'})-[:ENROLLED_IN]->(sec:Section {section_id:'second-class',name:'8A',year_group:'8',school_id:'school',tenant_id:'tenant',status:'active',homeroom_staff_member_id:'teacher'}) CREATE(u)-[:CREATED_SCHOOL_REQUEST]->(:ParentSchoolRequest {id:'request',public_id:'second-public',section_id:'second-class',school_id:'school',tenant_id:'tenant',kind:'leave',date:'2026-09-30',note:'Private request',status:'pending_review',createdAt:datetime()})-[:FOR_STUDENT]->(s)")).await.unwrap();
    assert!(repo::review(
        &g,
        &teacher,
        "second-class",
        "second-public",
        &Review {
            expected_status: "pending_review".into(),
            status: "declined".into(),
            note: "Second reply only".into()
        }
    )
    .await
    .unwrap());
    assert_eq!(
        repo::parent_snapshot(&g, "parent").await.unwrap()["messages"][0]["body"],
        "approved · Acknowledged"
    );
    assert_eq!(
        repo::parent_snapshot(&g, "parent-two").await.unwrap()["messages"][0]["body"],
        "declined · Second reply only"
    );
    assert!(!repo::review(
        &g,
        &teacher,
        "class",
        "second-public",
        &Review {
            expected_status: "pending_review".into(),
            status: "approved".into(),
            note: "Foreign".into()
        }
    )
    .await
    .unwrap());
    // Class announcements: the homeroom publishes, each Parent of the class reads and answers.
    let trip = PublishedRecord::Announcement {
        id: "trip".into(),
        title: "Museum visit".into(),
        body: "Year 7 visits the science museum.".into(),
        note: "Bring a hat.".into(),
        priority: "important".into(),
        approval: true,
        due_date: Some(school_today().to_string()),
        due_time: Some("17:00".into()),
    };
    let uniform = PublishedRecord::Announcement {
        id: "uniform".into(),
        title: "Batik on Friday".into(),
        body: "Students wear batik every Friday.".into(),
        note: String::new(),
        priority: "general".into(),
        approval: false,
        due_date: None,
        due_time: None,
    };
    assert!(trip.valid() && uniform.valid());
    assert!(!repo::publish(&g, &other, "class", &trip).await.unwrap());
    for record in [&trip, &uniform] {
        assert!(repo::publish(&g, &teacher, "class", record).await.unwrap());
        assert!(repo::publish(&g, &teacher, "class", record).await.unwrap());
    }
    let reply = |student: &str, response: &str| Rsvp {
        student_id: student.into(),
        response: response.into(),
    };
    let posted = |snapshot: &serde_json::Value, id: &str| {
        snapshot["announcements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["content"]["id"] == id)
            .cloned()
            .unwrap()
    };
    snapshot = repo::parent_snapshot(&g, "parent").await.unwrap();
    assert_eq!(snapshot["records"].as_array().unwrap().len(), 4);
    assert_eq!(snapshot["announcements"].as_array().unwrap().len(), 2);
    assert_eq!(posted(&snapshot, "trip")["studentId"], "child");
    assert_eq!(posted(&snapshot, "trip")["content"]["due_time"], "17:00");
    assert!(posted(&snapshot, "trip")["response"].is_null());
    assert_eq!(posted(&snapshot, "trip")["read"], false);
    // Approval answers only an announcement that asks for it, for the Parent's own child.
    assert!(!repo::rsvp(&g, "parent", "uniform", &reply("child", "approved")).await.unwrap());
    assert!(!repo::rsvp(&g, "parent", "trip", &reply("child", "attending")).await.unwrap());
    assert!(!repo::rsvp(&g, "parent", "trip", &reply("foreign", "approved")).await.unwrap());
    assert!(!repo::rsvp(&g, "parent-two", "trip", &reply("child-two", "approved")).await.unwrap());
    assert!(!repo::rsvp(&g, "parent-two", "trip", &reply("child", "approved")).await.unwrap());
    assert!(repo::rsvp(&g, "parent", "trip", &reply("child", "rejected")).await.unwrap());
    assert!(repo::rsvp(&g, "parent", "trip", &reply("child", "approved")).await.unwrap());
    assert!(repo::mark_read(&g, "parent", "uniform").await.unwrap());
    assert!(repo::mark_read(&g, "parent", "uniform").await.unwrap());
    assert!(!repo::mark_read(&g, "parent", "missing").await.unwrap());
    assert!(!repo::mark_read(&g, "parent-two", "uniform").await.unwrap());
    assert!(!repo::mark_read(&g, "foreign", "uniform").await.unwrap());
    snapshot = repo::parent_snapshot(&g, "parent").await.unwrap();
    assert_eq!(posted(&snapshot, "trip")["response"], "approved");
    assert_eq!(posted(&snapshot, "trip")["read"], true);
    assert!(posted(&snapshot, "uniform")["response"].is_null());
    assert_eq!(posted(&snapshot, "uniform")["read"], true);
    assert!(repo::parent_snapshot(&g, "parent-two").await.unwrap()["announcements"]
        .as_array()
        .unwrap()
        .is_empty());
    let board = repo::classroom(&g, &teacher, "class", &school_today().to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(board["announcements"].as_array().unwrap().len(), 2);
    assert!(board["records"]
        .as_array()
        .unwrap()
        .iter()
        .all(|record| record["kind"] != "announcement"));
    assert_eq!(board["announcementReplies"].as_array().unwrap().len(), 1);
    assert_eq!(board["announcementReplies"][0]["announcementId"], "trip");
    assert_eq!(board["announcementReplies"][0]["approved"], 1);
    assert_eq!(board["announcementReplies"][0]["rejected"], 0);
    // A subject Teacher sees the board but not who answered.
    let subject = repo::classroom(&g, &other, "class", &school_today().to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(subject["announcements"].as_array().unwrap().len(), 2);
    assert!(subject["announcementReplies"].as_array().unwrap().is_empty());
    // A correction remains current after the immutable old command is retried.
    let correction = PublishedRecord::Grade {
        id: "grade-correction".into(),
        student_id: "child".into(),
        subject: "Math".into(),
        term: "Term1".into(),
        score: 90.0,
        maximum: 100.0,
    };
    assert!(repo::publish(&g, &teacher, "class", &correction)
        .await
        .unwrap());
    assert!(repo::publish(&g, &teacher, "class", &records[3])
        .await
        .unwrap());
    let mut grade = g
        .execute(query(
            "MATCH(g:GradeEntry {applicant_student_id:'child'}) RETURN g.score AS score",
        ))
        .await
        .unwrap();
    assert_eq!(
        grade
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<f64>("score")
            .unwrap(),
        90.0
    );
    // An email alone cannot grant homeroom authority.
    g.run(query(
        "MATCH(sec:Section {section_id:'class'}) REMOVE sec.homeroom_staff_member_id",
    ))
    .await
    .unwrap();
    assert!(!repo::attendance(&g, &teacher, "class", &call)
        .await
        .unwrap());
    g.run(query(
        "MATCH(sec:Section {section_id:'class'}) SET sec.homeroom_staff_member_id='teacher'",
    ))
    .await
    .unwrap();
    // Acquire User lock, revoke while an authorized mutation waits, and deny its write.
    let mut tx = g.start_txn().await.unwrap();
    tx.run(query(
        "MATCH(u:User {id:'parent'}) SET u.school_portal_lock=coalesce(u.school_portal_lock,0)+1",
    ))
    .await
    .unwrap();
    let copy = g.clone();
    let waiting = tokio::spawn(async move {
        repo::preferences(
            &copy,
            "parent",
            &Preferences {
                version: 1,
                locale: "en".into(),
                theme: "system".into(),
                notifications: true,
            },
        )
        .await
        .unwrap()
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(!waiting.is_finished());
    tx.run(query("MATCH(u:User {id:'parent'}) SET u.role='staff'"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(!waiting.await.unwrap());
    g.run(query("MATCH(u:User {id:'parent'}) SET u.role='parent'"))
        .await
        .unwrap();
    // A long-lived class still serves a bounded recent window to every consumer.
    g.run(query("MATCH(sec:Section {section_id:'class'}) UNWIND range(1,1001) AS i CREATE(:SchoolPublishedRecord {key:'bulk'+toString(i),kind:'project',student_id:'child',payload:replace($payload,'bulk-project','bulk-project-'+toString(i)),created_at:datetime()})-[:FOR_SECTION]->(sec)").param("payload",serde_json::to_string(&PublishedRecord::Project{id:"bulk-project".into(),student_id:"child".into(),title:"Progress".into(),subject:"Science".into(),summary:"\u{0001}".repeat(4000),state:"in_progress".into()}).unwrap())).await.unwrap();
    let recent = repo::parent_snapshot(&g, "parent").await.unwrap();
    assert_eq!(recent["records"].as_array().unwrap().len(), 104);
    assert!(serde_json::to_vec(&recent).unwrap().len() < 8_000_000);
    assert_eq!(recent["messages"].as_array().unwrap().len(), 1);
    g.run(query(
        "MATCH(t:StaffMember {id:'teacher'}) SET t.membershipStatus='SUSPENDED'",
    ))
    .await
    .unwrap();
    assert!(repo::context(&g, &teacher).await.unwrap()["classes"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!repo::publish(&g, &teacher, "class", &records[0])
        .await
        .unwrap());
    g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
}
