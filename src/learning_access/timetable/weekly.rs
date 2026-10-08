//! Weekly timetable: one set of weekly slots published as dated meetings over a bounded range.
//! Each occurrence goes through the same calendar, time and conflict validation as a single
//! meeting; existing occurrences are kept and invalid ones are reported, never forced.
use super::{
    failure,
    model::Meeting,
    repository::{self, Family},
    validation, Failure,
};
use crate::school_portal::model::identifier;
use axum::http::{HeaderMap, StatusCode};
use chrono::{Datelike, NaiveDate, NaiveTime};
use neo4rs::{query, Graph, Txn};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Longest range one call may publish; callers apply a term month by month.
pub const MAX_DAYS: i64 = 31;
const WIB_MS: i64 = 25_200_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Slot {
    /// Stable slot name within the class timetable, part of every generated meeting id.
    pub key: String,
    /// ISO weekday, Monday = 1 .. Friday = 5.
    pub weekday: u8,
    /// Local school time (WIB) "HH:MM".
    pub start: String,
    pub end: String,
    pub course_id: String,
    pub subject: String,
    pub title: String,
    pub teacher_id: String,
    pub room_id: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Material {
    pub chapter_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lesson_id: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Weekly {
    pub school_id: String,
    pub tenant_id: String,
    pub class_id: String,
    pub academic_year: String,
    /// Inclusive WIB dates "YYYY-MM-DD".
    pub from: String,
    pub to: String,
    pub slots: Vec<Slot>,
    /// Optional per-course material order; the n-th meeting of a course since `sequenceFrom`
    /// teaches entry n (cycling).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub materials: BTreeMap<String, Vec<Material>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_from: Option<String>,
    pub operation_id: String,
}
fn slot_key(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 32
        && v.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn clock(v: &str) -> Option<NaiveTime> {
    (v.len() == 5)
        .then(|| NaiveTime::parse_from_str(v, "%H:%M").ok())
        .flatten()
}
fn date(v: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(v, "%Y-%m-%d").ok()
}
impl Weekly {
    pub fn valid(&self) -> bool {
        let (Some(from), Some(to)) = (date(&self.from), date(&self.to)) else {
            return false;
        };
        let keys: BTreeSet<_> = self.slots.iter().map(|s| s.key.as_str()).collect();
        [&self.school_id, &self.tenant_id, &self.class_id]
            .iter()
            .all(|s| identifier(s))
            && !self.academic_year.trim().is_empty()
            && self.academic_year.len() <= 32
            && !self.academic_year.contains('|')
            && !self.academic_year.chars().any(char::is_control)
            && from <= to
            && (to - from).num_days() < MAX_DAYS
            && self
                .sequence_from
                .as_deref()
                .is_none_or(|d| date(d).is_some_and(|d| d <= from))
            && (1..=60).contains(&self.slots.len())
            && keys.len() == self.slots.len()
            && self.slots.iter().all(|s| {
                let times = clock(&s.start).zip(clock(&s.end));
                slot_key(&s.key)
                    && (1..=5).contains(&s.weekday)
                    && times.is_some_and(|(a, b)| a < b)
                    && [&s.course_id, &s.subject, &s.teacher_id, &s.room_id]
                        .iter()
                        .all(|v| identifier(v))
                    && !s.title.trim().is_empty()
                    && s.title.len() <= 200
            })
            && self.materials.len() <= 60
            && self.materials.iter().all(|(course, list)| {
                identifier(course)
                    && list.len() <= 400
                    && list.iter().all(|m| {
                        identifier(&m.chapter_id) && m.lesson_id.as_deref().is_none_or(identifier)
                    })
            })
            && uuid::Uuid::parse_str(&self.operation_id)
                .is_ok_and(|id| id.hyphenated().to_string() == self.operation_id)
    }
    /// Every school-weekday occurrence in the range, ordered by start; material not yet assigned.
    pub fn occurrences(&self) -> Vec<(String, Meeting)> {
        let (Some(from), Some(to)) = (date(&self.from), date(&self.to)) else {
            return vec![];
        };
        let mut out = vec![];
        for day in from.iter_days().take_while(|d| *d <= to) {
            let weekday = day.weekday().number_from_monday() as u8;
            for s in self.slots.iter().filter(|s| s.weekday == weekday) {
                let at = |t: &str| {
                    clock(t).map(|t| day.and_time(t).and_utc().timestamp_millis() - WIB_MS)
                };
                let (Some(starts), Some(ends)) = (at(&s.start), at(&s.end)) else {
                    continue;
                };
                out.push((
                    s.key.clone(),
                    Meeting {
                        id: format!("wk-{}-{}", s.key, day.format("%Y%m%d")),
                        school_id: self.school_id.clone(),
                        tenant_id: self.tenant_id.clone(),
                        class_id: self.class_id.clone(),
                        teacher_id: s.teacher_id.clone(),
                        course_id: s.course_id.clone(),
                        subject: s.subject.clone(),
                        title: s.title.clone(),
                        starts_at: starts as u64,
                        ends_at: ends as u64,
                        academic_year: self.academic_year.clone(),
                        room_id: s.room_id.to_ascii_lowercase(),
                        chapter_id: None,
                        lesson_id: None,
                    },
                ));
            }
        }
        out.sort_by(|a, b| a.1.starts_at.cmp(&b.1.starts_at).then(a.1.id.cmp(&b.1.id)));
        out
    }
}
/// Assigns material entry n to the n-th meeting of each course, counting the course's earlier
/// published meetings (`earlier[course]`, starts since `sequenceFrom`) before this range.
pub fn assign_materials(
    occurrences: &mut [(String, Meeting)],
    materials: &BTreeMap<String, Vec<Material>>,
    earlier: &BTreeMap<String, Vec<u64>>,
) {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (_, m) in occurrences.iter_mut() {
        let Some(list) = materials.get(&m.course_id).filter(|l| !l.is_empty()) else {
            continue;
        };
        let before = earlier
            .get(&m.course_id)
            .map(|starts| starts.iter().filter(|s| **s < m.starts_at).count())
            .unwrap_or(0);
        let offset = seen.entry(m.course_id.clone()).or_default();
        let entry = &list[(before + *offset) % list.len()];
        *offset += 1;
        m.chapter_id = Some(entry.chapter_id.clone());
        m.lesson_id = entry.lesson_id.clone();
    }
}
fn field<T: serde::de::DeserializeOwned>(r: &neo4rs::Row, name: &str) -> Result<T, Failure> {
    r.get(name)
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))
}
pub(super) async fn apply(
    graph: &Graph,
    actor: &str,
    role: &str,
    headers: &HeaderMap,
    family: Family,
    input: &Weekly,
) -> Result<Value, Failure> {
    let mut tx = graph
        .start_txn()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    match apply_locked(&mut tx, actor, role, headers, family, input).await {
        Ok(value) => {
            tx.commit()
                .await
                .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
            Ok(value)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}
async fn apply_locked(
    tx: &mut Txn,
    actor: &str,
    role: &str,
    headers: &HeaderMap,
    family: Family,
    input: &Weekly,
) -> Result<Value, Failure> {
    let denied = || failure(StatusCode::FORBIDDEN);
    let conflict = || failure(StatusCode::CONFLICT);
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let teachers: BTreeSet<String> = input.slots.iter().map(|s| s.teacher_id.clone()).collect();
    if role == "teacher"
        && (teachers.iter().any(|t| t != actor) || !matches!(family, Family::Learning))
    {
        return Err(denied());
    }
    // Same lock order as single-meeting changes: school -> Teachers (sorted) -> Staff -> Section.
    repository::lock_school(tx, &input.school_id, &input.tenant_id).await?;
    for teacher in &teachers {
        repository::lock_teacher(tx, teacher).await?;
    }
    let mut ids: Vec<String> = teachers.iter().cloned().collect();
    ids.push(actor.to_string());
    ids.sort();
    ids.dedup();
    let count=repository::one(tx,query("UNWIND $ids AS id MATCH(a:StaffMember {id:id,membershipStatus:'ACTIVE'}) WITH a ORDER BY a.id SET a.timetable_write_lock=coalesce(a.timetable_write_lock,0)+1 RETURN count(a) AS count").param("ids",ids.clone())).await?.ok_or_else(denied)?;
    if field::<i64>(&count, "count")? != ids.len() as i64 {
        return Err(denied());
    }
    // Revalidate the caller after taking locks, through its own family (Admin Portal or Learning).
    let current = match family {
        Family::Portal => {
            super::super::administrator_actor(headers, &input.tenant_id, &input.school_id).await?
        }
        Family::Learning => {
            let staff =
                crate::school_portal::auth::teacher_in(headers, &input.school_id, &input.tenant_id)
                    .await?;
            let role = staff.role().to_string();
            (staff.staff_member_id, role)
        }
    };
    if current != (actor.to_string(), role.to_string()) {
        return Err(denied());
    }
    let authorized=format!("{} WITH a,sec WHERE sec.academic_year=$year AND (($role='owner' AND 'owner' IN coalesce(a.roles,[])) OR ($role IN ['teacher','school_admin'] AND NOT 'owner' IN coalesce(a.roles,[]))) SET sec.timetable_write_lock=coalesce(sec.timetable_write_lock,0)+1 RETURN timestamp() AS now",repository::authority(role));
    let row = repository::one(
        tx,
        query(&authorized)
            .param("actor", actor)
            .param("school", input.school_id.clone())
            .param("tenant", input.tenant_id.clone())
            .param("class", input.class_id.clone())
            .param("year", input.academic_year.clone())
            .param("role", role),
    )
    .await?
    .ok_or_else(denied)?;
    let now: u64 = field::<i64>(&row, "now")?
        .try_into()
        .map_err(|_| unavailable())?;
    let op = format!(
        "{}|{}|{}",
        input.tenant_id, input.school_id, input.operation_id
    );
    let request = serde_json::to_string(input).map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if let Some(r)=repository::one(tx,query("MATCH(o:LearningTimetableOperation {key:$op}) RETURN o.actor AS actor,o.role AS role,o.request AS request,o.receipt AS receipt").param("op",op.clone())).await? {
        if field::<String>(&r,"actor")?!=actor || field::<String>(&r,"role")?!=role || field::<String>(&r,"request")?!=request{return Err(conflict());}
        return serde_json::from_str(&field::<String>(&r,"receipt")?).map_err(|_|unavailable());
    }
    let mut occurrences = input.occurrences();
    if !input.materials.is_empty() {
        let since = input.sequence_from.as_deref().unwrap_or(&input.from);
        let (since_ms, _) =
            super::reads::window_ms(since, since).map_err(|_| failure(StatusCode::BAD_REQUEST))?;
        let first = occurrences
            .first()
            .map(|(_, m)| m.starts_at)
            .unwrap_or(since_ms);
        let mut rows=tx.execute(query("MATCH(p:LearningClassMeeting {school_id:$school,tenant_id:$tenant,class_id:$class}) WHERE coalesce(p.status,'published')='published' AND p.starts_at>=$since AND p.starts_at<$first RETURN p.starts_at AS starts,p.payload AS payload LIMIT 5001").param("school",input.school_id.clone()).param("tenant",input.tenant_id.clone()).param("class",input.class_id.clone()).param("since",since_ms as i64).param("first",first as i64)).await.map_err(|_|unavailable())?;
        let mut earlier: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        let mut n = 0;
        while let Some(r) = rows.next(tx.handle()).await.map_err(|_| unavailable())? {
            n += 1;
            if n > 5000 {
                return Err(unavailable());
            }
            let starts: i64 = field(&r, "starts")?;
            let payload: Value = serde_json::from_str(&field::<String>(&r, "payload")?)
                .map_err(|_| unavailable())?;
            let course = payload["courseId"].as_str().ok_or_else(unavailable)?;
            earlier
                .entry(course.to_string())
                .or_default()
                .push(starts as u64);
        }
        assign_materials(&mut occurrences, &input.materials, &earlier);
    }
    let (mut created, mut existing, mut skipped) = (vec![], 0, vec![]);
    for (slot, m) in &occurrences {
        if !m.valid() {
            return Err(failure(StatusCode::BAD_REQUEST));
        }
        if let Some(r)=repository::one(tx,query("MATCH(p:LearningClassMeeting {key:$key}) RETURN coalesce(p.status,'published') AS status").param("key",m.key())).await? {
            if field::<String>(&r,"status")?=="published"{existing+=1;}else{skipped.push(json!({"id":m.id,"slot":slot,"startsAt":m.starts_at,"issues":[{"code":"ARCHIVED"}]}));}
            continue;
        }
        let issues = validation::issues(tx, m, now).await?;
        if !issues.is_empty() {
            skipped.push(json!({"id":m.id,"slot":slot,"startsAt":m.starts_at,"issues":issues.iter().map(|c|json!({"code":c})).collect::<Vec<_>>()}));
            continue;
        }
        repository::publish(tx, actor, role, m, 0, 1).await?;
        created.push(m.id.clone());
    }
    let receipt = json!({"from":input.from,"to":input.to,"created":created.len(),"existing":existing,"skipped":skipped});
    repository::one(tx,query("CREATE(o:LearningTimetableOperation {key:$op,actor:$actor,role:$role,request:$request,receipt:$receipt,created_at:timestamp()}) CREATE(:LearningTimetableAudit {id:$audit,school_id:$school,tenant_id:$tenant,class_id:$class,actor_id:$actor,actor_role:$role,operation_id:$op,action:'weekly_apply',created:$created,receipt:$receipt,created_at:timestamp()}) RETURN o.key AS key").param("op",op).param("actor",actor).param("role",role).param("request",request).param("receipt",receipt.to_string()).param("audit",uuid::Uuid::new_v4().to_string()).param("school",input.school_id.clone()).param("tenant",input.tenant_id.clone()).param("class",input.class_id.clone()).param("created",created)).await?.ok_or_else(conflict)?;
    Ok(receipt)
}
#[cfg(test)]
mod tests;
