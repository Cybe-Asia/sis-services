//! Private TLS transport only; no Auth session/graph/business acceptance claim.
use super::*;
#[test]
fn private_ca_configuration_fails_closed_without_disabling_default_trust() {
    for malformed in [
        b"".as_slice(),
        b"not PEM",
        b"-----BEGIN PRIVATE KEY-----\nAA==\n-----END PRIVATE KEY-----",
        b"-----BEGIN CERTIFICATE-----\nAA==",
        b"-----BEGIN CERTIFICATE-----\n!invalid!\n-----END CERTIFICATE-----",
    ] {
        assert!(private_roots(malformed).is_err());
    }
    assert!(private_roots(&vec![b'x'; 262_145]).is_err());
    assert!(client_builder(None).unwrap().build().is_ok());
}
#[tokio::test]
#[ignore = "explicit synthetic loopback HTTPS fixture and CA required; no database/real credentials"]
async fn private_ca_https_success_hostname_failure_and_no_redirect() {
    let origin = std::env::var("SIS_LEARNING_TLS_TEST_ORIGIN").unwrap();
    let parsed = reqwest::Url::parse(&origin).unwrap();
    assert_eq!(parsed.scheme(), "https");
    assert_eq!(parsed.host_str(), Some("localhost"));
    let ca = std::env::var("SIS_LEARNING_TLS_TEST_CA_FILE").unwrap();
    let pem = std::fs::read(&ca).unwrap();
    assert_eq!(private_roots(&pem).unwrap().len(), 1);
    let mut bundle = pem.clone();
    bundle.extend_from_slice(&pem);
    assert_eq!(private_roots(&bundle).unwrap().len(), 2);
    for suffix in [
        b"junk".as_slice(),
        b"-----BEGIN CERTIFICATE-----\nbroken\n-----END CERTIFICATE-----",
        b"-----BEGIN PRIVATE KEY-----\nAA==\n-----END PRIVATE KEY-----",
    ] {
        let mut bad = pem.clone();
        bad.extend_from_slice(suffix);
        assert!(private_roots(&bad).is_err());
    }
    let port = parsed.port_or_known_default().unwrap();
    let trusted = client_builder(Some(&pem))
        .unwrap()
        .no_proxy()
        .build()
        .unwrap();
    assert!(trusted
        .get(&origin)
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    let untrusted = client_builder(None).unwrap().no_proxy().build().unwrap();
    assert!(untrusted.get(&origin).send().await.is_err());
    let wrong_host = client_builder(Some(&pem))
        .unwrap()
        .no_proxy()
        .resolve(
            "wrong-host.example.test",
            std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        )
        .build()
        .unwrap();
    assert!(wrong_host
        .get(format!("https://wrong-host.example.test:{port}/"))
        .send()
        .await
        .is_err());
    let config = crate::config::config::AppConfig {
        learning_service_origin: Some(origin.clone()),
        learning_service_ca: Some(pem.clone()),
        ..Default::default()
    };
    let input = Import {
        course_id: "course".into(),
        assessment_id: "assessment".into(),
        attempt_id: "attempt".into(),
        attempt_revision: 1,
        term: "Term 1".into(),
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        "Bearer synthetic-grade-export-transport-only"
            .parse()
            .unwrap(),
    );
    let exported = read(&config, &headers, &input).await.unwrap();
    assert_eq!(exported.course_id, "course");
    let mut redirect = input.clone();
    redirect.course_id = "redirect".into();
    assert_eq!(
        read(&config, &headers, &redirect).await.err().unwrap().0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    // Without the private anchor the default roots refuse the synthetic certificate.
    let default_trust = crate::config::config::AppConfig {
        learning_service_ca: None,
        ..config.clone()
    };
    assert_eq!(
        read(&default_trust, &headers, &input)
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}
