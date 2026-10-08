//! Canonical class timetable; Prework references this independently persisted owner.
use super::*;
use axum::{
    extract::{rejection::QueryRejection, DefaultBodyLimit},
    routing::get,
};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Scope {
    school_id: String,
    tenant_id: String,
    class_id: String,
    id: Option<String>,
    /// Optional [from, to) window on `startsAt` (ms); classes with a whole-year timetable read by window.
    from: Option<u64>,
    to: Option<u64>,
}
/// Widest read window: an academic year plus margins.
const MAX_WINDOW_MS: u64 = 400 * 86_400_000;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    school_id: String,
    tenant_id: String,
    class_id: String,
    id: String,
    expected_version: u32,
    teacher_id: Option<String>,
    course_id: String,
    subject: String,
    title: String,
    starts_at: u64,
    ends_at: u64,
    freeze: bool,
}
fn valid(s: &Scope) -> bool {
    valid_id(&s.school_id)
        && valid_id(&s.tenant_id)
        && valid_id(&s.class_id)
        && s.id.as_deref().is_none_or(valid_id)
        && match (s.from, s.to) {
            (None, None) => true,
            (Some(from), Some(to)) => from < to && to - from <= MAX_WINDOW_MS && to <= 4_102_444_800_000,
            _ => false,
        }
}
fn key(s: &Scope, id: &str) -> String {
    format!(
        "{}:{}{}:{}{}:{}{}:{}",
        s.school_id.len(),
        s.school_id,
        s.tenant_id.len(),
        s.tenant_id,
        s.class_id.len(),
        s.class_id,
        id.len(),
        id
    )
}
fn authority() -> String {
    let owner = crate::school_portal::staff_capability::owner("actor", "$school", "$tenant");
    format!("MATCH (actor:StaffMember {{id:$actor,membershipStatus:'ACTIVE'}}),(sec:Section {{section_id:$class,status:'active',school_id:$school,tenant_id:$tenant}}) WHERE ($role='owner' AND {owner}) OR ($role='teacher' AND NOT 'owner' IN coalesce(actor.roles,[]) AND 'teacher' IN coalesce(actor.roles,[]) AND $school IN coalesce(actor.schoolIds,[]) AND $tenant IN coalesce(actor.tenantIds,[]) AND (sec.homeroom_staff_member_id=actor.id OR EXISTS {{MATCH (:LearningTeachingGrant {{staff_member_id:actor.id,section_id:$class,status:'ACTIVE',school_id:$school,tenant_id:$tenant}})}}))")
}
const ASSIGNED: &str = "MATCH (teacher:StaffMember {id:$teacher,membershipStatus:'ACTIVE'}) WHERE 'teacher' IN coalesce(teacher.roles,[]) AND $school IN coalesce(teacher.schoolIds,[]) AND $tenant IN coalesce(teacher.tenantIds,[]) AND NOT EXISTS {MATCH(other:StaffMember {id:teacher.id}) WHERE other<>teacher} AND (sec.homeroom_staff_member_id=teacher.id OR EXISTS {MATCH (:LearningTeachingGrant {staff_member_id:teacher.id,section_id:$class,status:'ACTIVE',school_id:$school,tenant_id:$tenant})})";
fn assigned_teacher<'a>(
    role: &str,
    actor: &'a str,
    requested: Option<&'a str>,
    old: Option<&'a Value>,
) -> Result<&'a str, Failure> {
    let assigned = match old {
        Some(o) => o["teacherId"]
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))?,
        None if role == "owner" => requested.ok_or_else(|| failure(StatusCode::BAD_REQUEST))?,
        None => actor,
    };
    if (role == "teacher" && assigned != actor) || requested.is_some_and(|id| id != assigned) {
        return Err(failure(StatusCode::CONFLICT));
    }
    Ok(assigned)
}
pub(super) async fn migrate(
    graph: &neo4rs::Graph,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    graph.run(query("CREATE CONSTRAINT learning_class_meeting_key IF NOT EXISTS FOR (m:LearningClassMeeting) REQUIRE m.key IS UNIQUE")).await?;
    super::timetable::repository::migrate(graph).await?;
    Ok(())
}
/// Published meetings of one class with read-time display names (teacher, active catalogue room).
pub(crate) const READ: &str = "MATCH (m:LearningClassMeeting {school_id:$school,tenant_id:$tenant,class_id:$class}) WHERE coalesce(m.status,'published')='published' AND ($id='' OR m.id=$id) AND ($from IS NULL OR (m.starts_at>=$from AND m.starts_at<$to)) OPTIONAL MATCH (t:StaffMember {id:m.teacher_id}) OPTIONAL MATCH (room:SchoolRoom {key:m.tenant_id+'|'+m.school_id+'|'+toLower(coalesce(m.room_id,'')),status:'active'}) RETURN m.payload AS payload,t.displayName AS teacher_name,room.name AS room_name ORDER BY m.starts_at,m.id LIMIT 501";
fn view(r: neo4rs::Row) -> Result<Value, Failure> {
    let payload: String = r
        .get("payload")
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut value: Value =
        serde_json::from_str(&payload).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    // Read-time display names for the schedule card; never persisted in the meeting payload.
    if let Ok(Some(name)) = r.get::<Option<String>>("teacher_name") {
        let name = name.trim();
        if !name.is_empty() && name.len() <= 256 {
            value["teacherName"] = json!(name);
        }
    }
    if let Ok(Some(name)) = r.get::<Option<String>>("room_name") {
        let name = name.trim();
        if !name.is_empty() && name.len() <= 120 {
            value["roomName"] = json!(name);
        }
    }
    Ok(value)
}
async fn read(
    State(state): State<AppState>,
    q: Result<Query<Scope>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Json<Value>, Failure> {
    let Query(s) = q.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !valid(&s) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let _ = owner::class_actor(&state, &headers, &s.school_id, &s.tenant_id, &s.class_id).await?;
    let mut rows=state.graph.execute(query(READ).param("school",s.school_id.clone()).param("tenant",s.tenant_id.clone()).param("class",s.class_id.clone()).param("id",s.id.clone().unwrap_or_default()).param("from",s.from.map(|v| v as i64)).param("to",s.to.map(|v| v as i64))).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut items = vec![];
    while let Some(r) = rows
        .next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        if items.len() >= 500 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        items.push(view(r)?);
    }
    let _ = owner::class_actor(&state, &headers, &s.school_id, &s.tenant_id, &s.class_id).await?;
    Ok(Json(
        json!({"data":{"schoolId":s.school_id,"tenantId":s.tenant_id,"classId":s.class_id,"items":items}}),
    ))
}
async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Change>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(c) = body.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    let s = Scope {
        school_id: c.school_id,
        tenant_id: c.tenant_id,
        class_id: c.class_id,
        id: Some(c.id.clone()),
        from: None,
        to: None,
    };
    if !valid(&s)
        || !valid_id(&c.course_id)
        || !valid_id(&c.subject)
        || c.teacher_id.as_deref().is_some_and(|id| !valid_id(id))
        || c.title.trim().is_empty()
        || c.title.len() > 200
        || c.expected_version >= i32::MAX as u32
        || c.starts_at >= c.ends_at
        || c.ends_at - c.starts_at > 12 * 3600_000
        || c.starts_at > 4_102_444_800_000
        || !matches!(((c.starts_at + 25_200_000) / 86_400_000 + 3) % 7, 0..=4)
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (actor, role) =
        owner::class_actor(&state, &headers, &s.school_id, &s.tenant_id, &s.class_id).await?;
    if !matches!(role.as_str(), "teacher" | "owner") {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    if !c.freeze {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error":{"code":"DRAFT_REQUIRED"}})),
        ));
    }
    let meeting_key = key(&s, &c.id);
    let mut existing=state.graph.execute(query("MATCH(m:LearningClassMeeting {key:$key}) WHERE coalesce(m.status,'published')='published' RETURN m.teacher_id AS teacher").param("key",meeting_key.clone())).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let teacher = existing
        .next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .and_then(|r| r.get::<String>("teacher").ok())
        .filter(|id| valid_id(id))
        .ok_or_else(|| failure(StatusCode::CONFLICT))?;
    let mut tx = state
        .graph
        .start_txn()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let result = async {
        super::timetable::repository::lock_school(&mut tx,&s.school_id,&s.tenant_id).await?;
        super::timetable::repository::lock_teacher(&mut tx,&teacher).await?;
        let mut rows = tx.execute(query(&format!("{} MATCH (m:LearningClassMeeting {{key:$key}}) WHERE coalesce(m.status,'published')='published' SET m.lock=coalesce(m.lock,0)+1 RETURN m.version AS version,m.payload AS payload,m.teacher_id AS teacher,timestamp() AS now",authority()))
            .param("actor",actor.clone()).param("role",role.clone()).param("school",s.school_id.clone()).param("tenant",s.tenant_id.clone()).param("class",s.class_id.clone()).param("key",meeting_key.clone())).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let r=rows.next(&mut tx).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?.ok_or_else(||failure(StatusCode::FORBIDDEN))?;
        let version:i64=r.get("version").map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let now:i64=r.get("now").map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let persisted_teacher:Option<String>=r.get("teacher").map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let old=r.get::<Option<String>>("payload").map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?.map(|p|serde_json::from_str::<Value>(&p)).transpose().map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
        if version!=i64::from(c.expected_version)||old.as_ref().is_some_and(|o|o["frozen"]==true)||c.starts_at<=now as u64{return Err(failure(StatusCode::CONFLICT));}
        if c.freeze&&old.as_ref().is_none_or(|o|o["courseId"].as_str()!=Some(c.course_id.as_str())||o["subject"].as_str()!=Some(c.subject.as_str())||o["startsAt"].as_u64()!=Some(c.starts_at)||o["endsAt"].as_u64()!=Some(c.ends_at)||o["title"].as_str()!=Some(c.title.as_str())){return Err(failure(StatusCode::CONFLICT));}
        let teacher=assigned_teacher(&role,&actor,c.teacher_id.as_deref(),old.as_ref())?.to_string();
        if old.is_some() && persisted_teacher.as_deref()!=Some(teacher.as_str()) {return Err(failure(StatusCode::CONFLICT));}
        let mut saved=old.clone().ok_or_else(||failure(StatusCode::CONFLICT))?;
        saved["version"]=json!(version+1);saved["frozen"]=json!(true);
        // A queued writer must pass live Auth again, then rebind actor and actual
        // assigned Teacher from the current directory while holding the meeting lock.
        let current=owner::class_actor(&state,&headers,&s.school_id,&s.tenant_id,&s.class_id).await?;
        if current!=(actor.clone(),role.clone()){return Err(failure(StatusCode::FORBIDDEN));}
        let mut saved_rows=tx.execute(query(&format!("{} WITH actor,sec {ASSIGNED} MATCH (m:LearningClassMeeting {{key:$key}}) WHERE m.version=$expected SET m.school_id=$school,m.tenant_id=$tenant,m.class_id=$class,m.id=$id,m.teacher_id=teacher.id,m.version=$version,m.starts_at=$starts,m.payload=$payload CREATE (:LearningClassMeetingAudit {{id:$audit,meeting_key:$key,actor_id:actor.id,actor_role:$role,assigned_teacher_id:teacher.id,school_id:$school,tenant_id:$tenant,class_id:$class,version:$version,payload:$payload,created_at:timestamp()}}) RETURN m.version AS version",authority()))
            .param("key",meeting_key).param("school",s.school_id.clone()).param("tenant",s.tenant_id.clone()).param("class",s.class_id.clone()).param("id",c.id).param("expected",version).param("version",version+1).param("starts",c.starts_at as i64).param("payload",saved.to_string()).param("actor",actor).param("role",role).param("teacher",teacher).param("audit",uuid::Uuid::new_v4().to_string())).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
        if saved_rows.next(&mut tx).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?.is_none(){return Err(failure(StatusCode::FORBIDDEN));}
        Ok(saved)
    }.await;
    match result {
        Ok(saved) => {
            tx.commit()
                .await
                .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
            Ok(Json(json!({"data":saved})))
        }
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/learning/class-meetings",
            get(read).post(update),
        )
        .layer(DefaultBodyLimit::max(8192))
}

