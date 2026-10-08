use super::{failure, model::Scope, repository, Failure};
use crate::AppState;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use neo4rs::{query, Graph, Query as GraphQuery};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Selection {
    school_id: String,
    tenant_id: String,
}
pub(super) async fn learning_context(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(scope): Query<Scope>,
) -> Result<Json<Value>, Failure> {
    if !scope.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (actor, role) = super::super::owner::class_actor(
        &state,
        &headers,
        &scope.school_id,
        &scope.tenant_id,
        &scope.class_id,
    )
    .await?;
    if !matches!(role.as_str(), "teacher" | "owner") {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let body=format!("{} OPTIONAL MATCH(t:StaffMember {{membershipStatus:'ACTIVE'}}) WHERE 'teacher' IN coalesce(t.roles,[]) AND sec.school_id IN coalesce(t.schoolIds,[]) AND sec.tenant_id IN coalesce(t.tenantIds,[]) AND NOT EXISTS {{MATCH(other:StaffMember {{id:t.id}}) WHERE other<>t}} AND (sec.homeroom_staff_member_id=t.id OR EXISTS {{MATCH(:LearningTeachingGrant {{staff_member_id:t.id,section_id:sec.section_id,school_id:sec.school_id,tenant_id:sec.tenant_id,status:'ACTIVE'}})}}) AND ($own=false OR t.id=$actor) RETURN sec.academic_year AS year,t.id AS teacher,coalesce(t.displayName,t.name,t.id) AS name ORDER BY teacher LIMIT 201",repository::authority(&role));
    let mut rows = state
        .graph
        .execute(
            query(&body)
                .param("actor", actor.clone())
                .param("school", scope.school_id.clone())
                .param("tenant", scope.tenant_id.clone())
                .param("class", scope.class_id.clone())
                .param("own", role == "teacher"),
        )
        .await
        .map_err(|_| unavailable())?;
    let mut teachers = vec![];
    let mut year = None;
    let mut seen = std::collections::BTreeSet::new();
    while let Some(row) = rows.next().await.map_err(|_| unavailable())? {
        let actual: String = row.get("year").map_err(|_| unavailable())?;
        if actual.trim().is_empty()
            || actual.len() > 32
            || year.as_ref().is_some_and(|y| y != &actual)
        {
            return Err(unavailable());
        }
        year = Some(actual);
        if let Some(id) = row
            .get::<Option<String>>("teacher")
            .map_err(|_| unavailable())?
        {
            if teachers.len() == 200 || !seen.insert(id.clone()) {
                return Err(unavailable());
            }
            teachers
                .push(json!({"id":id,"name":row.get::<String>("name").map_err(|_|unavailable())?}));
        }
    }
    let year = year.ok_or_else(|| failure(StatusCode::FORBIDDEN))?;
    let mut rows=state.graph.execute(query("CALL { MATCH(m:LearningClassMeeting {school_id:$school,tenant_id:$tenant}) WHERE coalesce(m.status,'published')='published' AND coalesce(m.room_id,'')<>'' RETURN m.room_id AS room UNION MATCH(r:SchoolRoom {school_id:$school,tenant_id:$tenant,status:'active'}) RETURN r.id AS room } RETURN DISTINCT room ORDER BY room LIMIT 201").param("school",scope.school_id.clone()).param("tenant",scope.tenant_id.clone())).await.map_err(|_|unavailable())?;
    let mut rooms = vec![];
    while let Some(row) = rows.next().await.map_err(|_| unavailable())? {
        if rooms.len() == 200 {
            return Err(unavailable());
        }
        rooms.push(row.get::<String>("room").map_err(|_| unavailable())?);
    }
    if super::super::owner::class_actor(
        &state,
        &headers,
        &scope.school_id,
        &scope.tenant_id,
        &scope.class_id,
    )
    .await?
        != (actor.clone(), role.clone())
    {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    Ok(Json(
        json!({"responseCode":200,"data":{"schoolId":scope.school_id,"tenantId":scope.tenant_id,"classId":scope.class_id,"actor":{"id":actor,"role":role},"academicYear":year,"teachers":teachers,"rooms":rooms}}),
    ))
}
pub(super) async fn context(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(scope): Query<Selection>,
) -> Result<Json<Value>, Failure> {
    if ![&scope.school_id, &scope.tenant_id]
        .iter()
        .all(|id| crate::school_portal::model::identifier(id))
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (actor, _) =
        super::super::administrator_actor(&headers, &scope.tenant_id, &scope.school_id).await?;
    let authorized = repository::admin().replace("section_id:$class,", "");
    let mut rows=state.graph.execute(query(&format!("{authorized} OPTIONAL MATCH(t:StaffMember {{membershipStatus:'ACTIVE'}}) WHERE 'teacher' IN coalesce(t.roles,[]) AND sec.school_id IN coalesce(t.schoolIds,[]) AND sec.tenant_id IN coalesce(t.tenantIds,[]) AND NOT EXISTS {{MATCH(other:StaffMember {{id:t.id}}) WHERE other<>t}} AND (sec.homeroom_staff_member_id=t.id OR EXISTS {{MATCH(:LearningTeachingGrant {{staff_member_id:t.id,section_id:sec.section_id,school_id:sec.school_id,tenant_id:sec.tenant_id,status:'ACTIVE'}})}}) RETURN sec.section_id AS class,sec.name AS name,coalesce(sec.academic_year,'') AS year,t.id AS teacher,coalesce(t.displayName,t.name,t.id) AS teacher_name ORDER BY class,teacher LIMIT 2001")).param("actor",actor).param("school",scope.school_id.clone()).param("tenant",scope.tenant_id.clone())).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut classes = BTreeMap::<String, Value>::new();
    let mut count = 0;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        count += 1;
        if count > 2000 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        let id: String = r
            .get("class")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let row=classes.entry(id.clone()).or_insert_with(||json!({"classId":id,"name":r.get::<String>("name").unwrap_or_default(),"academicYear":r.get::<String>("year").unwrap_or_default(),"teachers":[]}));
        if let Some(id) = r
            .get::<Option<String>>("teacher")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        {
            row["teachers"].as_array_mut().unwrap().push(json!({"id":id,"name":r.get::<String>("teacher_name").map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?}));
        }
        if classes.len() > 200 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
    }
    if classes.is_empty() {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let mut rooms=state.graph.execute(query("CALL { MATCH(m:LearningClassMeeting {school_id:$school,tenant_id:$tenant}) WHERE coalesce(m.status,'published')='published' AND coalesce(m.room_id,'')<>'' RETURN m.room_id AS room UNION MATCH(r:SchoolRoom {school_id:$school,tenant_id:$tenant,status:'active'}) RETURN r.id AS room } RETURN DISTINCT room ORDER BY room LIMIT 201").param("school",scope.school_id).param("tenant",scope.tenant_id)).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut ids = vec![];
    while let Some(r) = rooms
        .next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        if ids.len() == 200 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        ids.push(
            r.get::<String>("room")
                .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?,
        );
    }
    Ok(Json(
        json!({"responseCode":200,"data":{"classes":classes.into_values().collect::<Vec<_>>(),"rooms":ids}}),
    ))
}
pub(super) async fn list(
    graph: &Graph,
    actor: &str,
    role: &str,
    s: &Scope,
) -> Result<Value, Failure> {
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let params = |q: GraphQuery| {
        q.param("actor", actor)
            .param("school", s.school_id.clone())
            .param("tenant", s.tenant_id.clone())
            .param("class", s.class_id.clone())
    };
    let mut check = graph
        .execute(params(query(&format!(
            "{} RETURN count(sec) AS count",
            repository::authority(role)
        ))))
        .await
        .map_err(|_| unavailable())?;
    if check
        .next()
        .await
        .map_err(|_| unavailable())?
        .and_then(|r| r.get::<i64>("count").ok())
        != Some(1)
    {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let mut rows=graph.execute(params(query("MATCH(d:LearningTimetableDraft {school_id:$school,tenant_id:$tenant,class_id:$class}) WHERE $teacher=false OR d.teacher_id=$actor OPTIONAL MATCH(p:LearningClassMeeting {key:d.key}) RETURN d.receipt AS receipt,coalesce(p.version,0) AS pv,p.payload AS published ORDER BY d.id LIMIT 201").param("teacher",role=="teacher"))).await.map_err(|_|unavailable())?;
    let mut items = vec![];
    while let Some(r) = rows.next().await.map_err(|_| unavailable())? {
        if items.len() == 200 {
            return Err(unavailable());
        }
        let mut value: Value =
            serde_json::from_str(&r.get::<String>("receipt").map_err(|_| unavailable())?)
                .map_err(|_| unavailable())?;
        value["publishedVersion"] = json!(r.get::<i64>("pv").map_err(|_| unavailable())?);
        if let Some(raw) = r
            .get::<Option<String>>("published")
            .map_err(|_| unavailable())?
        {
            let payload: Value = serde_json::from_str(&raw).map_err(|_| unavailable())?;
            value["publishedFrozen"] = json!(payload["frozen"] == true);
        }
        items.push(value);
    }
    let mut published=graph.execute(params(query("MATCH(m:LearningClassMeeting {school_id:$school,tenant_id:$tenant,class_id:$class}) WHERE coalesce(m.status,'published')='published' RETURN m.payload AS payload ORDER BY m.starts_at,m.id LIMIT 501"))).await.map_err(|_|unavailable())?;
    let mut records = vec![];
    while let Some(r) = published.next().await.map_err(|_| unavailable())? {
        if records.len() == 500 {
            return Err(unavailable());
        }
        records.push(
            serde_json::from_str::<Value>(&r.get::<String>("payload").map_err(|_| unavailable())?)
                .map_err(|_| unavailable())?,
        );
    }
    Ok(json!({"items":items,"published":records}))
}
/// Caller must supply a query resolving current authorized Section as `sec`.
pub(crate) const PUBLISHED:&str="MATCH(m:LearningClassMeeting {school_id:sec.school_id,tenant_id:sec.tenant_id,class_id:sec.section_id}) WHERE coalesce(m.status,'published')='published' AND coalesce(m.academic_year,sec.academic_year)=sec.academic_year";
pub(crate) async fn published(
    graph: &Graph,
    q: GraphQuery,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    collect_published(graph, q, None).await
}
pub(crate) async fn published_in_window(
    graph: &Graph,
    q: GraphQuery,
    start: u64,
    end: u64,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    collect_published(graph, q, Some((start, end))).await
}
pub(crate) fn window_ms(
    from: &str,
    to: &str,
) -> Result<(u64, u64), Box<dyn std::error::Error + Send + Sync>> {
    let start = chrono::NaiveDate::parse_from_str(from, "%Y-%m-%d")?
        .and_hms_opt(0, 0, 0)
        .ok_or("Invalid date")?
        .and_utc()
        .timestamp_millis()
        - 25_200_000;
    let end = chrono::NaiveDate::parse_from_str(to, "%Y-%m-%d")?
        .succ_opt()
        .ok_or("Invalid date")?
        .and_hms_opt(0, 0, 0)
        .ok_or("Invalid date")?
        .and_utc()
        .timestamp_millis()
        - 25_200_000;
    Ok((start.try_into()?, end.try_into()?))
}
async fn collect_published(
    graph: &Graph,
    q: GraphQuery,
    window: Option<(u64, u64)>,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let mut rows = graph.execute(q).await?;
    let mut items = vec![];
    let mut bytes = 0;
    let mut has_more = false;
    while let Some(row) = rows.next().await? {
        let raw: String = row.get("payload")?;
        let mut value: Value = serde_json::from_str(&raw)?;
        if !value.is_object() {
            return Err("Invalid class meeting".into());
        }
        if let Some((start, end)) = window {
            let begins = value["startsAt"]
                .as_u64()
                .ok_or("Invalid class meeting time")?;
            let ends = value["endsAt"]
                .as_u64()
                .ok_or("Invalid class meeting time")?;
            if begins >= end || ends <= start {
                continue;
            }
        }
        if items.len() == 200 || bytes + raw.len() > 350_000 {
            has_more = true;
            break;
        }
        if let Ok(id) = row.get::<String>("student") {
            value["studentId"] = json!(id);
        }
        items.push(value);
        bytes += raw.len();
    }
    Ok(json!({"items":items,"hasMore":has_more}))
}
