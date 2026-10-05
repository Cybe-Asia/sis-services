use super::*;
use crate::school_portal::{auth::Teacher, repository};
use serde_json::json;
fn member(role: &str, schools: &[&str], tenants: &[&str]) -> Teacher {
    Teacher {
        active: true,
        staff_member_id: "staff".into(),
        roles: vec![role.into()],
        school_ids: schools.iter().map(|s| s.to_string()).collect(),
        tenant_ids: tenants.iter().map(|s| s.to_string()).collect(),
    }
}
#[test]
fn owner_requires_explicit_complete_scope_and_preserves_real_role() {
    let global = member("owner", &[], &[]);
    assert!(global.valid());
    assert!(global.select(&Selection::default()).is_err());
    let selected = member("owner", &[], &[])
        .select(&Selection {
            school_id: Some("school".into()),
            tenant_id: Some("tenant".into()),
        })
        .unwrap();
    assert_eq!(selected.role(), "owner");
    assert_eq!(selected.staff_member_id, "staff");
    assert_eq!(selected.school_ids, vec!["school"]);
    assert!(!member("owner", &["school"], &[]).valid());
    assert!(!member("owner", &[], &["tenant"]).valid());
    assert!(!member("school_admin", &["school"], &["tenant"]).valid());
    assert!(member("teacher", &["school"], &["tenant"])
        .select(&Selection::default())
        .is_ok());
    assert!(member("owner", &["school"], &["tenant"])
        .select(&Selection {
            school_id: Some("foreign".into()),
            tenant_id: Some("tenant".into())
        })
        .is_err());
    assert!(Selection {
        school_id: Some("school".into()),
        tenant_id: None
    }
    .pair()
    .is_err());
    assert!(
        serde_json::from_value::<Selection>(json!({"school_id":"school","teacherId":"spoof"}))
            .is_err()
    );
}
async fn permitted(g: &Graph, id: &str, school: &str, tenant: &str) -> bool {
    let q = query(&format!(
        "MATCH(a:StaffMember {{id:$id}}) WHERE {} RETURN a.id AS id",
        authority("a", "'teacher'", "$school", "$tenant")
    ))
    .param("id", id)
    .param("school", school)
    .param("tenant", tenant);
    g.execute(q).await.unwrap().next().await.unwrap().is_some()
}
#[tokio::test]
#[ignore = "requires existing isolated3385; UUID fixtures only; never clears the graph"]
async fn owner_directory_scope_and_truthful_classroom_capabilities() {
    let uri = std::env::var("SIS_OWNER_TEST_NEO4J_URI").unwrap();
    assert!(matches!(
        uri.as_str(),
        "bolt://127.0.0.1:3385" | "127.0.0.1:3385"
    ));
    let g = Graph::new(
        uri,
        std::env::var("SIS_OWNER_TEST_NEO4J_USER").unwrap(),
        std::env::var("SIS_OWNER_TEST_NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    let mark = format!("owner-contract-{}", uuid::Uuid::new_v4());
    let school = format!("{mark}-school");
    let tenant = format!("{mark}-tenant");
    let actor = format!("{mark}-owner");
    let teacher = format!("{mark}-teacher");
    let section = format!("{mark}-class");
    let student = format!("{mark}-student");
    let q=query("CREATE(:School {school_id:$school,tenant_id:$tenant,owner_parity_tag:$mark}) CREATE(:StaffMember {id:$actor,membershipStatus:'ACTIVE',roles:['owner','school_admin'],schoolIds:[],tenantIds:[],teamIds:[],owner_parity_tag:$mark}) CREATE(:StaffMember {id:$teacher,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$school],tenantIds:[$tenant],owner_parity_tag:$mark}) CREATE(sec:Section {section_id:$class,name:'Synthetic class',year_group:'Year7',status:'active',school_id:$school,tenant_id:$tenant,homeroom_staff_member_id:$teacher,owner_parity_tag:$mark}) CREATE(:Student {studentId:$student,fullName:'Synthetic Student',owner_parity_tag:$mark})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant,owner_parity_tag:$mark})-[:ENROLLED_IN]->(sec)") .param("school",school.clone()).param("tenant",tenant.clone()).param("actor",actor.clone()).param("teacher",teacher.clone()).param("class",section.clone()).param("student",student.clone()).param("mark",mark.clone());
    g.run(q).await.unwrap();
    assert!(permitted(&g, &actor, &school, &tenant).await);
    assert!(!permitted(&g, &actor, &format!("{mark}-unknown"), &tenant).await);
    let owner = Teacher {
        staff_member_id: actor.clone(),
        school_ids: vec![school.clone()],
        tenant_ids: vec![tenant.clone()],
        ..member("owner", &[], &[])
    };
    let context = repository::context(&g, &owner).await.unwrap();
    assert_eq!(context["actor"]["role"], "owner");
    assert_eq!(context["classes"][0]["homeroom"], false);
    assert_eq!(context["classes"][0]["canManageHomeroom"], true);
    let actual_teacher = Teacher {
        staff_member_id: teacher.clone(),
        school_ids: vec![school.clone()],
        tenant_ids: vec![tenant.clone()],
        ..member("teacher", &[], &[])
    };
    let context = repository::context(&g, &actual_teacher).await.unwrap();
    assert_eq!(context["classes"][0]["homeroom"], true);
    assert_eq!(context["actor"]["role"], "teacher");
    for (schools, tenants, teams, active, roles, allowed) in [
        (
            vec![school.clone()],
            vec![tenant.clone()],
            vec![],
            "ACTIVE",
            vec!["owner", "school_admin"],
            true,
        ),
        (
            vec![school.clone()],
            vec![],
            vec![],
            "ACTIVE",
            vec!["owner", "school_admin"],
            false,
        ),
        (
            vec![],
            vec![],
            vec!["team".into()],
            "ACTIVE",
            vec!["owner", "school_admin"],
            false,
        ),
        (
            vec![school.clone()],
            vec![tenant.clone()],
            vec!["team".into()],
            "ACTIVE",
            vec!["owner", "school_admin"],
            false,
        ),
        (
            vec![school.clone()],
            vec![tenant.clone()],
            vec![],
            "SUSPENDED",
            vec!["owner"],
            false,
        ),
        (
            vec![school.clone()],
            vec![tenant.clone()],
            vec![],
            "ACTIVE",
            vec!["school_admin"],
            false,
        ),
    ] {
        g.run(query("MATCH(a:StaffMember {id:$actor}) SET a.schoolIds=$schools,a.tenantIds=$tenants,a.teamIds=$teams,a.membershipStatus=$active,a.roles=$roles").param("actor",actor.clone()).param("schools",schools).param("tenants",tenants).param::<Vec<String>>("teams",teams).param("active",active).param("roles",roles)).await.unwrap();
        assert_eq!(permitted(&g, &actor, &school, &tenant).await, allowed);
    }
    g.run(query("MATCH(a:StaffMember {id:$actor}) SET a.roles=['owner'],a.schoolIds=[],a.tenantIds=[],a.teamIds=[]").param("actor",actor.clone())).await.unwrap();
    g.run(
        query("CREATE(:School {school_id:$school,tenant_id:$tenant,owner_parity_tag:$duplicate})")
            .param("school", school.clone())
            .param("tenant", tenant.clone())
            .param("duplicate", format!("{mark}-duplicate")),
    )
    .await
    .unwrap();
    assert!(!permitted(&g, &actor, &school, &tenant).await);
    g.run(
        query("MATCH(n {owner_parity_tag:$duplicate}) DETACH DELETE n")
            .param("duplicate", format!("{mark}-duplicate")),
    )
    .await
    .unwrap();
    let raw_scope = Teacher {
        roles: vec!["teacher".into()],
        ..owner
    };
    assert!(
        repository::context(&g, &raw_scope).await.unwrap()["classes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    g.run(query("MATCH(n) WHERE n.owner_parity_tag=$mark DETACH DELETE n").param("mark", mark))
        .await
        .unwrap();
}

/// Synthetic membership adapter response only; does not claim real Auth issuance or human SSO.
#[tokio::test]
#[ignore = "requires existing isolated3385 and serialized env access; UUID fixtures only"]
async fn owner_queued_write_rechecks_session_after_resource_lock() {
    use axum::{routing::post, Json, Router};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct EnvRestore(Vec<(&'static str, Option<String>)>);
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                if let Some(value) = value {
                    std::env::set_var(key, value);
                } else {
                    std::env::remove_var(key);
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
    let g = Graph::new(
        uri,
        std::env::var("SIS_OWNER_TEST_NEO4J_USER").unwrap(),
        std::env::var("SIS_OWNER_TEST_NEO4J_PASSWORD").unwrap(),
    )
    .await
    .unwrap();
    let mark = format!("owner-queue-{}", uuid::Uuid::new_v4());
    let actor = format!("{mark}-actor");
    let school = format!("{mark}-school");
    let tenant = format!("{mark}-tenant");
    g.run(query("CREATE(:School {school_id:$school,tenant_id:$tenant,owner_parity_tag:$mark}) CREATE(:StaffMember {id:$actor,membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[],teamIds:[],owner_parity_tag:$mark}) CREATE(:OwnerParityResource {key:$mark,version:0,owner_parity_tag:$mark})").param("mark",mark.clone()).param("actor",actor.clone()).param("school",school.clone()).param("tenant",tenant.clone())).await.unwrap();
    let mode = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app=Router::new().route("/api/v1/auth-service/oauth/membership",post({let mode=mode.clone();let calls=calls.clone();let actor=actor.clone();move || {let mode=mode.clone();let calls=calls.clone();let actor=actor.clone();async move{calls.fetch_add(1,Ordering::SeqCst);if mode.load(Ordering::SeqCst)==1 {(StatusCode::UNAUTHORIZED,Json(json!({"error":"expired synthetic session"})))}else{(StatusCode::OK,Json(json!({"active":mode.load(Ordering::SeqCst)!=2,"staffMemberId":actor,"roles":["owner"],"schoolIds":[],"tenantIds":[]})))}}}}));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    std::env::set_var("APP_ENV", "test");
    std::env::set_var(
        "AUTH_SCHOOL_MEMBERSHIP_URL",
        format!("http://{address}/api/v1/auth-service/oauth/membership"),
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!(
            "Bearer {}.{}.synthetic",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#),
            URL_SAFE_NO_PAD.encode(r#"{"aud":"learning-api"}"#)
        )
        .parse()
        .unwrap(),
    );
    for (status, expected) in [(1, StatusCode::UNAUTHORIZED), (2, StatusCode::FORBIDDEN)] {
        mode.store(0, Ordering::SeqCst);
        calls.store(0, Ordering::SeqCst);
        let mut held = g.start_txn().await.unwrap();
        held.run(
            query("MATCH(r:OwnerParityResource {key:$mark}) SET r.version=r.version+1")
                .param("mark", mark.clone()),
        )
        .await
        .unwrap();
        let task = tokio::spawn({
            let g = g.clone();
            let headers = headers.clone();
            let actor = actor.clone();
            let school = school.clone();
            let tenant = tenant.clone();
            let mark = mark.clone();
            async move {
                let lock=query("MATCH(r:OwnerParityResource {key:$mark}) SET r.version=r.version+1 RETURN r.key AS key").param("mark",mark.clone());
                let write=query(&format!("MATCH(a:StaffMember {{id:$actor,membershipStatus:'ACTIVE'}}) WHERE {} CREATE(:OwnerParityAudit {{owner_parity_tag:$mark}}) RETURN a.id AS id",owner("a","$school","$tenant"))).param("actor",actor.clone()).param("school",school.clone()).param("tenant",tenant.clone()).param("mark",mark);
                live_write(
                    &g,
                    OwnerSession {
                        headers: &headers,
                        actor: &actor,
                        school: &school,
                        tenant: &tenant,
                        learning: true,
                    },
                    lock,
                    write,
                )
                .await
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(!task.is_finished());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "Auth must run after resource lock"
        );
        mode.store(status, Ordering::SeqCst);
        held.commit().await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(8), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().0, expected);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let mut rows=g.execute(query("MATCH(r:OwnerParityResource {key:$mark}) OPTIONAL MATCH(a:OwnerParityAudit {owner_parity_tag:$mark}) RETURN r.version AS version,count(a) AS audits").param("mark",mark.clone())).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>("audits").unwrap(), 0);
        assert_eq!(
            row.get::<i64>("version").unwrap(),
            if status == 1 { 1 } else { 2 }
        );
    }
    server.abort();
    drop(restore);
    g.run(query("MATCH(n {owner_parity_tag:$mark}) DETACH DELETE n").param("mark", mark))
        .await
        .unwrap();
}
