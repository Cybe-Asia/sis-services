use super::model::{Change, Row, ARCHIVED};
use super::repository::{self as repo, NOMENCLATURE_MANAGE};
use crate::academic_staffing::repository::{Failure, Session, Writer};
use neo4rs::{query, Graph};
use serde_json::Value;

fn writer<'a>(actor: &'a str, role: &'a str, school: &'a str, command: &'a str) -> Writer<'a> {
    Writer {
        session: Session::Unchecked,
        actor,
        role,
        school,
        tenant: "tenant",
        command,
        capability: NOMENCLATURE_MANAGE,
    }
}
fn code(result: Result<Value, Failure>) -> (u16, String, Value) {
    let (status, body) = result.expect_err("refused");
    (
        status.as_u16(),
        body.0["error"]["code"].as_str().unwrap().to_string(),
        body.0["error"].clone(),
    )
}
async fn count(g: &Graph, cypher: &str) -> i64 {
    let mut rows = g.execute(query(cypher)).await.unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>("n").unwrap()
}
fn kind<'a>(read: &'a Value, name: &str) -> &'a Value {
    read["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["kind"] == name)
        .unwrap()
}
fn rows_of(kind: &Value) -> Vec<Row> {
    kind["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let mut r = r.clone();
            let o = r.as_object_mut().unwrap();
            o.remove("version");
            o.retain(|_, v| !v.is_null());
            serde_json::from_value(r).unwrap()
        })
        .collect()
}
fn change(school: &str, version: u32, rows: Vec<Row>) -> Change {
    Change {
        tenant_id: "tenant".into(),
        school_id: school.into(),
        expected_version: version,
        rows,
    }
}

