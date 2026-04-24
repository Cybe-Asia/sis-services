// Minimal User repository for sis-services. auth-services owns the
// :User label; we only need the one function that walks the User's
// Lead edges for the parent-facing SIS endpoints.
//
// Why it lives here at all: the parent /me/sections, /me/grades,
// /me/attendance endpoints need to return data for EVERY child the
// authenticated parent has, across multiple Lead records (the "add
// another child" flow creates additional Leads sharing an email).
// That traversal starts at the Lead from the JWT and expands to
// siblings via the User edge.

use neo4rs::{query, Graph};

use crate::repositories::lead_repository::RepositoryError;

/// List every lead_id that a User (identified by the email linked to
/// `any_lead_id`) has ever owned. Returns at least the input when
/// the User edge is missing (new-parent cold path), so callers can
/// always treat this as the definitive list to scatter queries over.
pub async fn list_lead_ids_for_user_of_lead(
    graph: &Graph,
    any_lead_id: &str,
) -> Result<Vec<String>, RepositoryError> {
    let q = query(
        "MATCH (seed:Lead {lead_id: $seed}) \
         OPTIONAL MATCH (u:User) WHERE toLower(u.email) = toLower(seed.email) \
         OPTIONAL MATCH (u)-[:HAS_APPLICATION]->(l:Lead) \
         WITH seed, collect(DISTINCT l.lead_id) AS linked \
         RETURN CASE WHEN size(linked) = 0 THEN [seed.lead_id] ELSE linked END AS ids",
    )
    .param("seed", any_lead_id.to_string());
    let mut rs = graph
        .execute(q)
        .await
        .map_err(|e| RepositoryError::DbError(e.to_string()))?;
    if let Some(row) = rs
        .next()
        .await
        .map_err(|e| RepositoryError::DbError(e.to_string()))?
    {
        Ok(row.get::<Vec<String>>("ids").unwrap_or_default())
    } else {
        Ok(vec![any_lead_id.to_string()])
    }
}
