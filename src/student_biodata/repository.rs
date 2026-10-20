//! Graph access for Admin Student biodata. The administrator check (`ADMIN`) and the Student's
//! active enrolment at the named school are part of every statement, so a school can only read
//! or change its own enrolled Students.
use super::model::{diff, Biodata, Clean, Diff, Guardian, Scope};
use super::{failure, Failure};
use crate::learning_access::timetable::repository::one;
use axum::http::StatusCode;
use neo4rs::{query, Graph, Query, Row, Txn};
use serde_json::{json, Value};

const STUDENT: &str = "WITH actor MATCH (s:Student {studentId:$student}) WHERE EXISTS { MATCH (s)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',tenant_id:$tenant,school_id:$school}) } AND NOT EXISTS { MATCH (other:Student {studentId:$student}) WHERE other<>s }";
const LEAD: &str = "OPTIONAL MATCH (l:Lead)-[:HAS_STUDENT]->(s) WITH actor,s,collect(DISTINCT l) AS leads";
const GUARDIANS: &str = "MATCH (:Student {studentId:$student})-[:HAS_GUARDIAN]->(g:Guardian) RETURN g.guardian_id AS id,g.full_name AS name,g.relationship AS relationship,g.phone_primary AS phone1,g.phone_secondary AS phone2,g.email AS email ORDER BY coalesce(g.position,0),g.guardian_id LIMIT 21";

pub enum Saved {
    Version(i64),
    Conflict,
    NisnInUse,
    NotFound,
    Invalid,
}

