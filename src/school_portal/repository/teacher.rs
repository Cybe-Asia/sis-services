use super::super::{auth::Teacher, model::*};
use super::{teacher_query, values, Error, ENROLLED};
use axum::http::HeaderMap;
use neo4rs::{Graph, Query};
use serde_json::{json, Value};
// JSON serialization happens in Rust; no APOC dependency on production graphs.
pub async fn context(graph: &Graph, member: &Teacher) -> Result<Value, Error> {
    let mut rows=graph.execute(teacher_query(member,"RETURN DISTINCT sec.section_id AS id,sec.name AS name,sec.year_group AS year,sec.school_id AS school,sec.tenant_id AS tenant,homeroom,management ORDER BY name LIMIT 201",None,false,false)).await?;
    let mut items = vec![];
    while let Some(r) = rows.next().await? {
        if items.len() >= 200 {
            return Err("class limit exceeded".into());
        }
        items.push(json!({"id":r.get::<String>("id")?,"name":r.get::<String>("name")?,"year":r.get::<String>("year")?,"schoolId":r.get::<String>("school")?,"tenantId":r.get::<String>("tenant")?,"homeroom":r.get::<bool>("homeroom")?,"canManageHomeroom":r.get::<bool>("management")?}));
    }
    Ok(json!({"actor":{"id":member.staff_member_id,"role":member.role()},"classes":items}))
}
pub async fn classroom(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    day: &str,
) -> Result<Option<Value>, Error> {
    let mut check = graph
        .execute(teacher_query(
            member,
            "RETURN homeroom,management",
            Some(section),
            false,
            false,
        ))
        .await?;
    let Some(row) = check.next().await? else {
        return Ok(None);
    };
    let homeroom: bool = row.get("homeroom")?;
    let management: bool = row.get("management")?;
    let roster_query=format!("{ENROLLED} OPTIONAL MATCH(a:AttendanceRecord {{section_id:sec.section_id,applicant_student_id:s.studentId,date:$date}}) RETURN DISTINCT s.studentId AS id,s.fullName AS name,a.status AS status,a.arrived_at AS arrived,a.dismissed_at AS dismissed,a.notes AS note ORDER BY name LIMIT 201");
    let mut roster = graph
        .execute(
            teacher_query(member, &roster_query, Some(section), false, false).param("date", day),
        )
        .await?;
    let mut students = vec![];
    while let Some(r) = roster.next().await? {
        if students.len() >= 200 {
            return Err("roster limit exceeded".into());
        }
        students.push(json!({"id":r.get::<String>("id")?,"name":r.get::<String>("name")?,"status":r.get::<String>("status").ok(),"arrivedAt":r.get::<String>("arrived").ok(),"dismissedAt":r.get::<String>("dismissed").ok(),"note":r.get::<String>("note").unwrap_or_default()}));
    }
    // Full Growth reports include homeroom notes and large portfolios. They
    // belong to the selected-child report read, not a subject Teacher roster.
    let mut records=values(graph,teacher_query(member,"MATCH(r:SchoolPublishedRecord)-[:FOR_SECTION]->(sec) WHERE NOT coalesce(r.kind,'') IN ['growth','announcement'] RETURN r.payload AS payload ORDER BY r.created_at DESC LIMIT 101",Some(section),false,false)).await?;
    let records_have_more = records.len() > 100;
    records.truncate(100);
    // Announcements are served apart from `records`, so a client that predates them keeps reading
    // the class. Each row carries the time it was published.
    let mut published = graph.execute(teacher_query(member,"MATCH(r:SchoolPublishedRecord {kind:'announcement'})-[:FOR_SECTION]->(sec) RETURN r.payload AS payload,toString(r.created_at) AS created ORDER BY r.created_at DESC LIMIT 100",Some(section),false,false)).await?;
    let mut announcements = vec![];
    while let Some(r) = published.next().await? {
        announcements.push(json!({"content":serde_json::from_str::<Value>(&r.get::<String>("payload")?)?,"createdAt":r.get::<String>("created")?}));
    }
    let education_calendar = crate::education_calendar::repository::published(graph, teacher_query(member, &format!("{} RETURN DISTINCT r.payload AS payload,r.version AS version,r.start_date AS calendar_start,r.id AS calendar_id ORDER BY calendar_start,calendar_id LIMIT 101", crate::education_calendar::repository::PUBLISHED),Some(section),false,false)).await?;
    let class_meetings=crate::learning_access::timetable::reads::published(graph,teacher_query(member,&format!("{} RETURN DISTINCT m.payload AS payload,m.starts_at AS starts,m.id AS id ORDER BY starts,id LIMIT 201",crate::learning_access::timetable::reads::PUBLISHED),Some(section),false,false)).await?;
    let request_query=format!("{ENROLLED} MATCH(r:ParentSchoolRequest {{section_id:sec.section_id,school_id:sec.school_id,tenant_id:sec.tenant_id}})-[:FOR_STUDENT]->(s) RETURN DISTINCT r.public_id AS id,s.studentId AS student,s.fullName AS name,r.kind AS kind,r.date AS date,r.time AS time,r.reason AS reason,r.note AS note,r.status AS status,r.review_note AS reviewNote,r.createdAt AS createdAt ORDER BY createdAt DESC LIMIT 101");
    let mut requests = vec![];
    if homeroom || management {
        let mut rows = graph
            .execute(teacher_query(
                member,
                &request_query,
                Some(section),
                false,
                true,
            ))
            .await?;
        while let Some(r) = rows.next().await? {
            if requests.len() >= 100 {
                break;
            }
            requests.push(json!({"id":r.get::<String>("id")?,"studentId":r.get::<String>("student")?,"studentName":r.get::<String>("name")?,"kind":r.get::<String>("kind")?,"date":r.get::<String>("date")?,"time":r.get::<String>("time").ok(),"reason":r.get::<String>("reason").ok().filter(|v|!v.is_empty()),"note":r.get::<String>("note")?,"status":r.get::<String>("status")?,"reviewNote":r.get::<String>("reviewNote").ok()}));
        }
    }
    let rsvp_query=format!("{ENROLLED} MATCH(r:SchoolPublishedRecord {{kind:'event'}})-[:FOR_SECTION]->(sec) MATCH(u:User)-[:HAS_APPLICATION]->(:Lead)-[:HAS_STUDENT]->(s) MATCH(reply:SchoolRsvp) WHERE reply.key=u.id+'|'+s.studentId+'|'+r.key AND reply.response IN ['attending','declined'] RETURN r.payload AS event,s.studentId AS student,s.fullName AS name,sum(CASE WHEN reply.response='attending' THEN 1 ELSE 0 END) AS attending,sum(CASE WHEN reply.response='declined' THEN 1 ELSE 0 END) AS declined,max(reply.updated_at) AS updated ORDER BY updated DESC LIMIT 100");
    let mut replies = graph
        .execute(teacher_query(
            member,
            &rsvp_query,
            Some(section),
            false,
            false,
        ))
        .await?;
    let mut rsvps = vec![];
    while let Some(r) = replies.next().await? {
        let event: Value = serde_json::from_str(&r.get::<String>("event")?)?;
        rsvps.push(json!({"eventId":event["id"],"eventTitle":event["title"],"studentId":r.get::<String>("student")?,"studentName":r.get::<String>("name")?,"attending":r.get::<i64>("attending")?,"declined":r.get::<i64>("declined")?}));
    }
    // Who approved or rejected is the homeroom's business, like Parent requests.
    let mut announcement_replies = vec![];
    if homeroom || management {
        let reply_query=format!("{ENROLLED} MATCH(r:SchoolPublishedRecord {{kind:'announcement'}})-[:FOR_SECTION]->(sec) MATCH(u:User)-[:HAS_APPLICATION]->(:Lead)-[:HAS_STUDENT]->(s) MATCH(reply:SchoolRsvp) WHERE reply.key=u.id+'|'+s.studentId+'|'+r.key AND reply.response IN ['approved','rejected'] RETURN r.payload AS announcement,s.studentId AS student,s.fullName AS name,sum(CASE WHEN reply.response='approved' THEN 1 ELSE 0 END) AS approved,sum(CASE WHEN reply.response='rejected' THEN 1 ELSE 0 END) AS rejected,max(reply.updated_at) AS updated ORDER BY updated DESC LIMIT 200");
        let mut rows = graph
            .execute(teacher_query(
                member,
                &reply_query,
                Some(section),
                false,
                true,
            ))
            .await?;
        while let Some(r) = rows.next().await? {
            let announcement: Value = serde_json::from_str(&r.get::<String>("announcement")?)?;
            announcement_replies.push(json!({"announcementId":announcement["id"],"announcementTitle":announcement["title"],"studentId":r.get::<String>("student")?,"studentName":r.get::<String>("name")?,"approved":r.get::<i64>("approved")?,"rejected":r.get::<i64>("rejected")?}));
        }
    }
    Ok(Some(
        json!({"homeroom":homeroom,"canManageHomeroom":management,"students":students,"records":records,"announcements":announcements,"announcementReplies":announcement_replies,"educationCalendar":education_calendar,"classMeetings":class_meetings,"recordWindow":{"limit":100,"hasMore":records_have_more},"requests":requests,"rsvps":rsvps}),
    ))
}
async fn attendance_write(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    input: &RollCall,
    headers: Option<&HeaderMap>,
) -> Result<bool, Error> {
    let scope=format!("{ENROLLED} WITH t,sec,collect(DISTINCT s.studentId) AS roster WHERE all(id IN $ids WHERE id IN roster) UNWIND range(0,size($ids)-1) AS i MATCH(s:Student {{studentId:$ids[i]}}) MERGE(a:AttendanceRecord {{section_id:sec.section_id,applicant_student_id:s.studentId,date:$date}}) ON CREATE SET a.record_id='ATT-'+randomUUID() SET a.status=$statuses[i],a.arrived_at=$arrived[i],a.dismissed_at=$dismissed[i],a.notes=$notes[i],a.recorded_at=datetime(),a.recorded_by=t.id MERGE(s)-[:HAS_ATTENDANCE]->(a) WITH DISTINCT t,sec CREATE(:SchoolPortalAudit {{id:$audit,actor_id:t.id,actor_role:$actor_role,school_id:sec.school_id,tenant_id:sec.tenant_id,section_id:sec.section_id,kind:'attendance',date:$date,created_at:datetime()}}) RETURN true AS saved");
    let q = teacher_query(member, &scope, Some(section), true, true)
        .param(
            "ids",
            input
                .entries
                .iter()
                .map(|e| e.student_id.clone())
                .collect::<Vec<_>>(),
        )
        .param("date", input.date.clone())
        .param(
            "statuses",
            input
                .entries
                .iter()
                .map(|e| e.status.clone())
                .collect::<Vec<_>>(),
        )
        .param(
            "arrived",
            input
                .entries
                .iter()
                .map(|e| e.arrived_at.clone().unwrap_or_default())
                .collect::<Vec<_>>(),
        )
        .param(
            "dismissed",
            input
                .entries
                .iter()
                .map(|e| e.dismissed_at.clone().unwrap_or_default())
                .collect::<Vec<_>>(),
        )
        .param(
            "notes",
            input
                .entries
                .iter()
                .map(|e| e.note.clone())
                .collect::<Vec<_>>(),
        )
        .param("audit", uuid::Uuid::new_v4().to_string());
    execute_write(graph, member, section, q, headers).await
}
async fn publish_write(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    input: &PublishedRecord,
    headers: Option<&HeaderMap>,
) -> Result<bool, Error> {
    let grade = if let PublishedRecord::Grade {
        subject,
        term,
        score,
        maximum,
        ..
    } = input
    {
        Some((subject, term, *score, *maximum))
    } else {
        None
    };
    let grade_write = if grade.is_some() {
        " WITH t,sec,r MATCH(student:Student {studentId:$student}) FOREACH(ignore IN CASE WHEN r.write_attempt=$attempt THEN [1] ELSE [] END | MERGE(g:GradeEntry {section_id:sec.section_id,applicant_student_id:$student,subject:$subject,term:$term}) ON CREATE SET g.entry_id='GRADE-'+randomUUID() SET g.score=$score,g.max_score=$maximum,g.recorded_at=datetime(),g.recorded_by=t.id MERGE(student)-[:HAS_GRADE]->(g))"
    } else {
        ""
    };
    let body=format!("WHERE $student='' OR EXISTS {{ {ENROLLED} WHERE s.studentId=$student }} MERGE(r:SchoolPublishedRecord {{key:sec.section_id+'|'+$kind+'|'+$id}}) ON CREATE SET r.write_attempt=$attempt,r.payload=$payload,r.kind=$kind,r.student_id=$student,r.created_at=datetime(),r.actor_id=t.id WITH t,sec,r WHERE r.payload=$payload MERGE(r)-[:FOR_SECTION]->(sec) {grade_write} MERGE(a:SchoolPortalAudit {{id:'record|'+r.key}}) ON CREATE SET a.actor_id=t.id,a.actor_role=$actor_role,a.school_id=sec.school_id,a.tenant_id=sec.tenant_id,a.section_id=sec.section_id,a.kind=$kind,a.created_at=datetime() RETURN true AS saved");
    let mut q = teacher_query(
        member,
        &body,
        Some(section),
        true,
        matches!(
            input,
            PublishedRecord::Report { .. }
                | PublishedRecord::Growth { .. }
                | PublishedRecord::Announcement { .. }
        ),
    )
    .param("student", input.student().unwrap_or(""))
    .param("kind", input.kind())
    .param("id", input.id())
    .param("payload", serde_json::to_string(input)?)
    .param("attempt", uuid::Uuid::new_v4().to_string());
    if let Some((subject, term, score, maximum)) = grade {
        q = q
            .param("subject", subject.clone())
            .param("term", term.clone())
            .param("score", score)
            .param("maximum", maximum);
    }
    execute_write(graph, member, section, q, headers).await
}
async fn review_write(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    id: &str,
    input: &Review,
    headers: Option<&HeaderMap>,
) -> Result<bool, Error> {
    let body=format!("{ENROLLED} MATCH(r:ParentSchoolRequest {{public_id:$id,section_id:sec.section_id,school_id:sec.school_id,tenant_id:sec.tenant_id}})-[:FOR_STUDENT]->(s) MATCH(u:User)-[:CREATED_SCHOOL_REQUEST]->(r) WHERE r.status=$expected OR (r.status=$status AND r.reviewed_by=t.id AND r.review_note=$note) WITH DISTINCT t,sec,r,u SET r.status=$status,r.review_note=$note,r.reviewed_by=t.id,r.reviewed_at=coalesce(r.reviewed_at,datetime()) MERGE(n:SchoolNotice {{id:'school-request:'+r.public_id}}) ON CREATE SET n.title='School request reviewed',n.body=$notice,n.created_at=datetime(),n.is_read=false MERGE(u)-[:HAS_SCHOOL_NOTICE]->(n) MERGE(a:SchoolPortalAudit {{id:'review|'+r.public_id}}) ON CREATE SET a.actor_id=t.id,a.actor_role=$actor_role,a.school_id=sec.school_id,a.tenant_id=sec.tenant_id,a.section_id=sec.section_id,a.kind='request_review',a.status=$status,a.created_at=datetime() RETURN true AS saved");
    let q = teacher_query(member, &body, Some(section), true, true)
        .param("id", id)
        .param("expected", input.expected_status.clone())
        .param("status", input.status.clone())
        .param("note", input.note.clone())
        .param("notice", format!("{} · {}", input.status, input.note));
    execute_write(graph, member, section, q, headers).await
}

