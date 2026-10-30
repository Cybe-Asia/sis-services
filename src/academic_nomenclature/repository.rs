//! Graph reads and writes for school nomenclature. Missing kinds are seeded with the defaults of
//! spec §2 under the school lock the first time a school is read or saved. A save runs in one
//! transaction: caller lock, school lock, the capability decision from the graph (ADR 0018), the
//! session again, the idempotency receipt, then the rows, the kind's revision, the audit and the
//! receipt together, as subject plans do.
use super::model::{self, kind_key, row_key, Change, Row, SchoolFacts, KINDS};
use crate::academic_staffing::repository::{refuse, unavailable, Failure, Session, Writer};
use crate::authorization::facts::Db;
use crate::learning_access::timetable::repository::{lock_school, one};
use axum::http::StatusCode;
use neo4rs::{query, Graph, Query, Row as GraphRow, Txn};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The capability a save needs at the school. Owners and school administrators hold it on day one.
pub const NOMENCLATURE_MANAGE: &str = "academic.nomenclature.manage";

fn forbidden() -> Failure {
    refuse(StatusCode::FORBIDDEN, "FORBIDDEN", json!({}))
}
fn get<T: serde::de::DeserializeOwned>(row: &GraphRow, name: &str) -> Result<T, Failure> {
    row.get(name).map_err(unavailable)
}

pub async fn init(graph: &Graph) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for statement in include_str!("../../migrations/0008_academic_nomenclature.cypher")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        graph.run(query(statement)).await?;
    }
    Ok(())
}