#[cfg(test)]
mod owner_tests {
    use super::*;
    #[test]
    fn owner_meeting_teacher_selection_and_edit_preserve_assignment() {
        assert_eq!(
            assigned_teacher("owner", "real-owner", None, None)
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            assigned_teacher("owner", "real-owner", Some("assigned-teacher"), None).unwrap(),
            "assigned-teacher"
        );
        let old = json!({"teacherId":"assigned-teacher"});
        assert_eq!(
            assigned_teacher("owner", "real-owner", None, Some(&old)).unwrap(),
            "assigned-teacher"
        );
        assert_eq!(
            assigned_teacher("owner", "real-owner", Some("other-teacher"), Some(&old))
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            assigned_teacher("teacher", "assigned-teacher", None, None).unwrap(),
            "assigned-teacher"
        );
        assert!(
            assigned_teacher("teacher", "assigned-teacher", Some("other-teacher"), None).is_err()
        );
        assert!(assigned_teacher("teacher", "other-teacher", None, Some(&old)).is_err());
    }
    #[tokio::test]
    #[ignore = "requires existing isolated3385 and serialized env; synthetic membership adapter only"]
    async fn owner_meeting_preserves_teacher_freeze_cas_audit_and_queued_auth() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct EnvRestore(Vec<(&'static str, Option<String>)>);
        impl Drop for EnvRestore {
            fn drop(&mut self) {
                for (k, v) in &self.0 {
                    if let Some(v) = v {
                        std::env::set_var(k, v);
                    } else {
                        std::env::remove_var(k);
                    }
                }
            }
        }
        let restore = EnvRestore(
            ["APP_ENV", "AUTH_SCHOOL_MEMBERSHIP_URL"]
                .into_iter()
                .map(|k| (k, std::env::var(k).ok()))
                .collect(),
        );
        let uri = std::env::var("SIS_OWNER_TEST_NEO4J_URI").unwrap();
        assert!(matches!(
            uri.as_str(),
            "bolt://127.0.0.1:3385" | "127.0.0.1:3385"
        ));
        let graph = neo4rs::Graph::new(
            uri,
            std::env::var("SIS_OWNER_TEST_NEO4J_USER").unwrap(),
            std::env::var("SIS_OWNER_TEST_NEO4J_PASSWORD").unwrap(),
        )
        .await
        .unwrap();
        super::super::migrate(&graph).await.unwrap();
        let mark = format!("owner-meeting-{}", uuid::Uuid::new_v4());
        let actor = format!("{mark}-owner");
        let teacher = format!("{mark}-teacher");
        let other = format!("{mark}-other");
        let school = format!("{mark}-school");
        let tenant = format!("{mark}-tenant");
        let class = format!("{mark}-class");
        graph.run(query("CREATE(:School {school_id:$school,tenant_id:$tenant,owner_parity_tag:$mark}) CREATE(:StaffMember {id:$owner,membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[],teamIds:[],owner_parity_tag:$mark}) CREATE(:StaffMember {id:$teacher,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],owner_parity_tag:$mark}) CREATE(:StaffMember {id:$other,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],owner_parity_tag:$mark}) CREATE(:Section {section_id:$class,name:'Synthetic meeting class',status:'active',school_id:$school,tenant_id:$tenant,homeroom_staff_member_id:$teacher,owner_parity_tag:$mark})").param("mark",mark.clone()).param("owner",actor.clone()).param("teacher",teacher.clone()).param("other",other.clone()).param("school",school.clone()).param("tenant",tenant.clone()).param("class",class.clone())).await.unwrap();
        let mode = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app=Router::new().route("/api/v1/auth-service/oauth/membership",axum::routing::post({let mode=mode.clone();let calls=calls.clone();let actor=actor.clone();let teacher=teacher.clone();let school=school.clone();let tenant=tenant.clone();move|h:HeaderMap|{let mode=mode.clone();let calls=calls.clone();let actor=actor.clone();let teacher=teacher.clone();let school=school.clone();let tenant=tenant.clone();async move{
            calls.fetch_add(1,Ordering::SeqCst);if mode.load(Ordering::SeqCst)==1{return(StatusCode::UNAUTHORIZED,Json(json!({"error":"expired synthetic session"})));}
            if h.get("authorization").unwrap().to_str().unwrap().ends_with(".teacher"){(StatusCode::OK,Json(json!({"active":true,"staffMemberId":teacher,"roles":["teacher"],"schoolIds":[school],"tenantIds":[tenant]})))}else{(StatusCode::OK,Json(json!({"active":true,"staffMemberId":actor,"roles":["owner"],"schoolIds":[],"tenantIds":[]})))}
        }}}));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        std::env::set_var("APP_ENV", "test");
        std::env::set_var(
            "AUTH_SCHOOL_MEMBERSHIP_URL",
            format!("http://{address}/api/v1/auth-service/oauth/membership"),
        );
        let headers = |role: &str| {
            let mut h = HeaderMap::new();
            h.insert(
                "authorization",
                format!(
                    "Bearer {}.{}.{role}",
                    URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#),
                    URL_SAFE_NO_PAD.encode(r#"{"aud":"learning-api"}"#)
                )
                .parse()
                .unwrap(),
            );
            h
        };
        let state = AppState {
            graph: graph.clone(),
            config: crate::config::config::AppConfig {
                server_port: 0,
                neo4j_uri: "127.0.0.1:3385".into(),
                neo4j_user: String::new(),
                neo4j_password: String::new(),
                jwt_secret: String::new(),
            },
            http_client: reqwest::Client::new(),
        };
        let mut starts = chrono::Utc::now().timestamp_millis() as u64 + 48 * 3600_000;
        while ((starts + 25_200_000) / 86_400_000 + 3) % 7 > 4 {
            starts += 86_400_000;
        }
        let change =
            |id: &str, version: u32, assigned: Option<String>, title: &str, freeze: bool| Change {
                school_id: school.clone(),
                tenant_id: tenant.clone(),
                class_id: class.clone(),
                id: id.into(),
                expected_version: version,
                teacher_id: assigned,
                course_id: "course".into(),
                subject: "science".into(),
                title: title.into(),
                starts_at: starts,
                ends_at: starts + 3600_000,
                freeze,
            };
        assert_eq!(
            update(
                State(state.clone()),
                headers("owner"),
                Ok(Json(change("meeting", 0, None, "Original", false)))
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::BAD_REQUEST
        );
        for invalid in [&actor, &other] {
            assert_eq!(
                update(
                    State(state.clone()),
                    headers("owner"),
                    Ok(Json(change(
                        "meeting",
                        0,
                        Some(invalid.clone()),
                        "Original",
                        false
                    )))
                )
                .await
                .unwrap_err()
                .0,
                StatusCode::FORBIDDEN
            );
        }
        let saved = update(
            State(state.clone()),
            headers("owner"),
            Ok(Json(change(
                "meeting",
                0,
                Some(teacher.clone()),
                "Original",
                false,
            ))),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(saved["data"]["teacherId"], teacher);
        assert_eq!(saved["data"]["version"], 1);
        assert_eq!(
            update(
                State(state.clone()),
                headers("owner"),
                Ok(Json(change(
                    "meeting",
                    1,
                    Some(other.clone()),
                    "Changed",
                    false
                )))
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::CONFLICT
        );
        let saved = update(
            State(state.clone()),
            headers("owner"),
            Ok(Json(change("meeting", 1, None, "Changed", false))),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(saved["data"]["teacherId"], teacher);
        assert_eq!(saved["data"]["version"], 2);
        assert_eq!(
            update(
                State(state.clone()),
                headers("owner"),
                Ok(Json(change("meeting", 1, None, "Changed", false)))
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::CONFLICT
        );
        let saved = update(
            State(state.clone()),
            headers("owner"),
            Ok(Json(change("meeting", 2, None, "Changed", true))),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(saved["data"]["frozen"], true);
        assert_eq!(
            update(
                State(state.clone()),
                headers("owner"),
                Ok(Json(change("meeting", 3, None, "Changed", false)))
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            update(
                State(state.clone()),
                headers("teacher"),
                Ok(Json(change(
                    "teacher-meeting",
                    0,
                    Some(other),
                    "Teacher meeting",
                    false
                )))
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::CONFLICT
        );
        let saved = update(
            State(state.clone()),
            headers("teacher"),
            Ok(Json(change(
                "teacher-meeting",
                0,
                None,
                "Teacher meeting",
                false,
            ))),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(saved["data"]["teacherId"], teacher);
        let scope = Scope {
            school_id: school.clone(),
            tenant_id: tenant.clone(),
            class_id: class.clone(),
            id: None,
            from: None,
            to: None,
        };
        let teacher_key = key(&scope, "teacher-meeting");
        let mut held = graph.start_txn().await.unwrap();
        held.run(
            query("MATCH(m:LearningClassMeeting {key:$key}) SET m.lock=coalesce(m.lock,0)+1")
                .param("key", teacher_key.clone()),
        )
        .await
        .unwrap();
        calls.store(0, Ordering::SeqCst);
        let c = change("teacher-meeting", 1, None, "Owner edit", false);
        let h = headers("owner");
        let pending = tokio::spawn({
            let state = state.clone();
            async move { update(State(state), h, Ok(Json(c))).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(!pending.is_finished());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        mode.store(1, Ordering::SeqCst);
        held.commit().await.unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(8), pending)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err()
                .0,
            StatusCode::UNAUTHORIZED
        );
        let mut rows=graph.execute(query("MATCH(a:LearningClassMeetingAudit {school_id:$school,tenant_id:$tenant}) RETURN a.actor_id AS actor,a.actor_role AS role,a.assigned_teacher_id AS teacher").param("school",school.clone()).param("tenant",tenant.clone())).await.unwrap();
        let mut count = 0;
        while let Some(row) = rows.next().await.unwrap() {
            assert_eq!(row.get::<String>("teacher").unwrap(), teacher);
            let role = row.get::<String>("role").unwrap();
            assert_eq!(
                row.get::<String>("actor").unwrap(),
                if role == "owner" {
                    actor.clone()
                } else {
                    teacher.clone()
                }
            );
            count += 1;
        }
        assert_eq!(count, 4);
        let mut rows=graph.execute(query("MATCH(m:LearningClassMeeting {key:$key}) RETURN m.version AS version,m.teacher_id AS teacher").param("key",teacher_key)).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>("version").unwrap(), 1);
        assert_eq!(row.get::<String>("teacher").unwrap(), teacher);
        server.abort();
        drop(restore);
        graph.run(query("MATCH(n) WHERE n.owner_parity_tag=$mark OR (n.school_id=$school AND n.tenant_id=$tenant) DETACH DELETE n").param("mark",mark).param("school",school).param("tenant",tenant)).await.unwrap();
    }
}
