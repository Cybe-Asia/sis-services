//! Calendar changes cannot invalidate already published future teaching slots.
use super::{failure, invalid, Failure};
use crate::education_calendar::model::Change;
use axum::http::StatusCode;
use neo4rs::{query, Txn};
use serde_json::Value;
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
    let mut rows=tx.execute(query("MATCH(p:LearningClassMeeting {school_id:$school,tenant_id:$tenant}),(sec:Section {section_id:p.class_id,school_id:$school,tenant_id:$tenant}) WHERE coalesce(p.status,'published')='published' AND p.starts_at>timestamp() AND coalesce(p.academic_year,sec.academic_year)=$year AND ($check_all_classes OR $audience='school' OR p.class_id IN $sections) RETURN p.payload AS payload,sec.section_id AS class LIMIT 1001").param("school",e.school_id.clone()).param("tenant",e.tenant_id.clone()).param("year",e.academic_year.clone()).param("check_all_classes",e.kind=="academic_year").param("audience",e.audience.clone()).param("sections",e.section_ids.clone())).await.map_err(|_|unavailable())?;
    let mut meetings = vec![];
    while let Some(r) = rows.next(tx.handle()).await.map_err(|_| unavailable())? {
        if meetings.len() == 1000 {
            return Err(unavailable());
        }
        let raw: String = r.get("payload").map_err(|_| unavailable())?;
        let value: Value = serde_json::from_str(&raw).map_err(|_| unavailable())?;
        let start = value["startsAt"]
            .as_i64()
            .and_then(|v| chrono::DateTime::from_timestamp_millis(v + 25_200_000))
            .ok_or_else(unavailable)?
            .date_naive()
            .to_string();
        let end = value["endsAt"]
            .as_i64()
            .and_then(|v| v.checked_sub(1))
            .and_then(|v| chrono::DateTime::from_timestamp_millis(v + 25_200_000))
            .ok_or_else(unavailable)?
            .date_naive()
            .to_string();
        meetings.push((
            r.get::<String>("class").map_err(|_| unavailable())?,
            start,
            end,
        ));
    }
    if meetings.is_empty() {
        return Ok(());
    }
    if e.kind == "holiday" {
        if meetings
            .iter()
            .any(|(_, start, end)| start <= &e.end_date && end >= &e.start_date)
        {
            return Err(invalid(vec!["PUBLISHED_MEETING_CONFLICT"]));
        }
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
