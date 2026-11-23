//! Disposable graph + synthetic Auth and notification-services adapters. Nothing reaches a real device.
use super::*;
use neo4rs::{query, Graph};
use std::sync::{Arc, Mutex};

type Error = Box<dyn std::error::Error + Send + Sync>;
const TEST_KEY: &str = "synthetic-sis-student-push-integration-key";

fn credential(id: &str) -> String {
    let claims =
        json!({"sub":id,"role":"student","exp":Utc::now().timestamp()+3600,"credentialVersion":1});
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(TEST_KEY.as_bytes()),
    )
    .unwrap()
}

async fn serve(router: axum::Router) -> Result<String, Error> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Ok(origin)
}

#[tokio::test]
#[ignore = "explicit isolated SIS_STUDENT_TEST_NEO4J_URI=bolt://127.0.0.1:3385 only; never primary graph"]
async fn students_register_their_own_device_and_only_willing_students_of_the_class_are_pushed(
) -> Result<(), Error> {
    let uri = std::env::var("SIS_STUDENT_TEST_NEO4J_URI")?;
    if uri != "bolt://127.0.0.1:3385" {
        return Err("Dedicated isolated graph required".into());
    }
    let (user, password) = (
        std::env::var("SIS_STUDENT_TEST_NEO4J_USER")?,
        std::env::var("SIS_STUDENT_TEST_NEO4J_PASSWORD")?,
    );
    let graph = Graph::new(&uri, &user, &password).await?;
    repository::init(&graph).await?;
    crate::learning_access::migrate(&graph).await?;
    let m = format!("push-{}", uuid::Uuid::new_v4());
    let (a, b) = (format!("{m}-7a"), format!("{m}-7b"));
    graph.run(query("CREATE(:Section {section_id:$a,name:'7A',status:'active',school_id:$m,tenant_id:$m,push_tag:$m}) CREATE(:Section {section_id:$b,name:'7B',status:'active',school_id:$m,tenant_id:$m,push_tag:$m})").param("m",m.clone()).param("a",a.clone()).param("b",b.clone())).await?;
    // (n, class, binding status, extra User properties)
    let fixtures = [
        (1, &a, "ACTIVE", ""),
        (2, &a, "ACTIVE", ",student_notifications:false"),
        (3, &a, "ACTIVE", ",student_notifications:true,student_notify_assignments:false,student_notify_grades:true,student_notify_class_schedule:true,student_notify_announcements:true,student_notify_messages:true,student_notify_learning_reminders:true,student_locale:'id'"),
        (4, &a, "REVOKED", ""),
        (5, &b, "ACTIVE", ""),
    ];
    for (n, class, status, extra) in fixtures {
        graph.run(query(&format!("MATCH(sec:Section {{section_id:$class}}) CREATE(:User {{id:$u,role:'student',push_tag:$m{extra}}}) CREATE(:LearningStudentBinding {{user_id:$u,student_id:$s,school_id:$m,tenant_id:$m,status:$status,push_tag:$m}}) CREATE(:Student {{studentId:$s,fullName:'Synthetic',push_tag:$m}})-[:ENROLLED_AS]->(:EnrolledStudent {{status:'active',school_id:$m,tenant_id:$m,push_tag:$m}})-[:ENROLLED_IN]->(sec)"))
            .param("class",class.clone()).param("u",format!("{m}-u{n}")).param("s",format!("{m}-s{n}")).param("status",status).param("m",m.clone())).await?;
    }
    let u = |n: u8| format!("{m}-u{n}");
    let s = |n: u8| format!("{m}-s{n}");

    let result: Result<(), Error> = async {
        // Recipients: active binding in the class only; the switch and the category decide `notify`.
        let found = repository::recipients(&graph, &a, &m, &m, None, Kind::Assignment.category()).await?;
        let ids: Vec<(String, bool)> = found.iter().map(|r| (r.id.clone(), r.notify)).collect();
        assert_eq!(ids, vec![(format!("student:{}", u(1)), true), (format!("student:{}", u(2)), false), (format!("student:{}", u(3)), false)]);
        let graded = repository::recipients(&graph, &a, &m, &m, Some(&[s(3)]), Kind::ReportCard.category()).await?;
        assert_eq!(graded.len(), 1);
        assert!(graded[0].notify && graded[0].locale == "id");
        assert_eq!(repository::outside_class(&graph, &a, &m, &m, &[s(1), s(5)]).await?, vec![s(5)]);
        assert!(repository::outside_class(&graph, &a, &m, &m, &[s(1), s(4)]).await?.is_empty());
        assert!(repository::student_in_school(&graph, &u(1), &m, &m).await?);
        assert!(repository::student_in_school(&graph, &u(5), &m, &m).await?);
        assert!(!repository::student_in_school(&graph, &u(4), &m, &m).await?);
        assert!(!repository::student_in_school(&graph, &u(1), "other-school", &m).await?);
        assert!(repository::student_enrolled(&graph, &u(1)).await?);
        assert!(!repository::student_enrolled(&graph, &u(4)).await?);

        // Once per fact, and at most `cap` new facts per class and day.
        for (key, expected) in [("one", "fresh"), ("one", "duplicate"), ("two", "fresh"), ("three", "capped"), ("one", "duplicate")] {
            let got = match repository::claim(&graph, &format!("{m}|{key}"), "assignment.published", &b, &m, &m, "2026-11-20", 2, 1).await? {
                Claim::Fresh => "fresh",
                Claim::Duplicate => "duplicate",
                Claim::Capped => "capped",
            };
            assert_eq!(got, expected, "{key}");
        }

        // Synthetic Auth answers for whoever `who` holds; synthetic notification-services records calls.
        let who = Arc::new(Mutex::new(String::new()));
        let current = who.clone();
        let auth = axum::Router::new().route("/api/v1/auth-service/student/session", post(move || {
            let id = current.lock().unwrap().clone();
            async move { Json(json!({"active":true,"userId":id,"role":"student","managed":true,"credentialVersion":1})) }
        }));
        let auth_url = format!("{}/api/v1/auth-service/student/session", serve(auth).await?);
        let calls = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
        let seen = calls.clone();
        let notification = axum::Router::new().fallback(move |uri: axum::http::Uri, h: HeaderMap, Json(body): Json<Value>| {
            let seen = seen.clone();
            async move {
                if h.get("x-internal-token").and_then(|v| v.to_str().ok()) != Some("internal-test-token") {
                    return StatusCode::UNAUTHORIZED;
                }
                seen.lock().unwrap().push((uri.path().to_string(), body));
                StatusCode::OK
            }
        });
        let config = crate::config::config::AppConfig {
            jwt_secret: TEST_KEY.into(),
            auth_student_session_url: Some(auth_url),
            notification_service_url: Some(serve(notification).await?),
            notification_internal_token: Some("internal-test-token".into()),
            ..Default::default()
        };
        let state = AppState { graph: graph.clone(), config, http_client: reqwest::Client::new() };
        let sis = serve(router().with_state(state.clone())).await?;
        let client = reqwest::Client::builder().no_proxy().build()?;
        let devices = format!("{sis}/api/v1/sis-service/learning/owner/student/push-devices?schoolId={m}&tenantId={m}");
        let device = json!({"installationId":"3f2b8c1e-9d4a-4c55-8f0e-2a1b3c4d5e6f","token":"fcm-token","platform":"android","appVersion":"1.0.0","locale":"id"});

        *who.lock().unwrap() = u(1);
        let r = client.post(&devices).bearer_auth(credential(&u(1))).json(&device).send().await?;
        assert_eq!(r.status(), 200);
        assert_eq!(r.json::<Value>().await?, json!({"saved":true}));
        let r = client.delete(&devices).bearer_auth(credential(&u(1))).json(&json!({"installationId":"3f2b8c1e-9d4a-4c55-8f0e-2a1b3c4d5e6f"})).send().await?;
        assert_eq!(r.status(), 200);
        assert_eq!(client.post(&devices).json(&device).send().await?.status(), 401);
        // The app names no school: any active binding and class will do.
        let unscoped = format!("{sis}/api/v1/sis-service/learning/owner/student/push-devices");
        assert_eq!(client.post(&unscoped).bearer_auth(credential(&u(1))).json(&device).send().await?.status(), 200);
        assert_eq!(client.post(format!("{unscoped}?schoolId={m}")).bearer_auth(credential(&u(1))).json(&device).send().await?.status(), 400);
        *who.lock().unwrap() = u(4);
        assert_eq!(client.post(&devices).bearer_auth(credential(&u(4))).json(&device).send().await?.status(), 403);
        // A Student token cannot ask for a class push, even for the Student's own class.
        *who.lock().unwrap() = u(1);
        let r = client.post(format!("{sis}/api/v1/sis-service/learning/student-push?schoolId={m}&tenantId={m}"))
            .bearer_auth(credential(&u(1))).json(&json!({"kind":"assignment.published","ref":"A-1","classId":a})).send().await?;
        assert_eq!(r.status(), 403);
        {
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 3, "{calls:?}");
            assert_eq!(calls[0].0, "/api/push/v1/devices");
            assert_eq!(calls[0].1["recipientId"], format!("student:{}", u(1)));
            assert_eq!(calls[0].1["token"], "fcm-token");
            assert_eq!(calls[1].0, "/api/push/v1/devices/delete");
            assert_eq!(calls[1].1, json!({"recipientId":format!("student:{}", u(1)),"installationId":"3f2b8c1e-9d4a-4c55-8f0e-2a1b3c4d5e6f"}));
        }

        // The fan-out sends only to the willing Student and records the outcome.
        calls.lock().unwrap().clear();
        let key = format!("{m}|fan-out");
        assert!(matches!(repository::claim(&graph, &key, "assignment.published", &a, &m, &m, "2026-11-21", 20, 3).await?, Claim::Fresh));
        let n = Notice { kind: Kind::Assignment, reference: "A-9".into(), class_id: a.clone(), student_ids: None };
        fan_out(state.clone(), key.clone(), n, found, false).await;
        {
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 1, "{calls:?}");
            assert_eq!(calls[0].0, "/api/push/v1/send");
            assert_eq!(calls[0].1, json!({"recipientId":format!("student:{}", u(1)),"title":"New assignment","body":"You have a new assignment.","url":"/lessons?assignment=A-9"}));
        }
        let mut rows = graph.execute(query("MATCH(p:StudentPush {key:$key}) RETURN p.status AS status,p.pushed AS pushed,p.skipped AS skipped").param("key", key)).await?;
        let row = rows.next().await?.ok_or("claim missing")?;
        assert_eq!((row.get::<String>("status")?, row.get::<i64>("pushed")?, row.get::<i64>("skipped")?), ("sent".into(), 1, 2));
        Ok(())
    }
    .await;
    graph.run(query("MATCH(n) WHERE n.push_tag=$m OR (n:StudentPush AND (n.key STARTS WITH $m OR n.school_id=$m)) DETACH DELETE n").param("m", m.clone())).await?;
    result
}
