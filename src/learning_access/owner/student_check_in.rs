//! Student morning check-in from Learning: the attendance record of the school day in each enrolled section,
//! stamped with this server's WIB clock. A record that already exists (a homeroom roll-call or an earlier
//! check-in) is returned unchanged, so the homeroom teacher stays the authority over the day's attendance.
use super::*;
use chrono::{Duration, Timelike, Utc};

/// shortcut: one school start for every class (shared/student-attendance/v1: 07:30 WIB) and five minutes of
/// grace; read it from the class's first meeting of the day once every timetable is published.
const STARTS_AT: (u32, u32) = (7, 30);
const GRACE_MINUTES: u32 = 5;

/// `present` up to the start plus grace, `late` after it.
pub(super) fn status_at(hour: u32, minute: u32) -> &'static str {
    if hour * 60 + minute <= STARTS_AT.0 * 60 + STARTS_AT.1 + GRACE_MINUTES {
        "present"
    } else {
        "late"
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CheckInScope {
    school_id: String,
    tenant_id: String,
}

async fn write(s: &AppState, h: &HeaderMap, q: &CheckInScope) -> Result<Value, Failure> {
    if !valid_id(&q.school_id) || !valid_id(&q.tenant_id) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let scope = Scope {
        school_id: q.school_id.clone(),
        tenant_id: q.tenant_id.clone(),
        class_id: None,
        student_id: None,
        access: None,
    };
    let a = actor(&s.config, h, &scope).await?;
    if a.role != "student" {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let now = Utc::now() + Duration::hours(7);
    let date = now.date_naive().to_string();
    let arrived = format!("{:02}:{:02}", now.hour(), now.minute());
    let cypher = format!(
        "{STUDENT} WITH DISTINCT p,s,c \
         MERGE(r:AttendanceRecord {{section_id:c.section_id,applicant_student_id:s.studentId,date:$date}}) \
         ON CREATE SET r.record_id='ATT-'+randomUUID(),r.status=$status,r.arrived_at=$arrived, \
         r.recorded_at=datetime(),r.recorded_by=p.id,r.source='student_check_in' \
         MERGE(s)-[:HAS_ATTENDANCE]->(r) \
         RETURN DISTINCT s.studentId AS student,c.section_id AS section,r.status AS status, \
         r.arrived_at AS arrived,coalesce(r.source,'roll_call') AS source ORDER BY section"
    );
    let mut rows = s
        .graph
        .execute(
            query(&cypher)
                .param("principal", a.id.clone())
                .param("school", q.school_id.clone())
                .param("tenant", q.tenant_id.clone())
                .param("date", date.clone())
                .param("status", status_at(now.hour(), now.minute()))
                .param("arrived", arrived),
        )
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut first: Option<(String, String, Option<String>, String)> = None;
    let mut students = BTreeSet::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        let student: String = row
            .get("student")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        students.insert(student.clone());
        if first.is_none() {
            first = Some((
                student,
                row.get("status")
                    .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?,
                row.get::<String>("arrived").ok().filter(|v| !v.is_empty()),
                row.get("source")
                    .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?,
            ));
        }
    }
    // No active enrolment, or a binding that resolves to more than one Student, never writes for anyone.
    let (Some((student, status, arrived, source)), 1) = (first, students.len()) else {
        return Err(failure(StatusCode::FORBIDDEN));
    };
    Ok(json!({
        "contractVersion": 1,
        "databaseId": identity(&s.graph).await?,
        "actor": {"id": a.id, "role": a.role},
        "schoolId": q.school_id,
        "tenantId": q.tenant_id,
        "studentId": student,
        "date": date,
        "status": status,
        "arrivedAt": arrived,
        "source": source,
        "startsAt": format!("{:02}:{:02}", STARTS_AT.0, STARTS_AT.1),
    }))
}

pub(super) async fn handler(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<CheckInScope>,
) -> Result<Json<Value>, Failure> {
    tokio::time::timeout(std::time::Duration::from_secs(5), write(&s, &h, &q))
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .map(|data| Json(json!({ "data": data })))
}

#[cfg(test)]
mod tests {
    use super::status_at;
    #[test]
    fn on_time_until_five_minutes_after_the_start_then_late() {
        assert_eq!(status_at(6, 45), "present");
        assert_eq!(status_at(7, 35), "present");
        assert_eq!(status_at(7, 36), "late");
        assert_eq!(status_at(15, 19), "late");
    }
}
