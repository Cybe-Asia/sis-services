//! Synthetic Auth TLS adapter + isolated3385; no genuine login/acceptance claim.
use super::*;
use model::{Action, Change, Meeting};
use neo4rs::{query, Graph};
use serde_json::Value;
type Error = Box<dyn std::error::Error + Send + Sync>;
macro_rules! check {
    ($condition:expr,$message:expr) => {
        if !$condition {
            return Err($message.into());
        }
    };
}
struct Client {
    http: reqwest::Client,
    origin: String,
}
impl Client {
    async fn post(&self, path: &str, token: &str, body: &Value) -> Result<(u16, Value), Error> {
        let response = self
            .http
            .post(format!("{}{path}", self.origin))
            .bearer_auth(token)
            .json(body)
            .send()
            .await?;
        let status = response.status().as_u16();
        Ok((status, response.json().await?))
    }
    async fn get(&self, path: &str, token: &str) -> Result<(u16, Value), Error> {
        let response = self
            .http
            .get(format!("{}{path}", self.origin))
            .bearer_auth(token)
            .send()
            .await?;
        let status = response.status().as_u16();
        Ok((status, response.json().await?))
    }
    async fn apply(&self, c: &Change, token: &str, staff: bool) -> Result<(u16, Value), Error> {
        self.post(
            if staff {
                "/api/v1/sis-service/learning/timetable"
            } else {
                "/api/v1/sis-service/timetable"
            },
            token,
            &serde_json::to_value(c)?,
        )
        .await
    }
    async fn validated(&self, m: Meeting, token: &str) -> Result<Change, Error> {
        let mut c = command(m, Action::SaveDraft, 0, 0);
        let (status, saved) = self.apply(&c, token, false).await?;
        check!(status == 200, "Draft failed");
        check!(saved["data"]["status"] == "draft", "Save auto-published");
        c.action = Action::Validate;
        c.expected_version = 1;
        c.operation_id = uuid::Uuid::new_v4().to_string();
        let (status, valid) = self.apply(&c, token, false).await?;
        check!(
            status == 200 && valid["data"]["status"] == "validated",
            "Validation failed"
        );
        c.action = Action::Publish;
        c.expected_version = 2;
        c.operation_id = uuid::Uuid::new_v4().to_string();
        Ok(c)
    }
}
fn command(meeting: Meeting, action: Action, version: u32, pv: u32) -> Change {
    Change {
        meeting,
        expected_version: version,
        expected_published_version: pv,
        action,
        operation_id: uuid::Uuid::new_v4().to_string(),
    }
}
fn first_day() -> chrono::NaiveDate {
    use chrono::Datelike;
    let mut day = crate::school_portal::model::school_today() + chrono::Duration::days(14);
    while day.weekday().number_from_monday() != 1 {
        day += chrono::Duration::days(1);
    }
    day
}
fn day_string(offset: i64) -> String {
    (first_day() + chrono::Duration::days(offset)).to_string()
}
fn meeting(mark: &str, id: &str, class: &str, teacher: &str, room: &str, day: u64) -> Meeting {
    let starts = first_day()
        .and_hms_opt(2, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis() as u64
        + day * 86_400_000;
    Meeting {
        id: id.into(),
        school_id: format!("{mark}-school-a"),
        tenant_id: mark.into(),
        class_id: format!("{mark}-{class}"),
        teacher_id: format!("{mark}-{teacher}"),
        course_id: "canonical-course".into(),
        subject: "math".into(),
        title: "Synthetic scheduled lesson".into(),
        starts_at: starts,
        ends_at: starts + 3_600_000,
        academic_year: "2030/2031".into(),
        room_id: room.into(),
    }
}
fn staff_token(who: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    format!(
        "{}.{}.{who}",
        URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#),
        URL_SAFE_NO_PAD.encode(r#"{"aud":"learning-api"}"#)
    )
}
fn family_token(mark: &str, role: &str) -> String {
    jsonwebtoken::encode(&jsonwebtoken::Header::default(),&json!({"sub":format!("{mark}-{role}"),"role":role,"exp":chrono::Utc::now().timestamp()+1800}),&jsonwebtoken::EncodingKey::from_secret(std::env::var("JWT_SECRET").unwrap().as_bytes())).unwrap()
}
async fn calendar(
    client: &Client,
    mark: &str,
    school: &str,
    kind: &str,
    id: &str,
    start: &str,
    end: &str,
    action: &str,
    version: u32,
) -> Result<(u16, Value), Error> {
    client.post("/api/v1/sis-service/education-calendar","synthetic-owner-a",&json!({"entry":{"id":id,"schoolId":format!("{mark}-{school}"),"tenantId":mark,"academicYear":"2030/2031","kind":kind,"title":"Synthetic education date","description":"","startDate":start,"endDate":end,"audience":"school","sectionIds":[]},"expectedVersion":version,"action":action})).await
}

#[tokio::test]
#[ignore = "explicit isolated SIS_TIMETABLE_TEST_NEO4J_URI=bolt://127.0.0.1:3385 + synthetic TLS Auth fixture only"]
async fn timetable_concurrency_lifecycle_current_rights_and_publication() -> Result<(), Error> {
    let uri = std::env::var("SIS_TIMETABLE_TEST_NEO4J_URI")?;
    check!(
        uri == "bolt://127.0.0.1:3385",
        "Dedicated isolated3385 required"
    );
    let graph = Graph::new(
        uri,
        std::env::var("SIS_TIMETABLE_TEST_NEO4J_USER")?,
        std::env::var("SIS_TIMETABLE_TEST_NEO4J_PASSWORD")?,
    )
    .await?;
    let mark = std::env::var("SIS_TIMETABLE_TEST_MARK")?;
    check!(
        mark.starts_with("timetable-test-")
            && uuid::Uuid::parse_str(mark.trim_start_matches("timetable-test-")).is_ok(),
        "Explicit UUID fixture mark required"
    );
    repository::migrate(&graph).await?;
    let school_a = format!("{mark}-school-a");
    let school_b = format!("{mark}-school-b");
    graph.run(query("CREATE(:School {school_id:$school_a,tenant_id:$mark,tt_test:$mark}) CREATE(:School {school_id:$school_b,tenant_id:$mark,tt_test:$mark}) CREATE(:StaffMember {id:$owner_a,name:'Synthetic Owner A',membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[],teamIds:[],tt_test:$mark}) CREATE(:StaffMember {id:$owner_b,name:'Synthetic Owner B',membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[],teamIds:[],tt_test:$mark}) CREATE(:StaffMember {id:$teacher_a,name:'Synthetic Teacher A',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school_a,$school_b],tenantIds:[$mark],tt_test:$mark}) CREATE(:StaffMember {id:$teacher_b,name:'Synthetic Teacher B',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school_a,$school_b],tenantIds:[$mark],tt_test:$mark}) CREATE(a:Section {section_id:$class_a,name:'Synthetic A',status:'active',academic_year:'2030/2031',school_id:$school_a,tenant_id:$mark,homeroom_staff_member_id:$teacher_a,tt_test:$mark}) CREATE(:Section {section_id:$class_b,name:'Synthetic B',status:'active',academic_year:'2030/2031',school_id:$school_a,tenant_id:$mark,homeroom_staff_member_id:$teacher_b,tt_test:$mark}) CREATE(:Section {section_id:$class_c,name:'Synthetic C',status:'active',academic_year:'2030/2031',school_id:$school_b,tenant_id:$mark,homeroom_staff_member_id:$teacher_a,tt_test:$mark}) CREATE(:LearningTeachingGrant {key:$grant_a,staff_member_id:$teacher_b,section_id:$class_a,school_id:$school_a,tenant_id:$mark,status:'ACTIVE',tt_test:$mark}) CREATE(:LearningTeachingGrant {key:$grant_b,staff_member_id:$teacher_a,section_id:$class_b,school_id:$school_a,tenant_id:$mark,status:'ACTIVE',tt_test:$mark}) CREATE(:User {id:$parent,role:'parent',email:$email,tt_test:$mark})-[:HAS_APPLICATION]->(:Lead {lead_id:$parent,email:$email,status:'verified',tt_test:$mark})-[:HAS_STUDENT]->(s:Student {studentId:$student,fullName:'Synthetic Student',tt_test:$mark})-[:ENROLLED_AS]->(:EnrolledStudent {student_id:$enrolled,applicant_student_id:$student,status:'active',school_id:$school_a,tenant_id:$mark,tt_test:$mark})-[:ENROLLED_IN]->(a) CREATE(:User {id:$student,role:'student',email:$student_email,tt_test:$mark}) CREATE(:LearningStudentBinding {user_id:$student,student_id:$student,school_id:$school_a,tenant_id:$mark,status:'ACTIVE',version:1,tt_test:$mark})").param("mark",mark.clone()).param("school_a",school_a.clone()).param("school_b",school_b.clone()).param("owner_a",format!("{mark}-owner-a")).param("owner_b",format!("{mark}-owner-b")).param("teacher_a",format!("{mark}-teacher-a")).param("teacher_b",format!("{mark}-teacher-b")).param("class_a",format!("{mark}-class-a")).param("class_b",format!("{mark}-class-b")).param("class_c",format!("{mark}-class-c")).param("grant_a",format!("{mark}-grant-a")).param("grant_b",format!("{mark}-grant-b")).param("parent",format!("{mark}-parent")).param("student",format!("{mark}-student")).param("enrolled",format!("STU-{mark}")).param("email",format!("{mark}-parent@example.test")).param("student_email",format!("{mark}-student@example.test"))).await?;
    let state = AppState {
        graph: graph.clone(),
        config: crate::config::config::AppConfig {
            server_port: 0,
            neo4j_uri: String::new(),
            neo4j_user: String::new(),
            neo4j_password: String::new(),
            jwt_secret: String::new(),
        },
        http_client: reqwest::Client::new(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let router = Router::new()
        .merge(crate::learning_access::router())
        .merge(crate::education_calendar::router())
        .merge(crate::school_portal::router())
        .with_state(state);
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let ca =
        reqwest::Certificate::from_pem(&std::fs::read(std::env::var("AUTH_PORTAL_CA_FILE")?)?)?;
    let client = Client {
        http: reqwest::Client::builder()
            .no_proxy()
            .add_root_certificate(ca)
            .timeout(std::time::Duration::from_secs(15))
            .build()?,
        origin,
    };
    client
        .http
        .post(std::env::var("SIS_TIMETABLE_TEST_AUTH_CONTROL")?)
        .json(&json!({"active":true}))
        .send()
        .await?
        .error_for_status()?;
    let result = run(&graph, &client, &mark).await;
    server.abort();
    graph.run(query("MATCH(n) WHERE n.tt_test=$mark OR (n.school_id IN $schools AND n.tenant_id=$mark) OR (n:EducationCalendarLock AND n.key IN $locks) OR (n:LearningTimetableTeacherLock AND n.key IN $teachers) OR (n:LearningTimetableOperation AND any(prefix IN $prefixes WHERE n.key STARTS WITH prefix)) DETACH DELETE n").param("mark",mark.clone()).param("schools",vec![school_a.clone(),school_b.clone()]).param("locks",vec![format!("{mark}|{school_a}"),format!("{mark}|{school_b}")]).param("teachers",vec![format!("{mark}-teacher-a"),format!("{mark}-teacher-b")]).param("prefixes",vec![format!("{mark}|{school_a}|"),format!("{mark}|{school_b}|")])).await?;
    result
}
async fn run(graph: &Graph, client: &Client, mark: &str) -> Result<(), Error> {
    let owner = "synthetic-owner-a";
    let owner_b = "synthetic-owner-b";
    let teacher = staff_token("teacher-a");
    let bearer_owner = staff_token("owner-a");
    let scope = format!("schoolId={mark}-school-a&tenantId={mark}&classId={mark}-class-a");
    // No implicit school-year dates. Validation reports the missing authoritative year.
    let absent = command(
        meeting(mark, "missing-year", "class-a", "teacher-a", "r0", 0),
        Action::SaveDraft,
        0,
        0,
    );
    check!(absent.valid(), "Synthetic first draft has invalid input");
    let initial = client.apply(&absent, owner, false).await?;
    if initial.0 != 200 {
        return Err(format!(
            "Initial draft failed: {} {}",
            initial.0, initial.1["error"]["code"]
        )
        .into());
    }
    let mut validate = absent.clone();
    validate.action = Action::Validate;
    validate.expected_version = 1;
    validate.operation_id = uuid::Uuid::new_v4().to_string();
    let (_, invalid) = client.apply(&validate, owner, false).await?;
    check!(
        invalid["data"]["valid"] == false
            && invalid["data"]["issues"][0]["code"] == "ACADEMIC_YEAR_MISSING",
        "Missing year accepted"
    );
    for school in ["school-a", "school-b"] {
        check!(
            calendar(
                client,
                mark,
                school,
                "academic_year",
                "year",
                &day_string(-7),
                &day_string(180),
                "save_draft",
                0
            )
            .await?
            .0 == 200,
            "Year draft failed"
        );
        check!(
            calendar(
                client,
                mark,
                school,
                "academic_year",
                "year",
                &day_string(-7),
                &day_string(180),
                "publish",
                1
            )
            .await?
            .0 == 200,
            "Year publish failed"
        );
    }
    // All three conflict dimensions are checked again when simultaneous publishes race.
    for (dimension, day, left, right) in [
        (
            "ROOM_CONFLICT",
            0,
            meeting(mark, "room-left", "class-a", "teacher-a", "room-a", 0),
            meeting(mark, "room-right", "class-b", "teacher-b", "room-a", 0),
        ),
        (
            "CLASS_CONFLICT",
            1,
            meeting(mark, "class-left", "class-a", "teacher-a", "room-b", 1),
            meeting(mark, "class-right", "class-a", "teacher-b", "room-c", 1),
        ),
        (
            "TEACHER_CONFLICT",
            2,
            meeting(mark, "teacher-left", "class-a", "teacher-a", "room-d", 2),
            {
                let mut m = meeting(mark, "teacher-right", "class-c", "teacher-a", "room-e", 2);
                m.school_id = format!("{mark}-school-b");
                m
            },
        ),
    ] {
        let a = client.validated(left, owner).await?;
        let b = client.validated(right, owner_b).await?;
        let (a, b) = tokio::join!(
            client.apply(&a, owner, false),
            client.apply(&b, owner_b, false)
        );
        let a = a?;
        let b = b?;
        check!(
            (a.0 == 200 && b.0 == 409) || (b.0 == 200 && a.0 == 409),
            "Concurrent conflict committed both or denied both"
        );
        let denied = if a.0 == 409 { a.1 } else { b.1 };
        check!(
            denied["error"]["issues"]
                .as_array()
                .is_some_and(|issues| issues.iter().any(|i| i["code"] == dimension)),
            "Wrong conflict dimension"
        );
        let _ = day;
    }
    // Future holiday publication and year removal cannot invalidate published lessons.
    check!(
        calendar(
            client,
            mark,
            "school-a",
            "holiday",
            "blocked-holiday",
            &day_string(0),
            &day_string(0),
            "save_draft",
            0
        )
        .await?
        .0 == 200,
        "Holiday draft failed"
    );
    check!(
        calendar(
            client,
            mark,
            "school-a",
            "holiday",
            "blocked-holiday",
            &day_string(0),
            &day_string(0),
            "publish",
            1
        )
        .await?
        .0 == 409,
        "Holiday invalidated current timetable"
    );
    check!(
        calendar(
            client,
            mark,
            "school-a",
            "academic_year",
            "year",
            &day_string(-7),
            &day_string(180),
            "archive",
            2
        )
        .await?
        .0 == 409,
        "Year archive invalidated timetable"
    );
    // A deterministic learner-visible lesson does not depend on which racing writer won.
    let visible = client
        .validated(
            meeting(
                mark,
                "learner-visible",
                "class-a",
                "teacher-a",
                "room-visible",
                3,
            ),
            owner,
        )
        .await?;
    let (status, published) = client.apply(&visible, owner, false).await?;
    check!(
        status == 200 && published["data"]["publishedVersion"] == 1,
        "Publication failed"
    );
    // A year audience change must also inspect the classes it would exclude.
    let narrowed: crate::education_calendar::model::Change = serde_json::from_value(
        json!({"entry":{"id":"year","schoolId":format!("{mark}-school-a"),"tenantId":mark,"academicYear":"2030/2031","kind":"academic_year","title":"Synthetic education date","description":"","startDate":&day_string(-7),"endDate":&day_string(180),"audience":"sections","sectionIds":[format!("{mark}-class-b")]},"expectedVersion":2,"action":"publish"}),
    )?;
    let mut tx = graph.start_txn().await?;
    let guard = calendar_guard::check(&mut tx, &narrowed).await;
    tx.rollback().await?;
    check!(
        matches!(guard, Err((StatusCode::CONFLICT, _))),
        "Year audience narrowing lost an excluded class"
    );
    let replay = client.apply(&visible, owner, false).await?;
    check!(replay.1 == published, "Immutable publish retry changed");
    let mut audit=graph.execute(query("MATCH(a:LearningClassMeetingAudit {meeting_key:$key}) RETURN a.actor_id AS actor,a.actor_role AS role,a.assigned_teacher_id AS teacher").param("key",visible.meeting.key())).await?;
    let audit = audit.next().await?.ok_or("Publication audit missing")?;
    check!(
        audit.get::<String>("actor")? == format!("{mark}-owner-a")
            && audit.get::<String>("role")? == "owner"
            && audit.get::<String>("teacher")? == format!("{mark}-teacher-a"),
        "Owner principal was replaced by assigned Teacher"
    );
    let mut audits = graph
        .execute(
            query("MATCH(a:LearningTimetableAudit {operation_id:$op}) RETURN count(a) AS count")
                .param(
                    "op",
                    format!("{}|{}", visible.meeting.school_key(), visible.operation_id),
                ),
        )
        .await?;
    check!(
        audits.next().await?.unwrap().get::<i64>("count")? == 1,
        "Idempotent retry duplicated audit"
    );
    let mut altered = visible.clone();
    altered.meeting.title = "Changed replay".into();
    check!(
        client.apply(&altered, owner, false).await?.0 == 409,
        "Altered operation accepted"
    );
    // Half-open adjacent slots are legal.
    let mut adjacent = visible.meeting.clone();
    adjacent.id = "adjacent".into();
    adjacent.starts_at = adjacent.ends_at;
    adjacent.ends_at += 3_600_000;
    let adjacent = client.validated(adjacent, owner).await?;
    check!(
        client.apply(&adjacent, owner, false).await?.0 == 200,
        "Adjacent slot conflicted"
    );
    calendar_eligibility(client, mark, owner).await?;
    legacy_compatibility_and_cas(graph, client, mark, owner).await?;
    // The normal Teacher receives only self as an eligible Teacher and cannot impersonate another.
    let (status, context) = client
        .get(
            &format!("/api/v1/sis-service/learning/timetable/context?{scope}"),
            &teacher,
        )
        .await?;
    check!(
        status == 200
            && context["data"]["teachers"]
                .as_array()
                .is_some_and(|a| a.len() == 1)
            && context["data"]["actor"]["role"] == "teacher",
        "Teacher context widened"
    );
    let (status, context) = client
        .get(
            &format!("/api/v1/sis-service/learning/class-meetings/teachers?{scope}"),
            &bearer_owner,
        )
        .await?;
    check!(
        status == 200
            && context["data"]["teachers"]
                .as_array()
                .is_some_and(|a| a.len() == 2)
            && context["data"]["actor"]["role"] == "owner",
        "Owner teacher selection missing"
    );
    let own = command(
        meeting(
            mark,
            "teacher-own-draft",
            "class-a",
            "teacher-a",
            "own-room",
            4,
        ),
        Action::SaveDraft,
        0,
        0,
    );
    check!(
        client.apply(&own, &teacher, true).await?.0 == 200,
        "Own Teacher draft rejected"
    );
    let mut foreign = own.clone();
    foreign.meeting.id = "teacher-foreign".into();
    foreign.meeting.teacher_id = format!("{mark}-teacher-b");
    foreign.operation_id = uuid::Uuid::new_v4().to_string();
    check!(
        client.apply(&foreign, &teacher, true).await?.0 == 403,
        "Teacher assigned someone else"
    );
    // Student/Parent/Teacher snapshots use only authoritative published records.
    let student = family_token(mark, "student");
    let parent = family_token(mark, "parent");
    let date = day_string(3);
    for (path,token) in [
        (format!("/api/v1/sis-service/learning/owner/student/calendar?school_id={mark}-school-a&tenant_id={mark}&from={date}&to={date}"),student.clone()),
        (format!("/api/leads/v1/me/school/calendar?studentId={mark}-student&from={date}&to={date}"),parent.clone()),
        (format!("/api/leads/v1/teacher/school/classes/{mark}-class-a/calendar?from={date}&to={date}"),teacher.clone()),
    ] {
        let (status,data)=client.get(&path,&token).await?;
        check!(status==200 && data["data"]["classMeetings"]["items"].as_array().is_some_and(|a|a.iter().any(|m|m["id"]=="learner-visible") && a.iter().all(|m|m["id"]=="learner-visible" || m["id"]=="adjacent")),"Calendar window ignored authoritative lesson date");
    }
    check!(
        client
            .get(
                &format!("/api/v1/sis-service/learning/timetable?{scope}"),
                &student
            )
            .await?
            .0
            == 403,
        "Student accessed drafts"
    );
    for (path,token,collection) in [
        (format!("/api/v1/sis-service/learning/class-meetings?{scope}"),student.clone(),"/data/items"),
        (format!("/api/v1/sis-service/learning/owner/student/school?school_id={mark}-school-a&tenant_id={mark}"),student,"/data/classMeetings/items"),
        ("/api/leads/v1/me/school".into(),parent,"/data/classMeetings/items"),
        (format!("/api/leads/v1/teacher/school/classes/{mark}-class-a"),teacher.clone(),"/data/classMeetings/items"),
    ] {
        let (status,data)=client.get(&path,&token).await?;check!(status==200,"Current published consumer rejected");
        let items=data.pointer(collection).and_then(Value::as_array).ok_or("Published collection missing")?;
        check!(items.iter().any(|m|m["id"]=="learner-visible"),"Publication missing from consumer");
        check!(!items.iter().any(|m|m["id"]=="teacher-own-draft"),"Draft leaked to consumer");
    }
    // Existing freeze cannot erase resource metadata or bypass creation lifecycle.
    let m = &visible.meeting;
    let mut freeze = serde_json::to_value(m)?;
    freeze.as_object_mut().unwrap().remove("academicYear");
    freeze.as_object_mut().unwrap().remove("roomId");
    freeze["expectedVersion"] = json!(1);
    freeze["freeze"] = json!(true);
    let (status, frozen) = client
        .post(
            "/api/v1/sis-service/learning/class-meetings",
            &bearer_owner,
            &freeze,
        )
        .await?;
    check!(
        status == 200
            && frozen["data"]["frozen"] == true
            && frozen["data"]["roomId"] == "room-visible"
            && frozen["data"]["academicYear"] == "2030/2031",
        "Freeze changed publication metadata"
    );
    freeze["freeze"] = json!(false);
    check!(
        client
            .post(
                "/api/v1/sis-service/learning/class-meetings",
                &teacher,
                &freeze
            )
            .await?
            .1["error"]["code"]
            == "DRAFT_REQUIRED",
        "Legacy publish bypass open"
    );
    let edit = command(m.clone(), Action::SaveDraft, 3, 2);
    check!(
        client.apply(&edit, owner, false).await?.0 == 409,
        "Frozen publication edited"
    );
    // Queued operations must recheck current Auth after a held school resource lock.
    let mut lock = graph.start_txn().await?;
    let school = format!("{mark}-school-a");
    lock.run(
        query("MATCH(l:EducationCalendarLock {key:$key}) SET l.fixture_queue=1")
            .param("key", format!("{mark}|{school}")),
    )
    .await?;
    let queued = command(
        meeting(
            mark,
            "queued-revoked",
            "class-a",
            "teacher-a",
            "queued-room",
            4,
        ),
        Action::SaveDraft,
        0,
        0,
    );
    let mut pending = Box::pin(client.apply(&queued, owner, false));
    check!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut pending)
            .await
            .is_err(),
        "Queued write did not wait"
    );
    let control = std::env::var("SIS_TIMETABLE_TEST_AUTH_CONTROL")?;
    client
        .http
        .post(&control)
        .json(&json!({"active":false}))
        .send()
        .await?
        .error_for_status()?;
    lock.commit().await?;
    check!(pending.await?.0 == 401, "Queued revoked Auth accepted");
    client
        .http
        .post(&control)
        .json(&json!({"active":true}))
        .send()
        .await?
        .error_for_status()?;
    let mut rows=graph.execute(query("MATCH(d:LearningTimetableDraft {id:'queued-revoked',school_id:$school}) RETURN count(d) AS count").param("school",school)).await?;
    check!(
        rows.next().await?.unwrap().get::<i64>("count")? == 0,
        "Denied draft committed"
    );
    let mut evidence=graph.execute(query("MATCH(n) WHERE (n:LearningTimetableOperation AND n.key=$op) OR (n:LearningTimetableAudit AND n.operation_id=$op) RETURN count(n) AS count").param("op",format!("{}|{}",queued.meeting.school_key(),queued.operation_id))).await?;
    check!(
        evidence.next().await?.unwrap().get::<i64>("count")? == 0,
        "Denied queued write committed operation or audit"
    );
    Ok(())
}
async fn calendar_eligibility(client: &Client, mark: &str, owner: &str) -> Result<(), Error> {
    for (action, version) in [("save_draft", 0), ("publish", 1)] {
        check!(
            calendar(
                client,
                mark,
                "school-a",
                "holiday",
                "actual-holiday",
                &day_string(7),
                &day_string(7),
                action,
                version
            )
            .await?
            .0 == 200,
            "Applicable holiday setup failed"
        );
    }
    for (id, day, code) in [
        ("holiday-draft", 7, "SCHOOL_HOLIDAY"),
        ("outside-year", 185, "OUTSIDE_ACADEMIC_YEAR"),
    ] {
        let mut c = command(
            meeting(mark, id, "class-a", "teacher-a", "calendar-room", day),
            Action::SaveDraft,
            0,
            0,
        );
        check!(
            client.apply(&c, owner, false).await?.0 == 200,
            "Calendar negative draft failed"
        );
        c.action = Action::Validate;
        c.expected_version = 1;
        c.operation_id = uuid::Uuid::new_v4().to_string();
        let (status, result) = client.apply(&c, owner, false).await?;
        check!(
            status == 200
                && result["data"]["valid"] == false
                && result["data"]["version"] == 1
                && result["data"]["publishedVersion"] == 0
                && result["data"]["issues"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|i| i["code"] == code)),
            "Calendar eligibility or invalid validation CAS failed"
        );
    }
    Ok(())
}
async fn legacy_compatibility_and_cas(
    graph: &Graph,
    client: &Client,
    mark: &str,
    owner: &str,
) -> Result<(), Error> {
    let first = command(
        meeting(
            mark,
            "same-record-cas",
            "class-a",
            "teacher-a",
            "cas-room",
            4,
        ),
        Action::SaveDraft,
        0,
        0,
    );
    let mut other = first.clone();
    other.operation_id = uuid::Uuid::new_v4().to_string();
    other.meeting.title = "Concurrent draft".into();
    let (a, b) = tokio::join!(
        client.apply(&first, owner, false),
        client.apply(&other, owner, false)
    );
    let (a, b) = (a?, b?);
    check!(
        (a.0 == 200 && b.0 == 409) || (b.0 == 200 && a.0 == 409),
        "Simultaneous first-save CAS admitted both"
    );
    let m = meeting(
        mark,
        "legacy-publication",
        "class-a",
        "teacher-a",
        "explicit-legacy-room",
        8,
    );
    let mut old = serde_json::to_value(&m)?;
    old.as_object_mut().unwrap().remove("roomId");
    old.as_object_mut().unwrap().remove("academicYear");
    old["version"] = json!(1);
    old["frozen"] = json!(false);
    graph.run(query("CREATE(:LearningClassMeeting {key:$key,id:$id,school_id:$school,tenant_id:$tenant,class_id:$class,teacher_id:$teacher,starts_at:$starts,version:1,payload:$payload})").param("key",m.key()).param("id",m.id.clone()).param("school",m.school_id.clone()).param("tenant",m.tenant_id.clone()).param("class",m.class_id.clone()).param("teacher",m.teacher_id.clone()).param("starts",m.starts_at as i64).param("payload",old.to_string())).await?;
    let scope = format!("schoolId={mark}-school-a&tenantId={mark}&classId={mark}-class-a");
    let (_, listed) = client
        .get(
            &format!("/api/v1/sis-service/learning/class-meetings?{scope}"),
            &family_token(mark, "student"),
        )
        .await?;
    check!(
        listed["data"]["items"]
            .as_array()
            .is_some_and(|a| a.iter().any(|i| i == &old)),
        "Legacy publication was rewritten or hidden"
    );
    let mut c = command(m.clone(), Action::SaveDraft, 0, 1);
    check!(
        client.apply(&c, owner, false).await?.0 == 200,
        "Explicit legacy edit failed"
    );
    let mut rows = graph
        .execute(
            query("MATCH(p:LearningClassMeeting {key:$key}) RETURN p.payload AS payload")
                .param("key", m.key()),
        )
        .await?;
    check!(
        rows.next().await?.unwrap().get::<String>("payload")? == old.to_string(),
        "Saving legacy draft changed publication"
    );
    c.action = Action::Validate;
    c.expected_version = 1;
    c.operation_id = uuid::Uuid::new_v4().to_string();
    check!(
        client.apply(&c, owner, false).await?.1["data"]["valid"] == true,
        "Explicit legacy validation failed"
    );
    c.action = Action::Publish;
    c.expected_version = 2;
    c.operation_id = uuid::Uuid::new_v4().to_string();
    check!(
        client.apply(&c, owner, false).await?.1["data"]["publishedVersion"] == 2,
        "Explicit legacy publish failed"
    );
    c.action = Action::Archive;
    c.expected_version = 3;
    c.expected_published_version = 2;
    c.operation_id = uuid::Uuid::new_v4().to_string();
    let (status, receipt) = client.apply(&c, owner, false).await?;
    check!(
        status == 200
            && receipt["data"]["status"] == "archived"
            && receipt["data"]["publishedVersion"] == 3,
        "Authoritative publication archive failed"
    );
    let (_, listed) = client
        .get(
            &format!("/api/v1/sis-service/learning/class-meetings?{scope}"),
            &family_token(mark, "student"),
        )
        .await?;
    check!(
        listed["data"]["items"]
            .as_array()
            .is_some_and(|a| a.iter().all(|i| i["id"] != "legacy-publication")),
        "Archived publication still shown"
    );
    Ok(())
}
