use super::backfill::{key, placement, plan, Candidate, ROLLBACK};
use neo4rs::query;

fn class(student: &str, year: &str) -> Candidate {
    Candidate {
        key: key("t", "SCH-IISS", student, year),
        student: student.into(),
        tenant: "t".into(),
        school: "SCH-IISS".into(),
        year_code: year.into(),
        year: Some(year.into()),
        grade_code: Some("Y8".into()),
        year_group: Some("Year 8".into()),
        class_id: Some("8A".into()),
        class_name: Some("8A".into()),
        source: "backfill_class",
    }
}

#[test]
fn placements_read_the_stored_year_and_a_class_wins_the_same_year() {
    let p = placement("s1", "t", "SCH-IISS", "2025", Some("Year 7")).unwrap();
    assert_eq!(
        (
            p.key.as_str(),
            p.year_code.as_str(),
            p.grade_code.as_deref()
        ),
        ("t|SCH-IISS|s1|2025/2026", "2025/2026", Some("Y7"))
    );
    assert_eq!(
        placement("s1", "t", "SCH-IISS", "2025", Some("Year 7 Bilingual"))
            .unwrap()
            .grade_code,
        None,
        "an unrecognised grade is kept as text without a code"
    );
    assert!(placement("s1", "t", "SCH-IISS", "soon", None).is_none());
    let same_year = placement("s1", "t", "SCH-IISS", "2026", Some("Year 8")).unwrap();
    let planned = plan(vec![class("s1", "2026/2027")], vec![p, same_year]);
    assert_eq!(planned.len(), 2);
    assert_eq!(planned[0].source, "backfill_class");
    assert_eq!(planned[1].year_code, "2025/2026");
}

#[test]
fn every_writer_uses_one_key_and_clears_the_backfill_mark() {
    for source in ["roster", "handover"] {
        let clause = super::record(source);
        assert!(clause.contains(super::KEY));
        assert!(clause.contains("period.backfill_run=null"));
        assert!(clause.contains(&format!("period.source='{source}'")));
    }
    let handover = include_str!("../enrollment_handover/create.cypher");
    assert_eq!(handover.matches("/*ENROLMENT_PERIOD*/").count(), 1);
}

