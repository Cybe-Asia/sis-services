//! Graph reads and writes for subject plans and teaching assignments. Every write runs in one
//! transaction: caller lock, school lock, the capability decision from the graph (ADR 0018), the
//! session again, the idempotency receipt, then state, audit, grant sync and the receipt commit
//! together.
use super::model::{
    grade_order, next, plan_key, Command, Commands, Current, PlanChange, PlanCopy, PlanRow, ASSIGN,
    END,
};
use crate::authorization::facts::Db;
use crate::learning_access::timetable::repository::{lock_school, one};
use axum::{http::StatusCode, Json};
use neo4rs::{query, Graph, Query, Row, Txn};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub type Failure = crate::school_portal::Failure;
pub fn refuse(status: StatusCode, code: &str, detail: Value) -> Failure {
    let mut error = json!({ "code": code });
    if let (Some(e), Some(d)) = (error.as_object_mut(), detail.as_object()) {
        e.extend(d.clone());
    }
    (status, Json(json!({ "error": error })))
}
pub fn unavailable<E>(_: E) -> Failure {
    refuse(
        StatusCode::SERVICE_UNAVAILABLE,
        "DEPENDENCY_UNAVAILABLE",
        json!({}),
    )
}
fn forbidden() -> Failure {
    refuse(StatusCode::FORBIDDEN, "FORBIDDEN", json!({}))
}
fn conflict(code: &str, detail: Value) -> Failure {
    refuse(StatusCode::CONFLICT, code, detail)
}
fn get<T: serde::de::DeserializeOwned>(row: &Row, name: &str) -> Result<T, Failure> {
    row.get(name).map_err(unavailable)
}

pub async fn init(graph: &Graph) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for statement in include_str!("../../migrations/0005_academic_staffing.cypher")
        .split(';')
        .chain(include_str!("../../migrations/0006_grade_weights.cypher").split(';'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        graph.run(query(statement)).await?;
    }
    Ok(())
}

async fn rows(graph: &Graph, q: Query, max: usize) -> Result<Vec<Row>, Failure> {
    let mut stream = graph.execute(q).await.map_err(unavailable)?;
    let mut out = vec![];
    while let Some(row) = stream.next().await.map_err(unavailable)? {
        if out.len() == max {
            return Err(unavailable("Bound exceeded"));
        }
        out.push(row);
    }
    Ok(out)
}
async fn rows_in(tx: &mut Txn, q: Query, max: usize) -> Result<Vec<Row>, Failure> {
    let mut stream = tx.execute(q).await.map_err(unavailable)?;
    let mut out = vec![];
    while let Some(row) = stream.next(tx.handle()).await.map_err(unavailable)? {
        if out.len() == max {
            return Err(unavailable("Bound exceeded"));
        }
        out.push(row);
    }
    Ok(out)
}

/// How the caller's Auth session is checked again once the locks are held.
pub enum Session<'a> {
    Headers(&'a axum::http::HeaderMap),
    /// Graph tests only: they have no Auth fixture and check the graph rules alone.
    #[cfg(test)]
    Unchecked,
}
/// The capabilities these routes check (ADR 0018 slice 6). Owners and school administrators
/// hold all three on day one; Admin teachers read staffing.
pub const STAFFING_READ: &str = "academic.staffing.read";
pub const STAFFING_MANAGE: &str = "academic.staffing.manage";
pub const SUBJECT_PLAN_MANAGE: &str = "academic.subject_plan.manage";

/// Who writes, for which school, under which `Idempotency-Key`, and which capability the write
/// needs at the school.
pub struct Writer<'a> {
    pub session: Session<'a>,
    pub actor: &'a str,
    pub role: &'a str,
    pub school: &'a str,
    pub tenant: &'a str,
    pub command: &'a str,
    pub capability: &'a str,
}
impl Writer<'_> {
    fn receipt(&self) -> String {
        [self.actor, self.tenant, self.school, self.command].join("|")
    }
}

