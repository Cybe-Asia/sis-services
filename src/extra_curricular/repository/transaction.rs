use super::*;
use crate::extra_curricular::{application, ports::*};
struct GraphChange<'a> {
    tx: Txn,
    actor: &'a Actor,
    input: &'a Change,
    key: String,
    operation: String,
    request: String,
}
impl GraphChange<'_> {
    async fn row(&mut self, q: Query) -> Result<Option<neo4rs::Row>, Error> {
        one(&mut self.tx, q).await
    }
    fn scope(&self, q: &str) -> Query {
        scoped(q, self.actor, &self.input.scope).param("key", self.key.clone())
    }
}
pub async fn change(g: &Graph, a: &Actor, v: &Change) -> Result<Option<Value>, Error> {
    change_inner(g, a, v, None).await
}
pub async fn change_live(
    g: &Graph,
    a: &Actor,
    v: &Change,
    h: &axum::http::HeaderMap,
    path_role: &str,
) -> Result<Option<Value>, Error> {
    change_inner(g, a, v, Some((h, path_role))).await
}
async fn change_inner(
    g: &Graph,
    a: &Actor,
    v: &Change,
    live: Option<(&axum::http::HeaderMap, &str)>,
) -> Result<Option<Value>, Error> {
    let key = activity_key(&v.scope, &v.activity_id);
    let mut store = GraphChange {
        tx: g.start_txn().await?,
        actor: a,
        input: v,
        operation: format!("{}|{}|{}|{}", a.role, a.id, key, v.request_id),
        request: serde_json::to_string(v)?,
        key,
    };
    if a.role == "owner" && live.is_some() {
        store.row(query("MATCH(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) SET actor.owner_write_lock=coalesce(actor.owner_write_lock,0)+1 RETURN actor.id AS id").param("actor",a.id.clone())).await?.ok_or("Owner lock missing")?;
    }
    store.row(query("MERGE(l:ExtraCurricularLock {key:$key}) ON CREATE SET l.version=0 SET l.version=l.version+1 RETURN l.version AS version").param("key",store.key.clone())).await?.ok_or("lock missing")?;
    if matches!(a.role.as_str(), "owner" | "student") {
        if let Some((h, path_role)) = live {
            let current = match crate::extra_curricular::auth::actor(h, path_role, &v.scope).await {
                Ok(current) => current,
                Err(error) => {
                    let _ = store.tx.rollback().await;
                    return Err(Box::new(
                        crate::school_portal::staff_capability::SessionFailure(error),
                    ));
                }
            };
            if current.id != a.id || current.role != a.role {
                let _ = store.tx.rollback().await;
                return Err(Box::new(
                    crate::school_portal::staff_capability::SessionFailure(
                        crate::extra_curricular::failure(axum::http::StatusCode::FORBIDDEN),
                    ),
                ));
            }
        }
    }
    let result = application::execute(&mut store, a, v).await?;
    if result.is_some() {
        store.tx.commit().await?;
    } else {
        store.tx.rollback().await?;
    }
    result
        .map(serde_json::to_value)
        .transpose()
        .map_err(Into::into)
}
impl ChangeStore for GraphChange<'_> {
    type Error = Error;
    async fn authorized(&mut self) -> Result<bool, Error> {
        let q = self.scope(&format!(
            "{} RETURN count(*) AS count",
            authority(self.actor)
        ));
        Ok(self
            .row(q)
            .await?
            .is_some_and(|r| r.get::<i64>("count").unwrap_or(0) == 1))
    }
    async fn current(&mut self) -> Result<Option<StoredActivity>, Error> {
        let mut read = format!(
            "{} MATCH(activity:ExtraCurricularActivity {{key:$key}})",
            authority(self.actor)
        );
        if self.actor.role == "teacher" {
            read.push_str(" WHERE activity.coach_id=actor.id AND activity.archived=false");
        } else if matches!(self.actor.role.as_str(), "parent" | "student") {
            read.push_str(&format!(" WHERE {ELIGIBLE}"));
        }
        read.push_str(if matches!(self.actor.role.as_str(),"parent"|"student"){" RETURN activity.payload AS payload,activity.revision AS revision,activity.archived AS archived,student.studentId AS student"}else{" RETURN activity.payload AS payload,activity.revision AS revision,activity.archived AS archived"});
        let q = self.scope(&read);
        self.row(q)
            .await?
            .map(|r| {
                Ok(StoredActivity {
                    activity: serde_json::from_str(&r.get::<String>("payload")?)?,
                    revision: r.get::<i64>("revision")?.try_into()?,
                    archived: r.get("archived")?,
                    student_id: r.get::<String>("student").ok(),
                })
            })
            .transpose()
    }
    async fn replay(&mut self) -> Result<Replay, Error> {
        let q=query("MATCH(o:ExtraCurricularOperation {key:$op}) RETURN o.request AS request,o.result AS result").param("op",self.operation.clone());
        let Some(r) = self.row(q).await? else {
            return Ok(Replay::None);
        };
        if r.get::<String>("request")? != self.request {
            return Ok(Replay::Conflict);
        }
        Ok(Replay::Match(serde_json::from_str(
            &r.get::<String>("result")?,
        )?))
    }
    async fn catalog_references(&mut self, a: &Activity) -> Result<bool, Error> {
        let q=self.scope("MATCH(coach:StaffMember {id:$coach,membershipStatus:'ACTIVE'}) WHERE 'teacher' IN coalesce(coach.roles,[]) AND NOT EXISTS {MATCH(other:StaffMember {id:coach.id}) WHERE other<>coach} AND $school IN coalesce(coach.schoolIds,[]) AND $tenant IN coalesce(coach.tenantIds,[]) MATCH(sec:Section {school_id:$school,tenant_id:$tenant,academic_year:$year,status:'active'}) WHERE sec.section_id IN $sections RETURN count(DISTINCT sec.section_id) AS count").param("coach",a.coach_id.clone()).param("sections",a.section_ids.clone());
        Ok(self
            .row(q)
            .await?
            .is_some_and(|r| r.get::<i64>("count").unwrap_or(0) == a.section_ids.len() as i64))
    }
    async fn enrollment_count(&mut self, reserved: bool) -> Result<u32, Error> {
        let condition = if reserved {
            "WHERE e.status IN ['enrolled','pending_consent']"
        } else {
            ""
        };
        let q=query(&format!("MATCH(e:ExtraCurricularEnrollment {{activity_key:$key}}) {condition} RETURN count(e) AS count")).param("key",self.key.clone());
        Ok(self
            .row(q)
            .await?
            .ok_or("count missing")?
            .get::<i64>("count")?
            .try_into()?)
    }
    async fn save_catalog(&mut self, a: &Activity, revision: u32) -> Result<(), Error> {
        let q=self.scope("MERGE(activity:ExtraCurricularActivity {key:$key}) SET activity.id=$id,activity.school_id=$school,activity.tenant_id=$tenant,activity.academic_year=$year,activity.revision=$revision,activity.payload=$payload,activity.published=$published,activity.archived=false,activity.coach_id=$coach,activity.section_ids=$sections,activity.capacity=$capacity,activity.parent_consent=$consent RETURN activity.revision AS revision").param("id",a.id.clone()).param("revision",i64::from(revision)).param("payload",serde_json::to_string(a)?).param("published",a.published).param("coach",a.coach_id.clone()).param("sections",a.section_ids.clone()).param("capacity",i64::from(a.capacity)).param("consent",a.parent_consent);
        self.row(q).await?.ok_or("catalog write missing")?;
        Ok(())
    }
    async fn archive(&mut self, revision: u32) -> Result<(), Error> {
        let q=query("MATCH(a:ExtraCurricularActivity {key:$key}) SET a.archived=true,a.published=false,a.revision=$revision RETURN a.revision AS revision").param("key",self.key.clone()).param("revision",i64::from(revision));
        self.row(q).await?.ok_or("archive missing")?;
        Ok(())
    }
    async fn enrollment(&mut self, student: &str) -> Result<Option<Enrollment>, Error> {
        let q = query("MATCH(e:ExtraCurricularEnrollment {key:$ek}) RETURN e.payload AS payload")
            .param("ek", format!("{}|{student}", self.key));
        self.row(q)
            .await?
            .map(|r| Ok(serde_json::from_str(&r.get::<String>("payload")?)?))
            .transpose()
    }
    async fn save_enrollment(&mut self, e: &Enrollment) -> Result<(), Error> {
        let q=query("MERGE(e:ExtraCurricularEnrollment {key:$ek}) SET e.activity_key=$key,e.student_id=$student,e.status=$status,e.revision=$revision,e.payload=$payload,e.updated_at=datetime() RETURN e.revision AS revision").param("ek",format!("{}|{}",self.key,e.student_id)).param("key",self.key.clone()).param("student",e.student_id.clone()).param("status",e.status.clone()).param("revision",i64::from(e.revision)).param("payload",serde_json::to_string(e)?);
        self.row(q).await?.ok_or("enrollment missing")?;
        Ok(())
    }
    async fn roster_authorized(&mut self, ids: &[String]) -> Result<bool, Error> {
        let activity_scope = if self.actor.role == "owner" {
            "MATCH(activity:ExtraCurricularActivity {key:$key})"
        } else {
            "MATCH(activity:ExtraCurricularActivity {key:$key,coach_id:$actor})"
        };
        let q=self.scope(&format!("{} {activity_scope} {ROSTER} WITH DISTINCT student WHERE student.studentId IN $ids RETURN count(student) AS count",authority(self.actor))).param("ids",ids.to_vec());
        Ok(self
            .row(q)
            .await?
            .is_some_and(|r| r.get::<i64>("count").unwrap_or(0) == ids.len() as i64))
    }
    async fn meeting_revision(&mut self, meeting: &str) -> Result<u32, Error> {
        let q =
            query("MATCH(m:ExtraCurricularMeetingRecord {key:$mk}) RETURN m.revision AS revision")
                .param("mk", format!("{}|{meeting}", self.key));
        self.row(q)
            .await?
            .map(|r| Ok(r.get::<i64>("revision")?.try_into()?))
            .transpose()
            .map(|v| v.unwrap_or(0))
    }
    async fn save_attendance(
        &mut self,
        meeting: &str,
        revision: u32,
        marks: &[Mark],
    ) -> Result<(), Error> {
        let mk = format!("{}|{meeting}", self.key);
        for m in marks {
            let payload = json!({"meetingId":meeting,"studentId":m.student_id,"status":m.status,"revision":revision});
            let q=query("MERGE(r:ExtraCurricularAttendance {key:$ak}) SET r.activity_key=$key,r.student_id=$student,r.payload=$payload RETURN r.key AS key").param("ak",format!("{mk}|{}",m.student_id)).param("key",self.key.clone()).param("student",m.student_id.clone()).param("payload",serde_json::to_string(&payload)?);
            self.row(q).await?.ok_or("attendance missing")?;
        }
        let q=query("MERGE(m:ExtraCurricularMeetingRecord {key:$mk}) SET m.activity_key=$key,m.meeting_id=$meeting,m.revision=$revision RETURN m.revision AS revision").param("mk",mk).param("key",self.key.clone()).param("meeting",meeting).param("revision",i64::from(revision));
        self.row(q).await?.ok_or("meeting missing")?;
        Ok(())
    }
    async fn save_result(
        &mut self,
        meeting: &str,
        revision: u32,
        outcome: &str,
        score: Option<&str>,
    ) -> Result<(), Error> {
        let payload = json!({"meetingId":meeting,"outcome":outcome,"score":score,"revision":revision});
        let q=query("MERGE(m:ExtraCurricularMeetingRecord {key:$mk}) SET m.activity_key=$key,m.meeting_id=$meeting,m.revision=$revision,m.result=$payload RETURN m.revision AS revision").param("mk",format!("{}|{meeting}",self.key)).param("key",self.key.clone()).param("meeting",meeting).param("revision",i64::from(revision)).param("payload",serde_json::to_string(&payload)?);
        self.row(q).await?.ok_or("meeting missing")?;
        Ok(())
    }
    async fn outcome(&mut self, student: &str) -> Result<Option<Outcome>, Error> {
        let q = query("MATCH(r:ExtraCurricularOutcome {key:$ok}) RETURN r.payload AS payload")
            .param("ok", format!("{}|{student}", self.key));
        self.row(q)
            .await?
            .map(|r| Ok(serde_json::from_str(&r.get::<String>("payload")?)?))
            .transpose()
    }
    async fn save_outcome(&mut self, o: &Outcome) -> Result<(), Error> {
        let statement = if o.released {
            "MERGE(r:ExtraCurricularOutcome {key:$ok}) SET r.activity_key=$key,r.student_id=$student,r.revision=$revision,r.payload=$payload,r.released_payload=$payload,r.released_at=datetime() RETURN r.revision AS revision"
        } else {
            "MERGE(r:ExtraCurricularOutcome {key:$ok}) SET r.activity_key=$key,r.student_id=$student,r.revision=$revision,r.payload=$payload RETURN r.revision AS revision"
        };
        let q = query(statement)
            .param("ok", format!("{}|{}", self.key, o.student_id))
            .param("key", self.key.clone())
            .param("student", o.student_id.clone())
            .param("revision", i64::from(o.revision))
            .param("payload", serde_json::to_string(o)?);
        self.row(q).await?.ok_or("outcome missing")?;
        Ok(())
    }
    async fn audit(&mut self, r: &Receipt) -> Result<(), Error> {
        let q=query("CREATE(o:ExtraCurricularOperation {key:$op,request:$request,result:$result,created_at:datetime()}) CREATE(:ExtraCurricularAudit {key:$op,actor_id:$actor,role:$role,activity_key:$key,request:$request,result:$result,created_at:datetime()}) RETURN o.key AS key").param("op",self.operation.clone()).param("request",self.request.clone()).param("result",serde_json::to_string(r)?).param("actor",self.actor.id.clone()).param("role",self.actor.role.clone()).param("key",self.key.clone());
        self.row(q).await?.ok_or("audit missing")?;
        Ok(())
    }
}
