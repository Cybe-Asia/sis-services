use super::super::{
    auth::Teacher,
    model::extracurricular::{skill_levels, Attendance},
};
use super::{teacher_query, Error, ENROLLED};
use crate::extra_curricular::model::Activity;
use neo4rs::{query, Graph};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Enrolled Students of the class, for the rows of the read (a Student without a club still appears).
const ROSTER: &str = "WITH DISTINCT sec MATCH(s:Student)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:sec.school_id,tenant_id:sec.tenant_id})-[:ENROLLED_IN]->(sec) RETURN DISTINCT s.studentId AS id,s.fullName AS name ORDER BY name,id LIMIT 201";

/// Every current club enrolment of the class's Students in the class's own school and year.
/// Archived activities stay: a club that closed mid-year is still part of the year's report.
const ENROLMENTS: &str = "WITH DISTINCT sec,s MATCH(e:ExtraCurricularEnrollment {student_id:s.studentId,status:'enrolled'}) MATCH(a:ExtraCurricularActivity {key:e.activity_key,school_id:sec.school_id,tenant_id:sec.tenant_id}) WHERE sec.academic_year IS NULL OR a.academic_year=sec.academic_year RETURN s.studentId AS student,a.key AS key,a.payload AS payload,a.archived AS archived,a.academic_year AS year,e.payload AS enrollment ORDER BY student,key LIMIT 2001";

struct Club {
    activity: Activity,
    archived: bool,
    year: String,
    coach_name: String,
    /// Meetings for which the coach took attendance, for anyone.
    recorded_meetings: BTreeSet<String>,
    /// Attendance statuses per Student of this class.
    marks: BTreeMap<String, Vec<String>>,
    /// Released outcome per Student of this class.
    released: BTreeMap<String, Value>,
    results: Vec<Value>,
}

