use super::super::model::{Preferences, PublishedRecord, Rsvp};
use super::{parent_write_prefix, Error, PARENT, PARENT_SECTION};
use neo4rs::{query, Graph};
use serde_json::{json, Value};
pub async fn parent_snapshot(graph: &Graph, sub: &str) -> Result<Value, Error> {
    let mut owned = graph
        .execute(query(&format!("{PARENT} RETURN count(DISTINCT u)=1 AS owned")).param("sub", sub))
        .await?;
    if !owned
        .next()
        .await?
        .is_some_and(|r| r.get::<bool>("owned").unwrap_or(false))
    {
        return Err("Parent ownership unavailable".into());
    }
    let mut published = vec![];
    for kind in ["event", "project", "grade", "report"] {
        let mut records=graph.execute(query(&format!("{PARENT} WITH DISTINCT u,s {PARENT_SECTION} MATCH(r:SchoolPublishedRecord)-[:FOR_SECTION]->(sec) WHERE (r.student_id='' OR r.student_id=s.studentId) AND r.kind=$kind OPTIONAL MATCH(reply:SchoolRsvp {{key:u.id+'|'+s.studentId+'|'+r.key}}) RETURN DISTINCT s.studentId AS student,r.payload AS payload,toString(r.created_at) AS created,reply.response AS response ORDER BY created DESC LIMIT 100")).param("sub",sub).param("kind",kind)).await?;
        while let Some(r) = records.next().await? {
            let raw: String = r.get("payload")?;
            published.push(json!({"studentId":r.get::<String>("student")?,"content":serde_json::from_str::<Value>(&raw)?,"createdAt":r.get::<String>("created")?,"response":r.get::<String>("response").ok()}));
        }
    }
    let education_calendar = crate::education_calendar::repository::published(graph, query(&format!("{PARENT} WITH DISTINCT u,s {PARENT_SECTION} {} RETURN DISTINCT r.payload AS payload,r.version AS version,s.studentId AS student,r.start_date AS calendar_start,r.id AS calendar_id ORDER BY calendar_start,calendar_id,student LIMIT 101", crate::education_calendar::repository::PUBLISHED)).param("sub",sub)).await?;
    let class_meetings=crate::learning_access::timetable::reads::published(graph,query(&format!("{PARENT} WITH DISTINCT u,s {PARENT_SECTION} {} RETURN DISTINCT m.payload AS payload,s.studentId AS student,m.starts_at AS starts,m.id AS id ORDER BY starts,id,student LIMIT 201",crate::learning_access::timetable::reads::PUBLISHED)).param("sub",sub)).await?;
    let mut arrivals=graph.execute(query(&format!("{PARENT} WITH DISTINCT s {PARENT_SECTION} MATCH(a:AttendanceRecord {{section_id:sec.section_id,applicant_student_id:s.studentId}}) RETURN DISTINCT s.studentId AS student,a.date AS date,a.arrived_at AS arrived,a.dismissed_at AS dismissed ORDER BY date DESC LIMIT 1000")).param("sub",sub)).await?;
    let mut attendance = vec![];
    while let Some(r) = arrivals.next().await? {
        attendance.push(json!({"studentId":r.get::<String>("student")?,"date":r.get::<String>("date")?,"arrivedAt":r.get::<String>("arrived").ok().filter(|s|!s.is_empty()),"dismissedAt":r.get::<String>("dismissed").ok().filter(|s|!s.is_empty())}));
    }
    let mut notices=graph.execute(query(&format!("{PARENT} WITH DISTINCT u MATCH(u)-[:HAS_SCHOOL_NOTICE]->(n:SchoolNotice) RETURN DISTINCT n.id AS id,n.title AS title,n.body AS body,n.is_read AS read,toString(n.created_at) AS created ORDER BY created DESC LIMIT 100")).param("sub",sub)).await?;
    let mut messages = vec![];
    while let Some(r) = notices.next().await? {
        messages.push(json!({"id":r.get::<String>("id")?,"title":r.get::<String>("title")?,"body":r.get::<String>("body")?,"read":r.get::<bool>("read")?,"createdAt":r.get::<String>("created")?}));
    }
    let mut prefs=graph.execute(query(&format!("{PARENT} WITH DISTINCT u RETURN coalesce(u.school_preferences_version,0) AS version,coalesce(u.school_locale,'en') AS locale,coalesce(u.school_theme,'system') AS theme,coalesce(u.school_notifications,true) AS notifications")).param("sub",sub)).await?;
    let r = prefs.next().await?.ok_or("Parent missing")?;
    let preferences = json!({"version":r.get::<i64>("version")?,"locale":r.get::<String>("locale")?,"theme":r.get::<String>("theme")?,"notifications":r.get::<bool>("notifications")?});
    Ok(
        json!({"records":published,"educationCalendar":education_calendar,"classMeetings":class_meetings,"attendance":attendance,"messages":messages,"preferences":preferences}),
    )
}
pub async fn preferences(graph: &Graph, sub: &str, input: &Preferences) -> Result<bool, Error> {
    let q=query(&format!("{} WITH DISTINCT u WHERE coalesce(u.school_preferences_version,0)=$version SET u.school_preferences_version=$version+1,u.school_locale=$locale,u.school_theme=$theme,u.school_notifications=$notifications CREATE(:SchoolPortalAudit {{id:$audit,actor_id:u.id,kind:'preferences',created_at:datetime()}}) RETURN true AS saved",parent_write_prefix())).param("sub",sub).param("version",i64::from(input.version)).param("locale",input.locale.clone()).param("theme",input.theme.clone()).param("notifications",input.notifications).param("audit",uuid::Uuid::new_v4().to_string());
    let mut rows = graph.execute(q).await?;
    let saved = rows.next().await?.is_some();
    while rows.next().await?.is_some() {}
    Ok(saved)
}
pub async fn rsvp(graph: &Graph, sub: &str, id: &str, input: &Rsvp) -> Result<bool, Error> {
    let q=query(&format!("{PARENT} WITH DISTINCT u,s WHERE s.studentId=$student {PARENT_SECTION} WITH u,s,sec MATCH(r:SchoolPublishedRecord {{kind:'event'}})-[:FOR_SECTION]->(sec) WHERE r.key=sec.section_id+'|event|'+$id RETURN u.id AS owner,r.key AS key,r.payload AS payload")).param("sub",sub).param("student",input.student_id.clone()).param("id",id);
    let mut rows = graph.execute(q).await?;
    let Some(r) = rows.next().await? else {
        return Ok(false);
    };
    let raw: String = r.get("payload")?;
    let event: PublishedRecord = serde_json::from_str(&raw)?;
    if !matches!(event, PublishedRecord::Event { rsvp: true, .. }) {
        return Ok(false);
    }
    let key: String = r.get("key")?;
    let owner: String = r.get("owner")?;
    // Recheck ownership and current enrollment within the write transaction.
    let q=query(&format!("{} WITH DISTINCT u,s WHERE u.id=$owner AND s.studentId=$student {PARENT_SECTION} WITH DISTINCT u,s,sec MATCH(r:SchoolPublishedRecord {{key:$record,payload:$payload}})-[:FOR_SECTION]->(sec) MERGE(reply:SchoolRsvp {{key:u.id+'|'+s.studentId+'|'+r.key}}) SET reply.response=$response,reply.updated_at=datetime() CREATE(:SchoolPortalAudit {{id:$audit,actor_id:u.id,kind:'rsvp',school_id:sec.school_id,tenant_id:sec.tenant_id,section_id:sec.section_id,created_at:datetime()}}) RETURN true AS saved",parent_write_prefix())).param("sub",sub).param("owner",owner).param("student",input.student_id.clone()).param("record",key).param("payload",raw).param("response",if input.response=="clear"{""}else{&input.response}).param("audit",uuid::Uuid::new_v4().to_string());
    let mut rows = graph.execute(q).await?;
    let saved = rows.next().await?.is_some();
    while rows.next().await?.is_some() {}
    Ok(saved)
}
pub async fn mark_read(graph: &Graph, sub: &str, id: &str) -> Result<bool, Error> {
    let q=query(&format!("{} WITH DISTINCT u MATCH(u)-[:HAS_SCHOOL_NOTICE]->(n:SchoolNotice {{id:$id}}) SET n.is_read=true RETURN true AS saved",parent_write_prefix())).param("sub",sub).param("id",id);
    let mut rows = graph.execute(q).await?;
    let saved = rows.next().await?.is_some();
    while rows.next().await?.is_some() {}
    Ok(saved)
}

