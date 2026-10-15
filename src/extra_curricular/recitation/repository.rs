//! Graph and object-store side of recitation submissions. One node per activity, student and
//! verse (`ExtraCurricularRecitation`), the clip in the shared bucket under its own prefix.
use super::model::*;
use crate::extra_curricular::{
    auth::{authority, Actor, ELIGIBLE, ROSTER},
    model::Scope,
    policy,
    repository::{activity_key, scoped, Error},
};
use crate::school_articles::cover::Store;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use neo4rs::{query, Graph, Query as Cypher, Txn};
use serde_json::{json, Value};

pub enum Outcome {
    Done(Value),
    Forbidden,
    Conflict,
    Missing,
}
const PAGE: usize = 200;
pub fn object_key(scope: &Scope, activity: &str, student: &str, verse: &str, id: &str, mime: &str) -> String {
    format!(
        "sis/recitations/{}/{}/{activity}/{student}/{}-{id}.{}",
        scope.tenant_id,
        scope.school_id,
        verse.replace(':', "-"),
        extension(mime)
    )
}
fn record_key(key: &str, student: &str, verse: &str) -> String {
    format!("{key}|{student}|{verse}")
}
async fn first(tx: &mut Txn, q: Cypher) -> Result<Option<neo4rs::Row>, Error> {
    let mut rows = tx.execute(q).await?;
    let row = rows.next(tx.handle()).await?;
    if rows.next(tx.handle()).await?.is_some() {
        return Err("ambiguous row".into());
    }
    Ok(row)
}
fn activity_scope(actor: &Actor) -> &'static str {
    if actor.role == "owner" {
        "MATCH(activity:ExtraCurricularActivity {key:$key})"
    } else {
        "MATCH(activity:ExtraCurricularActivity {key:$key,coach_id:$actor})"
    }
}
/// A stored record as the API lists it.
fn item(row: &neo4rs::Row, name: Option<&str>) -> Result<Value, Error> {
    let verdict: Value = serde_json::from_str(&row.get::<String>("verdict")?)?;
    let review = row
        .get::<String>("review")
        .ok()
        .map(|raw| serde_json::from_str::<Value>(&raw))
        .transpose()?
        .unwrap_or(Value::Null);
    let mut value = json!({
        "studentId": row.get::<String>("student")?,
        "verse": row.get::<String>("verse")?,
        "revision": row.get::<i64>("revision")?,
        "status": row.get::<String>("status")?,
        "stars": row.get::<i64>("stars")?,
        "attempts": row.get::<i64>("attempts")?,
        "submittedAt": row.get::<String>("submitted_at")?,
        "audio": row.get::<String>("object").is_ok(),
        "verdict": verdict,
        "review": review,
    });
    if let Some(name) = name {
        value["studentName"] = json!(name);
    }
    Ok(value)
}
const FIELDS: &str = "r.student_id AS student,r.verse AS verse,r.revision AS revision,r.status AS status,r.stars AS stars,r.attempts AS attempts,toString(r.submitted_at) AS submitted_at,r.object_key AS object,r.verdict AS verdict,r.review AS review,r.key AS key";