/// Locks and checks in a fixed order. `Some` is the stored answer of an exact replay.
async fn guard(tx: &mut Txn, w: &Writer<'_>, fingerprint: &str) -> Result<Option<Value>, Failure> {
    // Caller before resource, the order Owner writes use elsewhere.
    one(tx, query("MATCH(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WITH actor LIMIT 1 SET actor.owner_write_lock=coalesce(actor.owner_write_lock,0)+1 RETURN actor.id AS id").param("actor", w.actor))
        .await?
        .ok_or_else(forbidden)?;
    lock_school(tx, w.school, w.tenant).await?;
    // The capability decision reads the caller's holdings under the locks, so an assignment or
    // template revoked meanwhile denies here.
    if !crate::authorization::reads::permits(
        &mut Db::Tx(tx),
        w.tenant,
        w.school,
        w.actor,
        w.capability,
    )
    .await?
    {
        return Err(forbidden());
    }
    // A role revoked while this request waited for the locks does not write.
    match w.session {
        Session::Headers(headers) => {
            let (actor, role) = crate::learning_access::school_staff_actor(
                headers,
                &w.tenant.to_string(),
                &w.school.to_string(),
            )
            .await?;
            if actor != w.actor || role != w.role {
                return Err(forbidden());
            }
        }
        #[cfg(test)]
        Session::Unchecked => {}
    }
    let stored = one(tx, query("MATCH(r:AcademicStaffingReceipt {key:$key}) RETURN r.fingerprint AS fingerprint,r.result AS result").param("key", w.receipt())).await?;
    match stored {
        None => Ok(None),
        Some(r) if get::<String>(&r, "fingerprint")? == fingerprint => Ok(Some(
            serde_json::from_str(&get::<String>(&r, "result")?).map_err(unavailable)?,
        )),
        Some(_) => Err(conflict("IDEMPOTENCY_KEY_REUSED", json!({}))),
    }
}

