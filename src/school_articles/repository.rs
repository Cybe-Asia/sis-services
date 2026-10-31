use super::{
    cover::{object_key, Store},
    model::{Article, Change},
    Scope,
};
use crate::education_calendar::repository::ADMIN;

/// Who writes: the Staff Member, and whether the route found them holding `school.content.manage`
/// at the school (ADR 0018 slice 6). A plain id is today's rule alone: an administrator or Owner.
#[derive(Clone, Copy)]
pub struct Actor<'a> {
    pub id: &'a str,
    pub capable: bool,
}
impl<'a> From<&'a str> for Actor<'a> {
    fn from(id: &'a str) -> Self {
        Self { id, capable: false }
    }
}
impl<'a> From<&'a String> for Actor<'a> {
    fn from(id: &'a String) -> Self {
        Self::from(id.as_str())
    }
}
/// `ADMIN`, or a staff member of the school whom the route found holding the capability
/// (`$capable`). Both read the caller's node, so a membership ended meanwhile still refuses.
fn manager() -> String {
    format!("{ADMIN} OR ($capable AND $school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[]) AND NOT EXISTS {{MATCH(other:StaffMember {{id:actor.id}}) WHERE other<>actor}})")
}
use neo4rs::{query, Graph, Query};
use serde_json::{json, Value};
pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub async fn init(graph: &Graph) -> Result<(), Error> {
    for (name, label) in [
        ("school_article_key", "SchoolArticle"),
        ("school_article_lock", "SchoolArticleLock"),
        ("school_article_audit", "SchoolArticleAudit"),
        ("school_article_view", "SchoolArticleView"),
        ("school_article_cover", "SchoolArticleCover"),
    ] {
        graph
            .run(query(&format!(
                "CREATE CONSTRAINT {name} IF NOT EXISTS FOR(n:{label}) REQUIRE n.key IS UNIQUE"
            )))
            .await?;
    }
    graph
        .run(query("CREATE INDEX school_article_view_article IF NOT EXISTS FOR(n:SchoolArticleView) ON(n.article_key)"))
        .await?;
    Ok(())
}