fn unavailable<E>(_: E) -> Failure {
    failure(StatusCode::SERVICE_UNAVAILABLE)
}
fn scoped(body: String, actor: &str, scope: &Scope) -> Query {
    query(&body)
        .param("actor", actor)
        .param("tenant", scope.tenant_id.clone())
        .param("school", scope.school_id.clone())
        .param("student", scope.student_id.clone())
}
fn text(row: &Row, name: &str) -> Option<String> {
    row.get::<Option<String>>(name).ok().flatten()
}
/// Stored values are shown as they are (bounded) so Admin can see and correct older entries.
fn shown(row: &Row, name: &str) -> Option<String> {
    text(row, name)
        .map(|v| v.trim().chars().take(256).collect::<String>())
        .filter(|v| !v.is_empty())
}
/// Older admissions rows used Indonesian words or initials; Admin edits the canonical value.
pub fn canonical(value: Option<String>, pairs: &[(&str, &'static str)]) -> Option<String> {
    let value = value?.trim().to_ascii_lowercase();
    pairs.iter().find(|(from, _)| *from == value).map(|(_, to)| to.to_string())
}
pub const GENDER_WORDS: [(&str, &str); 8] = [
    ("male", "male"), ("m", "male"), ("laki-laki", "male"), ("l", "male"),
    ("female", "female"), ("f", "female"), ("perempuan", "female"), ("p", "female"),
];
pub const RELATIONSHIP_WORDS: [(&str, &str); 6] = [
    ("father", "father"), ("ayah", "father"), ("mother", "mother"), ("ibu", "mother"),
    ("guardian", "guardian"), ("wali", "guardian"),
];

fn guardian_row(row: &Row) -> Result<(String, Guardian), Failure> {
    let id = text(row, "id").ok_or_else(|| unavailable(()))?;
    Ok((
        id,
        Guardian {
            full_name: text(row, "name").unwrap_or_default(),
            relationship: text(row, "relationship").unwrap_or_default(),
            phone_primary: text(row, "phone1").unwrap_or_default(),
            phone_secondary: text(row, "phone2"),
            email: text(row, "email"),
        },
    ))
}

/// The one school and tenant where the Student is actively enrolled; `None` when there is not
/// exactly one.
pub async fn enrolment(graph: &Graph, student: &str) -> Result<Option<Scope>, Failure> {
    let mut rows = graph.execute(query("MATCH (s:Student {studentId:$student})-[:ENROLLED_AS]->(e:EnrolledStudent {status:'active'}) RETURN DISTINCT e.school_id AS school,e.tenant_id AS tenant LIMIT 2").param("student", student)).await.map_err(unavailable)?;
    let mut found = vec![];
    while let Some(row) = rows.next().await.map_err(unavailable)? {
        found.push((text(&row, "school"), text(&row, "tenant")));
    }
    Ok(match found.as_slice() {
        [(Some(school), Some(tenant))] => Some(Scope { tenant_id: tenant.clone(), school_id: school.clone(), student_id: student.into() }).filter(Scope::valid),
        _ => None,
    })
}

/// The record Admin edits: biodata, the admissions account holder (relationship only) and the
/// guardians the school recorded. `None` when the Student is not actively enrolled there.
pub async fn read(graph: &Graph, actor: &str, scope: &Scope) -> Result<Option<Value>, Failure> {
    let admin = crate::education_calendar::repository::ADMIN;
    let mut rows = graph.execute(scoped(format!("{admin} {STUDENT} {LEAD} RETURN s.fullName AS name,s.dateOfBirth AS birth_date,s.nisn AS nisn,s.birthPlace AS birth_place,s.gender AS gender,s.emergencyContact AS emergency,coalesce(s.biodata_version,0) AS version,size(leads) AS lead_count,CASE WHEN size(leads)=1 THEN leads[0].parent_name END AS lead_name,CASE WHEN size(leads)=1 THEN leads[0].relationship END AS lead_relationship,CASE WHEN size(leads)=1 THEN leads[0].mobile END AS lead_phone1,CASE WHEN size(leads)=1 THEN leads[0].whatsapp END AS lead_phone2,CASE WHEN size(leads)=1 THEN leads[0].email END AS lead_email"), actor, scope)).await.map_err(unavailable)?;
    let Some(row) = rows.next().await.map_err(unavailable)? else {
        return Ok(None);
    };
    if rows.next().await.map_err(unavailable)?.is_some() {
        return Err(unavailable(()));
    }
    let lead = (row.get::<i64>("lead_count").map_err(unavailable)? == 1).then(|| {
        let phones: Vec<String> = [shown(&row, "lead_phone1"), shown(&row, "lead_phone2")]
            .into_iter()
            .flatten()
            .fold(vec![], |mut all, p| {
                if !all.contains(&p) {
                    all.push(p);
                }
                all
            });
        json!({"name":shown(&row,"lead_name"),"relationship":canonical(text(&row,"lead_relationship"),&RELATIONSHIP_WORDS),"phones":phones,"email":shown(&row,"lead_email")})
    });
    let mut guardians = vec![];
    let mut grows = graph
        .execute(scoped(GUARDIANS.into(), actor, scope))
        .await
        .map_err(unavailable)?;
    while let Some(g) = grows.next().await.map_err(unavailable)? {
        let (id, g) = guardian_row(&g)?;
        guardians.push(json!({"id":id,"fullName":g.full_name,"relationship":canonical(Some(g.relationship),&RELATIONSHIP_WORDS),"phonePrimary":g.phone_primary,"phoneSecondary":g.phone_secondary,"email":g.email}));
    }
    Ok(Some(json!({
        "contractVersion": 1,
        "tenantId": scope.tenant_id,
        "schoolId": scope.school_id,
        "studentId": scope.student_id,
        "fullName": shown(&row, "name"),
        "dateOfBirth": shown(&row, "birth_date"),
        "version": row.get::<i64>("version").map_err(unavailable)?,
        "biodata": {
            "nisn": shown(&row, "nisn"),
            "birthPlace": shown(&row, "birth_place"),
            "gender": canonical(text(&row, "gender"), &GENDER_WORDS),
            "emergencyContact": shown(&row, "emergency"),
        },
        "admissionsGuardian": lead,
        "guardians": guardians,
        "limits": {"guardians": super::model::MAX_GUARDIANS},
    })))
}

async fn rows(tx: &mut Txn, q: Query) -> Result<Vec<Row>, Failure> {
    let mut stream = tx.execute(q).await.map_err(unavailable)?;
    let mut all = vec![];
    while let Some(row) = stream.next(tx.handle()).await.map_err(unavailable)? {
        all.push(row);
    }
    Ok(all)
}

/// Replaces the biodata, the admissions relationship and the guardian list in one transaction,
/// CAS on `Student.biodata_version`. The audit row names the changed fields, never their values.
pub async fn save(graph: &Graph, actor: &str, change: &Clean, scope: &Scope, expected: u32) -> Result<Saved, Failure> {
    let mut tx = graph.start_txn().await.map_err(unavailable)?;
    let result = write(&mut tx, actor, change, scope, i64::from(expected)).await;
    match result {
        Ok(Saved::Version(v)) => {
            tx.commit().await.map_err(unavailable)?;
            Ok(Saved::Version(v))
        }
        other => {
            let _ = tx.rollback().await;
            other
        }
    }
}

async fn write(tx: &mut Txn, actor: &str, change: &Clean, scope: &Scope, expected: i64) -> Result<Saved, Failure> {
    let admin = crate::education_calendar::repository::ADMIN;
    // Taking the write lock on the Student first serialises concurrent saves of the same record.
    let Some(row) = one(tx, scoped(format!("{admin} {STUDENT} SET s.biodata_lock=coalesce(s.biodata_lock,0)+1 WITH actor,s {LEAD} RETURN coalesce(s.biodata_version,0) AS version,s.nisn AS nisn,s.birthPlace AS birth_place,s.gender AS gender,s.emergencyContact AS emergency,size(leads) AS lead_count,CASE WHEN size(leads)=1 THEN leads[0].relationship END AS relationship,CASE WHEN 'owner' IN coalesce(actor.roles,[]) THEN 'owner' ELSE 'school_admin' END AS role"), actor, scope)).await? else {
        return Ok(Saved::NotFound);
    };
    let version = row.get::<i64>("version").map_err(unavailable)?;
    if version != expected {
        return Ok(Saved::Conflict);
    }
    let lead_count = row.get::<i64>("lead_count").map_err(unavailable)?;
    if lead_count != 1 && change.relationship.is_some() {
        return Ok(Saved::Invalid);
    }
    let role = text(&row, "role").ok_or_else(|| unavailable(()))?;
    let old = Biodata {
        nisn: text(&row, "nisn"),
        birth_place: text(&row, "birth_place"),
        gender: text(&row, "gender"),
        emergency_contact: text(&row, "emergency"),
    };
    let old_relationship = if lead_count == 1 { text(&row, "relationship") } else { None };
    if let Some(nisn) = &change.biodata.nisn {
        let taken = one(tx, query("MATCH (o:Student {nisn:$nisn}) WHERE coalesce(o.studentId,'')<>$student RETURN count(o) AS n").param("nisn", nisn.clone()).param("student", scope.student_id.clone())).await?
            .map(|r| r.get::<i64>("n")).transpose().map_err(unavailable)?.unwrap_or(0);
        if taken > 0 {
            return Ok(Saved::NisnInUse);
        }
    }
    let old_guardians = rows(tx, scoped(GUARDIANS.into(), actor, scope))
        .await?
        .iter()
        .map(guardian_row)
        .collect::<Result<Vec<_>, _>>()?;
    let Ok(d) = diff(&old, &old_relationship, &old_guardians, change) else {
        return Ok(Saved::Conflict);
    };
    if d.is_empty() {
        return Ok(Saved::Version(version));
    }
    apply(tx, scope, change, &d, lead_count == 1).await?;
    audit(tx, actor, &role, scope, &d).await
}

async fn apply(tx: &mut Txn, scope: &Scope, change: &Clean, d: &Diff, has_lead: bool) -> Result<(), Failure> {
    let student = || scope.student_id.clone();
    if !d.removed.is_empty() {
        rows(tx, query("MATCH (:Student {studentId:$student})-[:HAS_GUARDIAN]->(g:Guardian) WHERE g.guardian_id IN $removed DETACH DELETE g").param("student", student()).param("removed", d.removed.clone())).await?;
    }
    for (position, (id, g)) in change.guardians.iter().enumerate() {
        let (body, id) = match id {
            Some(id) => ("MATCH (:Student {studentId:$student})-[:HAS_GUARDIAN]->(g:Guardian {guardian_id:$id}) SET g.updated_at=CASE WHEN $changed THEN datetime() ELSE g.updated_at END", id.clone()),
            None => ("MATCH (s:Student {studentId:$student}) CREATE (s)-[:HAS_GUARDIAN]->(g:Guardian {guardian_id:$id,school_id:$school,tenant_id:$tenant,created_at:datetime(),updated_at:datetime()})", uuid::Uuid::new_v4().to_string()),
        };
        let changed = d.updated.contains(&id);
        one(tx, query(&format!("{body} SET g.full_name=$name,g.relationship=$relationship,g.phone_primary=$phone1,g.phone_secondary=$phone2,g.email=$email,g.position=$position RETURN g.guardian_id AS id"))
            .param("student", student()).param("id", id).param("changed", changed)
            .param("school", scope.school_id.clone()).param("tenant", scope.tenant_id.clone())
            .param("name", g.full_name.clone()).param("relationship", g.relationship.clone())
            .param("phone1", g.phone_primary.clone()).param("phone2", g.phone_secondary.clone())
            .param("email", g.email.clone()).param("position", position as i64))
            .await?.ok_or_else(|| unavailable(()))?;
    }
    if has_lead && d.fields.contains(&"admissionsGuardianRelationship") {
        rows(tx, query("MATCH (l:Lead)-[:HAS_STUDENT]->(:Student {studentId:$student}) SET l.relationship=$relationship,l.relationship_updated_at=datetime()").param("student", student()).param("relationship", change.relationship.clone())).await?;
    }
    let b = &change.biodata;
    one(tx, query("MATCH (s:Student {studentId:$student}) SET s.nisn=$nisn,s.birthPlace=$birth_place,s.gender=$gender,s.emergencyContact=$emergency RETURN s.studentId AS id")
        .param("student", student()).param("nisn", b.nisn.clone()).param("birth_place", b.birth_place.clone())
        .param("gender", b.gender.clone()).param("emergency", b.emergency_contact.clone()))
        .await?.ok_or_else(|| unavailable(()))?;
    Ok(())
}

async fn audit(tx: &mut Txn, actor: &str, role: &str, scope: &Scope, d: &Diff) -> Result<Saved, Failure> {
    let row = one(tx, query("MATCH (s:Student {studentId:$student}) SET s.biodata_version=coalesce(s.biodata_version,0)+1,s.biodata_updated_at=datetime() CREATE (:StudentBiodataAudit {key:$audit,actor_id:$actor,actor_role:$role,student_id:$student,school_id:$school,tenant_id:$tenant,version:s.biodata_version,changed_fields:$fields,guardians_added:$added,guardians_updated:$updated,guardians_removed:$removed,guardians_reordered:$reordered,created_at:datetime()}) RETURN s.biodata_version AS version")
        .param("student", scope.student_id.clone()).param("school", scope.school_id.clone()).param("tenant", scope.tenant_id.clone())
        .param("audit", uuid::Uuid::new_v4().to_string()).param("actor", actor).param("role", role)
        .param("fields", d.fields.iter().map(|f| f.to_string()).collect::<Vec<_>>())
        .param("added", d.added as i64).param("updated", d.updated.len() as i64)
        .param("removed", d.removed.len() as i64).param("reordered", d.reordered))
        .await?.ok_or_else(|| unavailable(()))?;
    Ok(Saved::Version(row.get::<i64>("version").map_err(unavailable)?))
}

pub async fn init(graph: &Graph) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for statement in [
        "CREATE CONSTRAINT guardian_id IF NOT EXISTS FOR (g:Guardian) REQUIRE g.guardian_id IS UNIQUE",
        "CREATE CONSTRAINT student_biodata_audit_key IF NOT EXISTS FOR (a:StudentBiodataAudit) REQUIRE a.key IS UNIQUE",
    ] {
        graph.run(query(statement)).await?;
    }
    Ok(())
}
