use super::{auth::Actor, model::*, repository::*};
use neo4rs::{query, Graph};
use serde_json::json;
fn text(s: &str) -> Text {
    Text {
        en: s.into(),
        id: s.into(),
    }
}
fn activity(id: &str, coach: &str, section: &str) -> Activity {
    Activity {
        presentation: None,
        id: id.into(),
        title: text("Robotics"),
        summary: text("School robotics"),
        category: "stem".into(),
        image: None,
        coach_id: coach.into(),
        section_ids: vec![section.into()],
        capacity: 1,
        parent_consent: true,
        published: true,
        meetings: vec![Meeting {
            id: "meet-1".into(),
            date: "2026-10-08".into(),
            start: "15:00".into(),
            end: "16:00".into(),
            title: text("Build"),
            location: text("Lab"),
        }],
        criteria: vec![Criterion {
            id: "collaboration".into(),
            title: text("Collaboration"),
            levels: vec![text("Developing"), text("Secure")],
        }],
    }
}
#[test]
fn validation_rejects_unknown_authority_and_invalid_catalog() {
    let mut a = activity("activity", "coach", "class");
    assert!(a.valid());
    a.meetings[0].start = "é:00".into();
    assert!(!a.valid());
    a.meetings[0].start = "15:00".into();
    a.image = Some("/assets/../secret".into());
    assert!(!a.valid());
    let raw = json!({"scope":{"schoolId":"s","tenantId":"t","academicYear":"2026","studentId":null,"after":null},"activityId":"a","revision":0,"requestId":"r","command":{"action":"enroll","role":"admin"}});
    assert!(serde_json::from_value::<Change>(raw).is_err());
}
#[tokio::test]
#[ignore = "requires isolated EXTRACURRICULAR_TEST_BOLT=127.0.0.1:3223"]
async fn owner_lifecycle_capacity_consent_retry_release_and_isolation() {
    assert_eq!(
        std::env::var("EXTRACURRICULAR_TEST_BOLT").unwrap(),
        "127.0.0.1:3223"
    );
    let g = Graph::new(
        "127.0.0.1:3223",
        &std::env::var("NEO4J_USER").unwrap(),
        &std::env::var("NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    init(&g).await.unwrap();
    let mark = format!("extra-test-{}", uuid::Uuid::new_v4());
    let school = format!("{mark}-school");
    let tenant = format!("{mark}-tenant");
    let admin = format!("{mark}-admin");
    let coach = format!("{mark}-coach");
    let section = format!("{mark}-section");
    g.run(query("CREATE(:StaffMember {id:$admin,membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:[$school],tenantIds:[$tenant],test_key:$mark}) CREATE(:StaffMember {id:$coach,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],test_key:$mark}) CREATE(:Section {section_id:$section,name:'7A',status:'active',school_id:$school,tenant_id:$tenant,academic_year:'2026-2027',test_key:$mark})").param("admin",admin.clone()).param("coach",coach.clone()).param("school",school.clone()).param("tenant",tenant.clone()).param("section",section.clone()).param("mark",mark.clone())).await.unwrap();
    let mut students = vec![];
    for n in 0..2 {
        let student = format!("{mark}-child-{n}");
        let parent = format!("{mark}-parent-{n}");
        let user = format!("{mark}-student-{n}");
        g.run(query("MATCH(sec:Section {section_id:$section}) CREATE(u:User {id:$parent,email:$email,role:'parent',test_key:$mark})-[:HAS_APPLICATION]->(:Lead {lead_id:$parent,email:$email,status:'verified',test_key:$mark})-[:HAS_STUDENT]->(s:Student {studentId:$student,fullName:'Synthetic acceptance child',test_key:$mark})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant,test_key:$mark})-[:ENROLLED_IN]->(sec) CREATE(:User {id:$user,role:'student',test_key:$mark}) CREATE(:LearningStudentBinding {user_id:$user,student_id:$student,status:'ACTIVE',school_id:$school,tenant_id:$tenant,test_key:$mark})").param("section",section.clone()).param("parent",parent.clone()).param("email",format!("{parent}@example.test")).param("student",student.clone()).param("user",user.clone()).param("school",school.clone()).param("tenant",tenant.clone()).param("mark",mark.clone())).await.unwrap();
        students.push((student, parent, user));
    }
    let scope = Scope {
        school_id: school,
        tenant_id: tenant.clone(),
        academic_year: "2026-2027".into(),
        student_id: None,
        after: None,
    };
    let administrator = Actor {
        id: admin,
        role: "admin".into(),
    };
    let teacher = Actor {
        id: coach.clone(),
        role: "teacher".into(),
    };
    let mut v = Change {
        scope: scope.clone(),
        activity_id: "robotics".into(),
        revision: 0,
        request_id: "save".into(),
        command: Command::Save {
            activity: activity("robotics", &coach, &section),
        },
    };
    assert_eq!(
        change(&g, &administrator, &v).await.unwrap().unwrap()["revision"],
        1
    );
    assert!(list(&g, &administrator, &scope).await.unwrap().is_some());
    // Live Auth scopes narrow the graph context and staff projection.
    g.run(query("MATCH(t:StaffMember {id:$coach}) SET t.schoolIds=t.schoolIds+['ungranted-school'],t.tenantIds=t.tenantIds+['ungranted-tenant']").param("coach",coach.clone())).await.unwrap();
    let reader = super::repository::GraphContext(&g);
    let context = super::application::context(
        &reader,
        &administrator,
        None,
        vec![scope.school_id.clone()],
        vec![tenant.clone()],
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(context.classes.len(), 1);
    assert_eq!(context.staff.len(), 1);
    assert_eq!(context.staff[0].school_ids, vec![scope.school_id.clone()]);
    assert_eq!(context.staff[0].tenant_ids, vec![tenant.clone()]);
    let narrowed =
        super::application::context(&reader, &administrator, None, vec![], vec![tenant.clone()])
            .await
            .unwrap()
            .unwrap();
    assert!(narrowed.classes.is_empty() && narrowed.staff.is_empty());
    let narrowed = super::application::context(
        &reader,
        &teacher,
        None,
        vec![scope.school_id.clone()],
        vec![],
    )
    .await
    .unwrap()
    .unwrap();
    assert!(narrowed.classes.is_empty() && narrowed.staff.is_empty());

    let mut enroll = v.clone();
    enroll.revision = 1;
    enroll.request_id = "enroll".into();
    enroll.command = Command::Enroll {};
    let a0 = Actor {
        id: students[0].2.clone(),
        role: "student".into(),
    };
    let a1 = Actor {
        id: students[1].2.clone(),
        role: "student".into(),
    };
    let (r0, r1) = tokio::join!(change(&g, &a0, &enroll), change(&g, &a1, &enroll));
    let r0 = r0.unwrap();
    let r1 = r1.unwrap();
    assert_ne!(r0.is_some(), r1.is_some());
    let winner = if r0.is_some() { 0 } else { 1 };
    let student = &students[winner];
    let sa = Actor {
        id: student.2.clone(),
        role: "student".into(),
    };
    let pa = Actor {
        id: student.1.clone(),
        role: "parent".into(),
    };
    let receipt = change(&g, &sa, &enroll).await.unwrap().unwrap();
    assert_eq!(receipt["revision"], 1);
    assert_eq!(receipt["status"], "pending_consent");
    let mut approve = enroll.clone();
    approve.scope.student_id = Some(student.0.clone());
    approve.request_id = "approve".into();
    approve.command = Command::Approve {};
    assert!(change(&g, &sa, &approve).await.unwrap().is_none());
    assert_eq!(
        change(&g, &pa, &approve).await.unwrap().unwrap()["status"],
        "enrolled"
    );
    let foreign = Actor {
        id: students[1 - winner].1.clone(),
        role: "parent".into(),
    };
    assert!(list(&g, &foreign, &approve.scope).await.unwrap().is_none());
    let mut attendance = v.clone();
    attendance.request_id = "attendance".into();
    attendance.command = Command::Attendance {
        meeting_id: "meet-1".into(),
        marks: vec![Mark {
            student_id: student.0.clone(),
            status: "present".into(),
        }],
    };
    assert!(change(&g, &teacher, &attendance).await.unwrap().is_some());
    assert!(change(&g, &teacher, &attendance).await.unwrap().is_some());
    let mut outcome = attendance.clone();
    outcome.scope.student_id = Some(student.0.clone());
    outcome.request_id = "outcome".into();
    outcome.command = Command::Outcome {
        skills: vec![Skill {
            criterion_id: "collaboration".into(),
            level: 1,
        }],
        feedback: text("Worked carefully"),
        achievements: vec![text("Built a sensor")],
    };
    assert!(change(&g, &teacher, &outcome).await.unwrap().is_some());
    let read = list(&g, &pa, &approve.scope).await.unwrap().unwrap();
    assert!(read["items"][0]["outcome"].is_null());
    assert_eq!(read["items"][0]["attendance"][0]["status"], "present");
    outcome.command = Command::Release {};
    outcome.revision = 1;
    outcome.request_id = "release".into();
    assert!(change(&g, &teacher, &outcome).await.unwrap().is_some());
    let released =
        list(&g, &pa, &approve.scope).await.unwrap().unwrap()["items"][0]["outcome"].clone();
    assert_eq!(released["released"], true);
    assert!(crate::school_portal::model::date(
        released["releasedDate"].as_str().unwrap()
    ));
    outcome.command = Command::Outcome {
        skills: vec![Skill {
            criterion_id: "collaboration".into(),
            level: 0,
        }],
        feedback: text("New draft"),
        achievements: vec![],
    };
    outcome.revision = 2;
    outcome.request_id = "draft2".into();
    assert!(change(&g, &teacher, &outcome).await.unwrap().is_some());
    assert_eq!(
        list(&g, &pa, &approve.scope).await.unwrap().unwrap()["items"][0]["outcome"],
        released
    );
    v.revision = 1;
    let mut consent_change = v.clone();
    consent_change.request_id = "edit-consent-policy".into();
    if let Command::Save { activity } = &mut consent_change.command {
        activity.parent_consent = false;
    }
    assert!(change(&g, &administrator, &consent_change)
        .await
        .unwrap()
        .is_none());
    v.request_id = "edit-criteria".into();
    if let Command::Save { activity } = &mut v.command {
        activity.criteria.clear();
    }
    assert!(change(&g, &administrator, &v).await.unwrap().is_none());
    let mut withdraw = approve.clone();
    withdraw.revision = 2;
    withdraw.request_id = "withdraw".into();
    withdraw.command = Command::Withdraw {};
    assert_eq!(
        change(&g, &pa, &withdraw).await.unwrap().unwrap()["status"],
        "withdrawn"
    );
    let other = if winner == 0 { a1 } else { a0 };
    assert!(change(&g, &other, &enroll).await.unwrap().is_some());
    g.run(
        query("MATCH(t:StaffMember {id:$coach}) SET t.membershipStatus='SUSPENDED'")
            .param("coach", coach),
    )
    .await
    .unwrap();
    assert!(change(&g, &teacher, &attendance).await.unwrap().is_none());
    g.run(
        query("MATCH(b:LearningStudentBinding {user_id:$user}) SET b.status='REVOKED'")
            .param("user", student.2.clone()),
    )
    .await
    .unwrap();
    assert!(change(&g, &sa, &enroll).await.unwrap().is_none());
    let prefix = format!("{tenant}|");
    g.run(query("MATCH(n) WHERE n.test_key=$mark OR n.key STARTS WITH $prefix OR n.activity_key STARTS WITH $prefix DETACH DELETE n").param("mark",mark.clone()).param("prefix",prefix)).await.unwrap();
    g.run(
        query("MATCH(n:ExtraCurricularOperation) WHERE n.request CONTAINS $mark DETACH DELETE n")
            .param("mark", mark),
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires existing isolated3385; UUID fixtures only; never clears the graph"]
async fn owner_reads_coach_roster_draft_outcomes_and_current_revisions() {
    let uri = std::env::var("SIS_OWNER_TEST_NEO4J_URI").unwrap();
    assert!(matches!(
        uri.as_str(),
        "bolt://127.0.0.1:3385" | "127.0.0.1:3385"
    ));
    let g = Graph::new(
        uri,
        std::env::var("SIS_OWNER_TEST_NEO4J_USER").unwrap(),
        std::env::var("SIS_OWNER_TEST_NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    let mark = format!("owner-extra-{}", uuid::Uuid::new_v4());
    let owner = format!("{mark}-owner");
    let coach = format!("{mark}-coach");
    let other = format!("{mark}-other");
    let school = format!("{mark}-school");
    let tenant = format!("{mark}-tenant");
    let section = format!("{mark}-class");
    let student = format!("{mark}-student");
    let scope = Scope {
        school_id: school.clone(),
        tenant_id: tenant.clone(),
        academic_year: "2026-2027".into(),
        student_id: None,
        after: None,
    };
    let a = activity("robotics", &coach, &section);
    let key = activity_key(&scope, &a.id);
    let enrollment = json!({"studentId":student,"status":"enrolled","revision":4});
    let outcome = json!({"studentId":student,"released":false,"revision":9,"skills":[{"criterionId":"collaboration","level":1}],"feedback":{"en":"Draft","id":"Draft"},"achievements":[]});
    g.run(query("CREATE(:School {school_id:$school,tenant_id:$tenant,owner_parity_tag:$mark}) CREATE(:StaffMember {id:$owner,membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[],teamIds:[],owner_parity_tag:$mark}) CREATE(:StaffMember {id:$coach,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],owner_parity_tag:$mark}) CREATE(:StaffMember {id:$other,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],owner_parity_tag:$mark}) CREATE(sec:Section {section_id:$class,name:'Synthetic class',status:'active',school_id:$school,tenant_id:$tenant,academic_year:'2026-2027',owner_parity_tag:$mark}) CREATE(:Student {studentId:$student,fullName:'Synthetic Student',owner_parity_tag:$mark})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant,owner_parity_tag:$mark})-[:ENROLLED_IN]->(sec) CREATE(:ExtraCurricularActivity {key:$key,id:'robotics',school_id:$school,tenant_id:$tenant,academic_year:'2026-2027',coach_id:$coach,section_ids:[$class],payload:$payload,revision:3,archived:false,published:true,owner_parity_tag:$mark}) CREATE(:ExtraCurricularEnrollment {key:$enrollment_key,activity_key:$key,student_id:$student,status:'enrolled',payload:$enrollment,owner_parity_tag:$mark}) CREATE(:ExtraCurricularOutcome {key:$outcome_key,activity_key:$key,student_id:$student,revision:9,payload:$outcome,owner_parity_tag:$mark}) CREATE(:ExtraCurricularMeetingRecord {key:$meeting_key,activity_key:$key,meeting_id:'meet-1',revision:7,owner_parity_tag:$mark})")
        .param("mark",mark.clone()).param("owner",owner.clone()).param("coach",coach.clone()).param("other",other.clone()).param("school",school.clone()).param("tenant",tenant.clone()).param("class",section.clone()).param("student",student.clone()).param("key",key.clone()).param("payload",serde_json::to_string(&a).unwrap()).param("enrollment",enrollment.to_string()).param("outcome",outcome.to_string()).param("enrollment_key",format!("{key}|{student}")).param("outcome_key",format!("{key}|{student}")).param("meeting_key",format!("{key}|meet-1"))).await.unwrap();
    let actor = Actor {
        id: owner.clone(),
        role: "owner".into(),
    };
    let data = list(&g, &actor, &scope).await.unwrap().unwrap();
    let item = &data["items"][0];
    assert_eq!(item["activity"]["coachId"], coach);
    assert_eq!(item["roster"][0]["id"], student);
    assert_eq!(item["roster"][0]["outcome"][0]["revision"], 9);
    assert_eq!(item["meetingRevisions"]["meet-1"], 7);
    let hidden = list(
        &g,
        &Actor {
            id: other,
            role: "teacher".into(),
        },
        &scope,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(hidden["items"].as_array().unwrap().is_empty());
    let v = Change {
        scope: scope.clone(),
        activity_id: "robotics".into(),
        revision: 7,
        request_id: "owner-attendance".into(),
        command: Command::Attendance {
            meeting_id: "meet-1".into(),
            marks: vec![Mark {
                student_id: student.clone(),
                status: "present".into(),
            }],
        },
    };
    assert_eq!(
        change(&g, &actor, &v).await.unwrap().unwrap()["revision"],
        8
    );
    let mut v = Change {
        scope: Scope {
            student_id: Some(student.clone()),
            ..scope.clone()
        },
        activity_id: "robotics".into(),
        revision: 9,
        request_id: "owner-outcome".into(),
        command: Command::Outcome {
            skills: vec![Skill {
                criterion_id: "collaboration".into(),
                level: 1,
            }],
            feedback: text("Preserved coach, actual Owner operation"),
            achievements: vec![],
        },
    };
    assert_eq!(
        change(&g, &actor, &v).await.unwrap().unwrap()["revision"],
        10
    );
    v.request_id = "stale-outcome".into();
    assert!(change(&g, &actor, &v).await.unwrap().is_none());
    let data = list(&g, &actor, &scope).await.unwrap().unwrap();
    assert_eq!(data["items"][0]["activity"]["coachId"], coach);
    assert_eq!(data["items"][0]["meetingRevisions"]["meet-1"], 8);
    assert_eq!(data["items"][0]["roster"][0]["outcome"][0]["revision"], 10);
    let mut rows=g.execute(query("MATCH(a:ExtraCurricularAudit {actor_id:$owner,activity_key:$key}) RETURN collect(a.role) AS roles").param("owner",owner).param("key",key.clone())).await.unwrap();
    assert_eq!(
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<Vec<String>>("roles")
            .unwrap(),
        vec!["owner", "owner"]
    );
    g.run(query("MATCH(n) WHERE n.owner_parity_tag=$mark OR n.activity_key=$key OR n.key CONTAINS $mark DETACH DELETE n").param("mark",mark).param("key",key)).await.unwrap();
}
