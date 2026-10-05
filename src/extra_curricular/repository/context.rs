use super::Error;
use crate::extra_curricular::{model::*, ports::CatalogContext};
use neo4rs::{query, Graph};
use serde_json::json;
pub(super) async fn load(
    graph: &Graph,
    a: &Actor,
    student_id: Option<String>,
    live_schools: Vec<String>,
    live_tenants: Vec<String>,
) -> Result<CatalogContext, Error> {
    let role = &a.role;
    let body=match role.as_str(){
 "admin"|"teacher"|"owner"=>"MATCH(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) MATCH(sec:Section {status:'active'}) WHERE sec.school_id IN $schools AND sec.tenant_id IN $tenants AND ((NOT 'owner' IN coalesce(actor.roles,[]) AND $role IN coalesce(actor.roles,[]) AND sec.school_id IN coalesce(actor.schoolIds,[]) AND sec.tenant_id IN coalesce(actor.tenantIds,[])) OR ('owner' IN coalesce(actor.roles,[]) AND size(coalesce(actor.teamIds,[]))=0 AND ((size(coalesce(actor.schoolIds,[]))=0 AND size(coalesce(actor.tenantIds,[]))=0) OR (sec.school_id IN coalesce(actor.schoolIds,[]) AND sec.tenant_id IN coalesce(actor.tenantIds,[]))) AND NOT EXISTS {MATCH(other:StaffMember {id:actor.id}) WHERE other<>actor} AND EXISTS {MATCH(:School {school_id:sec.school_id,tenant_id:sec.tenant_id})} AND NOT EXISTS {MATCH(one:School {school_id:sec.school_id,tenant_id:sec.tenant_id}),(two:School {school_id:sec.school_id,tenant_id:sec.tenant_id}) WHERE one<>two})) AND coalesce(sec.academic_year,'')<>'' RETURN DISTINCT sec.school_id AS school,sec.tenant_id AS tenant,sec.academic_year AS year,sec.section_id AS section,sec.name AS name".into(),
 "student"=>"MATCH(actor:User {id:$actor,role:'student'}) MATCH(b:LearningStudentBinding {user_id:actor.id,status:'ACTIVE'}) MATCH(student:Student {studentId:b.student_id})-[:ENROLLED_AS]->(e:EnrolledStudent {status:'active',school_id:b.school_id,tenant_id:b.tenant_id})-[:ENROLLED_IN]->(sec:Section {status:'active',school_id:b.school_id,tenant_id:b.tenant_id}) RETURN DISTINCT sec.school_id AS school,sec.tenant_id AS tenant,sec.academic_year AS year,sec.section_id AS section,sec.name AS name".into(),
 "parent"=>format!("{} AND u.id=$actor AND s.studentId=$student WITH DISTINCT s MATCH(s)-[:ENROLLED_AS]->(e:EnrolledStudent {{status:'active'}})-[:ENROLLED_IN]->(sec:Section {{status:'active',school_id:e.school_id,tenant_id:e.tenant_id}}) RETURN DISTINCT sec.school_id AS school,sec.tenant_id AS tenant,sec.academic_year AS year,sec.section_id AS section,sec.name AS name",crate::school_portal::repository::PARENT),_=>return Err("invalid context role".into())};
    let mut rows = graph
        .execute(
            query(&format!(
                "{body} ORDER BY school,tenant,year,section LIMIT 201"
            ))
            .param("actor", a.id.clone())
            .param("sub", a.id.clone())
            .param("schools", live_schools.clone())
            .param("tenants", live_tenants.clone())
            .param("student", student_id.unwrap_or_default())
            .param(
                "role",
                if matches!(role.as_str(), "admin" | "owner") {
                    "school_admin"
                } else {
                    "teacher"
                },
            ),
        )
        .await
        .map_err(|e| -> Error { Box::new(e) })?;
    let mut classes = vec![];
    while let Some(r) = rows.next().await.map_err(|e| -> Error { Box::new(e) })? {
        if classes.len() == 200 {
            return Err("context response exceeds contract".into());
        }
        let get = |k| r.get::<String>(k).map_err(|e| -> Error { Box::new(e) });
        let scope = Scope {
            school_id: get("school")?,
            tenant_id: get("tenant")?,
            academic_year: get("year")?,
            student_id: None,
            after: None,
        };
        if !scope.valid() {
            return Err("context response exceeds contract".into());
        }
        classes.push(json!({"schoolId":scope.school_id,"tenantId":scope.tenant_id,"academicYear":scope.academic_year,"sectionId":get("section")?,"name":get("name")?}));
    }
    let mut staff = vec![];
    if matches!(role.as_str(), "admin" | "owner") {
        let mut rows=graph.execute(query("MATCH(a:StaffMember {id:$actor,membershipStatus:'ACTIVE'}),(t:StaffMember {membershipStatus:'ACTIVE'}) WHERE 'teacher' IN coalesce(t.roles,[]) AND any(x IN coalesce(t.schoolIds,[]) WHERE x IN $schools AND (x IN coalesce(a.schoolIds,[]) OR 'owner' IN coalesce(a.roles,[]))) AND any(x IN coalesce(t.tenantIds,[]) WHERE x IN $tenants AND (x IN coalesce(a.tenantIds,[]) OR 'owner' IN coalesce(a.roles,[]))) AND ((NOT 'owner' IN coalesce(a.roles,[]) AND 'school_admin' IN coalesce(a.roles,[])) OR ('owner' IN coalesce(a.roles,[]) AND size(coalesce(a.teamIds,[]))=0 AND ((size(coalesce(a.schoolIds,[]))=0 AND size(coalesce(a.tenantIds,[]))=0) OR ($schools[0] IN coalesce(a.schoolIds,[]) AND $tenants[0] IN coalesce(a.tenantIds,[]))) AND NOT EXISTS {MATCH(other:StaffMember {id:a.id}) WHERE other<>a} AND EXISTS {MATCH(:School {school_id:$schools[0],tenant_id:$tenants[0]})} AND NOT EXISTS {MATCH(one:School {school_id:$schools[0],tenant_id:$tenants[0]}),(two:School {school_id:$schools[0],tenant_id:$tenants[0]}) WHERE one<>two})) RETURN DISTINCT t.id AS id,coalesce(t.fullName,t.name,t.id) AS name,[x IN coalesce(t.schoolIds,[]) WHERE x IN $schools AND (x IN coalesce(a.schoolIds,[]) OR 'owner' IN coalesce(a.roles,[]))] AS schools,[x IN coalesce(t.tenantIds,[]) WHERE x IN $tenants AND (x IN coalesce(a.tenantIds,[]) OR 'owner' IN coalesce(a.roles,[]))] AS tenants ORDER BY id LIMIT 201").param("actor",a.id.clone()).param("schools",live_schools).param("tenants",live_tenants)).await.map_err(|e| -> Error { Box::new(e) })?;
        while let Some(r) = rows.next().await.map_err(|e| -> Error { Box::new(e) })? {
            if staff.len() == 200 {
                return Err("context response exceeds contract".into());
            }
            staff.push(json!({"id":r.get::<String>("id").map_err(|e| -> Error { Box::new(e) })?,"name":r.get::<String>("name").map_err(|e| -> Error { Box::new(e) })?,"schoolIds":r.get::<Vec<String>>("schools").map_err(|e| -> Error { Box::new(e) })?,"tenantIds":r.get::<Vec<String>>("tenants").map_err(|e| -> Error { Box::new(e) })?}));
        }
    }
    Ok(serde_json::from_value(
        json!({"contractVersion":1,"classes":classes,"staff":staff}),
    )?)
}
