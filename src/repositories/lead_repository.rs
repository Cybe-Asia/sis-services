// Minimal Lead repository for sis-services. admission-services owns
// the :Lead label and the full CRUD surface; sis-services just needs
// two read-only helpers for auth:
//
//   - find_email_by_lead_id        used by require_admin when the
//                                  bearer token sub is LEAD-* (magic
//                                  link flow), to resolve the admin's
//                                  email for the allowlist check.
//   - resolve_primary_lead_for_user   used by resolve_lead_id_from_bearer
//                                  when the bearer token sub is a
//                                  User.id UUID (password login flow).
//
// Both queries read Lead nodes that admission-services writes. Both
// use narrow cypher so we don't have to drag the full Lead model
// (email, name, address, school selection, children, intake, etc.)
// across the service boundary — we only need the one string each.

use neo4rs::{query, Graph};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("database error: {0}")]
    DbError(String),
    #[error("lead not found")]
    NotFound,
}

/// Return the `email` property of the Lead with the given id, or
/// None if no such Lead exists. We don't return the full Lead
/// struct because sis-services doesn't own that shape — keeping the
/// contract narrow avoids drift when admission-services evolves the
/// schema.
pub async fn find_email_by_lead_id(
    graph: &Graph,
    lead_id: &str,
) -> Result<Option<String>, RepositoryError> {
    let q = query("MATCH (l:Lead {lead_id: $id}) RETURN l.email AS email LIMIT 1")
        .param("id", lead_id.to_string());
    let mut rs = graph
        .execute(q)
        .await
        .map_err(|e| RepositoryError::DbError(e.to_string()))?;
    if let Some(row) = rs
        .next()
        .await
        .map_err(|e| RepositoryError::DbError(e.to_string()))?
    {
        return Ok(row.get::<String>("email").ok().filter(|s| !s.is_empty()));
    }
    Ok(None)
}

/// Walk the (User)-[:HAS_APPLICATION]->(Lead) edges to find the newest
/// Lead owned by a User. Same query as admission-services'; duplicated
/// rather than shared so we don't depend on admission-services being
/// deployed at a specific version. When /me flows diverge between the
/// two services we'll revisit.
pub async fn resolve_primary_lead_for_user(
    graph: &Graph,
    user_id: &str,
) -> Result<Option<String>, RepositoryError> {
    let q = query(
        "MATCH (u:User {id: $id})-[:HAS_APPLICATION]->(l:Lead) \
         RETURN l.lead_id AS lead_id \
         ORDER BY l.eoi_submitted_at DESC LIMIT 1",
    )
    .param("id", user_id.to_string());
    let mut rs = graph
        .execute(q)
        .await
        .map_err(|e| RepositoryError::DbError(e.to_string()))?;
    if let Some(row) = rs
        .next()
        .await
        .map_err(|e| RepositoryError::DbError(e.to_string()))?
    {
        return Ok(row.get::<String>("lead_id").ok().filter(|s| !s.is_empty()));
    }
    Ok(None)
}
