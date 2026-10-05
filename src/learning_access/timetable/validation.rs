use super::{failure, model::Meeting, Failure};
use axum::http::StatusCode;
use neo4rs::{query, Txn};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Published {
    school_id: String,
    tenant_id: String,
    class_id: String,
    teacher_id: String,
    starts_at: u64,
    ends_at: u64,
    room_id: Option<String>,
}
pub(super) async fn issues(
    tx: &mut Txn,
    m: &Meeting,
    now: u64,
) -> Result<Vec<&'static str>, Failure> {
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let mut issues = m.time_issues(now);
    let (start, end) = m.dates().ok_or_else(unavailable)?;
    let mut calendars=tx.execute(query("MATCH(r:EducationCalendarEntry {school_id:$school,tenant_id:$tenant,academic_year:$year,status:'published'}) WHERE r.audience='school' OR (r.audience='sections' AND $class IN r.section_ids) RETURN r.payload AS payload LIMIT 1001").param("school",m.school_id.clone()).param("tenant",m.tenant_id.clone()).param("year",m.academic_year.clone()).param("class",m.class_id.clone())).await.map_err(|_|unavailable())?;
    let mut years = vec![];
    let mut count = 0;
    while let Some(row) = calendars
        .next(tx.handle())
        .await
        .map_err(|_| unavailable())?
    {
        count += 1;
        if count > 1000 {
            return Err(unavailable());
        }
        let raw: String = row.get("payload").map_err(|_| unavailable())?;
        let e: crate::education_calendar::model::Entry =
            serde_json::from_str(&raw).map_err(|_| unavailable())?;
        if !e.valid()
            || e.school_id != m.school_id
            || e.tenant_id != m.tenant_id
            || e.academic_year != m.academic_year
        {
            return Err(unavailable());
        }
        if e.audience == "sections" && !e.section_ids.contains(&m.class_id) {
            return Err(unavailable());
        }
        let from = chrono::NaiveDate::parse_from_str(&e.start_date, "%Y-%m-%d")
            .map_err(|_| unavailable())?;
        let to = chrono::NaiveDate::parse_from_str(&e.end_date, "%Y-%m-%d")
            .map_err(|_| unavailable())?;
        if e.kind == "academic_year" {
            years.push((from, to));
        }
        if e.kind == "holiday" && from <= end && to >= start {
            issues.push("SCHOOL_HOLIDAY");
        }
    }
    match years.as_slice() {
        [] => issues.push("ACADEMIC_YEAR_MISSING"),
        [(from, to)] if *from <= start && *to >= end => {}
        [_] => issues.push("OUTSIDE_ACADEMIC_YEAR"),
        _ => issues.push("ACADEMIC_YEAR_AMBIGUOUS"),
    }
    // Every canonical legacy meeting was bounded to <=12h. Parse its real payload
    // for endsAt/room metadata: old records need no destructive backfill.
    let mut rows=tx.execute(query("MATCH(p:LearningClassMeeting) WHERE coalesce(p.status,'published')='published' AND p.key<>$key AND p.starts_at>=$earliest AND p.starts_at<$end AND (p.teacher_id=$teacher OR (p.school_id=$school AND p.tenant_id=$tenant AND (p.class_id=$class OR toLower(coalesce(p.room_id,''))=$room))) RETURN p.payload AS payload LIMIT 1001").param("key",m.key()).param("earliest",m.starts_at.saturating_sub(12*3_600_000) as i64).param("end",m.ends_at as i64).param("teacher",m.teacher_id.clone()).param("school",m.school_id.clone()).param("tenant",m.tenant_id.clone()).param("class",m.class_id.clone()).param("room",m.room_id.clone())).await.map_err(|_|unavailable())?;
    let mut count = 0;
    while let Some(row) = rows.next(tx.handle()).await.map_err(|_| unavailable())? {
        count += 1;
        if count > 1000 {
            return Err(unavailable());
        }
        let raw: String = row.get("payload").map_err(|_| unavailable())?;
        let old: Published = serde_json::from_str(&raw).map_err(|_| unavailable())?;
        if old.starts_at >= old.ends_at || old.ends_at - old.starts_at > 12 * 3_600_000 {
            return Err(unavailable());
        }
        if old.starts_at < m.ends_at && m.starts_at < old.ends_at {
            if old.teacher_id == m.teacher_id {
                issues.push("TEACHER_CONFLICT");
            }
            if old.school_id == m.school_id && old.tenant_id == m.tenant_id {
                if old.class_id == m.class_id {
                    issues.push("CLASS_CONFLICT");
                }
                if old
                    .room_id
                    .as_deref()
                    .is_some_and(|room| room.eq_ignore_ascii_case(&m.room_id))
                {
                    issues.push("ROOM_CONFLICT");
                }
            }
        }
    }
    issues.sort_unstable();
    issues.dedup();
    Ok(issues)
}