#[tokio::test]
#[ignore = "requires dedicated disposable SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223"]
async fn roster_moves_record_one_year_each_and_the_backfill_is_additive_and_reversible() {
    assert_eq!(
        std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap(),
        "127.0.0.1:3223"
    );
    let g = neo4rs::Graph::new("127.0.0.1:3223", "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    super::init(&g).await.unwrap();
    let k = uuid::Uuid::new_v4().to_string();
    let (tenant, iiss) = (format!("t-{k}"), format!("iiss-{k}"));
    g.run(query("CREATE(:Section {section_id:'7a-'+$k,name:'7A',school_id:$iiss,tenant_id:$t,academic_year:'2025',academic_year_code:'2025/2026',grade_code:'Y7',year_group:'Year 7',test_key:$k}) CREATE(:Section {section_id:'7b-'+$k,name:'7B',school_id:$iiss,tenant_id:$t,academic_year:'2025',academic_year_code:'2025/2026',grade_code:'Y7',year_group:'Year 7',test_key:$k}) CREATE(:Section {section_id:'8a-'+$k,name:'8A',school_id:$iiss,tenant_id:$t,academic_year:'2026',academic_year_code:'2026/2027',grade_code:'Y8',year_group:'Year 8',test_key:$k}) CREATE(:Section {section_id:'old-'+$k,name:'Old',school_id:$iiss,tenant_id:$t,academic_year:'someday',test_key:$k}) CREATE(s:Student {studentId:'s-'+$k,test_key:$k})-[:ENROLLED_AS]->(:EnrolledStudent {school_id:$iiss,tenant_id:$t,status:'active',entry_year:2024,year_group:'Year 6',test_key:$k}) CREATE(o:Student {studentId:'o-'+$k,test_key:$k})-[:ENROLLED_AS]->(oe:EnrolledStudent {school_id:$iiss,tenant_id:$t,status:'active',entry_year:2026,year_group:'Year 8',test_key:$k})-[:ENROLLED_IN]->(:Section {section_id:'8b-'+$k,name:'8B',school_id:$iiss,tenant_id:$t,academic_year:'2026',academic_year_code:'2026/2027',grade_code:'Y8',year_group:'Year 8',test_key:$k})")
        .param("k", k.clone()).param("t", tenant.clone()).param("iiss", iiss.clone())).await.unwrap();
    // The roster's move: the same edge replacement as `assign_students`, with the recording clause.
    let mv = |section: &str| {
        query(&format!("MATCH (sec:Section {{section_id:$sec}}) MATCH (s:Student {{studentId:$s}})-[:ENROLLED_AS]->(e:EnrolledStudent) OPTIONAL MATCH (e)-[old:ENROLLED_IN]->(:Section) DELETE old MERGE (e)-[r:ENROLLED_IN]->(sec) {} RETURN count(e) AS n", super::record("roster")))
            .param("sec", format!("{section}-{k}"))
            .param("s", format!("s-{k}"))
    };
    let periods = || async {
        let mut rows = g.execute(query("MATCH (p:SISEnrolmentPeriod {tenant_id:$t}) RETURN p.student_id AS s,p.academic_year_code AS y,p.section_name AS c,p.source AS src,p.backfill_run AS run ORDER BY s,y").param("t", tenant.clone())).await.unwrap();
        let mut out = vec![];
        while let Some(r) = rows.next().await.unwrap() {
            out.push((
                r.get::<String>("s").unwrap(),
                r.get::<String>("y").unwrap(),
                r.get::<Option<String>>("c").unwrap(),
                r.get::<String>("src").unwrap(),
                r.get::<Option<String>>("run").unwrap().is_some(),
            ));
        }
        out
    };
    // Before any move: the backfill sees one class (8B) and two placements; a dry run writes nothing.
    let dry = super::backfill::run(&g, false).await.unwrap();
    assert!(!dry.applied);
    assert!(periods().await.is_empty());
    for section in ["7a", "7b", "8a", "old"] {
        g.run(mv(section)).await.unwrap();
    }
    let s = format!("s-{k}");
    assert_eq!(
        periods().await,
        vec![
            (
                s.clone(),
                "2025/2026".into(),
                Some("7B".into()),
                "roster".into(),
                false
            ),
            (
                s.clone(),
                "2026/2027".into(),
                Some("8A".into()),
                "roster".into(),
                false
            ),
        ],
        "a move inside a year keeps the last class; a class without a year code records nothing"
    );
    // The backfill adds what is known for the rest: the other Student's class and the placements, not the moves.
    let applied = super::backfill::run(&g, true).await.unwrap();
    let mine = periods().await;
    let o = format!("o-{k}");
    assert!(mine.contains(&(
        o.clone(),
        "2026/2027".into(),
        Some("8B".into()),
        "backfill_class".into(),
        true
    )));
    assert!(mine.contains(&(
        s.clone(),
        "2024/2025".into(),
        None,
        "backfill_placement".into(),
        true
    )));
    assert_eq!(mine.len(), 4, "8B's placement year is the class year");
    assert!(
        applied.uncoded_classes.contains(&format!("old-{k}")),
        "the Student's class now has no year code: reported, not recorded"
    );
    super::backfill::run(&g, true).await.unwrap();
    assert_eq!(
        periods().await,
        mine,
        "a second run adds nothing for these Students"
    );
    // A later move into a year the backfill recorded takes the record over; the rollback then leaves it.
    g.run(query(&format!("MATCH (sec:Section {{section_id:$sec}}) MATCH (s:Student {{studentId:$s}})-[:ENROLLED_AS]->(e:EnrolledStudent) OPTIONAL MATCH (e)-[old:ENROLLED_IN]->(:Section) DELETE old MERGE (e)-[r:ENROLLED_IN]->(sec) {} RETURN count(e) AS n", super::record("roster"))).param("sec", format!("8a-{k}")).param("s", o.clone())).await.unwrap();
    g.run(query(ROLLBACK).param("run", applied.run.clone()))
        .await
        .unwrap();
    assert_eq!(
        periods().await,
        vec![
            (
                o,
                "2026/2027".into(),
                Some("8A".into()),
                "backfill_class".into(),
                false
            ),
            (
                s.clone(),
                "2025/2026".into(),
                Some("7B".into()),
                "roster".into(),
                false
            ),
            (
                s,
                "2026/2027".into(),
                Some("8A".into()),
                "roster".into(),
                false
            ),
        ]
    );
    g.run(query("MATCH (p:SISEnrolmentPeriod {tenant_id:$t}) DETACH DELETE p").param("t", tenant))
        .await
        .unwrap();
    g.run(query("MATCH (n {test_key:$k}) DETACH DELETE n").param("k", k))
        .await
        .unwrap();
}
