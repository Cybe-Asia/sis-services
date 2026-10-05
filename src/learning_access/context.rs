//! Minimal scoped Section/roster projection for governed learning access.
use super::*;
use std::collections::BTreeMap;
pub(super) const CONTEXT_QUERY:&str="MATCH(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) MATCH(sec:Section {status:'active'}) WHERE sec.school_id IN $schools AND sec.tenant_id IN $tenants AND ((NOT 'owner' IN coalesce(actor.roles,[]) AND 'school_admin' IN coalesce(actor.roles,[]) AND sec.school_id IN coalesce(actor.schoolIds,[]) AND sec.tenant_id IN coalesce(actor.tenantIds,[])) OR ('owner' IN coalesce(actor.roles,[]) AND size(coalesce(actor.teamIds,[]))=0 AND ((size(coalesce(actor.schoolIds,[]))=0 AND size(coalesce(actor.tenantIds,[]))=0) OR (sec.school_id IN coalesce(actor.schoolIds,[]) AND sec.tenant_id IN coalesce(actor.tenantIds,[]))) AND NOT EXISTS {MATCH(other:StaffMember {id:actor.id}) WHERE other<>actor} AND EXISTS {MATCH(:School {school_id:sec.school_id,tenant_id:sec.tenant_id})} AND NOT EXISTS {MATCH(one:School {school_id:sec.school_id,tenant_id:sec.tenant_id}),(two:School {school_id:sec.school_id,tenant_id:sec.tenant_id}) WHERE one<>two})) OPTIONAL MATCH(s:Student)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:sec.school_id,tenant_id:sec.tenant_id})-[:ENROLLED_IN]->(sec) RETURN DISTINCT sec.section_id AS section,sec.school_id AS school,sec.tenant_id AS tenant,sec.name AS name,s.studentId AS student,s.fullName AS student_name ORDER BY section,student LIMIT 1001";
pub(super) async fn context(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(selection): Query<crate::school_portal::staff_capability::Selection>,
) -> Result<Json<Value>, Failure> {
    let member = calendar_membership_for(&headers, &selection).await?;
    let mut rows = state
        .graph
        .execute(
            query(CONTEXT_QUERY)
                .param("actor", member.0)
                .param("schools", member.1)
                .param("tenants", member.2),
        )
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut classes: BTreeMap<String, Value> = BTreeMap::new();
    let mut count = 0;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        count += 1;
        if count > 1000 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        let section: String = row
            .get("section")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let school: String = row
            .get("school")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let tenant: String = row
            .get("tenant")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let name: String = row
            .get("name")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        if ![&section, &school, &tenant].iter().all(|id| valid_id(id))
            || name.is_empty()
            || name.len() > 256
        {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        let class = classes.entry(section.clone()).or_insert_with(|| json!({"sectionId":section,"schoolId":school,"tenantId":tenant,"name":name,"students":[]}));
        let student: Option<String> = row
            .get("student")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        if let Some(id) = student {
            let name: String = row
                .get("student_name")
                .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
            if !valid_id(&id) || name.is_empty() || name.len() > 256 {
                return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
            }
            let students = class["students"]
                .as_array_mut()
                .ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))?;
            if students.len() >= 200 {
                return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
            }
            students.push(json!({"id":id,"name":name}));
        }
        if classes.len() > 200 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
    }
    Ok(Json(
        json!({"data":{"classes":classes.into_values().collect::<Vec<_>>()}}),
    ))
}