/// Runs `work` inside the guarded transaction and commits it with the receipt.
macro_rules! guarded {
    ($graph:expr, $writer:expr, $fingerprint:expr, |$tx:ident| $work:expr) => {{
        let w: &Writer = $writer;
        let mut tx = $graph.start_txn().await.map_err(unavailable)?;
        let outcome: Result<(Value, bool), Failure> = async {
            if let Some(replayed) = guard(&mut tx, w, $fingerprint).await? {
                return Ok((replayed, true));
            }
            let $tx = &mut tx;
            let value: Value = $work.await?;
            $tx.run(query("CREATE(:AcademicStaffingReceipt {key:$key,fingerprint:$fingerprint,result:$result,actor_id:$actor,created_at:datetime()})")
                .param("key", w.receipt())
                .param("fingerprint", $fingerprint)
                .param("result", value.to_string())
                .param("actor", w.actor))
                .await
                .map_err(unavailable)?;
            Ok((value, false))
        }
        .await;
        match outcome {
            Ok((value, false)) => {
                tx.commit().await.map_err(unavailable)?;
                Ok(value)
            }
            Ok((value, true)) => {
                let _ = tx.rollback().await;
                Ok(value)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }};
}

fn audit(
    w: &Writer<'_>,
    year: &str,
    kind: &str,
    action: &str,
    record: &str,
    before: Value,
    after: Value,
) -> Query {
    query("CREATE(:AcademicStaffingAudit {key:$key,kind:$kind,action:$action,record_key:$record,actor_id:$actor,actor_role:$role,school_id:$school,tenant_id:$tenant,academic_year:$year,before:$before,after:$after,command_key:$command,created_at:datetime()})")
        .param("key", uuid::Uuid::new_v4().to_string())
        .param("kind", kind)
        .param("action", action)
        .param("record", record)
        .param("actor", w.actor)
        .param("role", w.role)
        .param("school", w.school)
        .param("tenant", w.tenant)
        .param("year", year)
        .param("before", before.to_string())
        .param("after", after.to_string())
        .param("command", w.command)
}

/// Stores one grade's rows under its revision. A subject that still has an active assignment in
/// a class of the grade may be archived but not removed.
async fn store_plan(
    tx: &mut Txn,
    w: &Writer<'_>,
    year: &str,
    grade: &str,
    expected: u32,
    rows: &[PlanRow],
    action: &str,
) -> Result<Value, Failure> {
    let key = plan_key(w.tenant, w.school, year, grade);
    let current = one(tx, query("MATCH(p:AcademicSubjectPlan {key:$key}) RETURN p.version AS version,p.payload AS payload").param("key", key.clone())).await?;
    let before = match &current {
        None if expected == 0 => Value::Null,
        Some(r) if get::<i64>(r, "version")? == i64::from(expected) => {
            serde_json::from_str(&get::<String>(r, "payload")?).map_err(unavailable)?
        }
        _ => return Err(conflict("REVISION_CONFLICT", json!({}))),
    };
    let codes: Vec<String> = rows.iter().map(|r| r.subject.clone()).collect();
    let active: Vec<String> = rows
        .iter()
        .filter(|r| r.active())
        .map(|r| r.subject.clone())
        .collect();
    let in_use = rows_in(tx, query("MATCH(a:TeachingAssignment {school_id:$school,tenant_id:$tenant,academic_year:$year,status:'active'}) WHERE NOT a.subject IN $codes MATCH(sec:Section {section_id:a.section_id,school_id:$school,tenant_id:$tenant,academic_year:$year}) WHERE coalesce(sec.year_group,'')=$grade OR (sec.grade_code IS NOT NULL AND sec.grade_code=$grade_code) RETURN DISTINCT a.subject AS subject ORDER BY subject LIMIT 41")
        .param("school", w.school).param("tenant", w.tenant).param("year", year).param("grade", grade).param("grade_code", crate::academic_nomenclature::model::grade_code(grade)).param("codes", codes.clone()), 41).await?;
    if !in_use.is_empty() {
        let subjects = in_use
            .iter()
            .map(|r| get::<String>(r, "subject"))
            .collect::<Result<Vec<_>, _>>()?;
        return Err(conflict("SUBJECT_IN_USE", json!({ "subjects": subjects })));
    }
    let after = serde_json::to_value(rows).map_err(unavailable)?;
    let saved = one(tx, query("MERGE(p:AcademicSubjectPlan {key:$key}) ON CREATE SET p.tenant_id=$tenant,p.school_id=$school,p.academic_year=$year,p.grade=$grade,p.version=0,p.created_at=datetime() SET p.version=p.version+1,p.grade_code=coalesce(p.grade_code,$grade_code),p.academic_year_code=coalesce(p.academic_year_code,$year_code),p.payload=$payload,p.subjects=$codes,p.active_subjects=$active,p.updated_by=$actor,p.updated_at=datetime() RETURN p.version AS version,toString(p.updated_at) AS updated")
        .param("key", key.clone()).param("tenant", w.tenant).param("school", w.school).param("year", year).param("grade", grade)
        .param("payload", after.to_string()).param("codes", codes).param("active", active).param("actor", w.actor)
        // Nomenclature codes beside the text (slice 5); kept once set, null while the text is not recognised.
        .param("grade_code", crate::academic_nomenclature::model::grade_code(grade)).param("year_code", crate::academic_nomenclature::model::academic_year_code(year))).await?
        .ok_or_else(|| unavailable("No plan"))?;
    tx.run(audit(
        w,
        year,
        "subject_plan",
        action,
        &key,
        before,
        after.clone(),
    ))
    .await
    .map_err(unavailable)?;
    Ok(json!({
        "grade": grade,
        "version": get::<i64>(&saved, "version")?,
        "rows": after,
        "updatedAt": get::<String>(&saved, "updated")?,
    }))
}

pub async fn save_plan(
    graph: &Graph,
    w: &Writer<'_>,
    change: &PlanChange,
) -> Result<Value, Failure> {
    let fingerprint = serde_json::to_string(change).map_err(unavailable)?;
    guarded!(graph, w, fingerprint.as_str(), |tx| store_plan(
        tx,
        w,
        &change.academic_year,
        &change.grade,
        change.expected_version,
        &change.rows,
        "save"
    ))
}

pub async fn copy_plan(graph: &Graph, w: &Writer<'_>, copy: &PlanCopy) -> Result<Value, Failure> {
    let fingerprint = serde_json::to_string(copy).map_err(unavailable)?;
    guarded!(graph, w, fingerprint.as_str(), |tx| async {
        let source = plan_key(w.tenant, w.school, &copy.from_year, &copy.grade);
        let found = one(
            tx,
            query("MATCH(p:AcademicSubjectPlan {key:$key}) RETURN p.payload AS payload")
                .param("key", source),
        )
        .await?;
        let rows: Vec<PlanRow> = match found {
            Some(r) => serde_json::from_str::<Vec<PlanRow>>(&get::<String>(&r, "payload")?)
                .map_err(unavailable)?
                .into_iter()
                .filter(PlanRow::active)
                .collect(),
            None => vec![],
        };
        if rows.is_empty() {
            return Err(refuse(StatusCode::NOT_FOUND, "NOT_FOUND", json!({})));
        }
        store_plan(
            tx,
            w,
            &copy.academic_year,
            &copy.grade,
            copy.expected_version,
            &rows,
            "copy",
        )
        .await
    })
}

fn refused_at(status: StatusCode, code: &str, index: usize) -> Failure {
    refuse(status, code, json!({ "index": index }))
}

/// `assign` checks the class, the grade's plan and the teacher before writing.
async fn eligible(
    tx: &mut Txn,
    w: &Writer<'_>,
    year: &str,
    c: &Command,
    index: usize,
) -> Result<(), Failure> {
    let r = one(tx, query("OPTIONAL MATCH(x:Section {section_id:$section}) WITH collect(x) AS found WITH found,[x IN found WHERE x.school_id=$school AND x.tenant_id=$tenant AND x.academic_year=$year AND x.status='active'] AS mine WITH size(found)=1 AND size(mine)=1 AS section_ok,CASE WHEN size(mine)=1 THEN coalesce(mine[0].year_group,'') ELSE '' END AS grade OPTIONAL MATCH(p:AcademicSubjectPlan {key:$plan+grade}) WITH section_ok,coalesce($subject IN p.active_subjects,false) AS subject_ok OPTIONAL MATCH(t:StaffMember {id:$staff}) WITH section_ok,subject_ok,collect(t) AS ts RETURN section_ok,subject_ok,size(ts)=1 AND ts[0].membershipStatus='ACTIVE' AND 'teacher' IN coalesce(ts[0].roles,[]) AND $school IN coalesce(ts[0].schoolIds,[]) AND $tenant IN coalesce(ts[0].tenantIds,[]) AS teacher_ok")
        .param("section", c.section_id.clone()).param("school", w.school).param("tenant", w.tenant).param("year", year)
        .param("plan", plan_key(w.tenant, w.school, year, ""))
        .param("subject", c.subject.clone()).param("staff", c.staff_member_id.clone())).await?
        .ok_or_else(|| unavailable("No row"))?;
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    if !get::<bool>(&r, "section_ok")? {
        return Err(refused_at(unprocessable, "CLASS_NOT_ACTIVE", index));
    }
    if !get::<bool>(&r, "subject_ok")? {
        return Err(refused_at(unprocessable, "SUBJECT_NOT_IN_PLAN", index));
    }
    if !get::<bool>(&r, "teacher_ok")? {
        return Err(refused_at(unprocessable, "TEACHER_NOT_ELIGIBLE", index));
    }
    Ok(())
}

/// Brings one teacher's Learning access to one class in line with their active assignments:
/// granted while any is active, revoked when the last one ended in this request.
async fn sync_grant(
    tx: &mut Txn,
    w: &Writer<'_>,
    staff: &str,
    section: &str,
    ended: bool,
) -> Result<Option<Value>, Failure> {
    let grant = format!("{staff}|{section}");
    let scoped = |body: &str| {
        query(body)
            .param("grant", grant.clone())
            .param("staff", staff)
            .param("section", section)
            .param("school", w.school)
            .param("tenant", w.tenant)
            .param("actor", w.actor)
            .param("role", w.role)
            .param("audit", uuid::Uuid::new_v4().to_string())
    };
    // The same section lock the Learning access grant command takes.
    one(tx, scoped("MATCH(sec:Section {section_id:$section,school_id:$school,tenant_id:$tenant}) WITH sec LIMIT 1 SET sec.learning_access_lock=coalesce(sec.learning_access_lock,0)+1 RETURN sec.section_id AS id")).await?;
    let held = one(tx, scoped("RETURN EXISTS {MATCH(:TeachingAssignment {staff_member_id:$staff,section_id:$section,school_id:$school,tenant_id:$tenant,status:'active'})} AS held")).await?
        .map(|r| get::<bool>(&r, "held"))
        .transpose()?
        .unwrap_or(false);
    if held {
        let r = one(tx, scoped("MERGE(g:LearningTeachingGrant {key:$grant}) ON CREATE SET g.version=0,g.staff_member_id=$staff,g.section_id=$section,g.school_id=$school,g.tenant_id=$tenant WITH g,g.staff_member_id=$staff AND g.section_id=$section AND g.school_id=$school AND g.tenant_id=$tenant AS ok,coalesce(g.status,'')<>'ACTIVE' AS change FOREACH(_ IN CASE WHEN ok AND change THEN [1] ELSE [] END | SET g.version=g.version+1,g.status='ACTIVE',g.source='teaching_assignment',g.updated_at=datetime() CREATE(:LearningAccessAudit {id:$audit,actor_id:$actor,actor_role:$role,target_id:$staff,record_key:$grant,kind:'teaching',source:'teaching_assignment',school_id:$school,tenant_id:$tenant,version:g.version,status:'ACTIVE',created_at:datetime()})) RETURN ok,change")).await?
            .ok_or_else(|| unavailable("No grant"))?;
        if !get::<bool>(&r, "ok")? {
            return Err(conflict("GRANT_CONFLICT", json!({ "sectionId": section })));
        }
        return Ok(get::<bool>(&r, "change")?
            .then(|| json!({"staffMemberId": staff, "sectionId": section, "status": "ACTIVE"})));
    }
    if !ended {
        return Ok(None);
    }
    let revoked = one(tx, scoped("MATCH(g:LearningTeachingGrant {key:$grant,staff_member_id:$staff,section_id:$section,school_id:$school,tenant_id:$tenant,status:'ACTIVE'}) SET g.version=g.version+1,g.status='REVOKED',g.source='teaching_assignment',g.updated_at=datetime() CREATE(:LearningAccessAudit {id:$audit,actor_id:$actor,actor_role:$role,target_id:$staff,record_key:$grant,kind:'teaching',source:'teaching_assignment',school_id:$school,tenant_id:$tenant,version:g.version,status:'REVOKED',created_at:datetime()}) RETURN g.version AS version")).await?;
    Ok(revoked.map(|_| json!({"staffMemberId": staff, "sectionId": section, "status": "REVOKED"})))
}

async fn apply(tx: &mut Txn, w: &Writer<'_>, batch: &Commands) -> Result<Value, Failure> {
    let year = batch.academic_year.as_str();
    let mut results = vec![];
    let mut touched: BTreeMap<(String, String), bool> = BTreeMap::new();
    for (index, c) in batch.commands.iter().enumerate() {
        let key = batch.key(c);
        let current = one(tx, query("MATCH(a:TeachingAssignment {key:$key}) RETURN a.status AS status,a.periods AS periods,a.version AS version").param("key", key.clone())).await?
            .map(|r| -> Result<Current, Failure> {
                Ok(Current {
                    status: get(&r, "status")?,
                    periods: get::<i64>(&r, "periods")?.try_into().map_err(unavailable)?,
                    version: get::<i64>(&r, "version")?.try_into().map_err(unavailable)?,
                })
            })
            .transpose()?;
        let (status, periods) = next(c, current.as_ref())
            .ok_or_else(|| refused_at(StatusCode::CONFLICT, "REVISION_CONFLICT", index))?;
        if c.action == ASSIGN {
            eligible(tx, w, year, c, index).await?;
        }
        let saved = one(tx, query("MERGE(a:TeachingAssignment {key:$key}) ON CREATE SET a.id=$id,a.tenant_id=$tenant,a.school_id=$school,a.academic_year=$year,a.section_id=$section,a.subject=$subject,a.staff_member_id=$staff,a.version=0,a.created_at=datetime() SET a.version=a.version+1,a.status=$status,a.periods=$periods,a.updated_by=$actor,a.updated_at=datetime(),a.ended_at=CASE WHEN $status='ended' THEN datetime() ELSE null END RETURN a.version AS version,toString(a.updated_at) AS updated")
            .param("key", key.clone()).param("id", uuid::Uuid::new_v4().to_string()).param("tenant", w.tenant).param("school", w.school).param("year", year)
            .param("section", c.section_id.clone()).param("subject", c.subject.clone()).param("staff", c.staff_member_id.clone())
            .param("status", status).param("periods", i64::from(periods)).param("actor", w.actor)).await?
            .ok_or_else(|| unavailable("No assignment"))?;
        let before = current
            .as_ref()
            .map(
                |cur| json!({"status": cur.status, "periods": cur.periods, "version": cur.version}),
            )
            .unwrap_or(Value::Null);
        let result = json!({
            "sectionId": c.section_id,
            "subject": c.subject,
            "staffMemberId": c.staff_member_id,
            "periods": periods,
            "status": status,
            "version": get::<i64>(&saved, "version")?,
            "updatedAt": get::<String>(&saved, "updated")?,
        });
        tx.run(audit(
            w,
            year,
            "teaching_assignment",
            &c.action,
            &key,
            before,
            result.clone(),
        ))
        .await
        .map_err(unavailable)?;
        *touched
            .entry((c.staff_member_id.clone(), c.section_id.clone()))
            .or_default() |= c.action == END;
        results.push(result);
    }
    let mut grants = vec![];
    for ((staff, section), ended) in touched {
        if let Some(change) = sync_grant(tx, w, &staff, &section, ended).await? {
            grants.push(change);
        }
    }
    Ok(json!({ "assignments": results, "grants": grants }))
}

pub async fn run_commands(
    graph: &Graph,
    w: &Writer<'_>,
    batch: &Commands,
) -> Result<Value, Failure> {
    let fingerprint = serde_json::to_string(batch).map_err(unavailable)?;
    guarded!(graph, w, fingerprint.as_str(), |tx| apply(tx, w, batch))
}

fn plan_rows(payload: &str) -> Result<Vec<PlanRow>, Failure> {
    serde_json::from_str(payload).map_err(unavailable)
}
fn scoped(body: &str, school: &str, tenant: &str, year: &str) -> Query {
    query(body)
        .param("school", school)
        .param("tenant", tenant)
        .param("year", year)
}

/// Each grade with active classes or a plan this year, and the other years' plans to copy from.
pub async fn plans(
    graph: &Graph,
    school: &str,
    tenant: &str,
    year: &str,
) -> Result<Value, Failure> {
    let mut grades: BTreeMap<String, (i64, Option<Value>)> = BTreeMap::new();
    for r in rows(graph, scoped("MATCH(sec:Section {school_id:$school,tenant_id:$tenant,academic_year:$year,status:'active'}) WHERE coalesce(sec.year_group,'')<>'' RETURN sec.year_group AS grade,count(sec) AS classes LIMIT 201", school, tenant, year), 200).await? {
        grades.insert(get(&r, "grade")?, (get(&r, "classes")?, None));
    }
    let mut sources: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in rows(graph, scoped("MATCH(p:AcademicSubjectPlan {school_id:$school,tenant_id:$tenant}) RETURN p.academic_year AS year,p.grade AS grade,p.version AS version,p.payload AS payload,toString(p.updated_at) AS updated ORDER BY year DESC,grade LIMIT 1001", school, tenant, year), 1000).await? {
        let (plan_year, grade): (String, String) = (get(&r, "year")?, get(&r, "grade")?);
        let rows = plan_rows(&get::<String>(&r, "payload")?)?;
        if plan_year == year {
            grades.entry(grade.clone()).or_insert((0, None)).1 = Some(json!({
                "version": get::<i64>(&r, "version")?,
                "rows": rows,
                "updatedAt": get::<String>(&r, "updated")?,
            }));
        } else if rows.iter().any(PlanRow::active) {
            sources.entry(plan_year).or_default().insert(grade);
        }
    }
    let mut ordered: Vec<(String, (i64, Option<Value>))> = grades.into_iter().collect();
    ordered.sort_by(|a, b| grade_order(&a.0, &b.0));
    let mut years: Vec<(String, BTreeSet<String>)> = sources.into_iter().collect();
    years.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(json!({
        "grades": ordered.into_iter().map(|(grade, (classes, plan))| json!({"grade": grade, "classes": classes, "plan": plan})).collect::<Vec<_>>(),
        "sources": years.into_iter().map(|(y, g)| {
            let mut g: Vec<String> = g.into_iter().collect();
            g.sort_by(|a, b| grade_order(a, b));
            json!({"academicYear": y, "grades": g})
        }).collect::<Vec<_>>(),
    }))
}

const SECTIONS: &str = "MATCH(sec:Section {school_id:$school,tenant_id:$tenant,academic_year:$year,status:'active'}) WHERE NOT EXISTS {MATCH(o:Section {section_id:sec.section_id}) WHERE o<>sec} OPTIONAL MATCH(h:StaffMember {id:sec.homeroom_staff_member_id}) WITH sec,collect(h) AS hs RETURN sec.section_id AS id,coalesce(sec.name,sec.section_id) AS name,coalesce(sec.year_group,'') AS grade,CASE WHEN size(hs)=1 THEN hs[0].id END AS homeroom,CASE WHEN size(hs)=1 THEN coalesce(hs[0].displayName,hs[0].fullName,hs[0].name,hs[0].id) END AS homeroom_name,size(hs)=1 AND hs[0].membershipStatus='ACTIVE' AND 'teacher' IN coalesce(hs[0].roles,[]) AND $school IN coalesce(hs[0].schoolIds,[]) AND $tenant IN coalesce(hs[0].tenantIds,[]) AS homeroom_active ORDER BY id LIMIT 201";
const ASSIGNMENTS: &str = "MATCH(a:TeachingAssignment {school_id:$school,tenant_id:$tenant,academic_year:$year}) WHERE ($section='' OR a.section_id=$section) AND ($staff='' OR a.staff_member_id=$staff) RETURN a.section_id AS section,a.subject AS subject,a.staff_member_id AS staff,a.periods AS periods,a.status AS status,a.version AS version,toString(a.updated_at) AS updated ORDER BY section,subject,staff LIMIT 4001";
const PEOPLE: &str = "MATCH(t:StaffMember) WHERE ('teacher' IN coalesce(t.roles,[]) AND t.membershipStatus='ACTIVE' AND $school IN coalesce(t.schoolIds,[]) AND $tenant IN coalesce(t.tenantIds,[])) OR t.id IN $referenced WITH t.id AS id,collect(t) AS ts RETURN id,coalesce(ts[0].displayName,ts[0].fullName,ts[0].name,id) AS name,size(ts)=1 AND ts[0].membershipStatus='ACTIVE' AND 'teacher' IN coalesce(ts[0].roles,[]) AND $school IN coalesce(ts[0].schoolIds,[]) AND $tenant IN coalesce(ts[0].tenantIds,[]) AS active ORDER BY name,id LIMIT 1001";

/// What the assignment matrix needs: classes with their homeroom, the year's plans, every
/// assignment (active and ended, with revisions) and the teachers who may be or are assigned.
pub async fn overview(
    graph: &Graph,
    school: &str,
    tenant: &str,
    year: &str,
    section: &str,
    staff: &str,
) -> Result<Value, Failure> {
    let mut sections = vec![];
    for r in rows(graph, scoped(SECTIONS, school, tenant, year), 200).await? {
        let homeroom = get::<Option<String>>(&r, "homeroom")?.map(|id| -> Result<Value, Failure> {
            Ok(json!({"staffMemberId": id, "name": get::<String>(&r, "homeroom_name")?, "active": get::<bool>(&r, "homeroom_active")?}))
        }).transpose()?;
        sections.push(json!({"sectionId": get::<String>(&r, "id")?, "name": get::<String>(&r, "name")?, "yearGroup": get::<String>(&r, "grade")?, "homeroom": homeroom}));
    }
    let mut assignments = vec![];
    let mut referenced = BTreeSet::new();
    for r in rows(
        graph,
        scoped(ASSIGNMENTS, school, tenant, year)
            .param("section", section)
            .param("staff", staff),
        4000,
    )
    .await?
    {
        let holder: String = get(&r, "staff")?;
        referenced.insert(holder.clone());
        assignments.push(json!({
            "sectionId": get::<String>(&r, "section")?,
            "subject": get::<String>(&r, "subject")?,
            "staffMemberId": holder,
            "periods": get::<i64>(&r, "periods")?,
            "status": get::<String>(&r, "status")?,
            "version": get::<i64>(&r, "version")?,
            "updatedAt": get::<String>(&r, "updated")?,
        }));
    }
    let mut teachers = vec![];
    for r in rows(
        graph,
        scoped(PEOPLE, school, tenant, year)
            .param("referenced", referenced.into_iter().collect::<Vec<_>>()),
        1000,
    )
    .await?
    {
        teachers.push(json!({"staffMemberId": get::<String>(&r, "id")?, "name": get::<String>(&r, "name")?, "active": get::<bool>(&r, "active")?}));
    }
    let mut plans = vec![];
    for r in rows(graph, scoped("MATCH(p:AcademicSubjectPlan {school_id:$school,tenant_id:$tenant,academic_year:$year}) RETURN p.grade AS grade,p.version AS version,p.payload AS payload ORDER BY grade LIMIT 201", school, tenant, year), 200).await? {
        plans.push(json!({"grade": get::<String>(&r, "grade")?, "version": get::<i64>(&r, "version")?, "rows": plan_rows(&get::<String>(&r, "payload")?)?}));
    }
    plans.sort_by(|a, b| {
        grade_order(
            a["grade"].as_str().unwrap_or(""),
            b["grade"].as_str().unwrap_or(""),
        )
    });
    Ok(
        json!({"sections": sections, "plans": plans, "teachers": teachers, "assignments": assignments}),
    )
}

/// Active plan rows and active assignments of one school year, for the teacher list.
pub async fn planned(
    graph: &Graph,
    school: &str,
    tenant: &str,
    year: &str,
    sections: &[String],
) -> Result<(Vec<(String, PlanRow)>, Vec<(String, String, String, u32)>), Failure> {
    let mut plans = vec![];
    for r in rows(graph, scoped("MATCH(p:AcademicSubjectPlan {school_id:$school,tenant_id:$tenant,academic_year:$year}) RETURN p.grade AS grade,p.payload AS payload ORDER BY grade LIMIT 201", school, tenant, year), 200).await? {
        let grade: String = get(&r, "grade")?;
        for row in plan_rows(&get::<String>(&r, "payload")?)? {
            if row.active() {
                plans.push((grade.clone(), row));
            }
        }
    }
    let mut assignments = vec![];
    for r in rows(graph, scoped("MATCH(a:TeachingAssignment {school_id:$school,tenant_id:$tenant,academic_year:$year,status:'active'}) WHERE a.section_id IN $sections RETURN a.section_id AS section,a.subject AS subject,a.staff_member_id AS staff,a.periods AS periods ORDER BY section,subject,staff LIMIT 4001", school, tenant, year).param("sections", sections.to_vec()), 4000).await? {
        assignments.push((
            get(&r, "section")?,
            get(&r, "subject")?,
            get(&r, "staff")?,
            get::<i64>(&r, "periods")?.try_into().map_err(unavailable)?,
        ));
    }
    Ok((plans, assignments))
}

pub mod weights;
