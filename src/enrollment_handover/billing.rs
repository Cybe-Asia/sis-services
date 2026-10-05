use super::{
    auth::Actor,
    repository::{directory, Error},
};
use neo4rs::Graph;
use serde_json::{json, Value};
pub async fn billing_candidates(
    graph: &Graph,
    actor: &Actor,
    school: &str,
    tenant: &str,
) -> Result<Value, Error> {
    let mut allowed=graph.execute(directory(actor,vec!["owner".into(),"finance_admin".into(),"finance_approver".into()],"AND school.school_id=$school AND school.tenant_id=$tenant RETURN school.school_id AS id").param("school",school).param("tenant",tenant)).await.map_err(|_|Error::Unavailable)?;
    if allowed
        .next()
        .await
        .map_err(|_| Error::Unavailable)?
        .is_none()
    {
        return Err(Error::Denied);
    }
    if allowed
        .next()
        .await
        .map_err(|_| Error::Unavailable)?
        .is_some()
    {
        return Err(Error::Denied);
    }
    let family = include_str!("../billing_owner/current_family.cypher")
        .replace("(u:User {id:$payer})", "(u:User)")
        .replace("(s:Student {studentId:$student})", "(s:Student)")
        .replace(
            "(other_user:User {id:$payer})",
            "(other_user:User {id:u.id})",
        )
        .replace(
            "(other_student:Student {studentId:$student})",
            "(other_student:Student {studentId:s.studentId})",
        );
    let body=format!("{family} WHERE e.school_id=$school AND e.tenant_id=$tenant RETURN u.id AS payer,s.studentId AS student,e.student_id AS permanent,s.fullName AS name,coalesce(u.fullName,'') AS parent,sec.section_id AS section,sec.name AS section_name ORDER BY student,payer LIMIT 201");
    let mut rows = graph
        .execute(directory(actor,vec!["owner".into(),"finance_admin".into(),"finance_approver".into()],&format!("AND school.school_id=$school AND school.tenant_id=$tenant CALL {{ {body} }} RETURN payer,student,permanent,name,parent,section,section_name")).param("school", school).param("tenant", tenant))
        .await
        .map_err(|_| Error::Unavailable)?;
    let mut candidates = Vec::new();
    while let Some(row) = rows.next().await.map_err(|_| Error::Unavailable)? {
        if candidates.len() == 200 {
            return Err(Error::Unavailable);
        }
        candidates.push(json!({"payerUserId":row.get::<String>("payer").map_err(|_|Error::Unavailable)?,"studentId":row.get::<String>("student").map_err(|_|Error::Unavailable)?,"enrolledStudentId":row.get::<String>("permanent").map_err(|_|Error::Unavailable)?,"studentName":row.get::<String>("name").map_err(|_|Error::Unavailable)?,"payerName":row.get::<String>("parent").map_err(|_|Error::Unavailable)?,"schoolId":school,"tenantId":tenant,"sectionId":row.get::<String>("section").map_err(|_|Error::Unavailable)?,"sectionName":row.get::<String>("section_name").map_err(|_|Error::Unavailable)?}));
    }
    Ok(json!({"contractVersion":1,"schoolId":school,"tenantId":tenant,"candidates":candidates}))
}

pub async fn families(graph: &Graph, actor: &Actor) -> Result<Value, Error> {
    let mut rows=graph.execute(directory(actor,vec!["owner".into(),"finance_admin".into(),"finance_approver".into()],"RETURN school.school_id AS school,school.tenant_id AS tenant,coalesce(school.name,school.school_id) AS name ORDER BY school,tenant LIMIT 65")).await.map_err(|_|Error::Unavailable)?;
    let mut scopes = Vec::new();
    while let Some(row) = rows.next().await.map_err(|_| Error::Unavailable)? {
        if scopes.len() == 64 {
            return Err(Error::Unavailable);
        }
        scopes.push((
            row.get::<String>("school")
                .map_err(|_| Error::Unavailable)?,
            row.get::<String>("tenant")
                .map_err(|_| Error::Unavailable)?,
            row.get::<String>("name").map_err(|_| Error::Unavailable)?,
        ));
    }
    if scopes.is_empty() {
        return Err(Error::Denied);
    }
    let mut families = Vec::new();
    for (school, tenant, name) in scopes {
        let payload = billing_candidates(graph, actor, &school, &tenant).await?;
        for row in payload["candidates"].as_array().ok_or(Error::Unavailable)? {
            if families.len() == 200 {
                return Err(Error::Unavailable);
            }
            let mut row = row.clone();
            row["schoolName"] = json!(name);
            families.push(row);
        }
    }
    Ok(json!(families))
}
