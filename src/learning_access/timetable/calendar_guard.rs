//! Calendar changes cannot invalidate already published future teaching slots.
use super::{failure, invalid, Failure};
use crate::education_calendar::model::Change;
use axum::http::StatusCode;
use neo4rs::{query, Txn};
pub(crate) async fn check(tx: &mut Txn, c: &Change) -> Result<(), Failure> {
    let e = &c.entry;
    if c.action == "save_draft"
        || !(e.kind == "academic_year" || e.kind == "holiday" && c.action == "publish")
    {
        return Ok(());
    }
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    // Replacing/removing year coverage also affects classes excluded by the proposed audience.
    // Holidays affect only the proposed audience; years must preserve every published lesson.
    let scope = |q: neo4rs::Query| {
        q.param("school", e.school_id.clone())
            .param("tenant", e.tenant_id.clone())
            .param("year", e.academic_year.clone())
            .param("audience", e.audience.clone())
            .param("sections", e.section_ids.clone())
    };
    const FUTURE: &str = "MATCH(p:LearningClassMeeting {school_id:$school,tenant_id:$tenant}),(sec:Section {section_id:p.class_id,school_id:$school,tenant_id:$tenant}) WHERE coalesce(p.status,'published')='published' AND p.starts_at>timestamp() AND coalesce(p.academic_year,sec.academic_year)=$year";
    if e.kind == "holiday" {
        // Only meetings overlapping the holiday's WIB dates can conflict: bounded by an index range.
        let (from, to) =
            super::reads::window_ms(&e.start_date, &e.end_date).map_err(|_| unavailable())?;
        let row = tx
            .execute(scope(query(&format!("{FUTURE} AND p.starts_at<$to AND coalesce(p.ends_at,p.starts_at+1)>$from AND ($audience='school' OR p.class_id IN $sections) RETURN count(p) AS n"))).param("from", from as i64).param("to", to as i64))
            .await
            .map_err(|_| unavailable())?
            .next(tx.handle())
            .await
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        if row.get::<i64>("n").map_err(|_| unavailable())? > 0 {
            return Err(invalid(vec!["PUBLISHED_MEETING_CONFLICT"]));
        }
        return Ok(());
    }
    // Academic years: stream indexed meeting times instead of parsing payloads; a whole year of
    // weekly timetables for every class stays far below the cap.
    let mut rows = tx
        .execute(scope(query(&format!("{FUTURE} RETURN p.starts_at AS starts,coalesce(p.ends_at,p.starts_at) AS ends,sec.section_id AS class LIMIT 20001"))))
        .await
        .map_err(|_| unavailable())?;
    let mut meetings = vec![];
    let date = |ms: i64| {
        chrono::DateTime::from_timestamp_millis(ms + 25_200_000)
            .map(|t| t.date_naive().to_string())
            .ok_or_else(unavailable)
    };
    while let Some(r) = rows.next(tx.handle()).await.map_err(|_| unavailable())? {
        if meetings.len() == 20_000 {
            return Err(unavailable());
        }
        let starts: i64 = r.get("starts").map_err(|_| unavailable())?;
        let ends: i64 = r.get("ends").map_err(|_| unavailable())?;
        meetings.push((
            r.get::<String>("class").map_err(|_| unavailable())?,
            date(starts)?,
            date(ends.max(starts + 1) - 1)?,
        ));
    }
    if meetings.is_empty() {
        return Ok(());
    }
    let mut rows=tx.execute(query("MATCH(r:EducationCalendarEntry {school_id:$school,tenant_id:$tenant,academic_year:$year,status:'published'}) WHERE r.id<>$id RETURN r.payload AS payload LIMIT 1001").param("school",e.school_id.clone()).param("tenant",e.tenant_id.clone()).param("year",e.academic_year.clone()).param("id",e.id.clone())).await.map_err(|_|unavailable())?;
    let mut years = vec![];
    let mut count = 0;
    while let Some(r) = rows.next(tx.handle()).await.map_err(|_| unavailable())? {
        count += 1;
        if count > 1000 {
            return Err(unavailable());
        }
        let raw: String = r.get("payload").map_err(|_| unavailable())?;
        let entry: crate::education_calendar::model::Entry =
            serde_json::from_str(&raw).map_err(|_| unavailable())?;
        if !entry.valid() {
            return Err(unavailable());
        }
        if entry.kind == "academic_year" {
            years.push(entry);
        }
    }
    if c.action == "publish" {
        years.push(e.clone());
    }
    for (class, start, end) in meetings {
        let applicable = years
            .iter()
            .filter(|year| year.audience == "school" || year.section_ids.contains(&class))
            .collect::<Vec<_>>();
        if applicable.len() != 1 || applicable[0].start_date > start || applicable[0].end_date < end
        {
            return Err(invalid(vec!["PUBLISHED_MEETING_CONFLICT"]));
        }
    }
    Ok(())
}
