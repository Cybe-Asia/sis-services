// AuditEvent — append-only event log for admin-initiated state changes.
// Kept deliberately thin: actor, action, target, optional diff JSON,
// timestamp. Per-event shape is intentionally schemaless (diff is a
// string) so we can instrument new mutations without migrating the
// node schema.
//
// Fire-and-forget: emit fns return Result but callers ignore errors —
// a failed audit write must NEVER block a user-initiated mutation.

use neo4rs::{query, Graph};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::repositories::lead_repository::RepositoryError;

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[allow(non_snake_case)]
pub struct AuditEvent {
    pub eventId: String,
    pub actorLeadId: String,
    pub actorEmail: Option<String>,
    pub action: String,
    /// One of: lead / application / student / offer / document_request /
    /// document_artifact / schedule / section / settings
    pub targetType: String,
    pub targetId: String,
    pub diff: Option<String>,
    pub createdAt: String,
}

/// Emit an audit event. Errors are logged at warn level by the caller
/// (this fn returns them faithfully, the caller decides to drop). Kept
/// synchronous-ish: one cypher write per call. Future: batch via a
/// channel + background flusher if volume demands.
pub async fn emit(
    graph: &Graph,
    actor_lead_id: &str,
    action: &str,
    target_type: &str,
    target_id: &str,
    diff: Option<&str>,
) -> Result<(), RepositoryError> {
    let event_id = format!("AUDIT-{}", Uuid::new_v4());
    let q = query(
        "CREATE (a:AuditEvent { \
            event_id: $id, actor_lead_id: $actor, action: $action, \
            target_type: $tt, target_id: $tid, \
            diff: $diff, created_at: datetime() \
         }) \
         WITH a \
         MATCH (l:Lead {lead_id: $actor}) \
         MERGE (l)-[:EMITTED]->(a)"
    )
    .param("id", event_id)
    .param("actor", actor_lead_id.to_string())
    .param("action", action.to_string())
    .param("tt", target_type.to_string())
    .param("tid", target_id.to_string())
    .param("diff", diff.unwrap_or(""));
    graph
        .run(q)
        .await
        .map_err(|e| RepositoryError::DbError(e.to_string()))?;
    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ListAuditFilters {
    pub actor_email: Option<String>,
    pub action: Option<String>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
}

pub async fn list_events(
    graph: &Graph,
    filters: &ListAuditFilters,
    limit: i64,
    skip: i64,
) -> Result<Vec<AuditEvent>, RepositoryError> {
    let mut wheres: Vec<String> = Vec::new();
    if filters.actor_email.is_some() {
        wheres.push("toLower(l.email) CONTAINS $actor_email".to_string());
    }
    if filters.action.is_some() { wheres.push("a.action = $action".to_string()); }
    if filters.target_type.is_some() { wheres.push("a.target_type = $tt".to_string()); }
    if filters.target_id.is_some() { wheres.push("a.target_id = $tid".to_string()); }
    let where_sql = if wheres.is_empty() { String::new() } else { format!("WHERE {}", wheres.join(" AND ")) };
    let cy = format!(
        "MATCH (a:AuditEvent) \
         OPTIONAL MATCH (l:Lead {{lead_id: a.actor_lead_id}}) \
         {where_sql} \
         RETURN \
           a.event_id AS eventId, \
           a.actor_lead_id AS actorLeadId, \
           coalesce(l.email, '') AS actorEmail, \
           a.action AS action, \
           a.target_type AS targetType, \
           a.target_id AS targetId, \
           a.diff AS diff, \
           toString(a.created_at) AS createdAt \
         ORDER BY a.created_at DESC \
         SKIP $skip LIMIT $limit"
    );
    let mut q = query(&cy).param("limit", limit).param("skip", skip);
    if let Some(s) = &filters.actor_email { q = q.param("actor_email", s.to_lowercase()); }
    if let Some(s) = &filters.action { q = q.param("action", s.clone()); }
    if let Some(s) = &filters.target_type { q = q.param("tt", s.clone()); }
    if let Some(s) = &filters.target_id { q = q.param("tid", s.clone()); }
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        out.push(AuditEvent {
            eventId: row.get::<String>("eventId").unwrap_or_default(),
            actorLeadId: row.get::<String>("actorLeadId").unwrap_or_default(),
            actorEmail: row.get::<String>("actorEmail").ok().filter(|s| !s.is_empty()),
            action: row.get::<String>("action").unwrap_or_default(),
            targetType: row.get::<String>("targetType").unwrap_or_default(),
            targetId: row.get::<String>("targetId").unwrap_or_default(),
            diff: row.get::<String>("diff").ok().filter(|s| !s.is_empty()),
            createdAt: row.get::<String>("createdAt").unwrap_or_default(),
        });
    }
    Ok(out)
}

/// Convenience helper — events targeting a specific applicant across
/// all target_types (student, offer, document_request, etc.).
pub async fn list_for_applicant(
    graph: &Graph,
    applicant_student_id: &str,
    limit: i64,
) -> Result<Vec<AuditEvent>, RepositoryError> {
    // Looks at student-keyed targets plus offers/decisions/doc_requests
    // owned by the student. For now we just match by student target_id;
    // cross-domain matches require a join via the graph which we'll
    // add when the audit surface grows.
    let q = query(
        "MATCH (a:AuditEvent {target_id: $id}) \
         OPTIONAL MATCH (l:Lead {lead_id: a.actor_lead_id}) \
         RETURN \
           a.event_id AS eventId, \
           a.actor_lead_id AS actorLeadId, \
           coalesce(l.email, '') AS actorEmail, \
           a.action AS action, \
           a.target_type AS targetType, \
           a.target_id AS targetId, \
           a.diff AS diff, \
           toString(a.created_at) AS createdAt \
         ORDER BY a.created_at DESC \
         LIMIT $limit"
    )
    .param("id", applicant_student_id.to_string())
    .param("limit", limit);
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        out.push(AuditEvent {
            eventId: row.get::<String>("eventId").unwrap_or_default(),
            actorLeadId: row.get::<String>("actorLeadId").unwrap_or_default(),
            actorEmail: row.get::<String>("actorEmail").ok().filter(|s| !s.is_empty()),
            action: row.get::<String>("action").unwrap_or_default(),
            targetType: row.get::<String>("targetType").unwrap_or_default(),
            targetId: row.get::<String>("targetId").unwrap_or_default(),
            diff: row.get::<String>("diff").ok().filter(|s| !s.is_empty()),
            createdAt: row.get::<String>("createdAt").unwrap_or_default(),
        });
    }
    Ok(out)
}