/// A staff member who may host a webinar of the school: active and teaching there.
pub(crate) fn host(alias: &str, school: &str, tenant: &str) -> String {
    format!("{alias}.membershipStatus='ACTIVE' AND 'teacher' IN coalesce({alias}.roles,[]) AND {school} IN coalesce({alias}.schoolIds,[]) AND {tenant} IN coalesce({alias}.tenantIds,[])")
}
fn change_query(actor: Actor<'_>, input: &Change) -> Result<Query, Error> {
    let e = &input.entry;
    let manager = manager();
    let key = format!("{}|{}|{}", e.tenant_id, e.school_id, e.id);
    // Authorization precedes the lock and is rechecked after it; the school lock serializes the
    // version check even for a first save. Only a draft is edited: a published article is taken
    // back to a draft first, so what families read never changes under them. A cover must be one
    // this school uploaded. State and audit commit together.
    // A host is named once, at a draft or publish: a host who later leaves still opens nothing,
    // because the room check reads the staff member again.
    let host = host("h", "$school", "$tenant");
    let cypher=format!("{manager} WITH actor MERGE(lock:SchoolArticleLock {{key:$lock}}) ON CREATE SET lock.version=0 SET lock.version=lock.version+1 WITH lock {manager} WITH actor WHERE $action IN ['unpublish','archive'] OR (all(id IN $sections WHERE EXISTS {{ MATCH(:Section {{section_id:id,school_id:$school,tenant_id:$tenant,status:'active'}}) }}) AND ($cover='' OR EXISTS {{ MATCH(:SchoolArticleCover {{key:$cover_key}}) }}) AND all(id IN $hosts WHERE EXISTS {{ MATCH(h:StaffMember {{id:id}}) WHERE {host} }})) OPTIONAL MATCH(old:SchoolArticle {{key:$key}}) WITH actor,old WHERE (old IS NULL AND $version=0 AND $action='save_draft') OR (old.version=$version AND old.school_id=$school AND old.tenant_id=$tenant AND ((old.status='draft' AND $action='save_draft') OR (old.payload=$payload AND ((old.status='draft' AND $action='publish') OR (old.status='published' AND $action='unpublish') OR (old.status IN ['draft','published'] AND $action='archive'))))) MERGE(r:SchoolArticle {{key:$key}}) ON CREATE SET r.id=$id,r.school_id=$school,r.tenant_id=$tenant,r.version=0,r.created_at=datetime() SET r.version=r.version+1,r.payload=$payload,r.status=$status,r.audience=$audience,r.section_ids=$sections,r.cover=$cover,r.kind=$kind,r.room_day=$room_day,r.host_ids=$hosts,r.updated_at=datetime(),r.published_at=CASE WHEN $action='publish' THEN datetime() ELSE r.published_at END CREATE(:SchoolArticleAudit {{key:$audit,actor_id:actor.id,actor_role:CASE WHEN 'owner' IN coalesce(actor.roles,[]) THEN 'owner' WHEN 'school_admin' IN coalesce(actor.roles,[]) THEN 'school_admin' ELSE 'staff' END,article_key:r.key,school_id:$school,tenant_id:$tenant,version:r.version,action:$action,created_at:datetime()}}) RETURN r.version AS version");
    Ok(query(&cypher)
        .param("actor", actor.id)
        .param("capable", actor.capable)
        .param("school", e.school_id.clone())
        .param("tenant", e.tenant_id.clone())
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
        .param("kind", e.kind.clone())
        .param("room_day", e.room_day().unwrap_or_default())
        .param("hosts", e.hosts.clone())
        .param("audience", e.audience.clone())
        .param("sections", e.section_ids.clone())
        .param("cover", e.cover.clone().unwrap_or_default())
        .param(
            "cover_key",
            format!(
                "{}|{}|{}",
                e.tenant_id,
                e.school_id,
                e.cover.as_deref().unwrap_or_default()
            ),
        )
        .param("audit", uuid::Uuid::new_v4().to_string()))
}
/// The new version, or `None` when the caller may not write here or read an older version.
pub async fn change<'a>(
    graph: &Graph,
    actor: impl Into<Actor<'a>>,
    input: &Change,
) -> Result<Option<i64>, Error> {
    let mut rows = graph.execute(change_query(actor.into(), input)?).await?;
    let result = rows
        .next()
        .await?
        .map(|r| r.get::<i64>("version"))
        .transpose()?;
    while rows.next().await?.is_some() {}
    Ok(result)
}
pub async fn list<'a>(
    graph: &Graph,
    actor: impl Into<Actor<'a>>,
    q: &Scope,
) -> Result<Value, Error> {
    let (actor, manager) = (actor.into(), manager());
    let mut rows=graph.execute(query(&format!("{manager} MATCH(r:SchoolArticle {{school_id:$school,tenant_id:$tenant}}) WHERE r.id>$after AND ($id='' OR r.id=$id) WITH r ORDER BY r.id LIMIT 101 OPTIONAL MATCH(v:SchoolArticleView {{article_key:r.key}}) WITH r,count(v) AS views RETURN r.payload AS payload,r.version AS version,r.status AS status,r.id AS id,toString(r.updated_at) AS updated,toString(r.published_at) AS published,views ORDER BY id")).param("actor",actor.id).param("capable",actor.capable).param("school",q.school_id.clone()).param("tenant",q.tenant_id.clone()).param("after",q.after.clone().unwrap_or_default()).param("id",q.id.clone().unwrap_or_default())).await?;
    let mut items = vec![];
    let mut next = None;
    let mut bytes = 0;
    while let Some(r) = rows.next().await? {
        let last = |items: &[Value]| {
            items
                .last()
                .and_then(|v| v["entry"]["id"].as_str())
                .map(str::to_string)
        };
        if items.len() == 100 {
            next = last(&items);
            break;
        }
        let entry: Article = serde_json::from_str(&r.get::<String>("payload")?)?;
        if !entry.valid() {
            return Err("invalid article record".into());
        }
        let item = json!({"entry":entry,"version":r.get::<i64>("version")?,"status":r.get::<String>("status")?,"updatedAt":r.get::<String>("updated")?,"publishedAt":r.get::<String>("published").ok(),"views":r.get::<i64>("views")?});
        let size = serde_json::to_vec(&item)?.len() + 1;
        if bytes + size > 1_300_000 {
            next = last(&items);
            break;
        }
        bytes += size;
        items.push(item);
    }
    Ok(json!({"items":items,"next":next}))
}
/// The staff members the caller may name as a webinar's host at this school, by name.
pub async fn hosts<'a>(
    graph: &Graph,
    actor: impl Into<Actor<'a>>,
    q: &Scope,
) -> Result<Value, Error> {
    let (actor, manager, host) = (actor.into(), manager(), host("h", "$school", "$tenant"));
    let mut rows=graph.execute(query(&format!("{manager} WITH actor MATCH(h:StaffMember) WHERE {host} RETURN DISTINCT h.id AS id,coalesce(h.displayName,h.email,h.id) AS name ORDER BY name,id LIMIT 501")).param("actor",actor.id).param("capable",actor.capable).param("school",q.school_id.clone()).param("tenant",q.tenant_id.clone())).await?;
    let mut items = vec![];
    while let Some(r) = rows.next().await? {
        if items.len() == 500 {
            return Err("host limit exceeded".into());
        }
        items.push(json!({"id":r.get::<String>("id")?,"name":r.get::<String>("name")?}));
    }
    Ok(json!({"hosts":items}))
}
/// Published for the caller's class. `sec` must already be a class the caller currently belongs to.
pub(crate) const PUBLISHED:&str="MATCH(a:SchoolArticle {school_id:sec.school_id,tenant_id:sec.tenant_id,status:'published'}) WHERE (a.audience='school' OR (a.audience='sections' AND sec.section_id IN a.section_ids))";
/// What a family may read of an article: the text and schedule, not the school's audience selection.
pub(crate) fn public(a: &Article) -> Value {
    json!({"id":a.id,"kind":a.kind,"category":a.category,"title":a.title,"body":a.body,"date":a.date,"start":a.start,"end":a.end,"joinUrl":a.join_url,"cover":a.cover})
}