pub async fn teacher_calendar(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    w: &crate::education_calendar::window::Window,
) -> Result<Option<Value>, Error> {
    let mut check = graph
        .execute(teacher_query(
            member,
            "RETURN sec.section_id AS id",
            Some(section),
            false,
            false,
        ))
        .await?;
    if check.next().await?.is_none() {
        return Ok(None);
    }
    let body = format!(
        "{} {}",
        crate::education_calendar::repository::PUBLISHED,
        w.clause()
    );
    let q = teacher_query(member, &body, Some(section), false, false)
        .param("from", w.from.clone())
        .param("to", w.to.clone())
        .param("after", w.cursor()?);
    let mut page = crate::education_calendar::window::page(graph, q).await?;
    let (start, end) = crate::learning_access::timetable::reads::window_ms(&w.from, &w.to)?;
    let q=teacher_query(member,&format!("{} AND m.starts_at<$end AND m.starts_at>=$earliest RETURN DISTINCT m.payload AS payload,m.starts_at AS starts,m.id AS id ORDER BY starts,id LIMIT 201",crate::learning_access::timetable::reads::PUBLISHED),Some(section),false,false).param("end",end as i64).param("earliest",start.saturating_sub(12*3_600_000) as i64);
    page["classMeetings"] =
        crate::learning_access::timetable::reads::published_in_window(graph, q, start, end).await?;
    Ok(Some(page))
}

