use super::model::{Command, Commands, PlanChange, PlanCopy, PlanRow};
use super::repository::{self as repo, Failure, Session, Writer};
use neo4rs::{query, Graph};
use serde_json::Value;

fn writer<'a>(actor: &'a str, role: &'a str, command: &'a str) -> Writer<'a> {
    Writer {
        session: Session::Unchecked,
        actor,
        role,
        school: "school",
        tenant: "tenant",
        command,
    }
}
fn code(result: Result<Value, Failure>) -> String {
    let (_, body) = result.expect_err("refused");
    body.0["error"]["code"].as_str().unwrap().to_string()
}
fn row(subject: &str, name: &str, framework: &str, periods: u32, required: bool) -> PlanRow {
    PlanRow {
        subject: subject.into(),
        name: name.into(),
        framework: framework.into(),
        periods_per_week: periods,
        required,
        status: "active".into(),
    }
}
fn plan(grade: &str, version: u32, rows: Vec<PlanRow>) -> PlanChange {
    PlanChange {
        tenant_id: "tenant".into(),
        school_id: "school".into(),
        academic_year: "2026/2027".into(),
        grade: grade.into(),
        expected_version: version,
        rows,
    }
}
fn cmd(
    action: &str,
    section: &str,
    subject: &str,
    staff: &str,
    periods: Option<u32>,
    version: u32,
) -> Command {
    Command {
        action: action.into(),
        section_id: section.into(),
        subject: subject.into(),
        staff_member_id: staff.into(),
        periods,
        expected_version: version,
    }
}
fn batch(commands: Vec<Command>) -> Commands {
    Commands {
        tenant_id: "tenant".into(),
        school_id: "school".into(),
        academic_year: "2026/2027".into(),
        commands,
    }
}
async fn count(g: &Graph, cypher: &str) -> i64 {
    let mut rows = g.execute(query(cypher)).await.unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>("n").unwrap()
}
async fn grant(g: &Graph, key: &str) -> Option<(String, i64)> {
    let mut rows = g
        .execute(query("MATCH(g:LearningTeachingGrant {key:$key}) RETURN g.status AS status,g.version AS version").param("key", key))
        .await
        .unwrap();
    rows.next()
        .await
        .unwrap()
        .map(|r| (r.get("status").unwrap(), r.get("version").unwrap()))
}

