use super::{
    super::{
        auth, failure,
        repository::{teacher_query, ENROLLED},
        Failure,
    },
    contract::{Export, Import},
    learning,
};
use axum::http::{HeaderMap, StatusCode};
use neo4rs::{query, Graph};
use serde_json::{json, Value};
fn unavailable(_: impl std::fmt::Debug) -> Failure {
    failure(StatusCode::SERVICE_UNAVAILABLE)
}
pub async fn init(graph: &Graph) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for (name, label) in [
        (
            "school_assessment_import_key",
            "SchoolAssessmentGradeImport",
        ),
        (
            "school_assessment_receipt_key",
            "SchoolAssessmentGradeReceipt",
        ),
    ] {
        graph
            .run(query(&format!(
                "CREATE CONSTRAINT {name} IF NOT EXISTS FOR(n:{label}) REQUIRE n.key IS UNIQUE"
            )))
            .await?;
    }
    Ok(())
}
fn scope(member: &auth::Teacher, section: &str, export: &Export, lock: bool) -> neo4rs::Query {
    let student_lock = if lock {
        "SET s.assessment_grade_lock=coalesce(s.assessment_grade_lock,0)+1 WITH t,sec,s"
    } else {
        "WITH t,sec,s"
    };
    teacher_query(member,&format!("{ENROLLED} WHERE s.studentId=$student AND sec.school_id=$school AND sec.tenant_id=$tenant {student_lock} RETURN DISTINCT sec.section_id AS section"),Some(section),lock,false)
        .param("student",export.student_id.clone()).param("school",export.school_id.clone()).param("tenant",export.tenant_id.clone())
}
pub async fn import(
    graph: Graph,
    headers: HeaderMap,
    section: String,
    input: Import,
    command: String,
) -> Result<Value, Failure> {
    let _ = auth::teacher(&headers).await?;
    let observed = learning::read(&headers, &input).await?;
    let member = auth::teacher_in(&headers, &observed.school_id, &observed.tenant_id).await?;
    if !observed.matches_for(&input, &section, &member.staff_member_id, member.is_owner()) {
        return Err(failure(StatusCode::CONFLICT));
    }
    let mut tx = graph.start_txn().await.map_err(unavailable)?;
    let outcome = locked(
        &mut tx, &headers, &section, &input, &command, &member, &observed,
    )
    .await;
    match outcome {
        Ok(value) => {
            tx.commit().await.map_err(unavailable)?;
            Ok(value)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}
async fn locked(
    tx: &mut neo4rs::Txn,
    headers: &HeaderMap,
    section: &str,
    input: &Import,
    command: &str,
    member: &auth::Teacher,
    observed: &Export,
) -> Result<Value, Failure> {
    let mut rows = tx
        .execute(scope(member, section, observed, true))
        .await
        .map_err(unavailable)?;
    if rows.next(&mut *tx).await.map_err(unavailable)?.is_none() {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    // Live Auth and Learning are rechecked AFTER node locks, even for exact replay.
    let current = auth::teacher_in(headers, &observed.school_id, &observed.tenant_id).await?;
    if current.staff_member_id != member.staff_member_id || current.role() != member.role() {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let export = learning::read(headers, input).await?;
    if export != *observed
        || !export.matches_for(input, section, &current.staff_member_id, current.is_owner())
    {
        return Err(failure(StatusCode::CONFLICT));
    }
    let mut rows = tx
        .execute(scope(&current, section, &export, false))
        .await
        .map_err(unavailable)?;
    if rows.next(&mut *tx).await.map_err(unavailable)?.is_none() {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let import_key = [
        export.school_id.as_str(),
        export.tenant_id.as_str(),
        section,
        &input.course_id,
        &input.assessment_id,
        &input.attempt_id,
    ]
    .join("|");
    let receipt_key = [
        current.staff_member_id.as_str(),
        export.school_id.as_str(),
        export.tenant_id.as_str(),
        section,
        command,
    ]
    .join("|");
    let fingerprint = serde_json::to_string(input).map_err(unavailable)?;
    let source = serde_json::to_string(&export).map_err(unavailable)?;
    let mut receipts=tx.execute(query("MATCH(r:SchoolAssessmentGradeReceipt {key:$key}) RETURN r.fingerprint AS fingerprint,r.import_key AS import_key").param("key",receipt_key.clone())).await.map_err(unavailable)?;
    if let Some(row) = receipts.next(&mut *tx).await.map_err(unavailable)? {
        if row.get::<String>("fingerprint").map_err(unavailable)? != fingerprint
            || row.get::<String>("import_key").map_err(unavailable)? != import_key
        {
            return Err(failure(StatusCode::CONFLICT));
        }
    }
    let mut imports=tx.execute(query("MATCH(g:SchoolAssessmentGradeImport {key:$key}) RETURN g.term AS term,g.source AS source,g.payload AS payload").param("key",import_key.clone())).await.map_err(unavailable)?;
    let grade = if let Some(row) = imports.next(&mut *tx).await.map_err(unavailable)? {
        if row.get::<String>("term").map_err(unavailable)? != input.term
            || row.get::<String>("source").map_err(unavailable)? != source
        {
            return Err(failure(StatusCode::CONFLICT));
        }
        serde_json::from_str::<Value>(&row.get::<String>("payload").map_err(unavailable)?)
            .map_err(unavailable)?
    } else {
        let id = format!("LMS-{}", uuid::Uuid::new_v4());
        let grade = json!({"id":id,"studentId":export.student_id,"subject":export.subject,"term":input.term,"score":f64::from(export.score_minor)/100.,"maximum":f64::from(export.maximum_minor)/100.,"source":export});
        let publication = json!({"kind":"grade","id":id,"student_id":export.student_id,"subject":export.subject,"term":input.term,"score":f64::from(export.score_minor)/100.,"maximum":f64::from(export.maximum_minor)/100.,"source":export});
        // Manual GradeEntry is deliberately untouched. Published score/provenance
        // and the attempt receipt are immutable and use their own natural keys.
        tx.run(query("MATCH(sec:Section {section_id:$section,school_id:$school,tenant_id:$tenant}) CREATE(g:SchoolAssessmentGradeImport {key:$key,term:$term,source:$source,payload:$payload,actor_id:$actor,actor_role:$actor_role,created_at:datetime()}) CREATE(r:SchoolPublishedRecord {key:$record,kind:'grade',student_id:$student,payload:$published,actor_id:$actor,actor_role:$actor_role,created_at:datetime()}) CREATE(r)-[:FOR_SECTION]->(sec) CREATE(g)-[:PUBLISHED_AS]->(r) CREATE(:SchoolPortalAudit {id:$audit,actor_id:$actor,actor_role:$actor_role,school_id:$school,tenant_id:$tenant,section_id:$section,kind:'assessment_grade_import',created_at:datetime()})")
            .param("section",section).param("school",export.school_id.clone()).param("tenant",export.tenant_id.clone()).param("key",import_key.clone()).param("term",input.term.clone()).param("source",source).param("payload",serde_json::to_string(&grade).map_err(unavailable)?).param("actor",current.staff_member_id.clone()).param("actor_role",current.role()).param("record",format!("{section}|grade|{id}")).param("student",export.student_id.clone()).param("published",serde_json::to_string(&publication).map_err(unavailable)?).param("audit",format!("assessment-grade|{id}"))).await.map_err(unavailable)?;
        grade
    };
    tx.run(query("MERGE(r:SchoolAssessmentGradeReceipt {key:$key}) ON CREATE SET r.fingerprint=$fingerprint,r.import_key=$import_key,r.created_at=datetime()")
        .param("key",receipt_key).param("fingerprint",fingerprint).param("import_key",import_key)).await.map_err(unavailable)?;
    Ok(json!({"saved":true,"grade":grade}))
}
