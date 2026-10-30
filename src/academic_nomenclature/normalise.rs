//! Nomenclature slice 5: the grade and academic-year codes beside the stored free text, for records written before
//! SIS set them on every write (`sis-service normalise-nomenclature [--apply]`, `docs/ACADEMIC_NOMENCLATURE.md`).
//! Additive only: a code is written where none is stored, never over another one, and the free text stays as it is.
//! A value the rules do not recognise is reported, never guessed. Without `--apply` nothing is written.
//! Every applied run leaves one `AcademicNomenclatureNormalisation` record, and its id beside each code it wrote
//! (`nomenclature_grade_run`, `nomenclature_year_run`), so a rollback removes exactly the codes it added.
use super::model::{academic_year_code, grade_code};
use neo4rs::{query, Graph};
use serde::Serialize;
use std::collections::BTreeMap;

/// The labels coded, with the property holding their key, their stored grade and their stored academic year.
const TARGETS: [(&str, &str, Option<&str>, &str); 3] = [
    ("Section", "section_id", Some("year_group"), "academic_year"),
    ("AcademicSubjectPlan", "key", Some("grade"), "academic_year"),
    (
        "AcademicGradeWeights",
        "key",
        Some("grade"),
        "academic_year",
    ),
];
/// More rows than this in one label stops the run; staging has a few hundred.
const MAX_ROWS: usize = 20_000;

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Counts {
    pub rows: usize,
    /// Codes this run writes (or would write in a dry run).
    pub coded: usize,
    /// Codes already stored and equal to the rule's.
    pub already: usize,
}
#[derive(Debug, Serialize, PartialEq)]
pub struct Unrecognised {
    pub label: String,
    pub key: String,
    pub field: String,
    pub value: String,
}
#[derive(Debug, Serialize, PartialEq)]
pub struct Report {
    pub run: String,
    pub applied: bool,
    pub counts: BTreeMap<String, Counts>,
    /// Stored text the rules do not recognise; left without a code.
    pub unrecognised: Vec<Unrecognised>,
    /// A stored code that differs from the rule's; left as it is.
    pub conflicts: Vec<Unrecognised>,
}

/// What one stored value becomes.
#[derive(Debug, PartialEq)]
enum Decision {
    Write(String),
    Already,
    Empty,
    Unrecognised,
    Conflict(String),
}
fn decide(stored_text: Option<&str>, stored_code: Option<&str>, code: Option<String>) -> Decision {
    if stored_text.map(str::trim).is_none_or(str::is_empty) {
        return Decision::Empty;
    }
    match (code, stored_code) {
        (None, _) => Decision::Unrecognised,
        (Some(c), Some(s)) if s == c => Decision::Already,
        (Some(_), Some(s)) => Decision::Conflict(s.to_string()),
        (Some(c), None) => Decision::Write(c),
    }
}

