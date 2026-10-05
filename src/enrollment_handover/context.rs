use super::{
    auth::Actor,
    repository::{directory, Error},
};
use neo4rs::Graph;
use serde_json::{json, Value};
const CURRENT: &str = include_str!("current.cypher");
pub async fn context(graph: &Graph, actor: &Actor) -> Result<Value, Error> {
    let mut rows=graph.execute(directory(actor,vec!["owner".into(),"school_admin".into(),"admissions_admin".into()],"RETURN school.school_id AS school,school.tenant_id AS tenant,coalesce(school.name,school.school_id) AS name ORDER BY school,tenant LIMIT 65")).await.map_err(|_|Error::Unavailable)?;
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
    let mut schools = Vec::new();
    let mut sections = Vec::new();
    let mut candidates = Vec::new();
    let family = include_str!("family.cypher")
        .split("WITH collect")
        .next()
        .ok_or(Error::Unavailable)?
        .replace("{id:$payer}", "{}")
        .replace("{studentId:$student}", "{}")
        .replace("lead_id:$lead,", "")
        .replace("{id:$payer}", "{id:u.id}")
        .replace("{studentId:$student}", "{studentId:s.studentId}");
    // Correlated uniqueness predicates remain bound to the matched canonical entities.
    let family = family
        .replace("(other_user:User {})", "(other_user:User {id:u.id})")
        .replace(
            "(other_student:Student {})",
            "(other_student:Student {studentId:s.studentId})",
        );
    let mut current = CURRENT
        .split("WITH u,l,s,actor")
        .nth(1)
        .ok_or(Error::Unavailable)?
        .to_string();
    for (parameter, field) in [
        ("$application", "app.application_id"),
        ("$student", "s.studentId"),
        ("$lead", "l.lead_id"),
        ("$offer", "o.offer_id"),
        ("$revision", "o.revision"),
        ("$hash", "o.pricing_snapshot_hash"),
        ("$payment", "p.payment_id"),
        ("$year", "o.target_year_group"),
        ("$academic", "o.academic_year"),
    ] {
        current = current.replace(parameter, field);
    }
    // Remove self-references from newly bound node patterns; check them after binding.
    current = current
        .replace("application_id:app.application_id,", "")
        .replace(
            "offer_id:o.offer_id,applicant_student_id:s.studentId,",
            "applicant_student_id:s.studentId,",
        )
        .replace(
            ",revision:o.revision,pricing_snapshot_hash:o.pricing_snapshot_hash",
            "",
        )
        .replace("payment_id:p.payment_id,", "");
    current=current.replace("MATCH (sec:Section {section_id:$section,school_id:$school,tenant_id:$tenant,status:'active',year_group:o.target_year_group,academic_year:o.academic_year})", "MATCH(sec:Section {school_id:$school,tenant_id:$tenant,status:'active',year_group:o.target_year_group,academic_year:o.academic_year})").replace("(other:Section {section_id:$section})","(other:Section {section_id:sec.section_id})");
    current=current.replace("s.applicantStatus IN ['enrolment_paid','handed_to_sis']","s.applicantStatus='enrolment_paid' AND NOT EXISTS {MATCH(s)-[:ENROLLED_AS]->()} AND NOT EXISTS {MATCH(:EnrolledStudent {applicant_student_id:s.studentId})} AND coalesce(app.application_id,'')<>'' AND coalesce(o.offer_id,'')<>'' AND coalesce(p.payment_id,'')<>'' AND coalesce(o.pricing_snapshot_hash,'')<>'' AND o.revision>0");
    current = current.replace(
        "WITH DISTINCT u,l,s,app,o,p,sec,school,actor",
        "WITH DISTINCT u,l,s,app,o,p,sec,school",
    );
    for (school, tenant, name) in scopes {
        schools.push(json!({"schoolId":school,"tenantId":tenant,"name":name}));
        let mut found=graph.execute(directory(actor,vec!["owner".into(),"school_admin".into(),"admissions_admin".into()],"AND school.school_id=$school AND school.tenant_id=$tenant CALL { MATCH(sec:Section {school_id:$school,tenant_id:$tenant,status:'active'}) WHERE NOT EXISTS {MATCH(other:Section {section_id:sec.section_id}) WHERE other<>sec} RETURN sec.section_id AS id,sec.name AS name,sec.year_group AS year,sec.academic_year AS academic ORDER BY name,id LIMIT 201 } RETURN id,name,year,academic").param("school",school.clone()).param("tenant",tenant.clone())).await.map_err(|_|Error::Unavailable)?;
        let mut count = 0;
        while let Some(row) = found.next().await.map_err(|_| Error::Unavailable)? {
            count += 1;
            if count > 200 || sections.len() >= 200 {
                return Err(Error::Unavailable);
            }
            sections.push(json!({"sectionId":row.get::<String>("id").map_err(|_|Error::Unavailable)?,"name":row.get::<String>("name").map_err(|_|Error::Unavailable)?,"yearGroup":row.get::<String>("year").map_err(|_|Error::Unavailable)?,"academicYear":row.get::<String>("academic").map_err(|_|Error::Unavailable)?,"schoolId":school,"tenantId":tenant}));
        }
        let body=format!("{family} WITH DISTINCT u,l,s {current} RETURN DISTINCT s.studentId AS student,u.id AS payer,l.lead_id AS admission,app.application_id AS application,o.offer_id AS offer,p.payment_id AS payment,o.revision AS revision,o.pricing_snapshot_hash AS hash,s.fullName AS name,coalesce(u.fullName,l.parent_name,'') AS parent,o.target_year_group AS year,o.academic_year AS academic ORDER BY student,offer LIMIT 201");
        let mut found = graph
            .execute(
                directory(actor,vec!["owner".into(),"school_admin".into(),"admissions_admin".into()],&format!("AND school.school_id=$school AND school.tenant_id=$tenant CALL {{ {body} }} RETURN student,payer,admission,application,offer,payment,revision,hash,name,parent,year,academic"))
                    .param("school", school.clone())
                    .param("tenant", tenant.clone()),
            )
            .await
            .map_err(|_| Error::Unavailable)?;
        let mut count = 0;
        while let Some(row) = found.next().await.map_err(|_| Error::Unavailable)? {
            count += 1;
            if count > 200 || candidates.len() >= 200 {
                return Err(Error::Unavailable);
            }
            candidates.push(json!({"studentId":row.get::<String>("student").map_err(|_|Error::Unavailable)?,"payerUserId":row.get::<String>("payer").map_err(|_|Error::Unavailable)?,"admissionId":row.get::<String>("admission").map_err(|_|Error::Unavailable)?,"applicationId":row.get::<String>("application").map_err(|_|Error::Unavailable)?,"offerId":row.get::<String>("offer").map_err(|_|Error::Unavailable)?,"paymentId":row.get::<String>("payment").map_err(|_|Error::Unavailable)?,"offerRevision":row.get::<i64>("revision").map_err(|_|Error::Unavailable)?,"pricingSnapshotHash":row.get::<String>("hash").map_err(|_|Error::Unavailable)?,"studentName":row.get::<String>("name").map_err(|_|Error::Unavailable)?,"payerName":row.get::<String>("parent").map_err(|_|Error::Unavailable)?,"yearGroup":row.get::<String>("year").map_err(|_|Error::Unavailable)?,"academicYear":row.get::<String>("academic").map_err(|_|Error::Unavailable)?,"schoolId":school,"tenantId":tenant}));
        }
    }
    Ok(json!({"contractVersion":1,"schools":schools,"sections":sections,"candidates":candidates}))
}