#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn subject_plans_and_assignments_are_revisioned_audited_and_grant_learning_access() {
    let uri = std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap();
    assert_eq!(uri, "127.0.0.1:3223");
    let g = Graph::new(uri, "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
    repo::init(&g).await.unwrap();
    repo::init(&g).await.unwrap();
    for statement in [
        "CREATE(:School {school_id:'school',tenant_id:'tenant'}) CREATE(:School {school_id:'school-2',tenant_id:'tenant'})",
        "CREATE(:StaffMember {id:'admin',membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'owner',membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[]}) CREATE(:StaffMember {id:'elsewhere',membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['school-2'],tenantIds:['tenant']})",
        "CREATE(:StaffMember {id:'t1',displayName:'Siti Rahmawati',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t2',displayName:'Budi Santoso',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t3',displayName:'Dewi Lestari',membershipStatus:'DISABLED',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t4',displayName:'Rina Kusuma',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school-2'],tenantIds:['tenant']})",
        "CREATE(:Section {section_id:'7A',name:'7A',year_group:'Year 7',academic_year:'2026/2027',school_id:'school',tenant_id:'tenant',status:'active',homeroom_staff_member_id:'t1'}) CREATE(:Section {section_id:'7B',name:'7B',year_group:'Year 7',academic_year:'2026/2027',school_id:'school',tenant_id:'tenant',status:'active'}) CREATE(:Section {section_id:'8A',name:'8A',year_group:'Year 8',academic_year:'2026/2027',school_id:'school',tenant_id:'tenant',status:'active'}) CREATE(:Section {section_id:'OLD-7A',name:'7A',year_group:'Year 7',academic_year:'2025/2026',school_id:'school',tenant_id:'tenant',status:'active'})",
        "CREATE(:LearningTeachingGrant {key:'t2|8A',staff_member_id:'t2',section_id:'8A',school_id:'school',tenant_id:'tenant',status:'ACTIVE',version:1})",
        "CREATE(:AcademicSubjectPlan {key:'tenant|school|2025/2026|Year 8',tenant_id:'tenant',school_id:'school',academic_year:'2025/2026',grade:'Year 8',version:2,payload:'[{\"subject\":\"mathematics\",\"name\":\"Matematika\",\"framework\":\"merdeka\",\"periodsPerWeek\":5,\"required\":true,\"status\":\"active\"},{\"subject\":\"computing\",\"name\":\"Cambridge Computing\",\"framework\":\"cambridge\",\"periodsPerWeek\":2,\"required\":false,\"status\":\"archived\"}]',subjects:['mathematics','computing'],active_subjects:['mathematics']})",
    ] {
        g.run(query(statement)).await.unwrap();
    }

    // Subject plan: created at revision 0, replayed by key, refused on reuse and stale revisions.
    let year7 = plan(
        "Year 7",
        0,
        vec![
            row("mathematics", "Matematika", "merdeka", 4, true),
            row(
                "science",
                "Cambridge Lower Secondary Science",
                "cambridge",
                4,
                true,
            ),
            row("quran", "Tahfiz Al-Qur'an", "iiec", 2, false),
        ],
    );
    let saved = repo::save_plan(
        &g,
        &writer("admin", "school_admin", "plan-year7-0001"),
        &year7,
    )
    .await
    .unwrap();
    assert_eq!(saved["version"], 1);
    assert_eq!(saved["rows"][1]["framework"], "cambridge");
    let replay = repo::save_plan(
        &g,
        &writer("admin", "school_admin", "plan-year7-0001"),
        &year7,
    )
    .await
    .unwrap();
    assert_eq!(replay, saved);
    let reused = plan(
        "Year 7",
        0,
        vec![row("art", "Seni Budaya", "merdeka", 2, false)],
    );
    assert_eq!(
        code(
            repo::save_plan(
                &g,
                &writer("admin", "school_admin", "plan-year7-0001"),
                &reused
            )
            .await
        ),
        "IDEMPOTENCY_KEY_REUSED"
    );
    assert_eq!(
        code(
            repo::save_plan(
                &g,
                &writer("admin", "school_admin", "plan-year7-0002"),
                &year7
            )
            .await
        ),
        "REVISION_CONFLICT"
    );
    for (actor, role) in [("elsewhere", "school_admin"), ("t1", "school_admin")] {
        assert_eq!(
            code(
                repo::save_plan(
                    &g,
                    &writer(actor, role, "plan-forbidden-01"),
                    &plan("Year 7", 1, vec![])
                )
                .await
            ),
            "FORBIDDEN"
        );
    }

    // Copy takes the active rows of the same grade from another year, under the revision rule.
    let copy = |from: &str, version: u32| PlanCopy {
        tenant_id: "tenant".into(),
        school_id: "school".into(),
        academic_year: "2026/2027".into(),
        grade: "Year 8".into(),
        from_year: from.into(),
        expected_version: version,
    };
    assert_eq!(
        code(
            repo::copy_plan(
                &g,
                &writer("owner", "owner", "copy-year8-none"),
                &copy("2024/2025", 0)
            )
            .await
        ),
        "NOT_FOUND"
    );
    let copied = repo::copy_plan(
        &g,
        &writer("owner", "owner", "copy-year8-0001"),
        &copy("2025/2026", 0),
    )
    .await
    .unwrap();
    assert_eq!(copied["version"], 1);
    assert_eq!(copied["rows"].as_array().unwrap().len(), 1);
    assert_eq!(copied["rows"][0]["subject"], "mathematics");
    assert_eq!(
        code(
            repo::copy_plan(
                &g,
                &writer("owner", "owner", "copy-year8-0002"),
                &copy("2025/2026", 0)
            )
            .await
        ),
        "REVISION_CONFLICT"
    );

    // Assignments: team teaching splits science in 7A; each assignment grants the class.
    let first = repo::run_commands(
        &g,
        &writer("admin", "school_admin", "assign-7a-0001"),
        &batch(vec![
            cmd("assign", "7A", "mathematics", "t1", Some(4), 0),
            cmd("assign", "7A", "science", "t1", Some(2), 0),
            cmd("assign", "7A", "science", "t2", Some(2), 0),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(first["assignments"].as_array().unwrap().len(), 3);
    assert_eq!(first["assignments"][0]["version"], 1);
    assert_eq!(first["grants"].as_array().unwrap().len(), 2);
    assert_eq!(grant(&g, "t1|7A").await, Some(("ACTIVE".into(), 1)));
    assert_eq!(grant(&g, "t2|7A").await, Some(("ACTIVE".into(), 1)));
    assert_eq!(
        count(&g, "MATCH(a:LearningAccessAudit {source:'teaching_assignment',status:'ACTIVE'}) RETURN count(a) AS n").await,
        2
    );

    // A refused command leaves the whole batch unwritten and names its index.
    for (bad, expected) in [
        (
            cmd("assign", "7B", "mathematics", "t3", Some(4), 0),
            "TEACHER_NOT_ELIGIBLE",
        ),
        (
            cmd("assign", "7B", "mathematics", "t4", Some(4), 0),
            "TEACHER_NOT_ELIGIBLE",
        ),
        (
            cmd("assign", "7B", "art", "t1", Some(2), 0),
            "SUBJECT_NOT_IN_PLAN",
        ),
        (
            cmd("assign", "OLD-7A", "mathematics", "t1", Some(4), 0),
            "CLASS_NOT_ACTIVE",
        ),
        (
            cmd("assign", "7A", "mathematics", "t1", Some(4), 0),
            "REVISION_CONFLICT",
        ),
    ] {
        let refused = repo::run_commands(
            &g,
            &writer(
                "admin",
                "school_admin",
                &format!("refused-{expected}-{}", bad.staff_member_id),
            ),
            &batch(vec![cmd("assign", "7B", "science", "t2", Some(4), 0), bad]),
        )
        .await;
        let (_, body) = refused.expect_err("refused");
        assert_eq!(body.0["error"]["code"], expected);
        assert_eq!(body.0["error"]["index"], 1);
    }
    assert_eq!(
        count(
            &g,
            "MATCH(a:TeachingAssignment {section_id:'7B'}) RETURN count(a) AS n"
        )
        .await,
        0
    );
    assert_eq!(grant(&g, "t2|7B").await, None);

    // Hours change in place; ending one of two subjects keeps the grant, ending the last revokes it.
    let changed = repo::run_commands(
        &g,
        &writer("admin", "school_admin", "hours-7a-0001"),
        &batch(vec![cmd(
            "change_hours",
            "7A",
            "mathematics",
            "t1",
            Some(5),
            1,
        )]),
    )
    .await
    .unwrap();
    assert_eq!(changed["assignments"][0]["periods"], 5);
    assert_eq!(changed["assignments"][0]["version"], 2);
    let ended = repo::run_commands(
        &g,
        &writer("owner", "owner", "end-7a-math-01"),
        &batch(vec![cmd("end", "7A", "mathematics", "t1", None, 2)]),
    )
    .await
    .unwrap();
    assert_eq!(ended["assignments"][0]["status"], "ended");
    assert_eq!(ended["grants"].as_array().unwrap().len(), 0);
    assert_eq!(grant(&g, "t1|7A").await, Some(("ACTIVE".into(), 1)));
    let last = repo::run_commands(
        &g,
        &writer("owner", "owner", "end-7a-sci-001"),
        &batch(vec![cmd("end", "7A", "science", "t1", None, 1)]),
    )
    .await
    .unwrap();
    assert_eq!(last["grants"][0]["status"], "REVOKED");
    assert_eq!(grant(&g, "t1|7A").await, Some(("REVOKED".into(), 2)));
    // Reopening an ended assignment at its revision grants the class again.
    let reopened = repo::run_commands(
        &g,
        &writer("admin", "school_admin", "reopen-7a-math"),
        &batch(vec![cmd("assign", "7A", "mathematics", "t1", Some(4), 3)]),
    )
    .await
    .unwrap();
    assert_eq!(reopened["assignments"][0]["version"], 4);
    assert_eq!(grant(&g, "t1|7A").await, Some(("ACTIVE".into(), 3)));

    // A subject with an active assignment may be archived, not removed.
    let without_science = plan(
        "Year 7",
        1,
        vec![row("mathematics", "Matematika", "merdeka", 4, true)],
    );
    let (_, body) = repo::save_plan(
        &g,
        &writer("admin", "school_admin", "plan-year7-0003"),
        &without_science,
    )
    .await
    .expect_err("in use");
    assert_eq!(body.0["error"]["code"], "SUBJECT_IN_USE");
    assert_eq!(body.0["error"]["subjects"], serde_json::json!(["science"]));
    let mut archived = year7.rows.clone();
    archived[1].status = "archived".into();
    let saved = repo::save_plan(
        &g,
        &writer("admin", "school_admin", "plan-year7-0004"),
        &plan("Year 7", 1, archived),
    )
    .await
    .unwrap();
    assert_eq!(saved["version"], 2);
    // An archived subject takes no new assignment.
    assert_eq!(
        code(
            repo::run_commands(
                &g,
                &writer("admin", "school_admin", "assign-archived"),
                &batch(vec![cmd("assign", "7B", "science", "t2", Some(4), 0)])
            )
            .await
        ),
        "SUBJECT_NOT_IN_PLAN"
    );

    // Every change is audited with its before and after state; receipts make replays free.
    assert_eq!(
        count(
            &g,
            "MATCH(a:AcademicStaffingAudit {kind:'subject_plan'}) RETURN count(a) AS n"
        )
        .await,
        3
    );
    assert_eq!(
        count(
            &g,
            "MATCH(a:AcademicStaffingAudit {kind:'teaching_assignment'}) RETURN count(a) AS n"
        )
        .await,
        7
    );
    assert_eq!(
        count(&g, "MATCH(a:AcademicStaffingAudit {kind:'teaching_assignment',action:'change_hours'}) WHERE a.before CONTAINS '\"periods\":4' AND a.after CONTAINS '\"periods\":5' RETURN count(a) AS n").await,
        1
    );

    // Reads.
    let plans = repo::plans(&g, "school", "tenant", "2026/2027")
        .await
        .unwrap();
    let grades: Vec<&str> = plans["grades"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["grade"].as_str().unwrap())
        .collect();
    assert_eq!(grades, vec!["Year 7", "Year 8"]);
    assert_eq!(plans["grades"][0]["classes"], 2);
    assert_eq!(plans["grades"][0]["plan"]["version"], 2);
    assert_eq!(plans["sources"][0]["academicYear"], "2025/2026");
    assert_eq!(plans["sources"][0]["grades"], serde_json::json!(["Year 8"]));
    let overview = repo::overview(&g, "school", "tenant", "2026/2027", "", "")
        .await
        .unwrap();
    assert_eq!(overview["sections"].as_array().unwrap().len(), 3);
    assert_eq!(
        overview["sections"][0]["homeroom"]["name"],
        "Siti Rahmawati"
    );
    assert_eq!(overview["sections"][1]["homeroom"], Value::Null);
    let statuses: Vec<String> = overview["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            format!(
                "{}:{}:{}:{}",
                a["subject"], a["staffMemberId"], a["status"], a["version"]
            )
        })
        .collect();
    assert_eq!(
        statuses,
        vec![
            "\"mathematics\":\"t1\":\"active\":4",
            "\"science\":\"t1\":\"ended\":2",
            "\"science\":\"t2\":\"active\":1"
        ]
    );
    let names: Vec<&str> = overview["teachers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Budi Santoso", "Siti Rahmawati"]);
    let mine = repo::overview(&g, "school", "tenant", "2026/2027", "", "t2")
        .await
        .unwrap();
    assert_eq!(mine["assignments"].as_array().unwrap().len(), 1);

    // The teacher list reads the same plans and assignments.
    let (from, to) = crate::academic_teachers::model::window_ms(
        chrono::NaiveDate::from_ymd_opt(2026, 10, 26).unwrap(),
    );
    let s = crate::academic_teachers::repository::snapshot(
        &g,
        "school",
        "tenant",
        "2026/2027",
        from,
        to,
    )
    .await
    .unwrap();
    assert_eq!(s.assignments.len(), 2);
    assert_eq!(s.plans.len(), 3);
    let r = crate::academic_teachers::model::report(&s).unwrap();
    let unassigned: Vec<(&str, &str)> = r
        .subject_warnings
        .iter()
        .map(|w| (w.class.section_id.as_str(), w.subject.as_str()))
        .collect();
    assert_eq!(
        unassigned,
        vec![("7B", "mathematics"), ("8A", "mathematics")]
    );
    let budi = r
        .teachers
        .iter()
        .find(|t| t.staff_member_id == "t2")
        .unwrap();
    assert_eq!(budi.planned_periods, 2);
}