/// Reads every coded label, decides each value and, with `apply`, writes the missing codes in one transaction per
/// label (only where the code is still missing, so a concurrent write wins).
pub async fn run(
    graph: &Graph,
    apply: bool,
) -> Result<Report, Box<dyn std::error::Error + Send + Sync>> {
    let run = uuid::Uuid::new_v4().to_string();
    let mut report = Report {
        run: run.clone(),
        applied: apply,
        counts: BTreeMap::new(),
        unrecognised: vec![],
        conflicts: vec![],
    };
    for (label, key, grade_field, year_field) in TARGETS {
        let grade_read = grade_field.map_or("null".to_string(), |f| format!("toString(n.{f})"));
        let mut rows = graph
            .execute(query(&format!(
                "MATCH(n:{label}) RETURN elementId(n) AS id,toString(n.{key}) AS key,{grade_read} AS grade,\
                 toString(n.{year_field}) AS year,n.grade_code AS grade_code,n.academic_year_code AS year_code LIMIT {}",
                MAX_ROWS + 1
            )))
            .await?;
        let counts = report.counts.entry(label.to_string()).or_default();
        let (mut ids, mut grades, mut years) = (vec![], vec![], vec![]);
        while let Some(row) = rows.next().await? {
            counts.rows += 1;
            if counts.rows > MAX_ROWS {
                return Err(format!("more than {MAX_ROWS} {label} rows; run in parts").into());
            }
            let id: String = row.get("id")?;
            let key: Option<String> = row.get("key")?;
            let key = key.unwrap_or_default();
            let grade: Option<String> = row.get("grade")?;
            let year: Option<String> = row.get("year")?;
            // Per field: its name, stored text and code, and the rule's code. "" in the write lists means none.
            let mut set = [String::new(), String::new()];
            for (slot, (field, text, stored, rule)) in [
                (
                    grade_field.unwrap_or("grade"),
                    grade.clone(),
                    row.get::<Option<String>>("grade_code")?,
                    grade.as_deref().and_then(grade_code).map(str::to_string),
                ),
                (
                    year_field,
                    year.clone(),
                    row.get::<Option<String>>("year_code")?,
                    year.as_deref().and_then(academic_year_code),
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let finding = || Unrecognised {
                    label: label.to_string(),
                    key: key.clone(),
                    field: field.to_string(),
                    value: text.clone().unwrap_or_default(),
                };
                match decide(text.as_deref(), stored.as_deref(), rule) {
                    Decision::Write(c) => {
                        counts.coded += 1;
                        set[slot] = c;
                    }
                    Decision::Already => counts.already += 1,
                    Decision::Empty => {}
                    Decision::Unrecognised => report.unrecognised.push(finding()),
                    Decision::Conflict(stored) => report.conflicts.push(Unrecognised {
                        value: format!(
                            "{} (stored code {stored})",
                            text.clone().unwrap_or_default()
                        ),
                        ..finding()
                    }),
                }
            }
            if set.iter().any(|c| !c.is_empty()) {
                let [g, y] = set;
                ids.push(id);
                grades.push(g);
                years.push(y);
            }
        }
        if apply && !ids.is_empty() {
            let mut tx = graph.start_txn().await?;
            tx.run(
                query(&format!(
                    "UNWIND range(0,size($ids)-1) AS i MATCH(n:{label}) WHERE elementId(n)=$ids[i] \
                     WITH n,nullIf($grades[i],'') AS g,nullIf($years[i],'') AS y \
                     SET n.nomenclature_grade_run=CASE WHEN n.grade_code IS NULL AND g IS NOT NULL THEN $run ELSE n.nomenclature_grade_run END,\
                     n.nomenclature_year_run=CASE WHEN n.academic_year_code IS NULL AND y IS NOT NULL THEN $run ELSE n.nomenclature_year_run END,\
                     n.grade_code=coalesce(n.grade_code,g),n.academic_year_code=coalesce(n.academic_year_code,y)"
                ))
                .param("ids", ids)
                .param("grades", grades)
                .param("years", years)
                .param("run", run.clone()),
            )
            .await?;
            tx.commit().await?;
        }
    }
    if apply {
        graph
            .run(
                query("CREATE(:AcademicNomenclatureNormalisation {run:$run,created_at:datetime(),report:$report})")
                    .param("run", run)
                    .param("report", serde_json::to_string(&report)?),
            )
            .await?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_coded_once_reported_when_unknown_and_never_overwritten() {
        assert_eq!(
            decide(Some("year7"), None, grade_code("year7").map(str::to_string)),
            Decision::Write("Y7".into())
        );
        assert_eq!(
            decide(Some("2026"), None, academic_year_code("2026")),
            Decision::Write("2026/2027".into())
        );
        assert_eq!(
            decide(Some("2026"), Some("2026/2027"), academic_year_code("2026")),
            Decision::Already
        );
        assert_eq!(
            decide(Some("2026"), Some("2025/2026"), academic_year_code("2026")),
            Decision::Conflict("2025/2026".into())
        );
        assert_eq!(
            decide(
                Some("Year 7 Bilingual"),
                None,
                grade_code("Year 7 Bilingual").map(str::to_string)
            ),
            Decision::Unrecognised
        );
        assert_eq!(decide(Some(" "), None, None), Decision::Empty);
        assert_eq!(decide(None, None, None), Decision::Empty);
    }
}