#[cfg(test)]
pub async fn attendance(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    input: &RollCall,
) -> Result<bool, Error> {
    attendance_write(graph, member, section, input, None).await
}
pub async fn attendance_live(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    input: &RollCall,
    headers: &HeaderMap,
) -> Result<bool, Error> {
    attendance_write(graph, member, section, input, Some(headers)).await
}

#[cfg(test)]
pub async fn publish(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    input: &PublishedRecord,
) -> Result<bool, Error> {
    publish_write(graph, member, section, input, None).await
}
pub async fn publish_live(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    input: &PublishedRecord,
    headers: &HeaderMap,
) -> Result<bool, Error> {
    publish_write(graph, member, section, input, Some(headers)).await
}

#[cfg(test)]
pub async fn review(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    id: &str,
    input: &Review,
) -> Result<bool, Error> {
    review_write(graph, member, section, id, input, None).await
}
pub async fn review_live(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    id: &str,
    input: &Review,
    headers: &HeaderMap,
) -> Result<bool, Error> {
    review_write(graph, member, section, id, input, Some(headers)).await
}

async fn execute_write(
    graph: &Graph,
    member: &Teacher,
    section: &str,
    q: Query,
    headers: Option<&HeaderMap>,
) -> Result<bool, Error> {
    if member.is_owner() {
        if let Some(headers) = headers {
            let school = member.school_ids.first().ok_or("missing Owner school")?;
            let tenant = member.tenant_ids.first().ok_or("missing Owner tenant")?;
            let lock = teacher_query(
                member,
                "RETURN sec.section_id AS id",
                Some(section),
                true,
                false,
            );
            return crate::school_portal::staff_capability::live_write(
                graph,
                crate::school_portal::staff_capability::OwnerSession {
                    headers,
                    actor: &member.staff_member_id,
                    school,
                    tenant,
                    learning: true,
                },
                lock,
                q,
            )
            .await
            .map(|row| row.is_some())
            .map_err(|e| {
                Box::new(crate::school_portal::staff_capability::SessionFailure(e)) as Error
            });
        }
    }
    let mut rows = graph.execute(q).await?;
    let saved = rows.next().await?.is_some();
    while rows.next().await?.is_some() {}
    Ok(saved)
}
