//! Graph reads and writes of grade weights. A save runs in the same guarded transaction as the
//! subject plans: caller lock, school lock, authority, session, receipt, then record and audit.
use super::super::weights::{
    key, resolve, Change, Target, WeightSet, ACTIVE, BUILTIN_SET, CLEARED,
};
use super::{audit, conflict, get, guard, one, rows, unavailable, Failure, Writer};
use crate::school_portal::staff_capability;
use axum::http::StatusCode;
use neo4rs::{query, Graph, Row};
use serde_json::{json, Value};

pub(crate) const LIST: &str = "MATCH(w:AcademicGradeWeights {school_id:$school,tenant_id:$tenant,academic_year:$year}) RETURN w.scope AS scope,coalesce(w.grade,'') AS grade,coalesce(w.subject,'') AS subject,coalesce(w.term,'') AS term,w.version AS version,w.status AS status,w.payload AS payload,toString(w.updated_at) AS updated ORDER BY scope,grade,subject,term LIMIT 4001";
pub(crate) const CURRENT: &str = "MATCH(w:AcademicGradeWeights {key:$key}) RETURN w.version AS version,w.status AS status,w.payload AS payload";
pub(crate) const IN_PLAN: &str = "OPTIONAL MATCH(p:AcademicSubjectPlan {key:$plan}) RETURN coalesce($subject IN p.active_subjects,false) AS ok";
pub(crate) const SAVE: &str = "MERGE(w:AcademicGradeWeights {key:$key}) ON CREATE SET w.tenant_id=$tenant,w.school_id=$school,w.academic_year=$year,w.scope=$scope,w.grade=$grade,w.subject=$subject,w.term=$term,w.version=0,w.created_at=datetime() SET w.version=w.version+1,w.status=$status,w.payload=$payload,w.updated_by=$actor,w.updated_at=datetime() RETURN w.version AS version,toString(w.updated_at) AS updated";
pub(crate) const SECTION: &str = "MATCH(sec:Section {section_id:$section,school_id:$school,tenant_id:$tenant}) WITH collect(sec) AS found WHERE size(found)=1 RETURN coalesce(found[0].academic_year,'') AS year,coalesce(found[0].year_group,'') AS grade";
pub(crate) const ACTIVE_SETS: &str = "MATCH(w:AcademicGradeWeights {status:'active'}) WHERE w.key IN [$subject_key,$school_key] RETURN w.key AS key,w.version AS version,w.payload AS payload";

fn set_of(payload: &str) -> Result<WeightSet, Failure> {
    let set: WeightSet = serde_json::from_str(payload).map_err(unavailable)?;
    set.valid()
        .then_some(set)
        .ok_or_else(|| unavailable("Corrupt weights"))
}
/// A stored record for the Admin view; a cleared record keeps its version and no weights.
fn view(r: &Row) -> Result<Value, Failure> {
    let status: String = get(r, "status")?;
    let set = match status.as_str() {
        ACTIVE => Some(set_of(&get::<String>(r, "payload")?)?),
        CLEARED => None,
        _ => return Err(unavailable("Corrupt status")),
    };
    Ok(json!({
        "version": get::<i64>(r, "version")?,
        "status": status,
        "weights": set.map(|s| s.weights),
        "passMark": set.map(|s| s.pass_mark),
        "updatedAt": get::<String>(r, "updated")?,
    }))
}

/// The year's school default and every subject override, with the built-in default beside them.
pub async fn list(graph: &Graph, school: &str, tenant: &str, year: &str) -> Result<Value, Failure> {
    let mut default = Value::Null;
    let mut subjects = vec![];
    for r in rows(
        graph,
        query(LIST)
            .param("school", school)
            .param("tenant", tenant)
            .param("year", year),
        4000,
    )
    .await?
    {
        let mut record = view(&r)?;
        match get::<String>(&r, "scope")?.as_str() {
            "school" => default = record,
            "subject" => {
                record["grade"] = json!(get::<String>(&r, "grade")?);
                record["subject"] = json!(get::<String>(&r, "subject")?);
                record["term"] = json!(get::<String>(&r, "term")?);
                subjects.push(record);
            }
            _ => return Err(unavailable("Corrupt scope")),
        }
    }
    Ok(json!({
        "builtin": {"weights": BUILTIN_SET.weights, "passMark": BUILTIN_SET.pass_mark},
        "school": default,
        "subjects": subjects,
    }))
}

