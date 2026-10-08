//! SIS owns the school room catalogue. Timetable meetings keep their room id; reads join the name.
use crate::{school_portal::model::identifier, AppState};
use axum::{
    extract::{rejection::JsonRejection, rejection::QueryRejection, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use neo4rs::{query, Graph};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
type Failure = (StatusCode, Json<Value>);
fn failure(status: StatusCode) -> Failure {
    (
        status,
        Json(
            json!({"error":{"code":match status {StatusCode::BAD_REQUEST=>"INVALID_INPUT",StatusCode::FORBIDDEN=>"FORBIDDEN",StatusCode::UNAUTHORIZED=>"UNAUTHORIZED",StatusCode::CONFLICT=>"REVISION_CONFLICT",_=>"DEPENDENCY_UNAVAILABLE"}}}),
        ),
    )
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Room {
    pub tenant_id: String,
    pub school_id: String,
    pub id: String,
    pub name: String,
    pub status: String,
}
impl Room {
    pub fn valid(&self) -> bool {
        identifier(&self.tenant_id)
            && identifier(&self.school_id)
            && identifier(&self.id)
            // Meetings store room ids lowercased; the catalogue key must match them.
            && self.id == self.id.to_ascii_lowercase()
            && !self.name.trim().is_empty()
            && self.name.trim() == self.name
            && self.name.len() <= 120
            && !self.name.chars().any(char::is_control)
            && ["active", "archived"].contains(&self.status.as_str())
    }
    fn key(&self) -> String {
        format!("{}|{}|{}", self.tenant_id, self.school_id, self.id)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub room: Room,
    pub expected_version: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Scope {
    tenant_id: String,
    school_id: String,
}
pub async fn init(graph: &Graph) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for (name, label) in [
        ("school_room_key", "SchoolRoom"),
        ("school_room_audit_key", "SchoolRoomAudit"),
    ] {
        graph
            .run(query(&format!(
                "CREATE CONSTRAINT {name} IF NOT EXISTS FOR(n:{label}) REQUIRE n.key IS UNIQUE"
            )))
            .await?;
    }
    Ok(())
}
/// Lists every room of one school (active and archived) for its administrators.
pub async fn rooms(
    graph: &Graph,
    actor: &str,
    tenant: &str,
    school: &str,
) -> Result<Value, Failure> {
    let admin = crate::education_calendar::repository::ADMIN;
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let mut rows=graph.execute(query(&format!("{admin} MATCH(r:SchoolRoom {{school_id:$school,tenant_id:$tenant}}) RETURN r.id AS id,r.name AS name,r.status AS status,r.version AS version ORDER BY id LIMIT 501")).param("actor",actor).param("school",school).param("tenant",tenant)).await.map_err(|_|unavailable())?;
    let mut items = vec![];
    while let Some(r) = rows.next().await.map_err(|_| unavailable())? {
        if items.len() == 500 {
            return Err(unavailable());
        }
        items.push(json!({"id":r.get::<String>("id").map_err(|_|unavailable())?,"name":r.get::<String>("name").map_err(|_|unavailable())?,"status":r.get::<String>("status").map_err(|_|unavailable())?,"version":r.get::<i64>("version").map_err(|_|unavailable())?}));
    }
    Ok(json!({"tenantId":tenant,"schoolId":school,"items":items}))
}
/// Creates or updates one room under the school lock (CAS on `expectedVersion`, 0 creates).
/// State and audit commit together; `None` is a revision conflict.
pub async fn save(graph: &Graph, actor: &str, change: &Change) -> Result<Option<i64>, Failure> {
    use crate::learning_access::timetable::repository::{lock_school, one};
    let admin = crate::education_calendar::repository::ADMIN;
    let r = &change.room;
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let mut tx = graph.start_txn().await.map_err(|_| unavailable())?;
    let result = async {
        lock_school(&mut tx, &r.school_id, &r.tenant_id).await?;
        let q=query(&format!("{admin} WITH actor OPTIONAL MATCH(old:SchoolRoom {{key:$key}}) WITH actor,old WHERE (old IS NULL AND $version=0) OR (old.version=$version AND old.school_id=$school AND old.tenant_id=$tenant) MERGE(r:SchoolRoom {{key:$key}}) ON CREATE SET r.id=$id,r.school_id=$school,r.tenant_id=$tenant,r.version=0,r.created_at=datetime() SET r.version=r.version+1,r.name=$name,r.status=$status,r.updated_at=datetime() CREATE(:SchoolRoomAudit {{key:$audit,actor_id:actor.id,actor_role:CASE WHEN 'owner' IN coalesce(actor.roles,[]) THEN 'owner' ELSE 'school_admin' END,room_key:r.key,school_id:$school,tenant_id:$tenant,version:r.version,name:$name,status:$status,created_at:datetime()}}) RETURN r.version AS version"))
            .param("actor", actor)
            .param("key", r.key())
            .param("id", r.id.clone())
            .param("school", r.school_id.clone())
            .param("tenant", r.tenant_id.clone())
            .param("name", r.name.clone())
            .param("status", r.status.clone())
            .param("version", i64::from(change.expected_version))
            .param("audit", uuid::Uuid::new_v4().to_string());
        one(&mut tx, q)
            .await?
            .map(|row| row.get::<i64>("version"))
            .transpose()
            .map_err(|_| unavailable())
    }
    .await;
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
async fn list(
    State(s): State<AppState>,
    h: HeaderMap,
    q: Result<Query<Scope>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !identifier(&q.tenant_id) || !identifier(&q.school_id) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let actor = crate::learning_access::administrator(&h, &q.tenant_id, &q.school_id).await?;
    rooms(&s.graph, &actor, &q.tenant_id, &q.school_id)
        .await
        .map(|data| Json(json!({"data":data})))
}
async fn change(
    State(s): State<AppState>,
    h: HeaderMap,
    j: Result<Json<Change>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(c) = j.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !c.room.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let actor =
        crate::learning_access::administrator(&h, &c.room.tenant_id, &c.room.school_id).await?;
    match save(&s.graph, &actor, &c).await? {
        Some(version) => Ok(Json(json!({"data":{"version":version}}))),
        None => Err(failure(StatusCode::CONFLICT)),
    }
}
pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/sis-service/school-rooms", get(list).post(change))
}
#[cfg(test)]
mod tests;
