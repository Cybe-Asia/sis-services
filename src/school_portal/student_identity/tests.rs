use super::*;
use axum::{routing::post, Router};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use std::sync::{
    atomic::{AtomicI64, AtomicUsize, Ordering},
    Arc,
};

const SYNTHETIC_KEY: &str = "synthetic-student-guard-test-key-only-20261004";
fn claims() -> Value {
    json!({"sub":"synthetic-student","role":"student","exp":chrono::Utc::now().timestamp()+3600,"credentialVersion":1})
}
fn token(value: &Value) -> String {
    encode(
        &Header::new(Algorithm::HS256),
        value,
        &EncodingKey::from_secret(SYNTHETIC_KEY.as_bytes()),
    )
    .unwrap()
}
fn dto(managed: bool, version: Option<i64>) -> Value {
    json!({"active":true,"userId":"synthetic-student","role":"student","managed":managed,"credentialVersion":version})
}
fn client(timeout: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .unwrap()
}
async fn fixture(status: u16, body: String) -> (reqwest::Url, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = endpoint(&format!(
        "http://{}{}",
        listener.local_addr().unwrap(),
        SESSION_PATH
    ))
    .unwrap();
    let router = Router::new().route(
        SESSION_PATH,
        post(move || async move { (StatusCode::from_u16(status).unwrap(), body) }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, task)
}

#[test]
fn only_signed_full_student_credentials_are_accepted() {
    let original = claims();
    let valid = signed(&token(&original), SYNTHETIC_KEY).unwrap();
    assert_eq!(valid.id, "synthetic-student");
    assert_eq!(valid.version, Some(1));
    let mut legacy = original.clone();
    legacy.as_object_mut().unwrap().remove("credentialVersion");
    assert!(signed(&token(&legacy), SYNTHETIC_KEY)
        .unwrap()
        .version
        .is_none());
    for (key, value) in [
        ("role", json!("parent")),
        ("role", json!("teacher")),
        ("role", json!("owner")),
        ("role", Value::Null),
        ("sub", json!("bad id")),
        ("sub", Value::Null),
        ("scope", json!("partial-registration")),
        ("scope", Value::Null),
        ("exp", json!(chrono::Utc::now().timestamp() - 1)),
        ("credentialVersion", json!(0)),
        ("credentialVersion", json!(-1)),
        ("credentialVersion", json!("1")),
        ("credentialVersion", Value::Null),
        ("credentialVersion", json!(1.5)),
        ("aud", json!("learning-api")),
    ] {
        let mut invalid = original.clone();
        invalid[key] = value;
        assert!(
            signed(&token(&invalid), SYNTHETIC_KEY).is_err(),
            "reject {key}"
        );
    }
    let mut missing = original.clone();
    missing.as_object_mut().unwrap().remove("exp");
    assert!(signed(&token(&missing), SYNTHETIC_KEY).is_err());
    assert!(signed(&token(&original), "different-synthetic-student-test-key").is_err());
    assert_eq!(
        signed(&token(&original), "short").err().unwrap().0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let hs384 = encode(
        &Header::new(Algorithm::HS384),
        &original,
        &EncodingKey::from_secret(SYNTHETIC_KEY.as_bytes()),
    )
    .unwrap();
    assert!(signed(&hs384, SYNTHETIC_KEY).is_err());
}

#[test]
fn exact_live_subject_role_and_credential_version_required() {
    let managed = SignedStudent {
        id: "synthetic-student".into(),
        version: Some(1),
    };
    let legacy = SignedStudent {
        id: "synthetic-student".into(),
        version: None,
    };
    let check = |signed: &SignedStudent, value: Value| {
        current(signed, &serde_json::to_vec(&value).unwrap())
    };
    assert!(check(&managed, dto(true, Some(1))).is_ok());
    assert!(check(&legacy, dto(false, None)).is_ok());
    // A legacy JWT cannot bypass an account that is now managed.
    assert!(check(&legacy, dto(true, Some(1))).is_err());
    for value in [
        dto(true, Some(2)),
        dto(true, Some(0)),
        dto(true, None),
        dto(false, Some(1)),
        dto(false, None),
    ] {
        assert!(check(&managed, value).is_err());
    }
    for (key, value) in [
        ("active", json!(false)),
        ("userId", json!("another-student")),
        ("role", json!("teacher")),
        ("managed", json!("true")),
        ("extra", json!(true)),
    ] {
        let mut invalid = dto(true, Some(1));
        invalid[key] = value;
        assert!(check(&managed, invalid).is_err(), "reject {key}");
    }
    for key in ["active", "userId", "role", "managed", "credentialVersion"] {
        let mut missing = dto(true, Some(1));
        missing.as_object_mut().unwrap().remove(key);
        assert!(check(&managed, missing).is_err(), "missing {key}");
    }
    assert!(current(&managed, b"not json").is_err());
    assert!(check(&managed, json!({"data":dto(true,Some(1))})).is_err());
}

#[test]
fn student_endpoint_requires_explicit_safe_exact_url() {
    for host in [
        "https://auth.example.test",
        "http://127.0.0.1:3218",
        "http://localhost:3218",
        "http://[::1]:3218",
    ] {
        assert!(endpoint(&format!("{host}{SESSION_PATH}")).is_ok());
    }
    for bad in [
        "",
        "http://auth.example.test/api/v1/auth-service/student/session",
        "https://auth.example.test/api/v1/auth-service/oauth/membership",
        "https://auth.example.test/api/v1/auth-service/student/session?x=1",
        "https://auth.example.test/api/v1/auth-service/student/session#x",
        "https://user:secret@auth.example.test/api/v1/auth-service/student/session",
        "https://auth.example.test/api/v1/auth-service/student/session/",
    ] {
        assert_eq!(
            endpoint(bad).err().unwrap().0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

#[tokio::test]
async fn live_auth_failures_and_bounded_body_never_fall_back() {
    let token = token(&claims());
    for (status, body, expected) in [
        (401, "".into(), StatusCode::UNAUTHORIZED),
        (403, "".into(), StatusCode::FORBIDDEN),
        (503, "".into(), StatusCode::SERVICE_UNAVAILABLE),
        (204, "".into(), StatusCode::SERVICE_UNAVAILABLE),
        (200, "malformed".into(), StatusCode::SERVICE_UNAVAILABLE),
        (
            200,
            "x".repeat(MAX_RESPONSE + 1),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let (url, task) = fixture(status, body).await;
        let result = verify(
            &token,
            signed(&token, SYNTHETIC_KEY).unwrap(),
            url,
            client(std::time::Duration::from_secs(1)),
        )
        .await;
        task.abort();
        assert_eq!(result.err().unwrap().0, expected);
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = endpoint(&format!(
        "http://{}{}",
        listener.local_addr().unwrap(),
        SESSION_PATH
    ))
    .unwrap();
    drop(listener);
    assert_eq!(
        verify(
            &token,
            signed(&token, SYNTHETIC_KEY).unwrap(),
            url,
            client(std::time::Duration::from_secs(1))
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn each_request_rechecks_rotation_and_does_not_cache_auth() {
    let version = Arc::new(AtomicI64::new(1));
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = endpoint(&format!(
        "http://{}{}",
        listener.local_addr().unwrap(),
        SESSION_PATH
    ))
    .unwrap();
    let current_version = version.clone();
    let count = calls.clone();
    let router = Router::new().route(
        SESSION_PATH,
        post(move |headers: HeaderMap| {
            let version = current_version.clone();
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                let claims = signed(bearer(&headers).unwrap(), SYNTHETIC_KEY).unwrap();
                assert_eq!(claims.id, "synthetic-student");
                axum::Json(dto(true, Some(version.load(Ordering::SeqCst))))
            }
        }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let old = token(&claims());
    assert!(verify(
        &old,
        signed(&old, SYNTHETIC_KEY).unwrap(),
        url.clone(),
        client(std::time::Duration::from_secs(1))
    )
    .await
    .is_ok());
    version.store(2, Ordering::SeqCst);
    assert_eq!(
        verify(
            &old,
            signed(&old, SYNTHETIC_KEY).unwrap(),
            url,
            client(std::time::Duration::from_secs(1))
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn redirects_are_not_followed_and_slow_auth_times_out() {
    let token = token(&claims());
    let (destination, destination_task) = fixture(200, dto(true, Some(1)).to_string()).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = endpoint(&format!(
        "http://{}{}",
        listener.local_addr().unwrap(),
        SESSION_PATH
    ))
    .unwrap();
    let router = Router::new().route(
        SESSION_PATH,
        post(move || async move {
            (
                StatusCode::TEMPORARY_REDIRECT,
                [("location", destination.to_string())],
            )
        }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    assert_eq!(
        verify(
            &token,
            signed(&token, SYNTHETIC_KEY).unwrap(),
            url,
            client(std::time::Duration::from_secs(1))
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    task.abort();
    destination_task.abort();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = endpoint(&format!(
        "http://{}{}",
        listener.local_addr().unwrap(),
        SESSION_PATH
    ))
    .unwrap();
    let router = Router::new().route(
        SESSION_PATH,
        post(|| async {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            axum::Json(dto(true, Some(1)))
        }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    assert_eq!(
        verify(
            &token,
            signed(&token, SYNTHETIC_KEY).unwrap(),
            url,
            client(std::time::Duration::from_millis(40))
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    task.abort();
}
