use super::{
    auth::{authority, Actor, ELIGIBLE, ROSTER},
    model::*,
};
use neo4rs::{query, Graph, Query, Txn};
use serde_json::{json, Value};
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub fn scoped(q: &str, a: &Actor, s: &Scope) -> Query {
    query(q)
        .param("actor", a.id.clone())
        .param("sub", a.id.clone())
        .param("school", s.school_id.clone())
        .param("tenant", s.tenant_id.clone())
        .param("year", s.academic_year.clone())
        .param("student", s.student_id.clone().unwrap_or_default())
}
pub fn activity_key(s: &Scope, id: &str) -> String {
    format!("{}|{}|{}|{}", s.tenant_id, s.school_id, s.academic_year, id)
}
pub async fn init(g: &Graph) -> Result<(), Error> {
    for (name, label) in [
        ("extra_activity", "ExtraCurricularActivity"),
        ("extra_lock", "ExtraCurricularLock"),
        ("extra_enrollment", "ExtraCurricularEnrollment"),
        ("extra_operation", "ExtraCurricularOperation"),
        ("extra_attendance", "ExtraCurricularAttendance"),
        ("extra_outcome", "ExtraCurricularOutcome"),
        ("extra_audit", "ExtraCurricularAudit"),
        ("extra_meeting", "ExtraCurricularMeetingRecord"),
    ] {
        g.run(query(&format!(
            "CREATE CONSTRAINT {name} IF NOT EXISTS FOR(n:{label}) REQUIRE n.key IS UNIQUE"
        )))
        .await?;
    }
    Ok(())
}
async fn one(tx: &mut Txn, q: Query) -> Result<Option<neo4rs::Row>, Error> {
    let mut r = tx.execute(q).await?;
    let result = r.next(tx.handle()).await?;
    if r.next(tx.handle()).await?.is_some() {
        return Err("ambiguous owner".into());
    }
    Ok(result)
}
pub async fn list(g: &Graph, a: &Actor, s: &Scope) -> Result<Option<Value>, Error> {
    let prefix = authority(a);
    let mut owned = g
        .execute(scoped(&format!("{prefix} RETURN count(*) AS count"), a, s))
        .await?;
    if owned
        .next()
        .await?
        .is_none_or(|r| r.get::<i64>("count").unwrap_or(0) != 1)
    {
        return Ok(None);
    }
    let filter=match a.role.as_str(){"admin"|"owner"=>"","teacher"=>" AND activity.coach_id=actor.id",_=>" AND ((activity.published=true AND activity.archived=false) OR EXISTS { MATCH(e:ExtraCurricularEnrollment {key:activity.key+'|'+student.studentId}) })"};
    let family = matches!(a.role.as_str(), "parent" | "student");
    let body = if family {
        format!("WITH actor,student,activity WHERE {ELIGIBLE} OPTIONAL MATCH(enrollment:ExtraCurricularEnrollment {{key:activity.key+'|'+student.studentId}}) RETURN DISTINCT activity.payload AS payload,activity.revision AS revision,activity.archived AS archived,activity.id AS id,student.studentId AS student,enrollment.payload AS enrollment")
    } else {
        "RETURN DISTINCT activity.payload AS payload,activity.revision AS revision,activity.archived AS archived,activity.id AS id".into()
    };
    let mut rows=g.execute(scoped(&format!("{prefix} MATCH(activity:ExtraCurricularActivity {{school_id:$school,tenant_id:$tenant,academic_year:$year}}) WHERE activity.id>$after{filter} {body} ORDER BY id LIMIT 101"),a,s).param("after",s.after.clone().unwrap_or_default())).await?;
    let mut items = vec![];
    let mut next = None;
    while let Some(row) = rows.next().await? {
        if items.len() == 100 {
            next = items
                .last()
                .and_then(|v: &Value| v["activity"]["id"].as_str().map(String::from));
            break;
        }
        let raw: String = row.get("payload")?;
        if raw.len() > 65536 {
            return Err("activity too large".into());
        }
        let activity: Activity = serde_json::from_str(&raw)?;
        if !activity.valid() {
            return Err("invalid stored activity".into());
        }
        let key = activity_key(s, &activity.id);
        let mut counts=g.execute(query("MATCH(e:ExtraCurricularEnrollment {activity_key:$key}) WHERE e.status IN ['enrolled','pending_consent'] RETURN count(e) AS count, sum(CASE WHEN e.status='enrolled' THEN 1 ELSE 0 END) AS enrolled_count").param("key",key.clone())).await?;
        let count_row = counts.next().await?.ok_or("count missing")?;
        let count = count_row.get::<i64>("count")?;
        let enrolled_count = count_row.get::<i64>("enrolled_count")?;
        let mut staff=g.execute(query("MATCH(t:StaffMember {id:$coach}) RETURN coalesce(t.fullName,t.name,t.id) AS name").param("coach",activity.coach_id.clone())).await?;
        let coach_name = staff
            .next()
            .await?
            .ok_or("coach missing")?
            .get::<String>("name")?;
        let mut item = json!({"activity":activity,"revision":row.get::<i64>("revision")?,"archived":row.get::<bool>("archived")?,"occupied":count,"enrolledCount":enrolled_count,"coachName":coach_name});
        // Match results are team records, visible to everyone who can see the activity.
        let mut recorded=g.execute(query("MATCH(m:ExtraCurricularMeetingRecord {activity_key:$key}) WHERE m.result IS NOT NULL RETURN m.meeting_id AS id,m.result AS result ORDER BY id LIMIT 101").param("key",key.clone())).await?;
        let mut results = serde_json::Map::new();
        while let Some(r) = recorded.next().await? {
            if results.len() == 100 {
                return Err("result limit".into());
            }
            let value: Value = serde_json::from_str(&r.get::<String>("result")?)?;
            results.insert(r.get::<String>("id")?, json!({"outcome":value["outcome"],"score":value["score"]}));
        }
        item["meetingResults"] = json!(results);
        if family {
            let student: String = row.get("student")?;
            item["studentId"] = json!(student);
            item["enrollment"] = row
                .get::<String>("enrollment")
                .ok()
                .map(|v| serde_json::from_str::<Value>(&v))
                .transpose()?
                .unwrap_or(Value::Null);
            item["attendance"] =
                records(g, "ExtraCurricularAttendance", &key, &student, false).await?;
            item["outcome"] = records(g, "ExtraCurricularOutcome", &key, &student, true)
                .await?
                .as_array()
                .and_then(|v| v.first())
                .cloned()
                .unwrap_or(Value::Null);
        }
        if matches!(a.role.as_str(), "teacher" | "owner") {
            let activity_scope = if a.role == "owner" {
                "MATCH(activity:ExtraCurricularActivity {key:$key})"
            } else {
                "MATCH(activity:ExtraCurricularActivity {key:$key,coach_id:$actor})"
            };
            let mut roster=g.execute(scoped(&format!("{} {activity_scope} {ROSTER} RETURN DISTINCT student.studentId AS id,student.fullName AS name,enrollment.payload AS enrollment ORDER BY id LIMIT 201",authority(a)),a,s).param("key",key.clone())).await?;
            let mut entries = vec![];
            while let Some(r) = roster.next().await? {
                if entries.len() == 200 {
                    return Err("roster too large".into());
                }
                let id: String = r.get("id")?;
                entries.push(json!({"id":id,"name":r.get::<String>("name")?,"enrollment":serde_json::from_str::<Value>(&r.get::<String>("enrollment")?)?,"attendance":records(g,"ExtraCurricularAttendance",&key,&id,false).await?,"outcome":records(g,"ExtraCurricularOutcome",&key,&id,false).await?}));
            }
            item["roster"] = json!(entries);
            let mut ms=g.execute(query("MATCH(m:ExtraCurricularMeetingRecord {activity_key:$key}) RETURN m.meeting_id AS id,m.revision AS revision").param("key",key.clone())).await?;
            let mut versions = serde_json::Map::new();
            while let Some(m) = ms.next().await? {
                versions.insert(m.get::<String>("id")?, json!(m.get::<i64>("revision")?));
            }
            item["meetingRevisions"] = json!(versions);
        }
        items.push(item);
    }
    let out = json!({"contractVersion":1,"items":items,"nextCursor":next});
    if serde_json::to_vec(&out)?.len() > 2_000_000 {
        return Err("response too large".into());
    }
    Ok(Some(out))
}
async fn records(
    g: &Graph,
    label: &str,
    key: &str,
    student: &str,
    released: bool,
) -> Result<Value, Error> {
    let field = if released {
        "released_payload"
    } else {
        "payload"
    };
    let mut rows=g.execute(query(&format!("MATCH(r:{label} {{activity_key:$key,student_id:$student}}) WHERE r.{field} IS NOT NULL RETURN r.{field} AS payload,CASE WHEN r.released_at IS NULL THEN null ELSE toString(date(r.released_at)) END AS released_date ORDER BY r.key LIMIT 101")).param("key",key).param("student",student)).await?;
    let mut items = vec![];
    while let Some(r) = rows.next().await? {
        if items.len() == 100 {
            return Err("record limit".into());
        }
        let raw: String = r.get("payload")?;
        if raw.len() > 65536 {
            return Err("record too large".into());
        }
        let mut value: Value = serde_json::from_str(&raw)?;
        if value["released"] == true {
            if let Ok(date) = r.get::<String>("released_date") {
                value["releasedDate"] = json!(date);
            }
        }
        items.push(value);
    }
    Ok(json!(items))
}

mod transaction;
pub use transaction::{change, change_live};

mod context;
pub struct GraphContext<'a>(pub &'a Graph);
impl super::ports::ContextReader for GraphContext<'_> {
    type Error = Error;
    async fn read(
        &self,
        a: &Actor,
        student: Option<String>,
        schools: Vec<String>,
        tenants: Vec<String>,
    ) -> Result<super::ports::CatalogContext, Error> {
        context::load(self.0, a, student, schools, tenants).await
    }
}
