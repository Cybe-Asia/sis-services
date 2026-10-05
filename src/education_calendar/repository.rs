use super::{
    model::{Change, Entry},
    Scope,
};
use neo4rs::{query, Graph, Query};
use serde_json::{json, Value};
pub type Error = Box<dyn std::error::Error + Send + Sync>;
const ADMIN:&str="MATCH(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WHERE ((NOT 'owner' IN coalesce(actor.roles,[]) AND 'school_admin' IN coalesce(actor.roles,[]) AND $school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[])) OR ('owner' IN coalesce(actor.roles,[]) AND size(coalesce(actor.teamIds,[]))=0 AND ((size(coalesce(actor.schoolIds,[]))=0 AND size(coalesce(actor.tenantIds,[]))=0) OR ($school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[]))) AND NOT EXISTS {MATCH(other:StaffMember {id:actor.id}) WHERE other<>actor} AND EXISTS {MATCH(:School {school_id:$school,tenant_id:$tenant})} AND NOT EXISTS {MATCH(one:School {school_id:$school,tenant_id:$tenant}),(two:School {school_id:$school,tenant_id:$tenant}) WHERE one<>two}))";
pub async fn init(graph: &Graph) -> Result<(), Error> {
    for (name, label) in [
        ("education_calendar_key", "EducationCalendarEntry"),
        ("education_calendar_lock", "EducationCalendarLock"),
        ("education_calendar_audit", "EducationCalendarAudit"),
    ] {
        graph
            .run(query(&format!(
                "CREATE CONSTRAINT {name} IF NOT EXISTS FOR(n:{label}) REQUIRE n.key IS UNIQUE"
            )))
            .await?;
    }
    Ok(())
}
pub async fn context(
    graph: &Graph,
    actor: &str,
    schools: &[String],
    tenants: &[String],
    after: &str,
) -> Result<Value, Error> {
    let mut rows=graph.execute(query("MATCH(a:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) MATCH(s:Section) WHERE s.school_id IN $schools AND s.tenant_id IN $tenants AND ((NOT 'owner' IN coalesce(a.roles,[]) AND 'school_admin' IN coalesce(a.roles,[]) AND s.school_id IN coalesce(a.schoolIds,[]) AND s.tenant_id IN coalesce(a.tenantIds,[])) OR ('owner' IN coalesce(a.roles,[]) AND size(coalesce(a.teamIds,[]))=0 AND ((size(coalesce(a.schoolIds,[]))=0 AND size(coalesce(a.tenantIds,[]))=0) OR (s.school_id IN coalesce(a.schoolIds,[]) AND s.tenant_id IN coalesce(a.tenantIds,[]))) AND NOT EXISTS {MATCH(other:StaffMember {id:a.id}) WHERE other<>a} AND EXISTS {MATCH(:School {school_id:s.school_id,tenant_id:s.tenant_id})} AND NOT EXISTS {MATCH(one:School {school_id:s.school_id,tenant_id:s.tenant_id}),(two:School {school_id:s.school_id,tenant_id:s.tenant_id}) WHERE one<>two})) AND coalesce(s.academic_year,'')<>'' AND s.status IN ['active','archived'] AND s.tenant_id+'|'+s.school_id+'|'+s.academic_year+'|'+s.section_id>$after RETURN DISTINCT s.tenant_id+'|'+s.school_id+'|'+s.academic_year+'|'+s.section_id AS cursor, s.section_id AS id,s.name AS name,s.school_id AS school,s.tenant_id AS tenant,s.academic_year AS year,s.status AS status ORDER BY cursor LIMIT 201").param("actor",actor).param("schools",schools.to_vec()).param("tenants",tenants.to_vec()).param("after",after)).await?;
    let mut classes = vec![];
    let mut last = String::new();
    let mut next = None;
    while let Some(r) = rows.next().await? {
        if classes.len() == 200 {
            use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
            next = Some(URL_SAFE_NO_PAD.encode(&last));
            break;
        }
        last = r.get::<String>("cursor")?;
        classes.push(json!({"sectionId":r.get::<String>("id")?,"name":r.get::<String>("name")?,"schoolId":r.get::<String>("school")?,"tenantId":r.get::<String>("tenant")?,"academicYear":r.get::<String>("year")?,"status":r.get::<String>("status")?}));
    }
    Ok(json!({"classes":classes,"next":next}))
}
fn change_query(actor: &str, input: &Change) -> Result<Query, Error> {
    let e = &input.entry;
    let key = format!("{}|{}|{}", e.tenant_id, e.school_id, e.id);
    // Authorization precedes the lock; recheck graph membership after acquiring it.
    // The school lock serializes CAS even for first creates. State and audit commit together.
    let cypher=format!("{ADMIN} WITH actor WHERE $action='archive' OR EXISTS {{ MATCH(s:Section {{school_id:$school,tenant_id:$tenant,academic_year:$year,status:'active'}}) }} MERGE(lock:EducationCalendarLock {{key:$lock}}) ON CREATE SET lock.version=0 SET lock.version=lock.version+1 WITH lock {ADMIN} WITH actor WHERE $action='archive' OR all(id IN $sections WHERE EXISTS {{ MATCH(s:Section {{section_id:id,school_id:$school,tenant_id:$tenant,academic_year:$year,status:'active'}}) }}) OPTIONAL MATCH(old:EducationCalendarEntry {{key:$key}}) WITH actor,old WHERE (old IS NULL AND $version=0 AND $action='save_draft') OR (old.version=$version AND old.school_id=$school AND old.tenant_id=$tenant AND old.academic_year=$year AND ((old.status='draft' AND $action='save_draft') OR (old.payload=$payload AND ((old.status='draft' AND $action='publish') OR (old.status IN ['draft','published'] AND $action='archive'))))) MERGE(r:EducationCalendarEntry {{key:$key}}) ON CREATE SET r.id=$id,r.school_id=$school,r.tenant_id=$tenant,r.academic_year=$year,r.version=0,r.created_at=datetime() SET r.version=r.version+1,r.payload=$payload,r.status=$status,r.audience=$audience,r.section_ids=$sections,r.start_date=$start,r.end_date=$end,r.updated_at=datetime() CREATE(:EducationCalendarAudit {{key:$audit,actor_id:actor.id,actor_role:CASE WHEN 'owner' IN coalesce(actor.roles,[]) THEN 'owner' ELSE 'school_admin' END,entry_key:r.key,school_id:$school,tenant_id:$tenant,academic_year:$year,version:r.version,action:$action,created_at:datetime()}}) RETURN r.version AS version");
    Ok(query(&cypher)
        .param("actor", actor)
        .param("school", e.school_id.clone())
        .param("tenant", e.tenant_id.clone())
        .param("year", e.academic_year.clone())
        .param("lock", format!("{}|{}", e.tenant_id, e.school_id))
        .param("key", key)
        .param("id", e.id.clone())
        .param("version", i64::from(input.expected_version))
        .param("action", input.action.clone())
        .param("payload", serde_json::to_string(e)?)
        .param(
            "status",
            match input.action.as_str() {
                "publish" => "published",
                "archive" => "archived",
                _ => "draft",
            },
        )
        .param("audience", e.audience.clone())
        .param("sections", e.section_ids.clone())
        .param("start", e.start_date.clone())
        .param("end", e.end_date.clone())
        .param("audit", uuid::Uuid::new_v4().to_string()))
}
pub async fn change(graph: &Graph, actor: &str, input: &Change) -> Result<Option<i64>, Error> {
    let mut rows = graph.execute(change_query(actor, input)?).await?;
    let result = rows
        .next()
        .await?
        .map(|r| r.get::<i64>("version"))
        .transpose()?;
    while rows.next().await?.is_some() {}
    Ok(result)
}
pub async fn change_live(
    graph: &Graph,
    actor: &str,
    role: &str,
    input: &Change,
    headers: &axum::http::HeaderMap,
) -> Result<Option<i64>, crate::school_portal::Failure> {
    let e = &input.entry;
    let unavailable = || super::failure(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    let mut tx = graph.start_txn().await.map_err(|_| unavailable())?;
    let result=async {
        crate::learning_access::timetable::repository::lock_school(&mut tx,&e.school_id,&e.tenant_id).await?;
        crate::learning_access::timetable::repository::one(&mut tx,query("MATCH(a:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) SET a.calendar_write_lock=coalesce(a.calendar_write_lock,0)+1 RETURN a.id AS id").param("actor",actor)).await?.ok_or_else(||super::failure(axum::http::StatusCode::FORBIDDEN))?;
        let current=crate::learning_access::administrator_actor(headers,&e.tenant_id,&e.school_id).await?;
        if current!=(actor.to_string(),role.to_string()){return Err(super::failure(axum::http::StatusCode::FORBIDDEN));}
        crate::learning_access::timetable::calendar_guard::check(&mut tx,input).await?;
        let q=change_query(actor,input).map_err(|_|unavailable())?;
        let row=crate::learning_access::timetable::repository::one(&mut tx,q).await?;
        row.map(|r|r.get::<i64>("version")).transpose().map_err(|_|unavailable())
    }.await;
    match result {
        Ok(Some(version)) => {
            tx.commit().await.map_err(|_| unavailable())?;
            Ok(Some(version))
        }
        Ok(None) => {
            let _ = tx.rollback().await;
            Ok(None)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}
pub async fn list(graph: &Graph, actor: &str, q: &Scope) -> Result<Value, Error> {
    let mut rows=graph.execute(query(&format!("{ADMIN} MATCH(r:EducationCalendarEntry {{school_id:$school,tenant_id:$tenant,academic_year:$year}}) WHERE r.id>$after AND ($id='' OR r.id=$id) RETURN r.payload AS payload,r.version AS version,r.status AS status,r.id AS id ORDER BY id LIMIT 101")).param("actor",actor).param("school",q.school_id.clone()).param("tenant",q.tenant_id.clone()).param("year",q.academic_year.clone()).param("after",q.after.clone().unwrap_or_default()).param("id",q.id.clone().unwrap_or_default())).await?;
    let mut items = vec![];
    let mut next = None;
    let mut bytes = 0;
    while let Some(r) = rows.next().await? {
        if items.len() == 100 {
            next = items
                .last()
                .and_then(|v: &Value| v["entry"]["id"].as_str())
                .map(str::to_string);
            break;
        }
        let entry: Entry = serde_json::from_str(&r.get::<String>("payload")?)?;
        if !entry.valid() {
            return Err("invalid calendar record".into());
        }
        let item = json!({"entry":entry,"version":r.get::<i64>("version")?,"status":r.get::<String>("status")?});
        let size = serde_json::to_vec(&item)?.len() + 1;
        if bytes + size > 1_300_000 {
            next = items
                .last()
                .and_then(|v: &Value| v["entry"]["id"].as_str())
                .map(str::to_string);
            break;
        }
        bytes += size;
        items.push(item);
    }
    Ok(json!({"items":items,"next":next}))
}
/// `prefix` must resolve current authorized class as `sec`; never accept a caller supplied class alone.
pub async fn published(graph: &Graph, prefix: Query) -> Result<Value, Error> {
    let mut rows = graph.execute(prefix).await?;
    let mut items = vec![];
    let mut more = false;
    let mut bytes = 0;
    while let Some(r) = rows.next().await? {
        if items.len() == 100 {
            more = true;
            break;
        }
        let entry: Entry = serde_json::from_str(&r.get::<String>("payload")?)?;
        if !entry.valid() {
            return Err("invalid calendar publication".into());
        }
        let item = json!({"entry":entry,"version":r.get::<i64>("version")?,"studentId":r.get::<String>("student").ok()});
        let size = serde_json::to_vec(&item)?.len() + 1;
        if bytes + size > 350_000 {
            more = true;
            break;
        }
        bytes += size;
        items.push(item);
    }
    Ok(json!({"items":items,"hasMore":more}))
}
// Retain one source and its identity. Academic year and explicit audience are both required.
pub const PUBLISHED:&str="MATCH(r:EducationCalendarEntry {school_id:sec.school_id,tenant_id:sec.tenant_id,academic_year:sec.academic_year,status:'published'}) WHERE (r.audience='school' OR (r.audience='sections' AND sec.section_id IN r.section_ids))";

#[cfg(test)]
mod database_tests;
