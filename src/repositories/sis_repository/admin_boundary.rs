//! Live directory/scope checks and atomic academic writes. No email authority.
use super::RepositoryError;
use crate::handlers::auth_helper::AdminActor;
use neo4rs::{query, Graph, Query, Row, Txn};

pub(super) const ACTOR: &str = "MATCH(u:User {id:$subject})-[link:STAFF_MEMBER]->(a:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WHERE u.staffMemberId=a.id AND any(role IN coalesce(a.roles,[]) WHERE role IN ['owner','school_admin']) AND size(coalesce(a.teamIds,[]))=0 AND datetime.realtime().epochSeconds<$expires AND ((size(coalesce(a.schoolIds,[]))>0 AND size(coalesce(a.tenantIds,[]))>0) OR ('owner' IN coalesce(a.roles,[]) AND size(coalesce(a.schoolIds,[]))=0 AND size(coalesce(a.tenantIds,[]))=0)) AND NOT EXISTS {MATCH(other:User {id:u.id}) WHERE other<>u} AND NOT EXISTS {MATCH(other:StaffMember {id:a.id}) WHERE other<>a} AND NOT EXISTS {MATCH(u)-[other:STAFF_MEMBER]->() WHERE other<>link} AND NOT EXISTS {MATCH(other:User)-[:STAFF_MEMBER]->(a) WHERE other<>u}";
pub(super) const SCHOOL: &str = "MATCH(school:School {school_id:$school,tenant_id:$tenant}) WHERE coalesce(school.school_id,'')<>'' AND coalesce(school.tenant_id,'')<>'' AND NOT EXISTS {MATCH(other:School {school_id:school.school_id}) WHERE other<>school} AND (('owner' IN coalesce(a.roles,[]) AND size(coalesce(a.schoolIds,[]))=0 AND size(coalesce(a.tenantIds,[]))=0) OR (school.school_id IN coalesce(a.schoolIds,[]) AND school.tenant_id IN coalesce(a.tenantIds,[])))";
// Orphan/foreign/ambiguous enrollment edges cannot expose a child's records.
pub(super) const CHILD: &str = "e.status='active' AND coalesce(e.student_id,'')<>'' AND coalesce(e.applicant_student_id,s.studentId)=s.studentId AND NOT EXISTS {MATCH(other:EnrolledStudent {applicant_student_id:s.studentId}) WHERE other<>e} AND e.school_id=sec.school_id AND e.tenant_id=sec.tenant_id AND coalesce(e.academic_year,sec.academic_year)=sec.academic_year AND coalesce(e.year_group,sec.year_group)=sec.year_group AND NOT EXISTS {MATCH(other:Student {studentId:s.studentId}) WHERE other<>s} AND NOT EXISTS {MATCH(s)-[:ENROLLED_AS]->(other:EnrolledStudent) WHERE other<>e} AND NOT EXISTS {MATCH(other:Student)-[:ENROLLED_AS]->(e) WHERE other<>s} AND NOT EXISTS {MATCH(other:EnrolledStudent {student_id:e.student_id}) WHERE other<>e} AND NOT EXISTS {MATCH(e)-[:ENROLLED_IN]->(other:Section) WHERE other<>sec}";