/// Stores or clears one weight set under its revision. An override names a subject that is an
/// active row of the grade's plan for that year.
pub async fn save(graph: &Graph, w: &Writer<'_>, change: &Change) -> Result<Value, Failure> {
    let fingerprint = serde_json::to_string(change).map_err(unavailable)?;
    let target = change.target().ok_or_else(|| unavailable("Unchecked"))?;
    guarded!(graph, w, fingerprint.as_str(), |tx| async {
        let year = change.academic_year.as_str();
        let record = key(w.tenant, w.school, year, &target);
        let (scope, grade, subject, term) = match &target {
            Target::School => ("school", None, None, None),
            Target::Subject {
                grade,
                subject,
                term,
            } => ("subject", Some(*grade), Some(*subject), Some(*term)),
        };
        if let (Some(grade), Some(subject), false) = (grade, subject, change.clear) {
            let ok = one(
                tx,
                query(IN_PLAN)
                    .param(
                        "plan",
                        super::super::model::plan_key(w.tenant, w.school, year, grade),
                    )
                    .param("subject", subject),
            )
            .await?
            .map(|r| get::<bool>(&r, "ok"))
            .transpose()?;
            if ok != Some(true) {
                return Err(super::refuse(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "SUBJECT_NOT_IN_PLAN",
                    json!({}),
                ));
            }
        }
        let current = one(tx, query(CURRENT).param("key", record.clone())).await?;
        let before = match &current {
            None if change.expected_version == 0 && !change.clear => Value::Null,
            Some(r) if get::<i64>(r, "version")? == i64::from(change.expected_version) => {
                let status: String = get(r, "status")?;
                if change.clear && status != ACTIVE {
                    return Err(conflict("REVISION_CONFLICT", json!({})));
                }
                match status.as_str() {
                    ACTIVE => serde_json::to_value(set_of(&get::<String>(r, "payload")?)?)
                        .map_err(unavailable)?,
                    _ => Value::Null,
                }
            }
            _ => return Err(conflict("REVISION_CONFLICT", json!({}))),
        };
        let after = match &change.set {
            Some(set) if !change.clear => serde_json::to_value(set).map_err(unavailable)?,
            _ => Value::Null,
        };
        let saved = one(
            tx,
            query(SAVE)
                .param("key", record.clone())
                .param("tenant", w.tenant)
                .param("school", w.school)
                .param("year", year)
                .param("scope", scope)
                .param("grade", grade)
                .param("subject", subject)
                .param("term", term)
                .param("status", if change.clear { CLEARED } else { ACTIVE })
                .param(
                    "payload",
                    if change.clear {
                        None
                    } else {
                        Some(after.to_string())
                    },
                )
                .param("actor", w.actor),
        )
        .await?
        .ok_or_else(|| unavailable("No weights"))?;
        tx.run(audit(
            w,
            year,
            "grade_weights",
            if change.clear { "clear" } else { "save" },
            &record,
            before,
            after.clone(),
        ))
        .await
        .map_err(unavailable)?;
        Ok::<Value, Failure>(json!({
            "grade": grade,
            "subject": subject,
            "term": term,
            "version": get::<i64>(&saved, "version")?,
            "status": if change.clear { CLEARED } else { ACTIVE },
            "weights": change.set.filter(|_| !change.clear).map(|s| s.weights),
            "passMark": change.set.filter(|_| !change.clear).map(|s| s.pass_mark),
            "updatedAt": get::<String>(&saved, "updated")?,
        }))
    })
}

/// Staff who may read a school's weights: its active teachers and administrators, and Owners
/// whose scope covers it.
pub(crate) fn reader() -> String {
    format!(
        "MATCH(a:StaffMember {{id:$actor}}) WHERE ({} OR {}) AND NOT EXISTS {{MATCH(other:StaffMember {{id:a.id}}) WHERE other<>a}} AND EXISTS {{MATCH(:School {{school_id:$school,tenant_id:$tenant}})}} RETURN count(a) AS count",
        staff_capability::authority("a", "'teacher'", "$school", "$tenant"),
        staff_capability::authority("a", "'school_admin'", "$school", "$tenant"),
    )
}

/// The weights that apply to one class, subject and term: the class's academic year and grade
/// pick the subject override, else the school default of that year, else the built-in default.
pub async fn for_section(
    graph: &Graph,
    actor: &str,
    school: &str,
    tenant: &str,
    section: &str,
    subject: &str,
    term: &str,
) -> Result<Value, Failure> {
    let allowed = rows(
        graph,
        query(&reader())
            .param("actor", actor)
            .param("school", school)
            .param("tenant", tenant),
        1,
    )
    .await?
    .first()
    .map(|r| get::<i64>(r, "count"))
    .transpose()?;
    if allowed != Some(1) {
        return Err(super::refuse(StatusCode::FORBIDDEN, "FORBIDDEN", json!({})));
    }
    let found = rows(
        graph,
        query(SECTION)
            .param("section", section)
            .param("school", school)
            .param("tenant", tenant),
        1,
    )
    .await?;
    let not_found = || super::refuse(StatusCode::NOT_FOUND, "NOT_FOUND", json!({}));
    let r = found.first().ok_or_else(not_found)?;
    let (year, grade): (String, String) = (get(r, "year")?, get(r, "grade")?);
    if year.is_empty() {
        return Err(not_found());
    }
    let subject_key = key(
        tenant,
        school,
        &year,
        &Target::Subject {
            grade: &grade,
            subject,
            term,
        },
    );
    let school_key = key(tenant, school, &year, &Target::School);
    let (mut by_subject, mut by_school) = (None, None);
    for r in rows(
        graph,
        query(ACTIVE_SETS)
            .param("subject_key", subject_key.clone())
            .param("school_key", school_key.clone()),
        2,
    )
    .await?
    {
        let found = (
            set_of(&get::<String>(&r, "payload")?)?,
            u32::try_from(get::<i64>(&r, "version")?).map_err(unavailable)?,
        );
        if get::<String>(&r, "key")? == subject_key {
            by_subject = Some(found);
        } else {
            by_school = Some(found);
        }
    }
    let (source, set, version) =
        resolve(if grade.is_empty() { None } else { by_subject }, by_school);
    Ok(json!({
        "schoolId": school,
        "tenantId": tenant,
        "sectionId": section,
        "academicYear": year,
        "grade": grade,
        "subject": subject,
        "term": term,
        "source": source.as_str(),
        "version": version,
        "weights": set.weights,
        "passMark": set.pass_mark,
    }))
}
