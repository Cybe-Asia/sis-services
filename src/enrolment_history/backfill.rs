//! `sis-service backfill-enrolment-history [--apply]`: records the years SIS already knows for Students placed before
//! `SISEnrolmentPeriod` existed. Two sources, both stored today: the class each enrolment is in now (with its coded
//! academic year), and the entry year and grade stored at placement (no class). Additive only: a record is created
//! where none exists for that Student, school and year, never changed. Without `--apply` nothing is written; the
//! report is one JSON line. An applied run marks what it created (`backfill_run`), so a rollback deletes exactly
//! that (`MATCH (p:SISEnrolmentPeriod {backfill_run:$run}) DETACH DELETE p`); a record a roster move has since
//! updated is no longer marked and stays.
use crate::academic_nomenclature::model::{academic_year_code, grade_code};
use neo4rs::{query, BoltType, Graph};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// More enrolments than this stops the run; staging has a few hundred.
const MAX_ROWS: usize = 20_000;
const CLASSES: &str = "MATCH (s:Student)-[:ENROLLED_AS]->(e:EnrolledStudent)-[:ENROLLED_IN]->(sec:Section) WHERE e.school_id=sec.school_id AND e.tenant_id=sec.tenant_id AND coalesce(s.studentId,'')<>'' RETURN s.studentId AS student,sec.tenant_id AS tenant,sec.school_id AS school,sec.academic_year_code AS year_code,sec.academic_year AS year,sec.grade_code AS grade_code,sec.year_group AS year_group,sec.section_id AS class_id,sec.name AS class_name LIMIT 20001";
const PLACEMENTS: &str = "MATCH (s:Student)-[:ENROLLED_AS]->(e:EnrolledStudent) WHERE e.entry_year IS NOT NULL AND coalesce(e.school_id,'')<>'' AND coalesce(e.tenant_id,'')<>'' AND coalesce(s.studentId,'')<>'' RETURN s.studentId AS student,e.tenant_id AS tenant,e.school_id AS school,toString(e.entry_year) AS entry,e.year_group AS year_group LIMIT 20001";
const EXISTING: &str = "UNWIND $keys AS k MATCH (p:SISEnrolmentPeriod {key:k}) RETURN p.key AS key";
const WRITE: &str = "UNWIND $rows AS r MERGE (p:SISEnrolmentPeriod {key:r.key}) ON CREATE SET p.student_id=r.student,p.school_id=r.school,p.tenant_id=r.tenant,p.academic_year_code=r.year_code,p.academic_year=r.year,p.grade_code=r.grade_code,p.year_group=r.year_group,p.section_id=r.class_id,p.section_name=r.class_name,p.source=r.source,p.first_seen_at=datetime(),p.updated_at=datetime(),p.backfill_run=$run RETURN count(p) AS n";
/// The rollback Job runs this text with cypher-shell (gitops); the database test runs it here.
#[cfg_attr(not(test), allow(dead_code))]
pub const ROLLBACK: &str =
    "MATCH (p:SISEnrolmentPeriod {backfill_run:$run}) DETACH DELETE p RETURN count(*) AS removed";

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Report {
    pub run: String,
    pub applied: bool,
    /// Enrolments in a class, and enrolments with a stored entry year.
    pub classes: usize,
    pub placements: usize,
    /// Years this run creates (or would create), by source.
    pub created: BTreeMap<String, usize>,
    /// Years already recorded (by a roster move, a handover or an earlier run).
    pub already: usize,
    /// Classes without an academic year code (run the nomenclature backfill first), at most 50 listed.
    pub uncoded_classes: Vec<String>,
    /// Placements whose stored grade the rules do not recognise; recorded without a grade code.
    pub unrecognised_grades: usize,
}

/// One year to record.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub key: String,
    pub student: String,
    pub tenant: String,
    pub school: String,
    pub year_code: String,
    pub year: Option<String>,
    pub grade_code: Option<String>,
    pub year_group: Option<String>,
    pub class_id: Option<String>,
    pub class_name: Option<String>,
    pub source: &'static str,
}
pub fn key(tenant: &str, school: &str, student: &str, year_code: &str) -> String {
    format!("{tenant}|{school}|{student}|{year_code}")
}
/// The placement's year: `2026` → `2026/2027` with the stored grade's code; `None` when the year is unreadable.
pub fn placement(
    student: &str,
    tenant: &str,
    school: &str,
    entry: &str,
    year_group: Option<&str>,
) -> Option<Candidate> {
    let year_code = academic_year_code(entry)?;
    Some(Candidate {
        key: key(tenant, school, student, &year_code),
        student: student.into(),
        tenant: tenant.into(),
        school: school.into(),
        year: Some(year_code.clone()),
        grade_code: year_group.and_then(grade_code).map(str::to_string),
        year_group: year_group.map(str::to_string),
        class_id: None,
        class_name: None,
        source: "backfill_placement",
        year_code,
    })
}
/// Class years first: where a class and a placement name the same year, the class is kept.
pub fn plan(classes: Vec<Candidate>, placements: Vec<Candidate>) -> Vec<Candidate> {
    let mut seen = BTreeSet::new();
    classes
        .into_iter()
        .chain(placements)
        .filter(|c| seen.insert(c.key.clone()))
        .collect()
}

