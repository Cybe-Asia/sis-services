use super::{
    auth::Actor,
    model::{Command, Receipt},
};
use neo4rs::{query, Graph, Query};
#[derive(Debug)]
pub enum Error {
    Denied,
    Conflict,
    Unavailable,
}
const FAMILY: &str = include_str!("family.cypher");
const CURRENT: &str = include_str!("current.cypher");
const CREATE: &str = include_str!("create.cypher");
const CONSTRAINTS: [(&str, &str, &str); 5] = [
    (
        "sis_handover_student",
        "SISEnrollmentHandover",
        "student_id",
    ),
    (
        "sis_enrollment_applicant",
        "EnrolledStudent",
        "applicant_student_id",
    ),
    ("sis_enrollment_permanent", "EnrolledStudent", "student_id"),
    ("sis_enrollment_number", "EnrolledStudent", "student_number"),
    ("sis_enrollment_audit", "SISEnrollmentAudit", "id"),
];
pub async fn migrate(graph: &Graph) -> Result<(), neo4rs::Error> {
    for (name, label, field) in CONSTRAINTS {
        graph.run(query(&format!("CREATE CONSTRAINT {name} IF NOT EXISTS FOR (n:{label}) REQUIRE n.{field} IS UNIQUE"))).await?;
    }
    Ok(())
}
fn parameters(body: &str, actor: &Actor, c: &Command) -> Query {
    query(body)
        .param("subject", actor.subject.clone())
        .param("actor", actor.staff.clone())
        .param("expires", actor.expires)
        .param("payer", c.payer_user_id.clone())
        .param("student", c.student_id.clone())
        .param("lead", c.admission_id.clone())
        .param("application", c.application_id.clone())
        .param("offer", c.offer_id.clone())
        .param("payment", c.payment_id.clone())
        .param("revision", i64::from(c.offer_revision))
        .param("hash", c.pricing_snapshot_hash.clone())
        .param("school", c.school_id.clone())
        .param("tenant", c.tenant_id.clone())
        .param("section", c.section_id.clone())
        .param("year", c.year_group.clone())
        .param("academic", c.academic_year.clone())
}
pub async fn execute(graph: &Graph, actor: &Actor, c: &Command) -> Result<Receipt, Error> {
    let mut schema = graph
        .execute(
            query("SHOW CONSTRAINTS YIELD name WHERE name IN $names RETURN count(*) AS count")
                .param(
                    "names",
                    CONSTRAINTS
                        .iter()
                        .map(|(name, _, _)| name.to_string())
                        .collect::<Vec<_>>(),
                ),
        )
        .await
        .map_err(|_| Error::Unavailable)?;
    if schema
        .next()
        .await
        .map_err(|_| Error::Unavailable)?
        .and_then(|r| r.get::<i64>("count").ok())
        != Some(5)
    {
        return Err(Error::Unavailable);
    }
    let mut tx = graph.start_txn().await.map_err(|_| Error::Unavailable)?;
    let outcome=async {
        // No mutation until current caller, family and paid offer are authorized.
        let mut pre=tx.execute(parameters(&format!("{FAMILY}{CURRENT} RETURN s.studentId AS student"),actor,c)).await.map_err(|_|Error::Unavailable)?;
        if pre.next(&mut tx).await.map_err(|_|Error::Unavailable)?.is_none(){return Err(Error::Denied);}
        if pre.next(&mut tx).await.map_err(|_|Error::Unavailable)?.is_some(){return Err(Error::Denied);}
        drop(pre);
        // All shared write locks are held before the final fresh authority read.
        tx.run(parameters("MERGE(h:SISEnrollmentHandover {student_id:$student}) ON CREATE SET h.version=0 SET h.lock=coalesce(h.lock,0)+1 WITH h MATCH(s:Student {studentId:$student}) SET s.sis_enrollment_lock=coalesce(s.sis_enrollment_lock,0)+1 WITH h,s MATCH(sec:Section {section_id:$section,school_id:$school,tenant_id:$tenant}) SET sec.sis_enrollment_lock=coalesce(sec.sis_enrollment_lock,0)+1",actor,c)).await.map_err(|_|Error::Unavailable)?;
        let request=serde_json::to_string(c).map_err(|_|Error::Unavailable)?;
        let permanent=format!("STU-{}",uuid::Uuid::new_v4());
        let number=format!("{}{}-{}",c.school_id.trim_start_matches("SCH-"),chrono::Utc::now().format("%Y"),&uuid::Uuid::new_v4().simple().to_string()[..12]);
        let q=parameters(&format!("{FAMILY}{CURRENT}{CREATE}"),actor,c).param("request",request).param("permanent",permanent).param("number",number).param("handover",format!("SISHAND-{}",uuid::Uuid::new_v4())).param("audit",format!("SISAUD-{}",uuid::Uuid::new_v4()));
        let mut rows=tx.execute(q).await.map_err(|_|Error::Conflict)?;
        let row=rows.next(&mut tx).await.map_err(|_|Error::Unavailable)?.ok_or(Error::Conflict)?;
        let receipt=Receipt {contract_version:1,version:1,student_id:c.student_id.clone(),enrolled_student_id:row.get("permanent").map_err(|_|Error::Unavailable)?,student_number:row.get("number").map_err(|_|Error::Unavailable)?,school_id:c.school_id.clone(),tenant_id:c.tenant_id.clone(),section_id:c.section_id.clone(),year_group:c.year_group.clone(),academic_year:c.academic_year.clone(),enrolment_date:row.get("date").map_err(|_|Error::Unavailable)?};
        if rows.next(&mut tx).await.map_err(|_|Error::Unavailable)?.is_some(){return Err(Error::Conflict);}
        Ok(receipt)
    }.await;
    match outcome {
        Ok(receipt) => {
            tx.commit().await.map_err(|_| Error::Unavailable)?;
            Ok(receipt)
        }
        Err(error) => {
            tx.rollback().await.map_err(|_| Error::Unavailable)?;
            Err(error)
        }
    }
}

const DIRECTORY: &str = "MATCH(staff_user:User {id:$subject})-[link:STAFF_MEMBER]->(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WHERE any(r IN coalesce(actor.roles,[]) WHERE r IN $roles) AND size(coalesce(actor.teamIds,[]))=0 AND datetime.realtime().epochSeconds<$expires AND NOT EXISTS {MATCH(other:User {id:$subject}) WHERE other<>staff_user} AND NOT EXISTS {MATCH(other:StaffMember {id:$actor}) WHERE other<>actor} AND NOT EXISTS {MATCH(staff_user)-[other:STAFF_MEMBER]->() WHERE other<>link} AND NOT EXISTS {MATCH(other:User)-[:STAFF_MEMBER]->(actor) WHERE other<>staff_user} WITH actor MATCH(school:School) WHERE (('owner' IN coalesce(actor.roles,[]) AND size(coalesce(actor.schoolIds,[]))=0 AND size(coalesce(actor.tenantIds,[]))=0) OR (school.school_id IN coalesce(actor.schoolIds,[]) AND school.tenant_id IN coalesce(actor.tenantIds,[]))) AND coalesce(school.school_id,'')<>'' AND coalesce(school.tenant_id,'')<>'' AND NOT EXISTS {MATCH(other:School {school_id:school.school_id,tenant_id:school.tenant_id}) WHERE other<>school}";
pub(super) fn directory(actor: &Actor, roles: Vec<String>, suffix: &str) -> Query {
    query(&format!("{DIRECTORY} {suffix}"))
        .param("subject", actor.subject.clone())
        .param("actor", actor.staff.clone())
        .param("expires", actor.expires)
        .param("roles", roles)
}
