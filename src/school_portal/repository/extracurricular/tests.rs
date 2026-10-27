use super::class_extracurricular;
use crate::school_portal::auth::Teacher;
use neo4rs::{query, Graph};
use serde_json::json;

fn member(id: &str, school: &str, tenant: &str) -> Teacher {
    Teacher {
        active: true,
        staff_member_id: id.into(),
        roles: vec!["teacher".into()],
        school_ids: vec![school.into()],
        tenant_ids: vec![tenant.into()],
    }
}

/// Seeds a class with a homeroom teacher, a subject teacher with a grant, two Students (one in a
/// club with marks, a draft and a released outcome, one without a club) and reads the class.
#[tokio::test]
#[ignore = "requires a disposable loopback graph in EXTRACURRICULAR_TEST_BOLT (127.0.0.1:<port>)"]
async fn homeroom_reads_released_club_records_and_a_subject_teacher_does_not() {
    let uri = std::env::var("EXTRACURRICULAR_TEST_BOLT").unwrap();
    assert!(uri.starts_with("127.0.0.1:"), "a disposable local graph only");
    let g = Graph::new(
        &uri,
        &std::env::var("NEO4J_USER").unwrap(),
        &std::env::var("NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    crate::extra_curricular::repository::init(&g).await.unwrap();
    let mark = format!("ekskul-homeroom-{}", uuid::Uuid::new_v4());
    let school = format!("{mark}-school");
    let tenant = format!("{mark}-tenant");
    let homeroom = format!("{mark}-homeroom");
    let subject = format!("{mark}-subject");
    let coach = format!("{mark}-coach");
    let section = format!("{mark}-section");
    let member_student = format!("{mark}-member");
    let other_student = format!("{mark}-other");
    let key = format!("{tenant}|{school}|2026-2027|robotics");
    let activity = json!({"id":"robotics","title":{"en":"Robotics","id":"Robotik"},"summary":{"en":"School robotics","id":"Robotik sekolah"},"category":"stem","image":null,"coachId":coach,"sectionIds":[section],"capacity":20,"parentConsent":false,"published":true,"competitive":true,"featured":false,
        "meetings":[{"id":"meet-1","date":"2026-10-08","start":"15:00","end":"16:00","title":{"en":"Build","id":"Rakit"},"location":{"en":"Lab","id":"Lab"},"kind":"training"},{"id":"meet-2","date":"2026-10-15","start":"15:00","end":"16:00","title":{"en":"Test","id":"Uji"},"location":{"en":"Lab","id":"Lab"},"kind":"training"},{"id":"match-1","date":"2026-10-17","start":"09:00","end":"11:00","title":{"en":"Regional round","id":"Babak regional"},"location":{"en":"Hall","id":"Aula"},"kind":"match"}],
        "criteria":[{"id":"collaboration","title":{"en":"Collaboration","id":"Kolaborasi"},"levels":[{"en":"Developing","id":"Berkembang"},{"en":"Secure","id":"Mantap"}]}]});
    let released = json!({"studentId":member_student,"revision":2,"released":true,"skills":[{"criterionId":"collaboration","level":1}],"feedback":{"en":"Worked carefully","id":"Bekerja teliti"},"achievements":[{"en":"Built a sensor","id":"Membuat sensor"}]});
    let draft = json!({"studentId":member_student,"revision":3,"released":false,"skills":[{"criterionId":"collaboration","level":0}],"feedback":{"en":"New draft","id":"Draf baru"},"achievements":[]});
    g.run(query("CREATE(:StaffMember {id:$homeroom,fullName:'Bu Ratna',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],test_key:$mark}) CREATE(:StaffMember {id:$subject,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],test_key:$mark}) CREATE(:StaffMember {id:$coach,fullName:'Pak Dimas',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],test_key:$mark}) CREATE(sec:Section {section_id:$section,name:'7A',status:'active',school_id:$school,tenant_id:$tenant,academic_year:'2026-2027',homeroom_staff_member_id:$homeroom,test_key:$mark}) CREATE(:LearningTeachingGrant {staff_member_id:$subject,section_id:$section,school_id:$school,tenant_id:$tenant,status:'ACTIVE',test_key:$mark}) CREATE(:Student {studentId:$member,fullName:'Aisha',test_key:$mark})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant,test_key:$mark})-[:ENROLLED_IN]->(sec) CREATE(:Student {studentId:$other,fullName:'Bima',test_key:$mark})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant,test_key:$mark})-[:ENROLLED_IN]->(sec) CREATE(:ExtraCurricularActivity {key:$key,id:'robotics',school_id:$school,tenant_id:$tenant,academic_year:'2026-2027',coach_id:$coach,section_ids:[$section],payload:$activity,revision:1,archived:false,published:true,test_key:$mark}) CREATE(:ExtraCurricularEnrollment {key:$key+'|'+$member,activity_key:$key,student_id:$member,status:'enrolled',revision:1,payload:$enrollment,test_key:$mark}) CREATE(:ExtraCurricularAttendance {key:$key+'|meet-1|'+$member,activity_key:$key,student_id:$member,payload:$mark1,test_key:$mark}) CREATE(:ExtraCurricularAttendance {key:$key+'|meet-2|'+$member,activity_key:$key,student_id:$member,payload:$mark2,test_key:$mark}) CREATE(:ExtraCurricularMeetingRecord {key:$key+'|meet-1',activity_key:$key,meeting_id:'meet-1',revision:1,test_key:$mark}) CREATE(:ExtraCurricularMeetingRecord {key:$key+'|meet-2',activity_key:$key,meeting_id:'meet-2',revision:1,test_key:$mark}) CREATE(:ExtraCurricularMeetingRecord {key:$key+'|match-1',activity_key:$key,meeting_id:'match-1',revision:1,result:$result,test_key:$mark}) CREATE(:ExtraCurricularOutcome {key:$key+'|'+$member,activity_key:$key,student_id:$member,revision:3,payload:$draft,released_payload:$released,released_at:datetime(),test_key:$mark})")
        .param("mark",mark.clone()).param("homeroom",homeroom.clone()).param("subject",subject.clone()).param("coach",coach.clone()).param("school",school.clone()).param("tenant",tenant.clone()).param("section",section.clone()).param("member",member_student.clone()).param("other",other_student.clone()).param("key",key.clone())
        .param("activity",activity.to_string())
        .param("enrollment",json!({"studentId":member_student,"revision":1,"status":"enrolled","role":{"en":"Captain","id":"Kapten"},"jersey":"#7","roleRevision":1}).to_string())
        .param("mark1",json!({"meetingId":"meet-1","studentId":member_student,"status":"present","revision":1}).to_string())
        .param("mark2",json!({"meetingId":"meet-2","studentId":member_student,"status":"late","revision":1}).to_string())
        .param("result",json!({"meetingId":"match-1","outcome":"win","score":"54–48","revision":1}).to_string())
        .param("draft",draft.to_string()).param("released",released.to_string())).await.unwrap();

    let read = class_extracurricular(&g, &member(&homeroom, &school, &tenant), &section)
        .await
        .unwrap()
        .expect("the homeroom teacher reads the class");
    assert_eq!(read["sectionId"], section);
    let students = read["students"].as_array().unwrap();
    assert_eq!(students.len(), 2, "a Student without a club still appears");
    let aisha = &students[0];
    assert_eq!(aisha["name"], "Aisha");
    let club = &aisha["activities"][0];
    assert_eq!(club["activityId"], "robotics");
    assert_eq!(club["title"]["id"], "Robotik");
    assert_eq!(club["coachName"], "Pak Dimas");
    assert_eq!(club["enrollment"]["role"]["en"], "Captain");
    assert_eq!(club["enrollment"]["jersey"], "#7");
    assert_eq!(club["meetings"], json!({"scheduled":3,"recorded":2}));
    assert_eq!(
        club["attendance"],
        json!({"present":1,"late":1,"absent":0,"excused":0,"recorded":2})
    );
    // Only the released outcome travels; the newer draft stays with the coach.
    assert_eq!(club["outcome"]["revision"], 2);
    assert_eq!(club["outcome"]["feedback"]["en"], "Worked carefully");
    assert_eq!(club["outcome"]["skills"][0]["label"]["id"], "Mantap");
    assert!(crate::school_portal::model::date(
        club["outcome"]["releasedDate"].as_str().unwrap()
    ));
    assert_eq!(club["results"][0]["outcome"], "win");
    assert_eq!(club["results"][0]["title"]["en"], "Regional round");
    assert!(students[1]["activities"].as_array().unwrap().is_empty());

    // A subject teacher's grant opens the classroom, not the homeroom's club records.
    assert!(class_extracurricular(&g, &member(&subject, &school, &tenant), &section)
        .await
        .unwrap()
        .is_none());
    // Nor does the coach, unless they are the homeroom teacher.
    assert!(class_extracurricular(&g, &member(&coach, &school, &tenant), &section)
        .await
        .unwrap()
        .is_none());
    g.run(query("MATCH(n) WHERE n.test_key=$mark DETACH DELETE n").param("mark", mark))
        .await
        .unwrap();
}
