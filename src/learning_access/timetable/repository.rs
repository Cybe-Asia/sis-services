use super::{
    failure, invalid,
    model::{Action, Change, Meeting},
    validation, Failure,
};
use axum::http::{HeaderMap, StatusCode};
use neo4rs::{query, Graph, Query, Row, Txn};
use serde_json::{json, Value};
#[derive(Clone, Copy)]
pub(crate) enum Family {
    Portal,
    Learning,
}

pub(crate) fn admin() -> String {
    let current = crate::school_portal::staff_capability::authority(
        "a",
        "'school_admin'",
        "$school",
        "$tenant",
    );
    format!("MATCH(a:StaffMember {{id:$actor,membershipStatus:'ACTIVE'}}),(sec:Section {{section_id:$class,school_id:$school,tenant_id:$tenant,status:'active'}}) WHERE {current} AND NOT EXISTS {{MATCH(other:StaffMember {{id:a.id}}) WHERE other<>a}} AND NOT EXISTS {{MATCH(other:Section {{section_id:sec.section_id}}) WHERE other<>sec}} AND EXISTS {{MATCH(:School {{school_id:$school,tenant_id:$tenant}})}} AND NOT EXISTS {{MATCH(one:School {{school_id:$school,tenant_id:$tenant}}),(two:School {{school_id:$school,tenant_id:$tenant}}) WHERE one<>two}}")
}
pub(crate) const TEACHER:&str="MATCH(t:StaffMember {id:$teacher,membershipStatus:'ACTIVE'}) WHERE 'teacher' IN coalesce(t.roles,[]) AND $school IN coalesce(t.schoolIds,[]) AND $tenant IN coalesce(t.tenantIds,[]) AND NOT EXISTS {MATCH(other:StaffMember {id:t.id}) WHERE other<>t} AND (sec.homeroom_staff_member_id=t.id OR EXISTS {MATCH(:LearningTeachingGrant {staff_member_id:t.id,section_id:sec.section_id,school_id:$school,tenant_id:$tenant,status:'ACTIVE'})})";
pub(crate) fn authority(role: &str) -> String {
    if role != "teacher" {
        return admin();
    }
    "MATCH(a:StaffMember {id:$actor,membershipStatus:'ACTIVE'}),(sec:Section {section_id:$class,school_id:$school,tenant_id:$tenant,status:'active'}) WHERE 'teacher' IN coalesce(a.roles,[]) AND NOT 'owner' IN coalesce(a.roles,[]) AND $school IN coalesce(a.schoolIds,[]) AND $tenant IN coalesce(a.tenantIds,[]) AND NOT EXISTS {MATCH(other:StaffMember {id:a.id}) WHERE other<>a} AND NOT EXISTS {MATCH(other:Section {section_id:sec.section_id}) WHERE other<>sec} AND (sec.homeroom_staff_member_id=a.id OR EXISTS {MATCH(:LearningTeachingGrant {staff_member_id:a.id,section_id:sec.section_id,school_id:$school,tenant_id:$tenant,status:'ACTIVE'})})".into()
}
pub(super) fn scoped(body: &str, actor: &str, m: &Meeting) -> Query {
    query(body)
        .param("actor", actor)
        .param("school", m.school_id.clone())
        .param("tenant", m.tenant_id.clone())
        .param("class", m.class_id.clone())
        .param("teacher", m.teacher_id.clone())
        .param("year", m.academic_year.clone())
        .param("key", m.key())
}
pub(crate) async fn one(tx: &mut Txn, q: Query) -> Result<Option<Row>, Failure> {
    let mut rows = tx
        .execute(q)
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let row = rows
        .next(tx.handle())
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if rows
        .next(tx.handle())
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .is_some()
    {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    Ok(row)
}
pub(crate) async fn lock_school(tx: &mut Txn, school: &str, tenant: &str) -> Result<(), Failure> {
    one(tx,query("MERGE(l:EducationCalendarLock {key:$lock}) ON CREATE SET l.version=0 SET l.version=l.version+1 RETURN l.key AS key").param("lock",format!("{tenant}|{school}"))).await?.ok_or_else(||failure(StatusCode::SERVICE_UNAVAILABLE))?;
    Ok(())
}
pub(crate) async fn lock_teacher(tx: &mut Txn, teacher: &str) -> Result<(), Failure> {
    one(tx,query("MERGE(l:LearningTimetableTeacherLock {key:$teacher}) ON CREATE SET l.version=0 SET l.version=l.version+1 RETURN l.key AS key").param("teacher",teacher)).await?.ok_or_else(||failure(StatusCode::SERVICE_UNAVAILABLE))?;
    Ok(())
}
pub(crate) async fn migrate(graph: &Graph) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for q in include_str!("../../../migrations/0004_timetable.cypher")
        .split(';')
        .map(str::trim)
        .filter(|q| !q.is_empty())
    {
        graph.run(query(q)).await?;
    }
    Ok(())
}
async fn schema(graph: &Graph) -> Result<(), Failure> {
    let unavailable = || failure(StatusCode::SERVICE_UNAVAILABLE);
    let mut rows=graph.execute(query("SHOW CONSTRAINTS YIELD name,type,entityType,labelsOrTypes,properties WHERE name IN ['learning_timetable_draft','learning_timetable_operation','learning_timetable_teacher_lock','education_calendar_lock','learning_class_meeting_key'] RETURN name,type,entityType,labelsOrTypes,properties")).await.map_err(|_|unavailable())?;
    let mut found = std::collections::BTreeSet::new();
    while let Some(row) = rows.next().await.map_err(|_| unavailable())? {
        let name: String = row.get("name").map_err(|_| unavailable())?;
        if !constraint_matches(
            &name,
            &row.get::<String>("type").map_err(|_| unavailable())?,
            &row.get::<String>("entityType").map_err(|_| unavailable())?,
            &row.get::<Vec<String>>("labelsOrTypes")
                .map_err(|_| unavailable())?,
            &row.get::<Vec<String>>("properties")
                .map_err(|_| unavailable())?,
        ) || !found.insert(name)
        {
            return Err(unavailable());
        }
    }
    if found.len() != 5 {
        return Err(unavailable());
    }
    Ok(())
}
fn constraint_matches(
    name: &str,
    kind: &str,
    entity: &str,
    labels: &[String],
    properties: &[String],
) -> bool {
    let label = match name {
        "learning_timetable_draft" => "LearningTimetableDraft",
        "learning_timetable_operation" => "LearningTimetableOperation",
        "learning_timetable_teacher_lock" => "LearningTimetableTeacherLock",
        "education_calendar_lock" => "EducationCalendarLock",
        "learning_class_meeting_key" => "LearningClassMeeting",
        _ => return false,
    };
    kind == "UNIQUENESS" && entity == "NODE" && labels == [label] && properties == ["key"]
}

