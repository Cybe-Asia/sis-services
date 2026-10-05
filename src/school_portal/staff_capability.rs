//! Shared current-directory owner capability. No role, identity or assignment writes.
use axum::http::{HeaderMap, StatusCode};
use neo4rs::{query, Graph, Query, Row};
use serde::Deserialize;
#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    #[serde(alias = "schoolId")]
    pub school_id: Option<String>,
    #[serde(alias = "tenantId")]
    pub tenant_id: Option<String>,
}
impl Selection {
    pub fn pair(&self) -> Result<Option<(&str, &str)>, ()> {
        match (&self.school_id, &self.tenant_id) {
            (None, None) => Ok(None),
            (Some(s), Some(t)) if super::model::identifier(s) && super::model::identifier(t) => {
                Ok(Some((s, t)))
            }
            _ => Err(()),
        }
    }
}
pub fn live_scope(
    owner: bool,
    schools: &[String],
    tenants: &[String],
    school: &str,
    tenant: &str,
) -> bool {
    (owner && schools.is_empty() && tenants.is_empty())
        || (schools.contains(&school.into()) && tenants.contains(&tenant.into()))
}
pub fn owner(alias: &str, school: &str, tenant: &str) -> String {
    format!("('owner' IN coalesce({alias}.roles,[]) AND size(coalesce({alias}.teamIds,[]))=0 AND ((size(coalesce({alias}.schoolIds,[]))=0 AND size(coalesce({alias}.tenantIds,[]))=0) OR ({school} IN coalesce({alias}.schoolIds,[]) AND {tenant} IN coalesce({alias}.tenantIds,[]))) AND NOT EXISTS {{MATCH(other:StaffMember {{id:{alias}.id}}) WHERE other<>{alias}}} AND EXISTS {{MATCH(:School {{school_id:{school},tenant_id:{tenant}}})}} AND NOT EXISTS {{MATCH(one:School {{school_id:{school},tenant_id:{tenant}}}),(two:School {{school_id:{school},tenant_id:{tenant}}}) WHERE one<>two}})")
}
pub fn authority(alias: &str, role: &str, school: &str, tenant: &str) -> String {
    format!("{alias}.membershipStatus='ACTIVE' AND ((NOT 'owner' IN coalesce({alias}.roles,[]) AND {role} IN coalesce({alias}.roles,[]) AND {school} IN coalesce({alias}.schoolIds,[]) AND {tenant} IN coalesce({alias}.tenantIds,[])) OR {})", owner(alias,school,tenant))
}

/// Carries the actual credential family through a queued Owner write.
pub struct OwnerSession<'a> {
    pub headers: &'a HeaderMap,
    pub actor: &'a str,
    pub school: &'a str,
    pub tenant: &'a str,
    pub learning: bool,
}
pub async fn live_write(
    graph: &Graph,
    session: OwnerSession<'_>,
    lock: Query,
    write: Query,
) -> Result<Option<Row>, super::Failure> {
    let unavailable = || super::failure(StatusCode::SERVICE_UNAVAILABLE);
    let mut tx = graph.start_txn().await.map_err(|_| unavailable())?;
    let result = async {
        // Consistent actor-before-resource ordering. The actual authority is
        // checked again after both locks; failed checks roll back this counter.
        let mut actor_lock = tx.execute(query("MATCH(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) SET actor.owner_write_lock=coalesce(actor.owner_write_lock,0)+1 RETURN actor.id AS id").param("actor", session.actor)).await.map_err(|_| unavailable())?;
        if actor_lock.next(&mut tx).await.map_err(|_| unavailable())?.is_none() { return Err(super::failure(StatusCode::FORBIDDEN)); }
        while actor_lock.next(&mut tx).await.map_err(|_| unavailable())?.is_some() {}
        let mut locked = tx.execute(lock).await.map_err(|_| unavailable())?;
        if locked.next(&mut tx).await.map_err(|_| unavailable())?.is_none() { return Err(super::failure(StatusCode::FORBIDDEN)); }
        while locked.next(&mut tx).await.map_err(|_| unavailable())?.is_some() {}
        let (actor, role) = if session.learning {
            let member = super::auth::teacher_in(session.headers, session.school, session.tenant).await?;
            (member.staff_member_id.clone(), member.role().to_string())
        } else {
            crate::learning_access::administrator_actor(session.headers, &session.tenant.to_string(), &session.school.to_string()).await?
        };
        if actor != session.actor || role != "owner" { return Err(super::failure(StatusCode::FORBIDDEN)); }
        let mut rows = tx.execute(write).await.map_err(|_| unavailable())?;
        let result = rows.next(&mut tx).await.map_err(|_| unavailable())?;
        // Every write here returns at most one acknowledgement. Ambiguity must
        // not commit multiple writes or immutable evidence.
        if rows.next(&mut tx).await.map_err(|_| unavailable())?.is_some() { return Err(unavailable()); }
        Ok(result)
    }.await;
    match result {
        Ok(Some(row)) => {
            tx.commit().await.map_err(|_| unavailable())?;
            Ok(Some(row))
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

#[derive(Debug)]
pub struct SessionFailure(pub super::Failure);
impl std::fmt::Display for SessionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Owner session or scope denied")
    }
}
impl std::error::Error for SessionFailure {}

#[cfg(test)]
mod tests;
