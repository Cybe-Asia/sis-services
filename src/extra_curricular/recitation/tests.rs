use super::model::*;
use super::repository::{self, Outcome};
use crate::extra_curricular::model::{Change, Command, Scope, Skill};
use crate::extra_curricular::{auth::Actor, repository::change};
use crate::school_articles::cover::Store;
use neo4rs::{query, Graph};
use serde_json::json;

fn webm() -> Vec<u8> {
    let mut bytes = vec![0x1A, 0x45, 0xDF, 0xA3];
    bytes.extend_from_slice(&[0u8; 64]);
    bytes
}
fn verdict() -> serde_json::Value {
    json!({"stars":2,"words":[{"index":0,"expected":"قل","heard":"قل","status":"ok"},{"index":1,"expected":"اعوذ","heard":"اعود","status":"unsure"}],"heardText":"قل اعود","durationSeconds":4.2})
}
fn submission(verse: &str, audio: &[u8]) -> serde_json::Value {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    json!({"scope":{"schoolId":"s","tenantId":"t","academicYear":"2026-2027","studentId":null,"after":null},"activityId":"tahfiz","verse":verse,"mime":"audio/webm","audioBase64":STANDARD.encode(audio),"verdict":verdict()})
}
#[test]
fn a_submission_is_bounded_and_identifies_nobody() {
    let ok: Submission = serde_json::from_value(submission("114:1", &webm())).unwrap();
    assert!(ok.valid());
    for verse in ["114", "0:1", "1:0", "1234:1", "114:1:1", "a:b", ""] {
        let bad: Submission = serde_json::from_value(submission(verse, &webm())).unwrap();
        assert!(!bad.valid(), "{verse}");
    }
    let mut with_student = submission("114:1", &webm());
    with_student["scope"]["studentId"] = json!("someone");
    assert!(!serde_json::from_value::<Submission>(with_student).unwrap().valid());
    let mut unknown = submission("114:1", &webm());
    unknown["studentId"] = json!("someone");
    assert!(serde_json::from_value::<Submission>(unknown).is_err());
    let mut stars = submission("114:1", &webm());
    stars["verdict"]["stars"] = json!(0);
    assert!(!serde_json::from_value::<Submission>(stars).unwrap().valid());
    let mut status = submission("114:1", &webm());
    status["verdict"]["words"][0]["status"] = json!("great");
    assert!(!serde_json::from_value::<Submission>(status).unwrap().valid());
    let mut order = submission("114:1", &webm());
    order["verdict"]["words"][1]["index"] = json!(5);
    assert!(!serde_json::from_value::<Submission>(order).unwrap().valid());
}
#[test]
fn a_clip_must_begin_like_its_type() {
    assert!(looks_like("audio/webm", &webm()));
    assert!(!looks_like("audio/ogg", &webm()));
    assert!(looks_like("audio/ogg", b"OggS\0\0\0\0\0\0\0\0\0\0\0\0"));
    assert!(looks_like("audio/mp4", b"\0\0\0\x18ftypisom\0\0\0\0"));
    assert!(looks_like("audio/wav", b"RIFF\0\0\0\0WAVEfmt "));
    assert!(looks_like("audio/mpeg", b"ID3\x04\0\0\0\0\0\0\0\0\0\0"));
    assert!(looks_like("audio/mpeg", b"\xFF\xFB\x90\0\0\0\0\0\0\0\0\0\0"));
    assert!(!looks_like("audio/webm", b"\x1A\x45\xDF"));
    assert!(!looks_like("image/png", b"\x89PNG\r\n\x1a\n\0\0\0\0\0"));
    assert_eq!(extension("audio/mp4"), "m4a");
}
#[test]
fn a_review_names_a_decision_and_bounded_skills() {
    let review = |value: serde_json::Value| serde_json::from_value::<Review>(value).map(|r| r.valid());
    let base = json!({"scope":{"schoolId":"s","tenantId":"t","academicYear":"2026","studentId":null,"after":null},"activityId":"tahfiz","studentId":"child","verse":"114:1","revision":1,"skills":[{"criterionId":"kelancaran","level":1}],"note":"Bagus","decision":"pass"});
    assert_eq!(review(base.clone()).unwrap(), true);
    let mut bad = base.clone();
    bad["decision"] = json!("maybe");
    assert_eq!(review(bad).unwrap(), false);
    let mut bad = base.clone();
    bad["skills"] = json!([]);
    assert_eq!(review(bad).unwrap(), false);
    let mut bad = base.clone();
    bad["skills"] = json!([{"criterionId":"a","level":0},{"criterionId":"a","level":1}]);
    assert_eq!(review(bad).unwrap(), false);
    let mut bad = base.clone();
    bad["note"] = json!("x".repeat(1001));
    assert_eq!(review(bad).unwrap(), false);
    let mut bad = base;
    bad["grade"] = json!(1);
    assert!(review(bad).is_err());
}
#[test]
fn a_query_accepts_only_its_own_cursor_and_filters() {
    let q = |extra: serde_json::Value| {
        let mut v = json!({"schoolId":"s","tenantId":"t","academicYear":"2026","activityId":"tahfiz"});
        for (k, value) in extra.as_object().unwrap() {
            v[k] = value.clone();
        }
        serde_json::from_value::<Query>(v).map(|q| q.valid())
    };
    assert_eq!(q(json!({})).unwrap(), true);
    assert_eq!(q(json!({"status":"reviewed","verse":"114:1","after":"dGVzdA"})).unwrap(), true);
    assert_eq!(q(json!({"status":"graded"})).unwrap(), false);
    assert_eq!(q(json!({"after":"a|b"})).unwrap(), false);
    assert!(q(json!({"role":"teacher"})).is_err());
}
#[test]
fn a_clip_has_its_own_place_in_the_shared_bucket() {
    let scope = Scope { school_id: "SCH".into(), tenant_id: "TEN".into(), academic_year: "2026".into(), student_id: None, after: None };
    assert_eq!(repository::object_key(&scope, "tahfiz", "child-1", "114:1", "c0ffee", "audio/webm"), "sis/recitations/TEN/SCH/tahfiz/child-1/114-1-c0ffee.webm");
}
#[tokio::test]
#[ignore = "requires isolated EXTRACURRICULAR_TEST_BOLT=127.0.0.1:3223"]
async fn a_student_submits_a_coach_grades_and_retention_removes_the_clip() {
    assert_eq!(std::env::var("EXTRACURRICULAR_TEST_BOLT").unwrap(), "127.0.0.1:3223");
    let g = Graph::new("127.0.0.1:3223", &std::env::var("NEO4J_USER").unwrap(), &std::env::var("NEO4J_PASSWORD").unwrap()).await.unwrap();
    crate::extra_curricular::repository::init(&g).await.unwrap();
    let store = Store::memory();
    let mark = format!("recitation-test-{}", uuid::Uuid::new_v4());
    let (school, tenant, admin, coach, other, section) = (format!("{mark}-school"), format!("{mark}-tenant"), format!("{mark}-admin"), format!("{mark}-coach"), format!("{mark}-other"), format!("{mark}-section"));
    g.run(query("CREATE(:StaffMember {id:$admin,membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:[$school],tenantIds:[$tenant],test_key:$mark}) CREATE(:StaffMember {id:$coach,membershipStatus:'ACTIVE',roles:['teacher'],fullName:'Ustadz',schoolIds:[$school],tenantIds:[$tenant],test_key:$mark}) CREATE(:StaffMember {id:$other,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],test_key:$mark}) CREATE(:Section {section_id:$section,name:'7A',status:'active',school_id:$school,tenant_id:$tenant,academic_year:'2026-2027',test_key:$mark})").param("admin",admin.clone()).param("coach",coach.clone()).param("other",other.clone()).param("school",school.clone()).param("tenant",tenant.clone()).param("section",section.clone()).param("mark",mark.clone())).await.unwrap();
    let mut students = vec![];
    for n in 0..2 {
        let (student, parent, user) = (format!("{mark}-child-{n}"), format!("{mark}-parent-{n}"), format!("{mark}-student-{n}"));
        g.run(query("MATCH(sec:Section {section_id:$section}) CREATE(u:User {id:$parent,email:$email,role:'parent',test_key:$mark})-[:HAS_APPLICATION]->(:Lead {lead_id:$parent,email:$email,status:'verified',test_key:$mark})-[:HAS_STUDENT]->(s:Student {studentId:$student,fullName:$name,test_key:$mark})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant,test_key:$mark})-[:ENROLLED_IN]->(sec) CREATE(:User {id:$user,role:'student',test_key:$mark}) CREATE(:LearningStudentBinding {user_id:$user,student_id:$student,status:'ACTIVE',school_id:$school,tenant_id:$tenant,test_key:$mark})").param("section",section.clone()).param("parent",parent.clone()).param("email",format!("{parent}@example.test")).param("student",student.clone()).param("name",format!("Child {n}")).param("user",user.clone()).param("school",school.clone()).param("tenant",tenant.clone()).param("mark",mark.clone())).await.unwrap();
        students.push((student, user));
    }
    let scope = Scope { school_id: school.clone(), tenant_id: tenant.clone(), academic_year: "2026-2027".into(), student_id: None, after: None };
    let administrator = Actor { id: admin, role: "admin".into() };
    let teacher = Actor { id: coach.clone(), role: "teacher".into() };
    let stranger = Actor { id: other, role: "teacher".into() };
    let activity: crate::extra_curricular::model::Activity = serde_json::from_value(json!({"id":"tahfiz","title":{"en":"Tahfiz","id":"Tahfiz"},"summary":{"en":"Juz 30","id":"Juz 30"},"category":"leadership","image":null,"coachId":coach,"sectionIds":[section],"capacity":10,"parentConsent":false,"published":true,"meetings":[],"criteria":[{"id":"kelancaran","title":{"en":"Fluency","id":"Kelancaran"},"levels":[{"en":"Developing","id":"Berkembang"},{"en":"Secure","id":"Lancar"}]},{"id":"tajwid","title":{"en":"Tajwid","id":"Tajwid"},"levels":[{"en":"Developing","id":"Berkembang"},{"en":"Secure","id":"Tepat"}]}]})).unwrap();
    let save = Change { scope: scope.clone(), activity_id: "tahfiz".into(), revision: 0, request_id: "save".into(), command: Command::Save { activity } };
    assert_eq!(change(&g, &administrator, &save).await.unwrap().unwrap()["revision"], 1);
    let student = Actor { id: students[0].1.clone(), role: "student".into() };
    let outsider = Actor { id: students[1].1.clone(), role: "student".into() };
    let mut sub: Submission = serde_json::from_value(submission("114:1", &webm())).unwrap();
    sub.scope = scope.clone();
    // Not enrolled yet: refused, nothing stored.
    assert!(matches!(repository::submit(&g, &store, &student, &sub, webm()).await.unwrap(), Outcome::Forbidden));
    let enroll = Change { scope: scope.clone(), activity_id: "tahfiz".into(), revision: 1, request_id: "enroll".into(), command: Command::Enroll {} };
    assert_eq!(change(&g, &student, &enroll).await.unwrap().unwrap()["status"], "enrolled");
    let first = match repository::submit(&g, &store, &student, &sub, webm()).await.unwrap() { Outcome::Done(v) => v, _ => panic!("first submission refused") };
    assert_eq!(first["attempts"], 1);
    assert_eq!(first["revision"], 1);
    let mut second_clip = webm();
    second_clip.push(7);
    let second = match repository::submit(&g, &store, &student, &sub, second_clip.clone()).await.unwrap() { Outcome::Done(v) => v, _ => panic!("second submission refused") };
    assert_eq!(second["attempts"], 2);
    assert_eq!(second["revision"], 2);
    // The student sees their verse; the coach sees the roster's; a stranger coach sees nothing.
    let q = Query { school_id: school.clone(), tenant_id: tenant.clone(), academic_year: "2026-2027".into(), activity_id: "tahfiz".into(), student_id: None, verse: None, status: None, after: None };
    let mine = repository::list(&g, &student, &q).await.unwrap().unwrap();
    assert_eq!(mine["items"].as_array().unwrap().len(), 1);
    assert_eq!(mine["items"][0]["verse"], "114:1");
    assert_eq!(mine["items"][0]["stars"], 2);
    assert!(mine["items"][0].get("studentName").is_none());
    assert!(repository::list(&g, &outsider, &q).await.unwrap().is_none());
    let queue = repository::list(&g, &teacher, &q).await.unwrap().unwrap();
    assert_eq!(queue["items"][0]["studentName"], "Child 0");
    assert_eq!(queue["items"][0]["status"], "submitted");
    assert!(repository::list(&g, &stranger, &q).await.unwrap().is_none());
    // The clip the coach hears is the newest one; the replaced one is gone from the store.
    let clip_query = Query { student_id: Some(students[0].0.clone()), verse: Some("114:1".into()), ..q.clone() };
    let clip = match repository::audio(&g, &store, &teacher, &clip_query).await.unwrap() { Outcome::Done(v) => v, _ => panic!("coach cannot hear the clip") };
    assert_eq!(clip["mime"], "audio/webm");
    assert_eq!(clip["bytes"].as_array().unwrap().len(), second_clip.len());
    assert!(matches!(repository::audio(&g, &store, &stranger, &clip_query).await.unwrap(), Outcome::Forbidden));
    assert!(matches!(repository::audio(&g, &store, &outsider, &clip_query).await.unwrap(), Outcome::Forbidden));
    assert!(matches!(repository::audio(&g, &store, &student, &clip_query).await.unwrap(), Outcome::Done(_)));
    // Grading needs every criterion, the revision the coach listened to, and a roster student.
    let mut review = Review { scope: scope.clone(), activity_id: "tahfiz".into(), student_id: students[0].0.clone(), verse: "114:1".into(), revision: 1, skills: vec![Skill { criterion_id: "kelancaran".into(), level: 1 }, Skill { criterion_id: "tajwid".into(), level: 0 }], note: Some("Ulangi mad".into()), decision: "retry".into() };
    assert!(matches!(repository::review(&g, &teacher, &review).await.unwrap(), Outcome::Conflict));
    review.revision = 2;
    assert!(matches!(repository::review(&g, &stranger, &review).await.unwrap(), Outcome::Forbidden));
    let short = Review { skills: vec![Skill { criterion_id: "kelancaran".into(), level: 1 }], ..review.clone() };
    assert!(matches!(repository::review(&g, &teacher, &short).await.unwrap(), Outcome::Forbidden));
    let graded = match repository::review(&g, &teacher, &review).await.unwrap() { Outcome::Done(v) => v, _ => panic!("review refused") };
    assert_eq!(graded["status"], "reviewed");
    assert_eq!(graded["revision"], 3);
    let mine = repository::list(&g, &student, &q).await.unwrap().unwrap();
    assert_eq!(mine["items"][0]["review"]["decision"], "retry");
    assert_eq!(mine["items"][0]["review"]["revision"], 2);
    assert_eq!(mine["items"][0]["status"], "reviewed");
    // A new take after the grade goes back to the queue and keeps the grade as history.
    let third = match repository::submit(&g, &store, &student, &sub, webm()).await.unwrap() { Outcome::Done(v) => v, _ => panic!("third submission refused") };
    assert_eq!(third["revision"], 4);
    let queue = repository::list(&g, &teacher, &Query { status: Some("submitted".into()), ..q.clone() }).await.unwrap().unwrap();
    assert_eq!(queue["items"].as_array().unwrap().len(), 1);
    assert_eq!(queue["items"][0]["review"]["revision"], 2);
    // Retention: a clip past its date disappears from the store and the record keeps the history.
    g.run(query("MATCH(r:ExtraCurricularRecitation) WHERE r.activity_key STARTS WITH $prefix SET r.delete_after=datetime()-duration({days:1})").param("prefix", format!("{tenant}|"))).await.unwrap();
    assert_eq!(repository::sweep(&g, &store).await.unwrap(), 1);
    assert!(matches!(repository::audio(&g, &store, &teacher, &clip_query).await.unwrap(), Outcome::Missing));
    let mine = repository::list(&g, &student, &q).await.unwrap().unwrap();
    assert_eq!(mine["items"][0]["audio"], false);
    assert_eq!(mine["items"][0]["review"]["decision"], "retry");
    g.run(query("MATCH(n {test_key:$mark}) DETACH DELETE n").param("mark", mark.clone())).await.unwrap();
    g.run(query("MATCH(n) WHERE n.key STARTS WITH $prefix OR n.activity_key STARTS WITH $prefix DETACH DELETE n").param("prefix", format!("{tenant}|"))).await.unwrap();
}