/// The extracurricular records of one class for its homeroom teacher or an Owner (audit gap P1b).
/// `None` when the caller is neither (a subject-only grant does not qualify, as for
/// `homeroom_summary`). Drafts are never read: only `released_payload` of an outcome.
pub async fn class_extracurricular(
    graph: &Graph,
    member: &Teacher,
    section: &str,
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
        .execute(teacher_query(member, ROSTER, Some(section), false, true))
        .await?;
    let mut roster: Vec<(String, String)> = vec![];
    while let Some(r) = rows.next().await? {
        if roster.len() >= 200 {
            return Err("roster limit exceeded".into());
        }
        roster.push((r.get::<String>("id")?, r.get::<String>("name")?));
    }
    let students: Vec<String> = roster.iter().map(|(id, _)| id.clone()).collect();
    let body = format!("{ENROLLED} {ENROLMENTS}");
    let mut rows = graph
        .execute(teacher_query(member, &body, Some(section), false, true))
        .await?;
    // (student, activity key) → enrolment; activity key → club.
    let mut enrolments: BTreeMap<String, Vec<(String, Value)>> = BTreeMap::new();
    let mut clubs: BTreeMap<String, Club> = BTreeMap::new();
    let mut count = 0;
    while let Some(r) = rows.next().await? {
        count += 1;
        if count > 2000 {
            return Err("enrolment limit exceeded".into());
        }
        let student: String = r.get("student")?;
        let key: String = r.get("key")?;
        if !clubs.contains_key(&key) {
            let raw: String = r.get("payload")?;
            if raw.len() > 65536 {
                return Err("activity too large".into());
            }
            let activity: Activity = serde_json::from_str(&raw)?;
            clubs.insert(
                key.clone(),
                Club {
                    activity,
                    archived: r.get::<bool>("archived").unwrap_or(false),
                    year: r.get::<String>("year").unwrap_or_default(),
                    coach_name: String::new(),
                    recorded_meetings: BTreeSet::new(),
                    marks: BTreeMap::new(),
                    released: BTreeMap::new(),
                    results: vec![],
                },
            );
        }
        let enrolment: Value = serde_json::from_str(&r.get::<String>("enrollment")?)?;
        enrolments
            .entry(student)
            .or_default()
            .push((key, enrolment));
    }
    for (key, club) in clubs.iter_mut() {
        let mut staff = graph
            .execute(
                query("MATCH(t:StaffMember {id:$coach}) RETURN coalesce(t.fullName,t.name,t.displayName,t.id) AS name LIMIT 1")
                    .param("coach", club.activity.coach_id.clone()),
            )
            .await?;
        club.coach_name = match staff.next().await? {
            Some(r) => r.get::<String>("name")?,
            None => club.activity.coach_id.clone(),
        };
        // Attendance of this class's Students only; the activity may span several classes.
        let mut marks = graph
            .execute(
                query("MATCH(r:ExtraCurricularAttendance {activity_key:$key}) RETURN r.student_id AS student,r.payload AS payload LIMIT 20001")
                    .param("key", key.clone()),
            )
            .await?;
        let mut seen = 0;
        while let Some(r) = marks.next().await? {
            seen += 1;
            if seen > 20000 {
                return Err("attendance limit exceeded".into());
            }
            let mark: Value = serde_json::from_str(&r.get::<String>("payload")?)?;
            if let Some(meeting) = mark["meetingId"].as_str() {
                club.recorded_meetings.insert(meeting.to_string());
            }
            let student: String = r.get("student")?;
            if students.contains(&student) {
                club.marks
                    .entry(student)
                    .or_default()
                    .push(mark["status"].as_str().unwrap_or("").to_string());
            }
        }
        let mut released = graph
            .execute(
                query("MATCH(r:ExtraCurricularOutcome {activity_key:$key}) WHERE r.released_payload IS NOT NULL AND r.student_id IN $students RETURN r.student_id AS student,r.released_payload AS payload,CASE WHEN r.released_at IS NULL THEN null ELSE toString(date(r.released_at)) END AS released_date LIMIT 201")
                    .param("key", key.clone())
                    .param("students", students.clone()),
            )
            .await?;
        while let Some(r) = released.next().await? {
            let raw: String = r.get("payload")?;
            if raw.len() > 65536 {
                return Err("outcome too large".into());
            }
            let outcome: Value = serde_json::from_str(&raw)?;
            let skills = outcome["skills"].as_array().cloned().unwrap_or_default();
            club.released.insert(
                r.get::<String>("student")?,
                json!({
                    "revision": outcome["revision"],
                    "releasedDate": r.get::<String>("released_date").ok(),
                    "skills": skill_levels(&club.activity, &skills),
                    "feedback": outcome["feedback"],
                    "achievements": outcome["achievements"],
                }),
            );
        }
        // Match results are team records, visible to everyone who can see the activity.
        let mut recorded = graph
            .execute(
                query("MATCH(m:ExtraCurricularMeetingRecord {activity_key:$key}) WHERE m.result IS NOT NULL RETURN m.meeting_id AS id,m.result AS result ORDER BY id LIMIT 101")
                    .param("key", key.clone()),
            )
            .await?;
        while let Some(r) = recorded.next().await? {
            if club.results.len() >= 100 {
                return Err("result limit exceeded".into());
            }
            let id: String = r.get("id")?;
            let result: Value = serde_json::from_str(&r.get::<String>("result")?)?;
            let meeting = club.activity.meetings.iter().find(|m| m.id == id);
            club.results.push(json!({
                "meetingId": id,
                "date": meeting.map(|m| m.date.clone()),
                "title": meeting.map(|m| m.title.clone()),
                "outcome": result["outcome"],
                "score": result["score"],
            }));
        }
    }
    let students: Vec<Value> = roster
        .into_iter()
        .map(|(id, name)| {
            let activities: Vec<Value> = enrolments
                .remove(&id)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(key, enrolment)| {
                    let club = clubs.get(&key)?;
                    let attendance = Attendance::tally(
                        club.marks
                            .get(&id)
                            .map(|v| v.iter().map(String::as_str).collect::<Vec<_>>())
                            .unwrap_or_default(),
                    );
                    Some(json!({
                        "activityId": club.activity.id,
                        "title": club.activity.title,
                        "category": club.activity.category,
                        "competitive": club.activity.competitive,
                        "archived": club.archived,
                        "academicYear": club.year,
                        "coachName": club.coach_name,
                        "enrollment": {
                            "status": enrolment["status"],
                            "role": enrolment["role"],
                            "roleStatus": enrolment["roleStatus"],
                            "jersey": enrolment["jersey"],
                        },
                        "meetings": {
                            "scheduled": club.activity.meetings.len(),
                            "recorded": club.recorded_meetings.len(),
                        },
                        "attendance": attendance,
                        "outcome": club.released.get(&id).cloned().unwrap_or(Value::Null),
                        "results": club.results,
                    }))
                })
                .collect();
            json!({"studentId": id, "name": name, "activities": activities})
        })
        .collect();
    let out = json!({"sectionId": section, "students": students});
    if serde_json::to_vec(&out)?.len() > 2_000_000 {
        return Err("response too large".into());
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests;