pub(super) fn forbidden() -> RepositoryError {
    RepositoryError::DbError("FORBIDDEN".into())
}
pub(super) fn invalid() -> RepositoryError {
    RepositoryError::DbError("INVALID_INPUT".into())
}
fn unavailable() -> RepositoryError {
    RepositoryError::DbError("DEPENDENCY_UNAVAILABLE".into())
}
pub(super) fn parameters(q: Query, actor: &AdminActor) -> Query {
    q.param("subject", actor.subject.clone())
        .param("actor", actor.staff.clone())
        .param("expires", actor.expires)
}
async fn rows(tx: &mut Txn, q: Query) -> Result<Vec<Row>, RepositoryError> {
    let mut stream = tx.execute(q).await.map_err(|_| unavailable())?;
    let mut out = Vec::new();
    while let Some(row) = stream.next(tx.handle()).await.map_err(|_| unavailable())? {
        if out.len() >= 5000 {
            return Err(unavailable());
        }
        out.push(row);
    }
    Ok(out)
}
async fn one(tx: &mut Txn, q: Query) -> Result<Row, RepositoryError> {
    let mut out = rows(tx, q).await?;
    if out.len() != 1 {
        return Err(forbidden());
    }
    Ok(out.remove(0))
}
pub(super) async fn check(graph: &Graph, actor: &AdminActor) -> Result<(), RepositoryError> {
    let mut rs = graph
        .execute(parameters(
            query(&format!("{ACTOR} RETURN a.id AS id")),
            actor,
        ))
        .await
        .map_err(|_| unavailable())?;
    if rs.next().await.map_err(|_| unavailable())?.is_none()
        || rs.next().await.map_err(|_| unavailable())?.is_some()
    {
        return Err(forbidden());
    }
    Ok(())
}
#[derive(Clone, Copy)]
pub(super) enum Resource<'a> {
    List(Option<&'a str>),
    School(&'a str, &'a str),
    Section(&'a str),
}

/// One transaction owns current authorization, scope, all student checks,
/// the write and its actual-Staff audit. Any denied member rolls back the batch.
pub(super) async fn execute(
    graph: &Graph,
    actor: &AdminActor,
    resource: Resource<'_>,
    q: Query,
    action: Option<(&str, &str)>,
    students: Option<(&[String], bool)>,
) -> Result<Vec<Row>, RepositoryError> {
    let mut tx = graph.start_txn().await.map_err(|_| unavailable())?;
    let result = locked(&mut tx, actor, resource, q, action, students).await;
    match result {
        Ok(value) => {
            tx.commit().await.map_err(|_| unavailable())?;
            Ok(value)
        }
        Err(e) => {
            tx.rollback().await.map_err(|_| unavailable())?;
            Err(e)
        }
    }
}
async fn locked(
    tx: &mut Txn,
    actor: &AdminActor,
    resource: Resource<'_>,
    q: Query,
    action: Option<(&str, &str)>,
    students: Option<(&[String], bool)>,
) -> Result<Vec<Row>, RepositoryError> {
    one(
        tx,
        parameters(query(&format!("{ACTOR} RETURN a.id AS id")), actor),
    )
    .await?;
    let scope = match resource {
        Resource::Section(id) => {
            let found = rows(tx,query("MATCH(sec:Section {section_id:$id}) RETURN sec.school_id AS school,sec.tenant_id AS tenant").param("id",id)).await?;
            if found.is_empty() {
                return Err(RepositoryError::NotFound);
            }
            if found.len() != 1 {
                return Err(forbidden());
            }
            Some((
                found[0].get::<String>("school").map_err(|_| forbidden())?,
                found[0].get::<String>("tenant").map_err(|_| forbidden())?,
            ))
        }
        Resource::School(school, tenant) => Some((school.into(), tenant.into())),
        Resource::List(Some(school)) => {
            let found = rows(
                tx,
                query("MATCH(school:School {school_id:$school}) RETURN school.tenant_id AS tenant")
                    .param("school", school),
            )
            .await?;
            if found.len() != 1 {
                return Err(forbidden());
            }
            Some((
                school.into(),
                found[0].get::<String>("tenant").map_err(|_| forbidden())?,
            ))
        }
        Resource::List(None) => None,
    };
    let scoped = |body: &str| {
        let mut q = parameters(query(body), actor);
        if let Some((school, tenant)) = &scope {
            q = q
                .param("school", school.clone())
                .param("tenant", tenant.clone());
        }
        q
    };
    if scope.is_some() {
        one(tx,scoped(&format!("{ACTOR} WITH a {SCHOOL} SET school.school_id=school.school_id RETURN school.school_id AS id"))).await?;
    }
    // Directory updates also lock Staff before User. Do not invert this order.
    one(
        tx,
        parameters(
            query("MATCH(a:StaffMember {id:$actor}) SET a.id=a.id RETURN a.id AS id"),
            actor,
        ),
    )
    .await?;
    one(
        tx,
        parameters(
            query("MATCH(u:User {id:$subject}) SET u.id=u.id RETURN u.id AS id"),
            actor,
        ),
    )
    .await?;
    // Admission handover locks Student before Section; keep the shared order.
    if let Some((ids, _)) = students {
        let locked=rows(tx,query("UNWIND $ids AS id MATCH(s:Student {studentId:id}) WITH DISTINCT s ORDER BY s.studentId SET s.studentId=s.studentId RETURN s.studentId AS id").param("ids",ids.to_vec())).await?;
        if locked.len() != ids.len() {
            return Err(forbidden());
        }
    }
    if let Resource::Section(id) = resource {
        one(tx,query("MATCH(sec:Section {section_id:$id}) SET sec.section_id=sec.section_id RETURN sec.section_id AS id").param("id",id)).await?;
    }
    let authority = if scope.is_some() {
        format!("{ACTOR} WITH a,u {SCHOOL}")
    } else {
        ACTOR.into()
    };
    let current=one(tx,scoped(&format!("{authority} RETURN CASE WHEN 'owner' IN coalesce(a.roles,[]) THEN 'owner' ELSE 'school_admin' END AS role"))).await?;
    if let Resource::Section(id) = resource {
        let (school, tenant) = scope.as_ref().ok_or_else(unavailable)?;
        one(tx,query("MATCH(sec:Section {section_id:$id,school_id:$school,tenant_id:$tenant}) RETURN sec.section_id AS id").param("id",id).param("school",school.clone()).param("tenant",tenant.clone())).await?;
        if let Some((ids, roster)) = students {
            let child = if roster {
                CHILD.to_string()
            } else {
                CHILD.replace("AND NOT EXISTS {MATCH(e)-[:ENROLLED_IN]->(other:Section) WHERE other<>sec}","AND NOT EXISTS {MATCH(e)-[:ENROLLED_IN]->(other:Section) WHERE coalesce(other.school_id,'')<>sec.school_id OR coalesce(other.tenant_id,'')<>sec.tenant_id OR coalesce(other.academic_year,'')<>sec.academic_year OR coalesce(other.year_group,'')<>sec.year_group}")
            };
            let roster_match = if roster {
                "MATCH(e)-[:ENROLLED_IN]->(sec)"
            } else {
                ""
            };
            let guarded=format!("MATCH(sec:Section {{section_id:$id,status:'active'}}) UNWIND $ids AS sid MATCH(s:Student {{studentId:sid}})-[:ENROLLED_AS]->(e:EnrolledStudent) {roster_match} WITH DISTINCT sec,s,e ORDER BY s.studentId SET e.student_id=e.student_id WITH sec,s,e WHERE {child} RETURN s.studentId AS id");
            let eligible = rows(
                tx,
                query(&guarded).param("id", id).param("ids", ids.to_vec()),
            )
            .await?;
            if eligible.len() != ids.len() {
                return Err(forbidden());
            }
        }
    }
    let result = rows(tx, parameters(q, actor)).await?;
    if action.is_some() {
        if result.len() != 1 {
            return Err(unavailable());
        }
        if let Some((ids, _)) = students {
            let row = &result[0];
            let saved = row
                .get::<i64>("n")
                .or_else(|_| row.get::<i64>("assigned"))
                .map_err(|_| unavailable())?;
            if saved != ids.len() as i64 {
                return Err(forbidden());
            }
        }
    }
    // A queued lock or an expired credential must never commit stale authority.
    one(tx, scoped(&format!("{authority} RETURN a.id AS id"))).await?;
    if let Some((action, target)) = action {
        let (school, tenant) = scope.as_ref().ok_or_else(unavailable)?;
        let role: String = current.get("role").map_err(|_| unavailable())?;
        tx.run(query("CREATE(audit:SISAcademicAudit {id:$id,actor_staff_member_id:$actor,actor_user_id:$subject,actor_role:$role,action:$action,section_id:$target,school_id:$school,tenant_id:$tenant,occurred_at:datetime()})").param("id",format!("SISACA-{}",uuid::Uuid::new_v4())).param("actor",actor.staff.clone()).param("subject",actor.subject.clone()).param("role",role).param("action",action).param("target",target).param("school",school.clone()).param("tenant",tenant.clone())).await.map_err(|_| unavailable())?;
    }
    Ok(result)
}

pub(super) fn ids_valid(ids: &[String], max: usize) -> bool {
    !ids.is_empty()
        && ids.len() <= max
        && ids.iter().all(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
        })
        && ids.iter().collect::<std::collections::BTreeSet<_>>().len() == ids.len()
}

#[cfg(test)]
mod tests;
