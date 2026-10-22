use super::super::{auth::Teacher, model::homeroom::Attendance};
use super::{teacher_query, Error, ENROLLED};
use chrono::NaiveDate;
use neo4rs::Graph;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Enrolled Students of a class with their attendance statuses in the range, one per day.
const ROSTER: &str = "WITH DISTINCT sec MATCH(s:Student)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:sec.school_id,tenant_id:sec.tenant_id})-[:ENROLLED_IN]->(sec) WITH DISTINCT sec,s OPTIONAL MATCH(a:AttendanceRecord {section_id:sec.section_id,applicant_student_id:s.studentId}) WHERE a.date>=$from AND a.date<=$to WITH s,collect(DISTINCT a) AS records RETURN s.studentId AS id,s.fullName AS name,[r IN records | coalesce(r.status,'')] AS statuses ORDER BY name,id LIMIT 201";

/// The class's attendance and one term's grades for its homeroom teacher or an Owner.
/// `None` when the caller is neither (a subject-only grant does not qualify).
pub async fn homeroom_summary(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    (from, to): (NaiveDate, NaiveDate),
    term: Option<&str>,
) -> Result<Option<Value>, Error> {
    let mut check = graph
        .execute(teacher_query(
            member,
            "RETURN DISTINCT sec.section_id AS id",
            Some(section),
            false,
            true,
        ))
        .await?;
    if check.next().await?.is_none() {
        return Ok(None);
    }
    let mut rows = graph
        .execute(
            teacher_query(member, ROSTER, Some(section), false, true)
                .param("from", from.to_string())
                .param("to", to.to_string()),
        )
        .await?;
    let mut students = vec![];
    while let Some(r) = rows.next().await? {
        if students.len() >= 200 {
            return Err("roster limit exceeded".into());
        }
        let statuses: Vec<String> = r.get("statuses")?;
        students.push((
            r.get::<String>("id")?,
            r.get::<String>("name")?,
            Attendance::tally(statuses.iter().map(String::as_str)),
        ));
    }
    let mut grades: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    if let Some(term) = term {
        let body = format!("{ENROLLED} WITH DISTINCT sec,s MATCH(g:GradeEntry {{section_id:sec.section_id,applicant_student_id:s.studentId,term:$term}}) RETURN s.studentId AS id,g.subject AS subject,toFloat(g.score) AS score,toFloat(g.max_score) AS maximum ORDER BY id,subject LIMIT 4001");
        let mut rows = graph
            .execute(teacher_query(member, &body, Some(section), false, true).param("term", term))
            .await?;
        let mut count = 0;
        while let Some(r) = rows.next().await? {
            count += 1;
            if count > 4000 {
                return Err("grade limit exceeded".into());
            }
            grades
                .entry(r.get::<String>("id")?)
                .or_default()
                .push(json!({
                    "subject": r.get::<String>("subject")?,
                    "score": r.get::<f64>("score")?,
                    "maxScore": r.get::<f64>("maximum")?,
                }));
        }
    }
    let students: Vec<Value> = students
        .into_iter()
        .map(|(id, name, attendance)| {
            let g = grades.remove(&id).unwrap_or_default();
            json!({"studentId": id, "name": name, "attendance": attendance, "grades": g})
        })
        .collect();
    Ok(Some(json!({
        "sectionId": section,
        "from": from.to_string(),
        "to": to.to_string(),
        "term": term,
        "students": students,
    })))
}
