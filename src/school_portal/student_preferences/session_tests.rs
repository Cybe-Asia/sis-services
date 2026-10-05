//! Disposable graph + synthetic Auth adapter evidence, never genuine SSO acceptance.
use super::*;
use axum::{routing::post, Router};
use std::{
    ffi::OsString,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
};

const TEST_KEY: &str = "synthetic-sis-student-session-integration-key";
struct EnvRestore(Vec<(&'static str, Option<OsString>)>);
impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}
fn credential(id: &str, version: Option<i64>) -> String {
    let mut claims = json!({"sub":id,"role":"student","exp":chrono::Utc::now().timestamp()+3600});
    if let Some(v) = version {
        claims["credentialVersion"] = json!(v);
    }
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(TEST_KEY.as_bytes()),
    )
    .unwrap()
}
fn denied(error: Error) -> Result<(), Error> {
    let error = error
        .downcast::<super::super::staff_capability::SessionFailure>()
        .map_err(|_| "Expected session failure")?;
    if error.0 .0 != StatusCode::UNAUTHORIZED {
        return Err("Expected credential denial".into());
    }
    Ok(())
}

#[tokio::test]
#[ignore = "explicit isolated SIS_STUDENT_TEST_NEO4J_URI=bolt://127.0.0.1:3385 only; never primary graph"]
async fn student_routes_and_queued_writes_require_current_auth() -> Result<(), Error> {
    let uri = std::env::var("SIS_STUDENT_TEST_NEO4J_URI")?;
    if uri != "bolt://127.0.0.1:3385" {
        return Err("Dedicated isolated graph required".into());
    }
    let user = std::env::var("SIS_STUDENT_TEST_NEO4J_USER")?;
    let password = std::env::var("SIS_STUDENT_TEST_NEO4J_PASSWORD")?;
    let graph = Graph::new(&uri, &user, &password).await?;
    let mark = format!("student-guard-{}", uuid::Uuid::new_v4());
    let scope = Scope {
        school_id: mark.clone(),
        tenant_id: mark.clone(),
    };
    graph.run(query("CREATE(:School {school_id:$mark,tenant_id:$mark,student_guard_tag:$mark}) CREATE(u:User {id:$mark,email:$email,role:'student',student_guard_tag:$mark}) CREATE(b:LearningStudentBinding {user_id:$mark,student_id:$mark,school_id:$mark,tenant_id:$mark,status:'ACTIVE',version:1,student_guard_tag:$mark}) CREATE(s:Student {studentId:$mark,fullName:'Synthetic guard Student',student_guard_tag:$mark})-[:ENROLLED_AS]->(en:EnrolledStudent {student_id:$permanent,applicant_student_id:$mark,student_number:$mark,status:'active',school_id:$mark,tenant_id:$mark,student_guard_tag:$mark})-[:ENROLLED_IN]->(sec:Section {section_id:$mark,name:'Synthetic class',status:'active',school_id:$mark,tenant_id:$mark,academic_year:'2026-2027',student_guard_tag:$mark})").param("mark",mark.clone()).param("email",format!("{mark}@example.test")).param("permanent",format!("STU-{mark}"))).await?;
    let mode = Arc::new(AtomicI64::new(1));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let auth_url = format!(
        "http://{}/api/v1/auth-service/student/session",
        listener.local_addr()?
    );
    let mock_id = mark.clone();
    let live = mode.clone();
    let auth=Router::new().route("/api/v1/auth-service/student/session",post(move || {
        let id=mock_id.clone(); let live=live.clone(); async move {
            let version=live.load(Ordering::SeqCst);
            if version<0 { (StatusCode::UNAUTHORIZED,Json(json!({}))) }
            else { (StatusCode::OK,Json(json!({"active":true,"userId":id,"role":"student","managed":version>0,"credentialVersion":if version>0 {Some(version)} else {None}}))) }
        }
    }));
    let auth_task = tokio::spawn(async move {
        axum::serve(listener, auth).await.unwrap();
    });
    let _restore = EnvRestore(
        [
            "JWT_SECRET",
            "AUTH_STUDENT_SESSION_URL",
            "AUTH_PORTAL_CA_FILE",
        ]
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect(),
    );
    std::env::set_var("JWT_SECRET", TEST_KEY);
    std::env::set_var("AUTH_STUDENT_SESSION_URL", &auth_url);
    std::env::remove_var("AUTH_PORTAL_CA_FILE");
    let state = AppState {
        graph: graph.clone(),
        config: crate::config::config::AppConfig {
            server_port: 0,
            neo4j_uri: uri,
            neo4j_user: user,
            neo4j_password: password,
            jwt_secret: TEST_KEY.into(),
        },
        http_client: reqwest::Client::new(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let router = Router::new()
        .merge(crate::learning_access::router())
        .merge(super::super::router())
        .merge(crate::extra_curricular::router())
        .with_state(state);
    let sis_task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let result: Result<(), Error>=async {
        let token=credential(&mark,Some(1)); let legacy=credential(&mark,None);
        let client=reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(8)).build()?;
        let selected=format!("school_id={mark}&tenant_id={mark}");
        let reads=[
            format!("/api/v1/sis-service/learning/owner/context?{selected}"),
            format!("/api/v1/sis-service/learning/owner/authorize?{selected}&class_id={mark}&student_id={mark}&access=learn"),
            format!("/api/v1/sis-service/learning/owner/student/school?{selected}"),
            format!("/api/v1/sis-service/learning/owner/student/calendar?{selected}&from=2026-10-01&to=2026-10-31"),
            format!("/api/v1/sis-service/learning/owner/student/preferences?{selected}"),
            "/api/v1/sis-service/activities/context/student".into(),
            format!("/api/v1/sis-service/activities/student?schoolId={mark}&tenantId={mark}&academicYear=2026-2027"),
            format!("/api/v1/sis-service/learning/class-meetings?schoolId={mark}&tenantId={mark}&classId={mark}"),
        ];
        for path in &reads {
            let response=client.get(format!("{origin}{path}")).bearer_auth(&token).send().await?;
            if response.status().as_u16()!=200 {return Err(format!("Current Student read rejected: {path} status {}",response.status()).into());}
        }
        // Auth remains canonical even if the same local JWT is otherwise valid.
        for (version, expected, used_token) in [(2,StatusCode::UNAUTHORIZED,&token),(-1,StatusCode::UNAUTHORIZED,&token),(1,StatusCode::UNAUTHORIZED,&legacy),(0,StatusCode::OK,&legacy)] {
            mode.store(version,Ordering::SeqCst);
            for path in &reads {
                let response=client.get(format!("{origin}{path}")).bearer_auth(used_token).send().await?;
                if response.status().as_u16()!=expected.as_u16() {return Err(format!("Student guard mismatch: {path} status {} expected {expected}",response.status()).into());}
            }
        }
        std::env::remove_var("AUTH_STUDENT_SESSION_URL");
        for path in &reads {
            if client.get(format!("{origin}{path}")).bearer_auth(&token).send().await?.status().as_u16()!=503 {return Err("Missing Student Auth configuration must deny".into());}
        }
        std::env::set_var("AUTH_STUDENT_SESSION_URL",&auth_url); mode.store(1,Ordering::SeqCst);
        let input=Preferences {version:0,locale:"id".into(),theme:"dark".into(),notifications:false};
        let mut headers=HeaderMap::new(); headers.insert("authorization",format!("Bearer {token}").parse()?);
        // A request passed its first current-session check before waiting for a resource.
        super::super::student_identity::actor(&headers).await.map_err(|_|"Initial current session rejected")?;
        let mut lock=graph.start_txn().await?;
        lock.run(query("MATCH(b:LearningStudentBinding {user_id:$mark}) SET b.guard_test_lock=coalesce(b.guard_test_lock,0)+1").param("mark",mark.clone())).await?;
        let mut queued=Box::pin(save_inner(&graph,&mark,&scope,&input,Some(&headers)));
        if tokio::time::timeout(std::time::Duration::from_millis(80),&mut queued).await.is_ok() {return Err("Preference write did not wait for binding lock".into());}
        mode.store(2,Ordering::SeqCst); lock.commit().await?;
        match queued.await {Err(error)=>denied(error)?,Ok(_)=>return Err("Queued preference write accepted rotated credential".into())}
        if load(&graph,&mark,&scope).await?.ok_or("Preferences missing")?["preferences"]["version"]!=0 {return Err("Denied preferences changed".into());}
        let extra_scope=crate::extra_curricular::model::Scope {school_id:mark.clone(),tenant_id:mark.clone(),academic_year:"2026-2027".into(),student_id:None,after:None};
        let key=crate::extra_curricular::repository::activity_key(&extra_scope,&mark);
        graph.run(query("CREATE(:ExtraCurricularLock {key:$key,version:0,student_guard_tag:$mark})").param("key",key.clone()).param("mark",mark.clone())).await?;
        let mut lock=graph.start_txn().await?;
        lock.run(query("MATCH(l:ExtraCurricularLock {key:$key}) SET l.guard_test_lock=1").param("key",key.clone())).await?;
        mode.store(1,Ordering::SeqCst);
        let actor=crate::extra_curricular::model::Actor {id:super::super::student_identity::actor(&headers).await.map_err(|_|"Initial extras session rejected")?,role:"student".into()};
        let change=crate::extra_curricular::model::Change {scope:extra_scope,activity_id:mark.clone(),revision:0,request_id:uuid::Uuid::new_v4().to_string(),command:crate::extra_curricular::model::Command::Enroll {}};
        let mut queued=Box::pin(crate::extra_curricular::repository::change_live(&graph,&actor,&change,&headers,"student"));
        if tokio::time::timeout(std::time::Duration::from_millis(80),&mut queued).await.is_ok() {return Err("Extras write did not wait for activity lock".into());}
        mode.store(2,Ordering::SeqCst); lock.commit().await?;
        match queued.await {Err(error)=>denied(error)?,Ok(_)=>return Err("Queued extras write accepted rotated credential".into())}
        let mut rows=graph.execute(query("MATCH(l:ExtraCurricularLock {key:$key}) OPTIONAL MATCH(a) WHERE (a:SchoolPortalAudit OR a:ExtraCurricularAudit) AND a.actor_id=$mark RETURN l.version AS version,count(a) AS audits").param("key",key).param("mark",mark.clone())).await?;
        let row=rows.next().await?.ok_or("Missing guard lock")?;
        if row.get::<i64>("version")?!=0 || row.get::<i64>("audits")?!=0 {return Err("Denied write committed lock counter or audit".into());}
        // A positive write retains CAS/retry behavior under the same current session.
        mode.store(1,Ordering::SeqCst);
        for _ in 0..2 {
            let saved=save_inner(&graph,&mark,&scope,&input,Some(&headers)).await?.ok_or("Current preferences failed")?;
            if saved["preferences"]["version"]!=1 {return Err("Preference CAS/replay changed".into());}
        }
        // Current local enrollment is still required even when Auth says active.
        graph.run(query("MATCH(b:LearningStudentBinding {user_id:$mark}) SET b.status='REVOKED'").param("mark",mark.clone())).await?;
        if load(&graph,&mark,&scope).await?.is_some() {return Err("SIS current binding bypassed".into());}
        Ok(())
    }.await;
    sis_task.abort();
    auth_task.abort();
    graph.run(query("MATCH(n) WHERE n.student_guard_tag=$mark OR (n:SchoolPortalAudit AND n.actor_id=$mark) OR (n:ExtraCurricularAudit AND n.actor_id=$mark) DETACH DELETE n").param("mark",mark)).await?;
    result
}