/// Keeps an already normalized photograph for this school and returns the identifier an article
/// names it by, or `None` when the caller may not write here. The object is written before its
/// record: a failed write leaves at most an object nothing refers to.
pub async fn add_cover<'a>(
    graph: &Graph,
    store: &Store,
    actor: impl Into<Actor<'a>>,
    tenant: &str,
    school: &str,
    photograph: Vec<u8>,
) -> Result<Option<String>, Error> {
    let (actor, manager) = (actor.into(), manager());
    let scoped = |cypher: &str| {
        query(cypher)
            .param("actor", actor.id)
            .param("capable", actor.capable)
            .param("school", school)
            .param("tenant", tenant)
    };
    let mut rows = graph
        .execute(scoped(&format!("{manager} RETURN actor.id AS id LIMIT 1")))
        .await?;
    if rows.next().await?.is_none() {
        return Ok(None);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let object = object_key(tenant, school, &id);
    let size = photograph.len() as i64;
    store.put(&object, photograph).await?;
    let mut rows=graph.execute(scoped(&format!("{manager} CREATE(c:SchoolArticleCover {{key:$key,id:$id,school_id:$school,tenant_id:$tenant,object_key:$object,size:$size,actor_id:actor.id,created_at:datetime()}}) RETURN c.id AS id")).param("key",format!("{tenant}|{school}|{id}")).param("id",id.clone()).param("object",object).param("size",size)).await?;
    let saved = rows.next().await?.is_some();
    while rows.next().await?.is_some() {}
    Ok(saved.then_some(id))
}
/// A cover of this school for its administrator, whether or not an article shows it yet.
pub async fn admin_cover<'a>(
    graph: &Graph,
    store: &Store,
    actor: impl Into<Actor<'a>>,
    tenant: &str,
    school: &str,
    cover: &str,
) -> Result<Option<Vec<u8>>, Error> {
    let (actor, manager) = (actor.into(), manager());
    let mut rows=graph.execute(query(&format!("{manager} MATCH(c:SchoolArticleCover {{key:$key,school_id:$school,tenant_id:$tenant}}) RETURN c.object_key AS object LIMIT 1")).param("actor",actor.id).param("capable",actor.capable).param("school",school).param("tenant",tenant).param("key",format!("{tenant}|{school}|{cover}"))).await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let object: String = row.get("object")?;
    drop(rows);
    Ok(Some(store.get(&object).await?))
}

#[cfg(test)]
mod database_tests;