fn text(row: &neo4rs::Row, name: &str) -> Option<String> {
    row.get::<Option<String>>(name)
        .ok()
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub async fn run(
    graph: &Graph,
    apply: bool,
) -> Result<Report, Box<dyn std::error::Error + Send + Sync>> {
    let run = uuid::Uuid::new_v4().to_string();
    let mut report = Report {
        run: run.clone(),
        applied: apply,
        ..Report::default()
    };
    let mut classes = vec![];
    let mut rows = graph.execute(query(CLASSES)).await?;
    while let Some(row) = rows.next().await? {
        report.classes += 1;
        if report.classes > MAX_ROWS {
            return Err(format!("more than {MAX_ROWS} enrolled classes; run in parts").into());
        }
        let (Some(student), Some(tenant), Some(school)) = (
            text(&row, "student"),
            text(&row, "tenant"),
            text(&row, "school"),
        ) else {
            continue;
        };
        let class_id = text(&row, "class_id");
        let Some(year_code) = text(&row, "year_code") else {
            if report.uncoded_classes.len() < 50 {
                if let Some(id) = &class_id {
                    if !report.uncoded_classes.contains(id) {
                        report.uncoded_classes.push(id.clone());
                    }
                }
            }
            continue;
        };
        classes.push(Candidate {
            key: key(&tenant, &school, &student, &year_code),
            year: text(&row, "year"),
            grade_code: text(&row, "grade_code"),
            year_group: text(&row, "year_group"),
            class_name: text(&row, "class_name"),
            class_id,
            source: "backfill_class",
            student,
            tenant,
            school,
            year_code,
        });
    }
    let mut placements = vec![];
    let mut rows = graph.execute(query(PLACEMENTS)).await?;
    while let Some(row) = rows.next().await? {
        report.placements += 1;
        if report.placements > MAX_ROWS {
            return Err(format!("more than {MAX_ROWS} placements; run in parts").into());
        }
        let (Some(student), Some(tenant), Some(school), Some(entry)) = (
            text(&row, "student"),
            text(&row, "tenant"),
            text(&row, "school"),
            text(&row, "entry"),
        ) else {
            continue;
        };
        let year_group = text(&row, "year_group");
        if let Some(c) = placement(&student, &tenant, &school, &entry, year_group.as_deref()) {
            if year_group.is_some() && c.grade_code.is_none() {
                report.unrecognised_grades += 1;
            }
            placements.push(c);
        }
    }
    let planned = plan(classes, placements);
    let keys: Vec<String> = planned.iter().map(|c| c.key.clone()).collect();
    let mut existing = BTreeSet::new();
    for part in keys.chunks(1000) {
        let mut rows = graph
            .execute(query(EXISTING).param("keys", part.to_vec()))
            .await?;
        while let Some(row) = rows.next().await? {
            existing.insert(row.get::<String>("key")?);
        }
    }
    let todo: Vec<&Candidate> = planned
        .iter()
        .filter(|c| !existing.contains(&c.key))
        .collect();
    report.already = planned.len() - todo.len();
    for c in &todo {
        *report.created.entry(c.source.to_string()).or_default() += 1;
    }
    if apply && !todo.is_empty() {
        let rows: Vec<BoltType> = todo
            .iter()
            .map(|c| {
                let opt = |v: &Option<String>| {
                    v.clone()
                        .map_or(BoltType::Null(neo4rs::BoltNull), BoltType::from)
                };
                BoltType::from(HashMap::from([
                    ("key".to_string(), BoltType::from(c.key.clone())),
                    ("student".to_string(), BoltType::from(c.student.clone())),
                    ("tenant".to_string(), BoltType::from(c.tenant.clone())),
                    ("school".to_string(), BoltType::from(c.school.clone())),
                    ("year_code".to_string(), BoltType::from(c.year_code.clone())),
                    ("year".to_string(), opt(&c.year)),
                    ("grade_code".to_string(), opt(&c.grade_code)),
                    ("year_group".to_string(), opt(&c.year_group)),
                    ("class_id".to_string(), opt(&c.class_id)),
                    ("class_name".to_string(), opt(&c.class_name)),
                    ("source".to_string(), BoltType::from(c.source)),
                ]))
            })
            .collect();
        let mut tx = graph.start_txn().await?;
        for part in rows.chunks(1000) {
            tx.run(
                query(WRITE)
                    .param("rows", part.to_vec())
                    .param("run", run.clone()),
            )
            .await?;
        }
        tx.commit().await?;
    }
    if apply {
        graph
            .run(
                query("CREATE(:SISEnrolmentHistoryBackfill {run:$run,created_at:datetime(),report:$report})")
                    .param("run", run)
                    .param("report", serde_json::to_string(&report)?),
            )
            .await?;
    }
    Ok(report)
}
