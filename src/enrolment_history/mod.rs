//! The class a Student was in for each academic year of each school (`SISEnrolmentPeriod`, Learning ADR 0017 slice 5,
//! `docs/LEARNING_STUDENT_HISTORY.md`). A roster move replaces the Student's class (`ENROLLED_IN`); before that, every
//! writer of the edge records the year here, so the Learning pathway reads exact years instead of estimating them.
//! One record per Student × school × academic year code: a move inside a year updates it, so the last class wins.
//! Additive: nothing else reads or changes because of it. Records written before this existed come from the backfill
//! (`sis-service backfill-enrolment-history [--apply]`).
use neo4rs::{query, Graph};
pub mod backfill;
#[cfg(test)]
mod tests;

pub async fn init(graph: &Graph) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for statement in include_str!("../../migrations/0009_enrolment_history.cypher")
        .split(';')
        .map(|s| {
            s.lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.trim().is_empty())
    {
        graph.run(query(&statement)).await?;
    }
    Ok(())
}

/// The key of a record: tenant, school, Student and academic year code.
pub const KEY: &str = "sec.tenant_id+'|'+sec.school_id+'|'+s.studentId+'|'+sec.academic_year_code";

/// A Cypher clause recording the year of Student `s` in Section `sec` (both bound), written by `source` (`roster`,
/// `handover`). A Section without an academic year code records nothing (the nomenclature backfill codes it). A record
/// the backfill created becomes the writer's: its `backfill_run` is cleared, so a backfill rollback keeps it.
pub fn record(source: &str) -> String {
    debug_assert!(source.chars().all(|c| c.is_ascii_lowercase()));
    format!(
        " FOREACH(_period IN CASE WHEN coalesce(sec.academic_year_code,'')<>'' AND coalesce(sec.school_id,'')<>'' \
         AND coalesce(sec.tenant_id,'')<>'' AND coalesce(s.studentId,'')<>'' THEN [1] ELSE [] END | \
         MERGE (period:SISEnrolmentPeriod {{key:{KEY}}}) \
         ON CREATE SET period.student_id=s.studentId,period.school_id=sec.school_id,period.tenant_id=sec.tenant_id,\
         period.academic_year_code=sec.academic_year_code,period.first_seen_at=datetime(),period.source='{source}' \
         SET period.section_id=sec.section_id,period.section_name=sec.name,period.grade_code=sec.grade_code,\
         period.year_group=sec.year_group,period.academic_year=sec.academic_year,period.updated_at=datetime(),\
         period.backfill_run=null) "
    )
}

/// The Student's recorded years in the tenant, oldest first, at most `MAX_YEARS`.
pub const READ: &str = "MATCH (p:SISEnrolmentPeriod {tenant_id:$tenant,student_id:$student}) OPTIONAL MATCH (school:School {school_id:p.school_id,tenant_id:$tenant}) RETURN p.school_id AS school_id,coalesce(school.school_name,school.name) AS school_name,p.academic_year_code AS year_code,p.academic_year AS year,p.grade_code AS grade_code,p.year_group AS year_group,p.section_id AS class_id,p.section_name AS class_name,p.source AS source ORDER BY year_code,school_id LIMIT 25";
pub const MAX_YEARS: usize = 24;