#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn nomenclature_seeds_per_school_and_saves_are_revisioned_audited_and_capability_checked() {
    let uri = std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap();
    assert_eq!(uri, "127.0.0.1:3223");
    let g = Graph::new(uri, "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
    repo::init(&g).await.unwrap();
    repo::init(&g).await.unwrap();
    // The school lock's uniqueness comes from the timetable schema.
    crate::learning_access::timetable::repository::migrate(&g)
        .await
        .unwrap();
    for statement in [
        "CREATE(:School {school_id:'SCH-IISS',tenant_id:'tenant',school_name:'International Islamic Secondary School'}) CREATE(:School {school_id:'SCH-IIHS',tenant_id:'tenant',school_name:'International Islamic High School'})",
        "CREATE(:StaffMember {id:'admin',membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['SCH-IISS'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'owner',membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[]}) CREATE(:StaffMember {id:'elsewhere',membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['SCH-IIHS'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'teacher',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['SCH-IISS'],tenantIds:['tenant']})",
        "CREATE(:Section {section_id:'7A',year_group:'Year 7',academic_year:'2025/2026',school_id:'SCH-IISS',tenant_id:'tenant',status:'active'}) CREATE(:Section {section_id:'7B',year_group:'Grade 7',academic_year:'2026',school_id:'SCH-IISS',tenant_id:'tenant',status:'active'})",
        "CREATE(:AcademicSubjectPlan {key:'tenant|SCH-IISS|2026/2027|Year 7',tenant_id:'tenant',school_id:'SCH-IISS',academic_year:'2026/2027',grade:'Year 7',version:1,payload:'[{\"subject\":\"mathematics\",\"name\":\"Matematika\",\"framework\":\"merdeka\",\"periodsPerWeek\":5,\"required\":true,\"status\":\"active\"},{\"subject\":\"computing\",\"name\":\"Cambridge Computing\",\"framework\":\"cambridge\",\"periodsPerWeek\":2,\"required\":false,\"status\":\"active\"},{\"subject\":\"history\",\"name\":\"Sejarah\",\"framework\":\"merdeka\",\"periodsPerWeek\":2,\"required\":false,\"status\":\"archived\"}]',subjects:['mathematics','computing','history'],active_subjects:['mathematics','computing']})",
    ] {
        g.run(query(statement)).await.unwrap();
    }

    // The Learning read of an unseeded school answers the defaults and writes nothing.
    let (early_tag, early) = repo::learning_read(&g, "tenant", "SCH-IISS", None)
        .await
        .unwrap();
    let early = early.unwrap();
    assert!(early_tag.ends_with("d\""), "{early_tag}");
    assert_eq!(kind(&early, "grade")["version"], 0);
    assert_eq!(kind(&early, "grade")["rows"][0]["code"], "Y7");
    assert_eq!(kind(&early, "grade")["rows"][0]["version"], 0);
    assert_eq!(
        count(&g, "MATCH(k:AcademicNomenclatureKind) RETURN count(k) AS n").await,
        0
    );
    assert!(
        repo::learning_read(&g, "tenant", "SCH-IISS", Some(&early_tag))
            .await
            .unwrap()
            .1
            .is_some(),
        "defaults are never a 304"
    );
    assert!(repo::learning_read(&g, "tenant", "SCH-NONE", None)
        .await
        .is_err());

    // First reads of the two schools at once seed each school once.
    let (a, b, c) = tokio::join!(
        repo::read(
            &g,
            "tenant",
            "SCH-IISS",
            "admin",
            "school_admin",
            false,
            None
        ),
        repo::read(&g, "tenant", "SCH-IISS", "teacher", "teacher", false, None),
        repo::read(&g, "tenant", "SCH-IIHS", "owner", "owner", false, None),
    );
    let (tag, read) = a.unwrap();
    assert_eq!(b.unwrap().0, tag);
    let iihs = c.unwrap().1.unwrap();
    assert_eq!(
        count(&g, "MATCH(k:AcademicNomenclatureKind) RETURN count(k) AS n").await,
        22
    );
    assert_eq!(
        count(
            &g,
            "MATCH(a:AcademicNomenclatureAudit {action:'seed'}) RETURN count(a) AS n"
        )
        .await,
        22
    );
    assert_eq!(count(&g, "MATCH(n:AcademicNomenclature) WITH n.key AS k,count(*) AS c WHERE c>1 RETURN count(k) AS n").await, 0);
    let read = read.unwrap();
    let codes = |k: &Value| {
        k["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["code"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(codes(kind(&read, "grade")), ["Y7", "Y8", "Y9"]);
    assert_eq!(codes(kind(&iihs, "grade")), ["Y10", "Y11", "Y12"]);
    assert_eq!(codes(kind(&read, "unit")), ["IISS"]);
    assert_eq!(kind(&read, "unit")["rows"][0]["shortEn"], "IISS");
    assert_eq!(
        codes(kind(&read, "academic_year")),
        ["2025/2026", "2026/2027"]
    );
    assert_eq!(codes(kind(&read, "subject")), ["mathematics", "computing"]);
    assert!(
        codes(kind(&iihs, "subject")).is_empty(),
        "no plans, no subjects"
    );
    assert_eq!(kind(&read, "subject")["rows"][0]["labelEn"], "Mathematics");
    assert_eq!(kind(&read, "grade")["version"], 1);
    assert_eq!(kind(&read, "grade")["closed"], true);
    assert_eq!(read["mappings"]["grade"][3]["stage"], "IGCSE");

    // A known ETag answers without rows; another read seeds nothing more.
    let (again, unchanged) = repo::read(
        &g,
        "tenant",
        "SCH-IISS",
        "admin",
        "school_admin",
        false,
        Some(&tag),
    )
    .await
    .unwrap();
    assert_eq!((again.as_str(), unchanged.is_none()), (tag.as_str(), true));
    assert_eq!(
        count(
            &g,
            "MATCH(a:AcademicNomenclatureAudit) RETURN count(a) AS n"
        )
        .await,
        22
    );
    // A school that does not exist (or exists twice) is never seeded.
    let (status, error, _) = code(
        repo::read(&g, "tenant", "SCH-NONE", "owner", "owner", false, None)
            .await
            .map(|r| r.1.unwrap()),
    );
    assert_eq!((status, error.as_str()), (404, "NOT_FOUND"));

    // Relabel Year 7 and move Year 9 first: only the changed rows take the new version.
    let mut grades = rows_of(kind(&read, "grade"));
    grades[0].label_id = "Kelas 7".into();
    grades[2].order = 10;
    let saved = repo::save(
        &g,
        &writer("admin", "school_admin", "SCH-IISS", "grade-0001"),
        "grade",
        &change("SCH-IISS", 1, grades.clone()),
    )
    .await
    .unwrap();
    assert_eq!(saved["version"], 2);
    let versions: Vec<i64> = ["Y7", "Y8", "Y9"]
        .iter()
        .map(|c| {
            saved["rows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["code"] == *c)
                .unwrap()["version"]
                .as_i64()
                .unwrap()
        })
        .collect();
    assert_eq!(versions, [2, 1, 2]);
    assert_eq!(count(&g, "MATCH(a:AcademicNomenclatureAudit {action:'save',kind:'grade',actor_id:'admin',command_key:'grade-0001',version:2}) RETURN count(a) AS n").await, 1);

    // Exact replay returns the stored answer; the same key with another body and stale revisions are refused.
    let replay = repo::save(
        &g,
        &writer("admin", "school_admin", "SCH-IISS", "grade-0001"),
        "grade",
        &change("SCH-IISS", 1, grades.clone()),
    )
    .await
    .unwrap();
    assert_eq!(replay, saved);
    assert_eq!(
        count(
            &g,
            "MATCH(a:AcademicNomenclatureAudit {action:'save'}) RETURN count(a) AS n"
        )
        .await,
        1
    );
    let mut other = grades.clone();
    other[1].label_en = "Year Eight".into();
    let (_, error, _) = code(
        repo::save(
            &g,
            &writer("admin", "school_admin", "SCH-IISS", "grade-0001"),
            "grade",
            &change("SCH-IISS", 2, other.clone()),
        )
        .await,
    );
    assert_eq!(error, "IDEMPOTENCY_KEY_REUSED");
    let (status, error, detail) = code(
        repo::save(
            &g,
            &writer("admin", "school_admin", "SCH-IISS", "grade-0002"),
            "grade",
            &change("SCH-IISS", 1, other.clone()),
        )
        .await,
    );
    assert_eq!(
        (status, error.as_str(), detail["version"].as_i64()),
        (409, "REVISION_CONFLICT", Some(2))
    );

    // Codes are never removed, and closed kinds take no new code.
    let (status, error, detail) = code(
        repo::save(
            &g,
            &writer("admin", "school_admin", "SCH-IISS", "grade-0003"),
            "grade",
            &change("SCH-IISS", 2, other[..2].to_vec()),
        )
        .await,
    );
    assert_eq!((status, error.as_str()), (409, "CODE_REMOVED"));
    assert_eq!(detail["codes"], serde_json::json!(["Y9"]));
    let mut added = other.clone();
    added.push(Row {
        code: "Y13".into(),
        label_id: "Kelas XIII".into(),
        label_en: "Year 13".into(),
        short_id: None,
        short_en: None,
        order: 11,
        status: "active".into(),
    });
    let (status, error, _) = code(
        repo::save(
            &g,
            &writer("admin", "school_admin", "SCH-IISS", "grade-0004"),
            "grade",
            &change("SCH-IISS", 2, added),
        )
        .await,
    );
    assert_eq!((status, error.as_str()), (422, "CODE_NOT_ALLOWED"));

    // Archive Year 8: active reads drop it, the editor's read keeps it.
    other[1].status = ARCHIVED.into();
    repo::save(
        &g,
        &writer("admin", "school_admin", "SCH-IISS", "grade-0005"),
        "grade",
        &change("SCH-IISS", 2, other),
    )
    .await
    .unwrap();
    let (tag2, active) = repo::read(
        &g,
        "tenant",
        "SCH-IISS",
        "teacher",
        "teacher",
        false,
        Some(&tag),
    )
    .await
    .unwrap();
    assert_ne!(tag2, tag);
    assert_eq!(codes(kind(&active.unwrap(), "grade")), ["Y7", "Y9"]);
    let all = repo::read(
        &g,
        "tenant",
        "SCH-IISS",
        "admin",
        "school_admin",
        true,
        None,
    )
    .await
    .unwrap()
    .1
    .unwrap();
    assert_eq!(codes(kind(&all, "grade")), ["Y7", "Y8", "Y9"]);

    // Open kinds take new codes: a subject the plans do not have yet.
    let mut subjects = rows_of(kind(&all, "subject"));
    subjects.push(Row {
        code: "islamic_studies".into(),
        label_id: "Dirasah Islamiyah".into(),
        label_en: "Islamic Studies".into(),
        short_id: Some("DI".into()),
        short_en: None,
        order: 3,
        status: "active".into(),
    });
    let saved = repo::save(
        &g,
        &writer("owner", "owner", "SCH-IISS", "subject-0001"),
        "subject",
        &change("SCH-IISS", 1, subjects),
    )
    .await
    .unwrap();
    assert_eq!(saved["rows"][2]["shortId"], "DI");

    // Who may save: the school's administrator and Owners; not a teacher, not another school's admin.
    let teacher_try = repo::save(
        &g,
        &writer("teacher", "school_admin", "SCH-IISS", "term-0001"),
        "term",
        &change("SCH-IISS", 1, rows_of(kind(&all, "term"))),
    )
    .await;
    assert_eq!(code(teacher_try).1, "FORBIDDEN");
    let elsewhere = repo::save(
        &g,
        &writer("elsewhere", "school_admin", "SCH-IISS", "term-0002"),
        "term",
        &change("SCH-IISS", 1, rows_of(kind(&all, "term"))),
    )
    .await;
    assert_eq!(code(elsewhere).1, "FORBIDDEN");
    assert_eq!(
        count(
            &g,
            "MATCH(a:AcademicNomenclatureAudit {kind:'term',action:'save'}) RETURN count(a) AS n"
        )
        .await,
        0
    );

    // A save may be the school's first touch: it seeds, then checks the revision it was given.
    g.run(query(
        "CREATE(:School {school_id:'SCH-NEW',tenant_id:'tenant',school_name:'Sekolah Baru'})",
    ))
    .await
    .unwrap();
    let pattern = vec![Row {
        code: "default".into(),
        label_id: "{grade_short}{letter}".into(),
        label_en: "{grade}-{letter}".into(),
        short_id: None,
        short_en: None,
        order: 1,
        status: "active".into(),
    }];
    let saved = repo::save(
        &g,
        &writer("owner", "owner", "SCH-NEW", "pattern-0001"),
        "section_pattern",
        &change("SCH-NEW", 1, pattern),
    )
    .await
    .unwrap();
    assert_eq!(saved["version"], 2);
    assert_eq!(
        count(
            &g,
            "MATCH(k:AcademicNomenclatureKind {school_id:'SCH-NEW'}) RETURN count(k) AS n"
        )
        .await,
        11
    );
    let fresh = repo::read(&g, "tenant", "SCH-NEW", "owner", "owner", false, None)
        .await
        .unwrap()
        .1
        .unwrap();
    assert_eq!(kind(&fresh, "unit")["rows"][0]["labelEn"], "Sekolah Baru");
    assert_eq!(kind(&fresh, "grade")["rows"].as_array().unwrap().len(), 6);

    // Once stored, the Learning read is the staff read without archived rows, and revalidates.
    let (staff_tag, staff) = repo::read(&g, "tenant", "SCH-IISS", "owner", "owner", false, None)
        .await
        .unwrap();
    let (learning_tag, learning) = repo::learning_read(&g, "tenant", "SCH-IISS", None)
        .await
        .unwrap();
    assert_eq!(learning_tag, staff_tag);
    assert_eq!(learning, staff);
    assert!(
        repo::learning_read(&g, "tenant", "SCH-IISS", Some(&learning_tag))
            .await
            .unwrap()
            .1
            .is_none()
    );
}