#[cfg(test)]
mod schema_tests {
    use super::*;
    #[test]
    fn same_name_wrong_label_property_type_or_entity_cannot_authorize_mutex() {
        let label = vec!["LearningTimetableTeacherLock".into()];
        let key = vec!["key".into()];
        assert!(constraint_matches(
            "learning_timetable_teacher_lock",
            "UNIQUENESS",
            "NODE",
            &label,
            &key
        ));
        for (kind, entity, labels, properties) in [
            ("EXISTS", "NODE", label.clone(), key.clone()),
            ("UNIQUENESS", "RELATIONSHIP", label.clone(), key.clone()),
            ("UNIQUENESS", "NODE", vec!["Other".into()], key.clone()),
            ("UNIQUENESS", "NODE", label.clone(), vec!["id".into()]),
            (
                "UNIQUENESS",
                "NODE",
                label.clone(),
                vec!["key".into(), "id".into()],
            ),
        ] {
            assert!(!constraint_matches(
                "learning_timetable_teacher_lock",
                kind,
                entity,
                &labels,
                &properties
            ));
        }
    }
}
fn field<T: serde::de::DeserializeOwned>(r: &Row, name: &str) -> Result<T, Failure> {
    r.get(name)
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))
}
pub(super) async fn change(
    graph: &Graph,
    actor: &str,
    role: &str,
    headers: &HeaderMap,
    family: Family,
    input: &Change,
) -> Result<Value, Failure> {
    schema(graph).await?;
    let mut tx = graph
        .start_txn()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let result = change_locked(&mut tx, actor, role, headers, family, input).await;
    match result {
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
async fn change_locked(
    tx: &mut Txn,
    actor: &str,
    role: &str,
    headers: &HeaderMap,
    family: Family,
    input: &Change,
) -> Result<Value, Failure> {
    let m = &input.meeting;
    let denied = || failure(StatusCode::FORBIDDEN);
    let conflict = || failure(StatusCode::CONFLICT);
    if role == "teacher" && (m.teacher_id != actor || !matches!(family, Family::Learning)) {
        return Err(denied());
    }
    // School -> global Teacher mutex -> sorted actual Staff -> Section -> meeting.
    // Calendar writers use the same school-first order; no actor/school inversion.
    lock_school(tx, &m.school_id, &m.tenant_id).await?;
    lock_teacher(tx, &m.teacher_id).await?;
    let mut ids = vec![actor.to_string(), m.teacher_id.clone()];
    ids.sort();
    ids.dedup();
    let count=one(tx,query("UNWIND $ids AS id MATCH(a:StaffMember {id:id,membershipStatus:'ACTIVE'}) WITH a ORDER BY a.id SET a.timetable_write_lock=coalesce(a.timetable_write_lock,0)+1 RETURN count(a) AS count").param("ids",ids.clone())).await?.ok_or_else(denied)?;
    if field::<i64>(&count, "count")? != ids.len() as i64 {
        return Err(denied());
    }
    let current = match family {
        Family::Portal => {
            super::super::administrator_actor(headers, &m.tenant_id, &m.school_id).await?
        }
        Family::Learning => {
            let staff =
                crate::school_portal::auth::teacher_in(headers, &m.school_id, &m.tenant_id).await?;
            let role = staff.role().to_string();
            (staff.staff_member_id, role)
        }
    };
    if current != (actor.to_string(), role.to_string()) {
        return Err(denied());
    }
    let authorized=format!("{} WITH a,sec {TEACHER} WITH a,sec,t WHERE sec.academic_year=$year AND (($role='owner' AND 'owner' IN coalesce(a.roles,[])) OR ($role IN ['teacher','school_admin'] AND NOT 'owner' IN coalesce(a.roles,[]))) SET t.timetable_write_lock=coalesce(t.timetable_write_lock,0)+1,sec.timetable_write_lock=coalesce(sec.timetable_write_lock,0)+1 RETURN timestamp() AS now",authority(role));
    let row = one(tx, scoped(&authorized, actor, m).param("role", role))
        .await?
        .ok_or_else(denied)?;
    let now: u64 = field::<i64>(&row, "now")?
        .try_into()
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let op = format!("{}|{}", m.school_key(), input.operation_id);
    let request = serde_json::to_string(input).map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if let Some(r)=one(tx,query("MATCH(o:LearningTimetableOperation {key:$op}) RETURN o.actor AS actor,o.role AS role,o.request AS request,o.receipt AS receipt").param("op",op.clone())).await? {
        if field::<String>(&r,"actor")?!=actor || field::<String>(&r,"role")?!=role || field::<String>(&r,"request")?!=request{return Err(conflict());}
        return serde_json::from_str(&field::<String>(&r,"receipt")?).map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    let published=one(tx,query("MATCH(p:LearningClassMeeting {key:$key}) SET p.lock=coalesce(p.lock,0)+1 RETURN p.version AS version,p.payload AS payload,coalesce(p.status,'published') AS status").param("key",m.key())).await?;
    let pv = published
        .as_ref()
        .map(|r| field::<i64>(r, "version"))
        .transpose()?
        .unwrap_or(0);
    if pv != i64::from(input.expected_published_version) {
        return Err(conflict());
    }
    let old = published
        .as_ref()
        .map(|r| field::<String>(r, "payload"))
        .transpose()?
        .map(|p| serde_json::from_str::<Value>(&p))
        .transpose()
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if let Some(old) = &old {
        if old["teacherId"].as_str() != Some(m.teacher_id.as_str())
            || old["frozen"] == true
            || old["startsAt"].as_u64().is_none_or(|start| start <= now)
        {
            return Err(conflict());
        }
    }
    let draft=one(tx,query("MATCH(d:LearningTimetableDraft {key:$key}) SET d.lock=coalesce(d.lock,0)+1 RETURN d.version AS version,d.payload AS payload,d.status AS status").param("key",m.key())).await?;
    let version = draft
        .as_ref()
        .map(|r| field::<i64>(r, "version"))
        .transpose()?
        .unwrap_or(0);
    if version != i64::from(input.expected_version) {
        return Err(conflict());
    }
    let mut status = "draft";
    let mut next_version = version + 1;
    let mut next_pv = pv;
    let mut issues = vec![];
    if input.action != Action::SaveDraft {
        let Some(draft) = &draft else {
            return Err(conflict());
        };
        let saved: Meeting = serde_json::from_str(&field::<String>(draft, "payload")?)
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        if saved != *m {
            return Err(conflict());
        }
        let state = field::<String>(draft, "status")?;
        match input.action {
            Action::Validate if state == "draft" => {
                issues = validation::issues(tx, m, now).await?;
                if issues.is_empty() {
                    status = "validated";
                } else {
                    next_version = version;
                }
            }
            Action::Publish if state == "validated" => {
                issues = validation::issues(tx, m, now).await?;
                if !issues.is_empty() {
                    return Err(invalid(issues));
                }
                status = "published";
                next_pv = pv + 1;
                publish(tx, actor, role, m, pv, next_pv).await?;
            }
            Action::Archive if state != "archived" => {
                status = "archived";
                if let Some(mut old) = old {
                    let active = published
                        .as_ref()
                        .map(|r| field::<String>(r, "status"))
                        .transpose()?
                        .as_deref()
                        == Some("published");
                    if active {
                        next_pv = pv + 1;
                        old["version"] = json!(next_pv);
                        one(tx,query("MATCH(p:LearningClassMeeting {key:$key,version:$expected}) SET p.status='archived',p.version=$version,p.payload=$payload RETURN p.key AS key").param("key",m.key()).param("expected",pv).param("version",next_pv).param("payload",old.to_string())).await?.ok_or_else(conflict)?;
                    }
                }
            }
            _ => return Err(conflict()),
        }
    }
    let receipt = json!({"status":status,"version":next_version,"publishedVersion":next_pv,"meeting":m,"issues":issues.iter().map(|code|json!({"code":code})).collect::<Vec<_>>(),"valid":issues.is_empty()});
    if next_version != version {
        one(tx,scoped("MERGE(d:LearningTimetableDraft {key:$key}) SET d.school_id=$school,d.tenant_id=$tenant,d.class_id=$class,d.id=$id,d.teacher_id=$teacher,d.academic_year=$year,d.version=$version,d.published_version=$pv,d.status=$status,d.payload=$payload,d.receipt=$receipt RETURN d.key AS key",actor,m).param("id",m.id.clone()).param("version",next_version).param("pv",next_pv).param("status",status).param("payload",serde_json::to_string(m).map_err(|_|failure(StatusCode::BAD_REQUEST))?).param("receipt",receipt.to_string())).await?.ok_or_else(conflict)?;
    }
    one(tx,scoped("CREATE(o:LearningTimetableOperation {key:$op,actor:$actor,role:$role,request:$request,receipt:$receipt,created_at:timestamp()}) CREATE(:LearningTimetableAudit {id:$audit,school_id:$school,tenant_id:$tenant,class_id:$class,meeting_key:$key,actor_id:$actor,actor_role:$role,assigned_teacher_id:$teacher,operation_id:$op,action:$action,version:$version,published_version:$pv,receipt:$receipt,created_at:timestamp()}) RETURN o.key AS key",actor,m).param("op",op).param("role",role).param("request",request).param("receipt",receipt.to_string()).param("audit",uuid::Uuid::new_v4().to_string()).param("action",serde_json::to_value(input.action).map_err(|_|failure(StatusCode::BAD_REQUEST))?.as_str().unwrap_or("")).param("version",next_version).param("pv",next_pv)).await?.ok_or_else(conflict)?;
    Ok(receipt)
}
pub(super) async fn publish(
    tx: &mut Txn,
    actor: &str,
    role: &str,
    m: &Meeting,
    previous: i64,
    version: i64,
) -> Result<(), Failure> {
    let mut payload = serde_json::to_value(m).map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    payload["version"] = json!(version);
    payload["frozen"] = json!(false);
    let q=format!("{} WITH a,sec {TEACHER} MERGE(p:LearningClassMeeting {{key:$key}}) ON CREATE SET p.version=0 WITH a,sec,t,p WHERE p.version=$expected WITH a,sec,t,p SET p.id=$id,p.school_id=$school,p.tenant_id=$tenant,p.class_id=$class,p.teacher_id=t.id,p.academic_year=$year,p.room_id=$room,p.starts_at=$starts,p.ends_at=$ends,p.version=$version,p.status='published',p.payload=$payload CREATE(:LearningClassMeetingAudit {{id:$audit,meeting_key:$key,actor_id:a.id,actor_role:$role,assigned_teacher_id:t.id,school_id:$school,tenant_id:$tenant,class_id:$class,version:$version,payload:$payload,created_at:timestamp()}}) RETURN p.key AS key",authority(role));
    one(
        tx,
        scoped(&q, actor, m)
            .param("expected", previous)
            .param("id", m.id.clone())
            .param("room", m.room_id.clone())
            .param("starts", m.starts_at as i64)
            .param("ends", m.ends_at as i64)
            .param("version", version)
            .param("payload", payload.to_string())
            .param("role", role)
            .param("audit", uuid::Uuid::new_v4().to_string()),
    )
    .await?
    .ok_or_else(|| failure(StatusCode::FORBIDDEN))?;
    Ok(())
}