pub async fn parent_calendar(
    graph: &Graph,
    sub: &str,
    w: &crate::education_calendar::window::Window,
) -> Result<Option<Value>, Error> {
    let student = w.student_id.as_deref().ok_or("missing student")?;
    let mut check=graph.execute(query(&format!("{PARENT} AND s.studentId=$student WITH DISTINCT u,s {PARENT_SECTION} RETURN count(DISTINCT u)=1 AS allowed")).param("sub",sub).param("student",student)).await?;
    if !check
        .next()
        .await?
        .is_some_and(|r| r.get::<bool>("allowed").unwrap_or(false))
    {
        return Ok(None);
    }
    let q = query(&format!(
        "{PARENT} AND s.studentId=$student WITH DISTINCT u,s {PARENT_SECTION} {} {}",
        crate::education_calendar::repository::PUBLISHED,
        w.clause()
    ))
    .param("sub", sub)
    .param("student", student)
    .param("from", w.from.clone())
    .param("to", w.to.clone())
    .param("after", w.cursor()?);
    let mut page = crate::education_calendar::window::page(graph, q).await?;
    let (start, end) = crate::learning_access::timetable::reads::window_ms(&w.from, &w.to)?;
    let q=query(&format!("{PARENT} AND s.studentId=$student WITH DISTINCT u,s {PARENT_SECTION} {} AND m.starts_at<$end AND m.starts_at>=$earliest RETURN DISTINCT m.payload AS payload,s.studentId AS student,m.starts_at AS starts,m.id AS id ORDER BY starts,id LIMIT 201",crate::learning_access::timetable::reads::PUBLISHED)).param("sub",sub).param("student",student).param("end",end as i64).param("earliest",start.saturating_sub(12*3_600_000) as i64);
    page["classMeetings"] =
        crate::learning_access::timetable::reads::published_in_window(graph, q, start, end).await?;
    Ok(Some(page))
}