async fn rows_in(tx: &mut Txn, q: Query, max: usize) -> Result<Vec<GraphRow>, Failure> {
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
async fn rows(graph: &Graph, q: Query, max: usize) -> Result<Vec<GraphRow>, Failure> {
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

const KIND_VERSIONS: &str = "MATCH(k:AcademicNomenclatureKind {tenant_id:$tenant,school_id:$school}) RETURN k.kind AS kind,k.version AS version ORDER BY kind LIMIT 64";
const SCHOOL: &str = "MATCH(sc:School {school_id:$school,tenant_id:$tenant}) WITH collect(sc) AS ss WHERE size(ss)=1 RETURN coalesce(ss[0].school_name,ss[0].name,$school) AS name";
const YEARS: &str = "MATCH(s:Section {tenant_id:$tenant,school_id:$school}) WHERE s.academic_year IS NOT NULL RETURN DISTINCT toString(s.academic_year) AS year LIMIT 201";
const PLANS: &str = "MATCH(p:AcademicSubjectPlan {tenant_id:$tenant,school_id:$school}) RETURN p.payload AS payload ORDER BY p.academic_year DESC,p.grade LIMIT 201";
const ROWS: &str = "MATCH(n:AcademicNomenclature {tenant_id:$tenant,school_id:$school}) WHERE $kind IS NULL OR n.kind=$kind RETURN n.kind AS kind,n.code AS code,n.label_id AS label_id,n.label_en AS label_en,n.short_id AS short_id,n.short_en AS short_en,n.order AS order,n.status AS status,n.version AS version ORDER BY kind,n.order,code LIMIT 1101";

/// The versions of the kinds a school has, by kind.
async fn versions(
    graph: &Graph,
    tenant: &str,
    school: &str,
) -> Result<BTreeMap<String, i64>, Failure> {
    let mut out = BTreeMap::new();
    for r in rows(
        graph,
        query(KIND_VERSIONS)
            .param("tenant", tenant)
            .param("school", school),
        63,
    )
    .await?
    {
        out.insert(get::<String>(&r, "kind")?, get::<i64>(&r, "version")?);
    }
    Ok(out)
}

/// A strong validator for a read: the kinds the school has and the sum of their versions. Every
/// save raises one version by one, so the sum only grows; archived rows change the answer too.
pub fn etag(versions: &BTreeMap<String, i64>, archived: bool) -> String {
    let sum: i64 = versions.values().sum();
    format!(
        "\"n{}-{}-{}{}\"",
        model::CONTRACT_VERSION,
        versions.len(),
        sum,
        if archived { "a" } else { "" }
    )
}

/// The read of a school, `None` when its versions still match `known` (the caller's ETag).
pub async fn read(
    graph: &Graph,
    tenant: &str,
    school: &str,
    actor: &str,
    role: &str,
    archived: bool,
    known: Option<&str>,
) -> Result<(String, Option<Value>), Failure> {
    let mut current = versions(graph, tenant, school).await?;
    if current.len() < KINDS.len() {
        // A concurrent first read that won the race leaves nothing to seed; only a seed that
        // left kinds missing is an error.
        let seeded = seed(graph, tenant, school, actor, role).await;
        current = versions(graph, tenant, school).await?;
        if current.len() < KINDS.len() {
            seeded?;
        }
    }
    let tag = etag(&current, archived);
    if known == Some(tag.as_str()) {
        return Ok((tag, None));
    }
    let stored = rows(
        graph,
        query(ROWS)
            .param("tenant", tenant)
            .param("school", school)
            .param("kind", Option::<String>::None),
        1100,
    )
    .await?;
    let by_kind = stored_rows(&stored, archived)?;
    Ok((tag, Some(answer(&current, by_kind))))
}

/// Stored rows by kind, archived ones only when asked for.
fn stored_rows(
    stored: &[GraphRow],
    archived: bool,
) -> Result<BTreeMap<String, Vec<Value>>, Failure> {
    let mut by_kind: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for r in stored {
        let status = get::<String>(r, "status")?;
        if !archived && status != model::ACTIVE {
            continue;
        }
        by_kind
            .entry(get::<String>(r, "kind")?)
            .or_default()
            .push(row_json(r)?);
    }
    Ok(by_kind)
}
fn answer(current: &BTreeMap<String, i64>, mut by_kind: BTreeMap<String, Vec<Value>>) -> Value {
    let kinds: Vec<Value> = KINDS
        .iter()
        .map(|k| {
            json!({
                "kind": k,
                "closed": model::closed(k),
                "version": current.get(*k).copied().unwrap_or(0),
                "rows": by_kind.remove(*k).unwrap_or_default(),
            })
        })
        .collect();
    json!({ "kinds": kinds, "mappings": { "grade": model::mappings() } })
}

/// The read Learning callers get (slice 4): active rows only, and nothing is written. A kind the
/// school has not stored yet answers its defaults at version 0; such an answer is never a 304, so
/// a later seed or a new subject plan shows at the next read.
pub async fn learning_read(
    graph: &Graph,
    tenant: &str,
    school: &str,
    known: Option<&str>,
) -> Result<(String, Option<Value>), Failure> {
    let current = versions(graph, tenant, school).await?;
    let complete = KINDS.iter().all(|k| current.contains_key(*k));
    let mut tag = etag(&current, false);
    if !complete {
        tag.insert(tag.len() - 1, 'd');
    } else if known == Some(tag.as_str()) {
        return Ok((tag, None));
    }
    let stored = rows(
        graph,
        query(ROWS)
            .param("tenant", tenant)
            .param("school", school)
            .param("kind", Option::<String>::None),
        1100,
    )
    .await?;
    let mut by_kind = stored_rows(&stored, false)?;
    if !complete {
        let mut tx = graph.start_txn().await.map_err(unavailable)?;
        let found = facts(&mut tx, tenant, school).await;
        let _ = tx.rollback().await;
        let found = found?;
        for kind in KINDS.iter().filter(|k| !current.contains_key(**k)) {
            let rows = model::defaults(kind, &found)
                .into_iter()
                .filter(|r| r.status == model::ACTIVE)
                .map(|r| {
                    let mut v = serde_json::to_value(&r).map_err(unavailable)?;
                    v["version"] = json!(0);
                    Ok(v)
                })
                .collect::<Result<Vec<_>, Failure>>()?;
            by_kind.insert((*kind).to_string(), rows);
        }
    }
    Ok((tag, Some(answer(&current, by_kind))))
}

/// One stored row as reads and saves answer it, with the version at which it last changed.
fn row_json(r: &GraphRow) -> Result<Value, Failure> {
    Ok(json!({
        "code": get::<String>(r, "code")?,
        "labelId": get::<String>(r, "label_id")?,
        "labelEn": get::<String>(r, "label_en")?,
        "shortId": get::<Option<String>>(r, "short_id")?,
        "shortEn": get::<Option<String>>(r, "short_en")?,
        "order": get::<i64>(r, "order")?,
        "status": get::<String>(r, "status")?,
        "version": get::<i64>(r, "version")?,
    }))
}

async fn facts(tx: &mut Txn, tenant: &str, school: &str) -> Result<SchoolFacts, Failure> {
    let name = one(
        tx,
        query(SCHOOL)
            .param("school", school)
            .param("tenant", tenant),
    )
    .await?
    .ok_or_else(|| refuse(StatusCode::NOT_FOUND, "NOT_FOUND", json!({})))?;
    let years = rows_in(
        tx,
        query(YEARS).param("tenant", tenant).param("school", school),
        200,
    )
    .await?
    .iter()
    .map(|r| get::<String>(r, "year"))
    .collect::<Result<Vec<_>, _>>()?;
    let mut subjects = vec![];
    for r in rows_in(
        tx,
        query(PLANS).param("tenant", tenant).param("school", school),
        200,
    )
    .await?
    {
        let payload: Vec<crate::academic_staffing::model::PlanRow> =
            serde_json::from_str(&get::<String>(&r, "payload")?).unwrap_or_default();
        subjects.extend(
            payload
                .into_iter()
                .filter(|p| p.active())
                .map(|p| (p.subject, p.name)),
        );
    }
    Ok(SchoolFacts {
        school_id: school.into(),
        school_name: get(&name, "name")?,
        years,
        subjects,
        today: crate::academic_teachers::model::today_wib(chrono::Utc::now().timestamp_millis()),
    })
}

fn audit(
    tenant: &str,
    school: &str,
    kind: &str,
    action: &str,
    actor: &str,
    role: &str,
    command: &str,
    version: i64,
    before: Value,
    after: Value,
) -> Query {
    query("CREATE(:AcademicNomenclatureAudit {key:$key,kind:$kind,action:$action,record_key:$record,actor_id:$actor,actor_role:$role,school_id:$school,tenant_id:$tenant,version:$version,before:$before,after:$after,command_key:$command,created_at:datetime()})")
        .param("key", uuid::Uuid::new_v4().to_string())
        .param("kind", kind)
        .param("action", action)
        .param("record", kind_key(tenant, school, kind))
        .param("actor", actor)
        .param("role", role)
        .param("school", school)
        .param("tenant", tenant)
        .param("version", version)
        .param("before", before.to_string())
        .param("after", after.to_string())
        .param("command", command)
}

fn store(
    statement: &str,
    tenant: &str,
    school: &str,
    kind: &str,
    row: &Row,
    version: i64,
    actor: &str,
) -> Query {
    query(statement)
        .param("key", row_key(tenant, school, kind, &row.code))
        .param("tenant", tenant)
        .param("school", school)
        .param("kind", kind)
        .param("code", row.code.as_str())
        .param("label_id", row.label_id.as_str())
        .param("label_en", row.label_en.as_str())
        .param("short_id", row.short_id.clone())
        .param("short_en", row.short_en.clone())
        .param("order", i64::from(row.order))
        .param("status", row.status.as_str())
        .param("version", version)
        .param("actor", actor)
}
const UPSERT: &str = "MERGE(n:AcademicNomenclature {key:$key}) ON CREATE SET n.tenant_id=$tenant,n.school_id=$school,n.kind=$kind,n.code=$code,n.created_at=datetime() SET n.label_id=$label_id,n.label_en=$label_en,n.short_id=$short_id,n.short_en=$short_en,n.order=$order,n.status=$status,n.version=$version,n.updated_by=$actor,n.updated_at=datetime()";

/// Creates every kind the school does not have yet, with its defaults, at version 1.
async fn seed_missing(
    tx: &mut Txn,
    tenant: &str,
    school: &str,
    actor: &str,
    role: &str,
) -> Result<(), Failure> {
    let present: BTreeSet<String> = rows_in(
        tx,
        query(KIND_VERSIONS)
            .param("tenant", tenant)
            .param("school", school),
        63,
    )
    .await?
    .iter()
    .map(|r| get::<String>(r, "kind"))
    .collect::<Result<_, _>>()?;
    if KINDS.iter().all(|k| present.contains(*k)) {
        return Ok(());
    }
    let school_facts = facts(tx, tenant, school).await?;
    for kind in KINDS.iter().filter(|k| !present.contains(**k)) {
        let defaults = model::defaults(kind, &school_facts);
        for row in &defaults {
            tx.run(store(UPSERT, tenant, school, kind, row, 1, actor))
                .await
                .map_err(unavailable)?;
        }
        tx.run(query("CREATE(:AcademicNomenclatureKind {key:$key,tenant_id:$tenant,school_id:$school,kind:$kind,version:1,updated_by:$actor,created_at:datetime(),updated_at:datetime()})")
            .param("key", kind_key(tenant, school, kind)).param("tenant", tenant).param("school", school).param("kind", *kind).param("actor", actor))
            .await
            .map_err(unavailable)?;
        let after = serde_json::to_value(&defaults).map_err(unavailable)?;
        tx.run(audit(
            tenant,
            school,
            kind,
            "seed",
            actor,
            role,
            "",
            1,
            Value::Null,
            after,
        ))
        .await
        .map_err(unavailable)?;
    }
    Ok(())
}

/// Seeds the missing kinds of a school under its lock; concurrent first reads seed once.
pub async fn seed(
    graph: &Graph,
    tenant: &str,
    school: &str,
    actor: &str,
    role: &str,
) -> Result<(), Failure> {
    let mut tx = graph.start_txn().await.map_err(unavailable)?;
    let outcome = async {
        lock_school(&mut tx, school, tenant).await?;
        seed_missing(&mut tx, tenant, school, actor, role).await
    }
    .await;
    match outcome {
        Ok(()) => tx.commit().await.map_err(unavailable),
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}

fn receipt(w: &Writer<'_>) -> String {
    [w.actor, w.tenant, w.school, w.command].join("|")
}

/// Locks and checks in a fixed order. `Some` is the stored answer of an exact replay.
async fn guard(tx: &mut Txn, w: &Writer<'_>, fingerprint: &str) -> Result<Option<Value>, Failure> {
    one(tx, query("MATCH(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WITH actor LIMIT 1 SET actor.owner_write_lock=coalesce(actor.owner_write_lock,0)+1 RETURN actor.id AS id").param("actor", w.actor))
        .await?
        .ok_or_else(forbidden)?;
    lock_school(tx, w.school, w.tenant).await?;
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
    match w.session {
        Session::Headers(headers) => {
            let (actor, role) = crate::learning_access::administrator_actor(
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
    let stored = one(tx, query("MATCH(r:AcademicNomenclatureReceipt {key:$key}) RETURN r.fingerprint AS fingerprint,r.result AS result").param("key", receipt(w))).await?;
    match stored {
        None => Ok(None),
        Some(r) if get::<String>(&r, "fingerprint")? == fingerprint => Ok(Some(
            serde_json::from_str(&get::<String>(&r, "result")?).map_err(unavailable)?,
        )),
        Some(_) => Err(refuse(
            StatusCode::CONFLICT,
            "IDEMPOTENCY_KEY_REUSED",
            json!({}),
        )),
    }
}

fn stored_row(r: &GraphRow) -> Result<Row, Failure> {
    Ok(Row {
        code: get(r, "code")?,
        label_id: get(r, "label_id")?,
        label_en: get(r, "label_en")?,
        short_id: get(r, "short_id")?,
        short_en: get(r, "short_en")?,
        order: u32::try_from(get::<i64>(r, "order")?).map_err(unavailable)?,
        status: get(r, "status")?,
    })
}

/// Replaces one kind's rows. Every stored code stays (archive instead of removing); closed kinds
/// take no new code.
async fn apply(
    tx: &mut Txn,
    w: &Writer<'_>,
    kind: &str,
    change: &Change,
) -> Result<Value, Failure> {
    seed_missing(tx, w.tenant, w.school, w.actor, w.role).await?;
    let current = one(
        tx,
        query("MATCH(k:AcademicNomenclatureKind {key:$key}) RETURN k.version AS version")
            .param("key", kind_key(w.tenant, w.school, kind)),
    )
    .await?
    .ok_or_else(|| unavailable("No kind"))?;
    let version: i64 = get(&current, "version")?;
    if version != i64::from(change.expected_version) {
        return Err(refuse(
            StatusCode::CONFLICT,
            "REVISION_CONFLICT",
            json!({ "version": version }),
        ));
    }
    let before: Vec<Row> = rows_in(
        tx,
        query(ROWS)
            .param("tenant", w.tenant)
            .param("school", w.school)
            .param("kind", kind),
        1100,
    )
    .await?
    .iter()
    .map(stored_row)
    .collect::<Result<_, _>>()?;
    let sent: BTreeSet<&str> = change.rows.iter().map(|r| r.code.as_str()).collect();
    let removed: Vec<&str> = before
        .iter()
        .map(|r| r.code.as_str())
        .filter(|c| !sent.contains(c))
        .collect();
    if !removed.is_empty() {
        return Err(refuse(
            StatusCode::CONFLICT,
            "CODE_REMOVED",
            json!({ "codes": removed }),
        ));
    }
    if model::closed(kind) {
        let known: BTreeSet<&str> = before.iter().map(|r| r.code.as_str()).collect();
        let added: Vec<&str> = sent
            .iter()
            .copied()
            .filter(|c| !known.contains(c))
            .collect();
        if !added.is_empty() {
            return Err(refuse(
                StatusCode::UNPROCESSABLE_ENTITY,
                "CODE_NOT_ALLOWED",
                json!({ "codes": added }),
            ));
        }
    }
    let next = version + 1;
    let old: BTreeMap<&str, &Row> = before.iter().map(|r| (r.code.as_str(), r)).collect();
    for row in &change.rows {
        // A row keeps its version when only other rows changed; a moved row has changed.
        if old.get(row.code.as_str()).is_some_and(|o| *o == row) {
            continue;
        }
        tx.run(store(UPSERT, w.tenant, w.school, kind, row, next, w.actor))
            .await
            .map_err(unavailable)?;
    }
    let saved = one(tx, query("MATCH(k:AcademicNomenclatureKind {key:$key}) SET k.version=$next,k.updated_by=$actor,k.updated_at=datetime() RETURN toString(k.updated_at) AS updated")
        .param("key", kind_key(w.tenant, w.school, kind)).param("next", next).param("actor", w.actor))
        .await?
        .ok_or_else(|| unavailable("No kind"))?;
    let after = serde_json::to_value(&change.rows).map_err(unavailable)?;
    tx.run(audit(
        w.tenant,
        w.school,
        kind,
        "save",
        w.actor,
        w.role,
        w.command,
        next,
        serde_json::to_value(&before).map_err(unavailable)?,
        after,
    ))
    .await
    .map_err(unavailable)?;
    let rows: Vec<Value> = rows_in(
        tx,
        query(ROWS)
            .param("tenant", w.tenant)
            .param("school", w.school)
            .param("kind", kind),
        1100,
    )
    .await?
    .iter()
    .map(row_json)
    .collect::<Result<_, _>>()?;
    Ok(json!({
        "kind": kind,
        "closed": model::closed(kind),
        "version": next,
        "updatedAt": get::<String>(&saved, "updated")?,
        "rows": rows,
    }))
}

pub async fn save(
    graph: &Graph,
    w: &Writer<'_>,
    kind: &str,
    change: &Change,
) -> Result<Value, Failure> {
    let fingerprint = format!(
        "{kind}|{}",
        serde_json::to_string(change).map_err(unavailable)?
    );
    let mut tx = graph.start_txn().await.map_err(unavailable)?;
    let outcome: Result<(Value, bool), Failure> = async {
        if let Some(replayed) = guard(&mut tx, w, &fingerprint).await? {
            return Ok((replayed, true));
        }
        let value = apply(&mut tx, w, kind, change).await?;
        tx.run(query("CREATE(:AcademicNomenclatureReceipt {key:$key,fingerprint:$fingerprint,result:$result,actor_id:$actor,created_at:datetime()})")
            .param("key", receipt(w))
            .param("fingerprint", fingerprint.as_str())
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
}