/// Replaces the verse's submission with this take. The Student must be enrolled in a published,
/// unarchived activity of their own section. The previous clip is removed once the new one is kept.
pub async fn submit(
    g: &Graph,
    store: &Store,
    actor: &Actor,
    s: &Submission,
    audio: Vec<u8>,
) -> Result<Outcome, Error> {
    if actor.role != "student" {
        return Ok(Outcome::Forbidden);
    }
    let key = activity_key(&s.scope, &s.activity_id);
    let mut tx = g.start_txn().await?;
    first(&mut tx, query("MERGE(l:ExtraCurricularLock {key:$key}) ON CREATE SET l.version=0 SET l.version=l.version+1 RETURN l.version AS version").param("key", key.clone())).await?.ok_or("lock missing")?;
    let allowed = scoped(&format!("{} MATCH(activity:ExtraCurricularActivity {{key:$key}}) WHERE activity.published=true AND activity.archived=false AND {ELIGIBLE} MATCH(:ExtraCurricularEnrollment {{key:activity.key+'|'+student.studentId,status:'enrolled'}}) RETURN student.studentId AS student", authority(actor)), actor, &s.scope).param("key", key.clone());
    let Some(row) = first(&mut tx, allowed).await? else {
        tx.rollback().await?;
        return Ok(Outcome::Forbidden);
    };
    let student: String = row.get("student")?;
    let rk = record_key(&key, &student, &s.verse);
    let previous = first(&mut tx, query("MATCH(r:ExtraCurricularRecitation {key:$rk}) RETURN r.revision AS revision,r.attempts AS attempts,r.object_key AS object").param("rk", rk.clone())).await?;
    let (revision, attempts, old_object) = match &previous {
        Some(p) => (
            p.get::<i64>("revision")? + 1,
            p.get::<i64>("attempts")? + 1,
            p.get::<String>("object").ok(),
        ),
        None => (1, 1, None),
    };
    let object = object_key(&s.scope, &s.activity_id, &student, &s.verse, &uuid::Uuid::new_v4().to_string(), &s.mime);
    let size = audio.len() as i64;
    store.put_as(&object, audio, &s.mime).await?;
    let written = first(&mut tx, query("MERGE(r:ExtraCurricularRecitation {key:$rk}) SET r.activity_key=$key,r.student_id=$student,r.verse=$verse,r.revision=$revision,r.attempts=$attempts,r.status='submitted',r.stars=$stars,r.object_key=$object,r.mime=$mime,r.size=$size,r.verdict=$verdict,r.submitted_at=datetime(),r.delete_after=datetime()+duration({days:$days}),r.purged_at=null RETURN toString(r.submitted_at) AS submitted_at").param("rk",rk).param("key",key.clone()).param("student",student.clone()).param("verse",s.verse.clone()).param("revision",revision).param("attempts",attempts).param("stars",i64::from(s.verdict.stars)).param("object",object.clone()).param("mime",s.mime.clone()).param("size",size).param("verdict",serde_json::to_string(&s.verdict)?).param("days",UNREVIEWED_RETENTION_DAYS)).await?;
    let Some(written) = written else {
        tx.rollback().await?;
        let _ = store.delete(&object).await;
        return Err("recitation write missing".into());
    };
    let submitted_at: String = written.get("submitted_at")?;
    let result = json!({"verse":s.verse,"revision":revision,"attempts":attempts,"status":"submitted","submittedAt":submitted_at});
    audit(&mut tx, actor, &key, json!({"recitation":"submit","verse":s.verse,"size":size}), &result).await?;
    if let Err(error) = tx.commit().await {
        let _ = store.delete(&object).await;
        return Err(error.into());
    }
    if let Some(old) = old_object {
        if let Err(error) = store.delete(&old).await {
            tracing::warn!(%error, "replaced recitation clip was not removed");
        }
    }
    Ok(Outcome::Done(result))
}
async fn audit(tx: &mut Txn, actor: &Actor, key: &str, request: Value, result: &Value) -> Result<(), Error> {
    let op = format!("{}|{}|{key}|recitation|{}", actor.role, actor.id, uuid::Uuid::new_v4());
    first(tx, query("CREATE(a:ExtraCurricularAudit {key:$op,actor_id:$actor,role:$role,activity_key:$key,request:$request,result:$result,created_at:datetime()}) RETURN a.key AS key").param("op",op).param("actor",actor.id.clone()).param("role",actor.role.clone()).param("key",key).param("request",serde_json::to_string(&request)?).param("result",serde_json::to_string(result)?)).await?.ok_or("audit missing")?;
    Ok(())
}
/// The roster a coach may see: student id to name. The Owner sees every activity's roster.
async fn roster(g: &Graph, actor: &Actor, scope: &Scope, key: &str) -> Result<Option<Vec<(String, String)>>, Error> {
    let mut rows = g.execute(scoped(&format!("{} {} {ROSTER} RETURN DISTINCT student.studentId AS id,student.fullName AS name ORDER BY id LIMIT 201", authority(actor), activity_scope(actor)), actor, scope).param("key", key)).await?;
    let mut out = vec![];
    while let Some(row) = rows.next().await? {
        if out.len() == 200 {
            return Err("roster too large".into());
        }
        out.push((row.get::<String>("id")?, row.get::<String>("name").unwrap_or_default()));
    }
    // An activity the coach does not run has no roster for them; so does one with nobody enrolled yet,
    // which is told apart by the activity itself.
    if out.is_empty() {
        let mut owned = g.execute(scoped(&format!("{} {} RETURN count(activity) AS count", authority(actor), activity_scope(actor)), actor, scope).param("key", key)).await?;
        if owned.next().await?.is_none_or(|r| r.get::<i64>("count").unwrap_or(0) != 1) {
            return Ok(None);
        }
    }
    Ok(Some(out))
}
/// The Student's own identity for this scope, when they are a current member of the activity.
async fn own(g: &Graph, actor: &Actor, scope: &Scope, key: &str) -> Result<Option<String>, Error> {
    let mut rows = g.execute(scoped(&format!("{} MATCH(activity:ExtraCurricularActivity {{key:$key}}) MATCH(:ExtraCurricularEnrollment {{key:activity.key+'|'+student.studentId}}) RETURN student.studentId AS student", authority(actor)), actor, scope).param("key", key)).await?;
    let found = rows.next().await?.map(|r| r.get::<String>("student")).transpose()?;
    if rows.next().await?.is_some() {
        return Err("ambiguous student".into());
    }
    Ok(found)
}
/// A Student lists their own verses; a coach lists the roster's, newest first within a page.
pub async fn list(g: &Graph, actor: &Actor, q: &Query) -> Result<Option<Value>, Error> {
    let scope = q.scope();
    let key = activity_key(&scope, &q.activity_id);
    let (students, names): (Vec<String>, Vec<(String, String)>) = match actor.role.as_str() {
        "student" => {
            let Some(student) = own(g, actor, &scope, &key).await? else {
                return Ok(None);
            };
            if q.student_id.as_ref().is_some_and(|s| s != &student) {
                return Ok(None);
            }
            (vec![student], vec![])
        }
        "teacher" | "owner" => {
            let Some(roster) = roster(g, actor, &scope, &key).await? else {
                return Ok(None);
            };
            let ids = roster.iter().map(|(id, _)| id.clone()).filter(|id| q.student_id.as_ref().is_none_or(|s| s == id)).collect();
            (ids, roster)
        }
        _ => return Ok(None),
    };
    let after = match q.after.as_deref() {
        Some(cursor) => String::from_utf8(URL_SAFE_NO_PAD.decode(cursor).map_err(|_| "bad cursor")?).map_err(|_| "bad cursor")?,
        None => String::new(),
    };
    let mut rows = g.execute(query(&format!("MATCH(r:ExtraCurricularRecitation {{activity_key:$key}}) WHERE r.key>$after AND r.student_id IN $students AND ($verse='' OR r.verse=$verse) AND ($status='' OR r.status=$status) RETURN {FIELDS} ORDER BY r.key LIMIT {}", PAGE + 1)).param("key", key).param("after", after).param("students", students).param("verse", q.verse.clone().unwrap_or_default()).param("status", q.status.clone().unwrap_or_default())).await?;
    let mut items = vec![];
    let mut next = None;
    while let Some(row) = rows.next().await? {
        if items.len() == PAGE {
            next = Some(URL_SAFE_NO_PAD.encode(last_key(&items)?));
            break;
        }
        let student: String = row.get("student")?;
        let name = names.iter().find(|(id, _)| id == &student).map(|(_, n)| n.as_str());
        let mut value = item(&row, if actor.role == "student" { None } else { name.or(Some("")) })?;
        value["key"] = json!(row.get::<String>("key")?);
        items.push(value);
    }
    for value in &mut items {
        value.as_object_mut().map(|o| o.remove("key"));
    }
    Ok(Some(json!({"contractVersion":1,"items":items,"nextCursor":next})))
}
fn last_key(items: &[Value]) -> Result<String, Error> {
    items
        .last()
        .and_then(|v| v["key"].as_str())
        .map(String::from)
        .ok_or_else(|| "cursor missing".into())
}
/// The clip of one submission: the Student's own, or one of the coach's roster.
pub async fn audio(g: &Graph, store: &Store, actor: &Actor, q: &Query) -> Result<Outcome, Error> {
    let (Some(verse), Some(wanted)) = (q.verse.as_deref(), q.student_id.as_deref()) else {
        return Ok(Outcome::Missing);
    };
    let scope = q.scope();
    let key = activity_key(&scope, &q.activity_id);
    let allowed = match actor.role.as_str() {
        "student" => own(g, actor, &scope, &key).await?.is_some_and(|s| s == wanted),
        "teacher" | "owner" => roster(g, actor, &scope, &key).await?.is_some_and(|r| r.iter().any(|(id, _)| id == wanted)),
        _ => false,
    };
    if !allowed {
        return Ok(Outcome::Forbidden);
    }
    let mut rows = g.execute(query("MATCH(r:ExtraCurricularRecitation {key:$rk}) RETURN r.object_key AS object,r.mime AS mime").param("rk", record_key(&key, wanted, verse))).await?;
    let Some(row) = rows.next().await? else {
        return Ok(Outcome::Missing);
    };
    let (Ok(object), Ok(mime)) = (row.get::<String>("object"), row.get::<String>("mime")) else {
        return Ok(Outcome::Missing);
    };
    drop(rows);
    let bytes = store.get_bounded(&object, MAX_AUDIO_BYTES).await?;
    Ok(Outcome::Done(json!({"mime":mime,"bytes":bytes})))
}
/// The coach grades the submission at the revision they listened to; a newer take conflicts.
pub async fn review(g: &Graph, actor: &Actor, r: &Review) -> Result<Outcome, Error> {
    if !matches!(actor.role.as_str(), "teacher" | "owner") {
        return Ok(Outcome::Forbidden);
    }
    let key = activity_key(&r.scope, &r.activity_id);
    let mut tx = g.start_txn().await?;
    first(&mut tx, query("MERGE(l:ExtraCurricularLock {key:$key}) ON CREATE SET l.version=0 SET l.version=l.version+1 RETURN l.version AS version").param("key", key.clone())).await?.ok_or("lock missing")?;
    let current = first(&mut tx, scoped(&format!("{} {} WHERE activity.archived=false RETURN activity.payload AS payload", authority(actor), activity_scope(actor)), actor, &r.scope).param("key", key.clone())).await?;
    let Some(current) = current else {
        tx.rollback().await?;
        return Ok(Outcome::Forbidden);
    };
    let activity: crate::extra_curricular::model::Activity = serde_json::from_str(&current.get::<String>("payload")?)?;
    let on_roster = first(&mut tx, scoped(&format!("{} {} {ROSTER} WITH DISTINCT student WHERE student.studentId=$wanted RETURN count(student) AS count", authority(actor), activity_scope(actor)), actor, &r.scope).param("key", key.clone()).param("wanted", r.student_id.clone())).await?;
    if !on_roster.is_some_and(|row| row.get::<i64>("count").unwrap_or(0) == 1) || !policy::skills(&activity, &r.skills) {
        tx.rollback().await?;
        return Ok(Outcome::Forbidden);
    }
    let rk = record_key(&key, &r.student_id, &r.verse);
    let Some(record) = first(&mut tx, query("MATCH(r:ExtraCurricularRecitation {key:$rk}) RETURN r.revision AS revision").param("rk", rk.clone())).await? else {
        tx.rollback().await?;
        return Ok(Outcome::Missing);
    };
    if record.get::<i64>("revision")? != i64::from(r.revision) {
        tx.rollback().await?;
        return Ok(Outcome::Conflict);
    }
    let revision = i64::from(r.revision) + 1;
    let reviewed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let grade = Grade {
        revision: r.revision,
        reviewer_id: actor.id.clone(),
        skills: r.skills.clone(),
        note: r.note.clone(),
        decision: r.decision.clone(),
        reviewed_at: reviewed_at.clone(),
    };
    first(&mut tx, query("MATCH(r:ExtraCurricularRecitation {key:$rk}) SET r.revision=$revision,r.status='reviewed',r.review=$review,r.reviewed_at=datetime($at),r.reviewer_id=$reviewer,r.delete_after=datetime($at)+duration({days:$days}) RETURN r.key AS key").param("rk", rk).param("revision", revision).param("reviewer", actor.id.clone()).param("days", REVIEWED_RETENTION_DAYS).param("at", reviewed_at.clone()).param("review", serde_json::to_string(&grade)?)).await?.ok_or("review write missing")?;
    let result = json!({"verse":r.verse,"revision":revision,"status":"reviewed","reviewedAt":reviewed_at,"decision":r.decision});
    audit(&mut tx, actor, &key, json!({"recitation":"review","verse":r.verse,"studentId":r.student_id,"decision":r.decision,"skills":r.skills}), &result).await?;
    tx.commit().await?;
    Ok(Outcome::Done(result))
}
/// Removes clips past their retention date; the record stays as the grade history, without audio.
pub async fn sweep(g: &Graph, store: &Store) -> Result<u32, Error> {
    let mut rows = g.execute(query("MATCH(r:ExtraCurricularRecitation) WHERE r.object_key IS NOT NULL AND r.delete_after<datetime() RETURN r.key AS key,r.object_key AS object LIMIT 100")).await?;
    let mut due = vec![];
    while let Some(row) = rows.next().await? {
        due.push((row.get::<String>("key")?, row.get::<String>("object")?));
    }
    drop(rows);
    let mut removed = 0;
    for (key, object) in due {
        store.delete(&object).await?;
        g.run(query("MATCH(r:ExtraCurricularRecitation {key:$key,object_key:$object}) SET r.object_key=null,r.mime=null,r.purged_at=datetime()").param("key", key).param("object", object)).await?;
        removed += 1;
    }
    Ok(removed)
}