fn weights(body: &str) -> super::weights::Change {
    serde_json::from_str(&format!(
        r#"{{"tenantId":"tenant","schoolId":"school","academicYear":"2026/2027",{body}}}"#
    ))
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn grade_weights_resolve_subject_then_school_then_default_and_are_audited() {
    let uri = std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap();
    assert_eq!(uri, "127.0.0.1:3223");
    let g = Graph::new(uri, "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
    repo::init(&g).await.unwrap();
    for statement in [
        "CREATE(:School {school_id:'school',tenant_id:'tenant'})",
        "CREATE(:StaffMember {id:'admin',membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t1',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'parent-ish',membershipStatus:'ACTIVE',roles:['finance'],schoolIds:['school'],tenantIds:['tenant']})",
        "CREATE(:Section {section_id:'7A',name:'7A',year_group:'Year 7',academic_year:'2026/2027',school_id:'school',tenant_id:'tenant',status:'active'})",
        "CREATE(:AcademicSubjectPlan {key:'tenant|school|2026/2027|Year 7',tenant_id:'tenant',school_id:'school',academic_year:'2026/2027',grade:'Year 7',version:1,payload:'[]',subjects:['mathematics','art'],active_subjects:['mathematics']})",
    ] {
        g.run(query(statement)).await.unwrap();
    }
    let read = |actor: &'static str, subject: &'static str| {
        let g = g.clone();
        async move {
            repo::weights::for_section(&g, actor, "school", "tenant", "7A", subject, "semester_1")
                .await
        }
    };
    // Nothing saved: the built-in 20/20/20/15/25 and KKM 75.
    let builtin = read("t1", "mathematics").await.unwrap();
    assert_eq!(
        (builtin["source"].clone(), builtin["passMark"].clone()),
        (Value::from("default"), Value::from(75))
    );
    assert_eq!(builtin["weights"]["pas"], 25);
    assert_eq!(builtin["grade"], "Year 7");
    assert_eq!(code(read("parent-ish", "mathematics").await), "FORBIDDEN");

    let set = r#""set":{"weights":{"chapterQuiz":10,"quiz":20,"assignment":30,"pts":15,"pas":25},"passMark":70}"#;
    let school = repo::weights::save(
        &g,
        &writer("admin", "school_admin", "weights-school-01"),
        &weights(&format!(r#""expectedVersion":0,{set}"#)),
    )
    .await
    .unwrap();
    assert_eq!(school["version"], 1);
    let by_school = read("t1", "mathematics").await.unwrap();
    assert_eq!(by_school["source"], "school");
    assert_eq!(by_school["weights"]["assignment"], 30);
    assert_eq!(by_school["passMark"], 70);

    let subject = r#""grade":"Year 7","subject":"mathematics","term":"semester_1""#;
    let override_set = r#""set":{"weights":{"chapterQuiz":20,"quiz":20,"assignment":20,"pts":20,"pas":20},"passMark":80}"#;
    assert_eq!(
        code(
            repo::weights::save(
                &g,
                &writer("admin", "school_admin", "weights-art-0001"),
                &weights(&format!(
                    r#""grade":"Year 7","subject":"art","term":"semester_1","expectedVersion":0,{override_set}"#
                )),
            )
            .await
        ),
        "SUBJECT_NOT_IN_PLAN"
    );
    let saved = repo::weights::save(
        &g,
        &writer("admin", "school_admin", "weights-math-001"),
        &weights(&format!(r#"{subject},"expectedVersion":0,{override_set}"#)),
    )
    .await
    .unwrap();
    assert_eq!(
        (saved["version"].clone(), saved["status"].clone()),
        (Value::from(1), Value::from("active"))
    );
    let by_subject = read("t1", "mathematics").await.unwrap();
    assert_eq!(
        (
            by_subject["source"].clone(),
            by_subject["version"].clone(),
            by_subject["passMark"].clone()
        ),
        (Value::from("subject"), Value::from(1), Value::from(80))
    );
    // Another term of the same subject still uses the school default.
    let other_term = repo::weights::for_section(
        &g,
        "t1",
        "school",
        "tenant",
        "7A",
        "mathematics",
        "semester_2",
    )
    .await
    .unwrap();
    assert_eq!(other_term["source"], "school");
    assert_eq!(
        code(
            repo::weights::save(
                &g,
                &writer("admin", "school_admin", "weights-math-002"),
                &weights(&format!(r#"{subject},"expectedVersion":0,{override_set}"#)),
            )
            .await
        ),
        "REVISION_CONFLICT"
    );
    let cleared = repo::weights::save(
        &g,
        &writer("admin", "school_admin", "weights-math-003"),
        &weights(&format!(r#"{subject},"expectedVersion":1,"clear":true"#)),
    )
    .await
    .unwrap();
    assert_eq!(
        (cleared["version"].clone(), cleared["status"].clone()),
        (Value::from(2), Value::from("cleared"))
    );
    assert_eq!(read("t1", "mathematics").await.unwrap()["source"], "school");

    let listed = repo::weights::list(&g, "school", "tenant", "2026/2027")
        .await
        .unwrap();
    assert_eq!(listed["school"]["weights"]["chapterQuiz"], 10);
    assert_eq!(listed["subjects"][0]["status"], "cleared");
    assert_eq!(listed["subjects"][0]["weights"], Value::Null);
    assert_eq!(listed["builtin"]["passMark"], 75);
    assert_eq!(
        count(
            &g,
            "MATCH(a:AcademicStaffingAudit {kind:'grade_weights'}) RETURN count(a) AS n"
        )
        .await,
        3
    );
    assert_eq!(
        count(&g, "MATCH(a:AcademicStaffingAudit {kind:'grade_weights',action:'clear'}) WHERE a.after='null' RETURN count(a) AS n").await,
        1
    );
}
