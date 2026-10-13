use super::{parent_write_prefix, teacher_query, Error, PARENT, PARENT_SECTION};
use crate::school_articles::{model::Article, repository as articles};
use neo4rs::{query, Graph};
use serde_json::{json, Value};

/// Articles and webinars published for a class one of this Parent's children is placed in, newest
/// first, each once for the family, with how many families opened it and whether this one did.
pub async fn published(graph: &Graph, sub: &str) -> Result<Value, Error> {
    let mut rows=graph.execute(query(&format!("{PARENT} WITH DISTINCT u,s {PARENT_SECTION} {} WITH DISTINCT u,a OPTIONAL MATCH(mine:SchoolArticleView {{key:u.id+'|'+a.key}}) OPTIONAL MATCH(v:SchoolArticleView {{article_key:a.key}}) WITH a,mine,count(v) AS views RETURN a.payload AS payload,toString(a.published_at) AS published,views,mine IS NOT NULL AS read ORDER BY published DESC LIMIT 30",articles::PUBLISHED)).param("sub",sub)).await?;
    let mut items = vec![];
    while let Some(r) = rows.next().await? {
        let article: Article = serde_json::from_str(&r.get::<String>("payload")?)?;
        if !article.valid() {
            return Err("invalid article publication".into());
        }
        items.push(json!({"content":articles::public(&article),"publishedAt":r.get::<String>("published")?,"views":r.get::<i64>("views")?,"read":r.get::<bool>("read")?}));
    }
    Ok(Value::Array(items))
}
/// Records that this Parent opened a published article their child's class received. One view per
/// family account however often it is opened.
pub async fn view(graph: &Graph, sub: &str, id: &str) -> Result<bool, Error> {
    let q=query(&format!("{} WITH DISTINCT u,s {PARENT_SECTION} WITH DISTINCT u,sec {} AND a.id=$id WITH DISTINCT u,a MERGE(v:SchoolArticleView {{key:u.id+'|'+a.key}}) ON CREATE SET v.article_key=a.key,v.viewed_at=datetime() RETURN true AS saved",parent_write_prefix(),articles::PUBLISHED)).param("sub",sub).param("id",id);
    let mut rows = graph.execute(q).await?;
    let saved = rows.next().await?.is_some();
    while rows.next().await?.is_some() {}
    Ok(saved)
}
/// The cover of a published article this Parent's child's class received, or `None` when there is
/// no such article or it shows no cover.
pub async fn cover(
    graph: &Graph,
    store: &crate::school_articles::cover::Store,
    sub: &str,
    id: &str,
) -> Result<Option<Vec<u8>>, Error> {
    let q=query(&format!("{PARENT} WITH DISTINCT u,s {PARENT_SECTION} {} AND a.id=$id AND coalesce(a.cover,'')<>'' WITH DISTINCT a MATCH(c:SchoolArticleCover {{key:a.tenant_id+'|'+a.school_id+'|'+a.cover}}) RETURN c.object_key AS object LIMIT 1",articles::PUBLISHED)).param("sub",sub).param("id",id);
    let mut rows = graph.execute(q).await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let object: String = row.get("object")?;
    drop(rows);
    Ok(Some(store.get(&object).await?))
}
/// Published webinars without an outside join address, from yesterday on, of the schools where
/// this staff member has a class or is the Owner. These are the rooms they may open.
pub async fn hostable(
    graph: &Graph,
    member: &crate::school_portal::auth::Teacher,
) -> Result<Value, Error> {
    let from = crate::school_portal::model::school_today()
        .pred_opt()
        .ok_or("date out of range")?
        .to_string();
    let mut rows=graph.execute(teacher_query(member,"MATCH(a:SchoolArticle {school_id:sec.school_id,tenant_id:sec.tenant_id,status:'published'}) RETURN DISTINCT a.payload AS payload,a.id AS id ORDER BY id LIMIT 201",None,false,false)).await?;
    let mut items = vec![];
    while let Some(r) = rows.next().await? {
        let article: Article = serde_json::from_str(&r.get::<String>("payload")?)?;
        if !article.valid() {
            return Err("invalid article publication".into());
        }
        if article.kind != "webinar"
            || article.join_url.is_some()
            || article.date.as_deref().is_none_or(|day| day < from.as_str())
        {
            continue;
        }
        items.push(json!({"id":article.id,"title":article.title,"date":article.date,"start":article.start,"end":article.end,"schoolId":article.school_id,"tenantId":article.tenant_id}));
        if items.len() == 50 {
            break;
        }
    }
    Ok(Value::Array(items))
}
