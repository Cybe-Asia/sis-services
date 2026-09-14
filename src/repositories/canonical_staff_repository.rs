//! Read-only bridge to the auth-owned Staff Directory. Never resolve permissions by email.
use neo4rs::{query, Graph};

#[derive(Debug, PartialEq)]
pub enum CanonicalAdmin {
    NotLinked,
    Denied,
    Owner(String),
}

#[derive(Debug, PartialEq)]
pub enum CanonicalStaff {
    NotLinked,
    Denied,
    Active { id: String, roles: Vec<String> },
}

pub async fn resolve_admin(graph: &Graph, user_id: &str) -> Result<CanonicalAdmin, String> {
    Ok(match resolve_staff(graph, user_id).await? {
        CanonicalStaff::NotLinked => CanonicalAdmin::NotLinked,
        CanonicalStaff::Active { id, roles } if roles.iter().any(|r| r == "owner") => {
            CanonicalAdmin::Owner(id)
        }
        _ => CanonicalAdmin::Denied,
    })
}

// Preserve all links, including inactive memberships, so suspension and ambiguous
// identity links cannot fall through to the legacy email allowlist.
const LOOKUP: &str = "MATCH (u:User {id:$userId})-[:STAFF_MEMBER]->(s:StaffMember) \
 RETURN s.id AS id, coalesce(s.membershipStatus,'') AS status, \
 coalesce(s.roles,[]) AS roles, coalesce(s.tenantIds,[]) AS tenantIds, \
 coalesce(s.schoolIds,[]) AS schoolIds, coalesce(s.teamIds,[]) AS teamIds LIMIT 2";

pub async fn resolve_staff(graph: &Graph, user_id: &str) -> Result<CanonicalStaff, String> {
    let lookup = if user_id.starts_with("LEAD-") {
        LOOKUP.replace(
            "MATCH (u:User {id:$userId})",
            "MATCH (u:User)-[:HAS_APPLICATION]->(:Lead {lead_id:$userId}) MATCH (u)",
        )
    } else {
        LOOKUP.to_string()
    };
    let mut rows = graph
        .execute(query(&lookup).param("userId", user_id.to_string()))
        .await
        .map_err(|_| "Staff authorization unavailable".to_string())?;
    let Some(row) = rows
        .next()
        .await
        .map_err(|_| "Staff authorization unavailable".to_string())?
    else {
        return Ok(CanonicalStaff::NotLinked);
    };
    if rows
        .next()
        .await
        .map_err(|_| "Staff authorization unavailable".to_string())?
        .is_some()
    {
        return Ok(CanonicalStaff::Denied);
    }
    let id = row.get::<String>("id").unwrap_or_default();
    let status = row.get::<String>("status").unwrap_or_default();
    let roles = row.get::<Vec<String>>("roles").unwrap_or_default();
    let tenants = row
        .get::<Vec<String>>("tenantIds")
        .map_err(|_| "Invalid staff scope".to_string())?;
    let schools = row
        .get::<Vec<String>>("schoolIds")
        .map_err(|_| "Invalid staff scope".to_string())?;
    let teams = row
        .get::<Vec<String>>("teamIds")
        .map_err(|_| "Invalid staff scope".to_string())?;
    // These legacy endpoints expose global queues and mutations. Only active,
    // unscoped memberships can use this bridge; callers enforce role capabilities.
    // Scoped staff remain denied until a resource-level guard is present.
    if !id.is_empty()
        && status == "ACTIVE"
        && !roles.is_empty()
        && tenants.is_empty()
        && schools.is_empty()
        && teams.is_empty()
    {
        Ok(CanonicalStaff::Active { id, roles })
    } else {
        Ok(CanonicalStaff::Denied)
    }
}
