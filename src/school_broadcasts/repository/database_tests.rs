//! Broadcast audiences, isolation and idempotency against a disposable loopback graph. The test owns
//! the records it creates (marked with `test_key`, or named by its own broadcast ids) and removes them.
use super::*;
use crate::school_articles::repository::Actor;
use crate::school_broadcasts::{prepare, Broadcast};
use axum::http::StatusCode;

struct Fixture {
    graph: Graph,
    key: String,
}

impl Fixture {
    async fn connect() -> Self {
        let uri = std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap();
        assert!(uri.starts_with("127.0.0.1:"));
        let password = std::env::var("SCHOOL_PORTAL_TEST_PASSWORD")
            .unwrap_or_else(|_| "parent-otp-fixture-password".into());
        let graph = Graph::new(uri, "neo4j", password).await.unwrap();
        init(&graph).await.unwrap();
        let key = uuid::Uuid::new_v4().to_string();
        // School `key` has 7A (grade code Y7) and 8A (only a year group); `other-key` has one class.
        graph.run(query("CREATE(:StaffMember {id:'admin-'+$key,membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:[$key],tenantIds:[$key],test_key:$key}) \
            CREATE(:StaffMember {id:'teacher-'+$key,membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:[$key],tenantIds:[$key],test_key:$key}) \
            CREATE(:StaffMember {id:'other-admin-'+$key,membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['other-'+$key],tenantIds:[$key],test_key:$key}) \
            CREATE(:Section {section_id:'7a-'+$key,name:'7A',grade_code:'Y7',year_group:'Year 7',school_id:$key,tenant_id:$key,status:'active',test_key:$key}) \
            CREATE(:Section {section_id:'8a-'+$key,name:'8A',year_group:'Year 8',school_id:$key,tenant_id:$key,status:'active',test_key:$key}) \
            CREATE(:Section {section_id:'x-'+$key,name:'X1',grade_code:'Y7',school_id:'other-'+$key,tenant_id:$key,status:'active',test_key:$key})")
            .param("key", key.clone())).await.unwrap();
        for (who, section, school, locale, announcements) in [
            ("p7", "7a", key.clone(), "id", true),
            ("p8", "8a", key.clone(), "en", false),
            ("px", "x", format!("other-{key}"), "id", true),
        ] {
            graph.run(query("MATCH(sec:Section {section_id:$section+'-'+$key}) CREATE(u:User {id:$who+'-'+$key,email:$who+'-'+$key+'@example.test',role:'parent',school_locale:$locale,school_notify_announcements:$announcements,test_key:$key})-[:HAS_APPLICATION]->(:Lead {lead_id:'lead-'+$who+'-'+$key,email:$who+'-'+$key+'@example.test',status:'verified',test_key:$key})-[:HAS_STUDENT]->(:Student {studentId:'child-'+$who+'-'+$key,fullName:'Fixture child',test_key:$key})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$key,test_key:$key})-[:ENROLLED_IN]->(sec)")
                .param("key", key.clone()).param("who", who).param("section", section).param("school", school).param("locale", locale).param("announcements", announcements)).await.unwrap();
        }
        Self { graph, key }
    }
    fn id(&self, who: &str) -> String {
        format!("{who}-{}", self.key)
    }
    fn broadcast(&self, id: &str, audience: Audience, dry_run: bool) -> Broadcast {
        Broadcast {
            tenant_id: self.key.clone(),
            school_id: self.key.clone(),
            id: format!("{id}-{}", self.key),
            audience,
            title: "Libur sekolah".into(),
            body: "Sekolah libur pada 25 Desember.".into(),
            dry_run,
        }
    }
    async fn reached(&self, audience: Audience) -> Vec<String> {
        recipients(&self.graph, &self.key, &self.key, &audience)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect()
    }
    async fn count(&self, cypher: &str) -> i64 {
        let mut rows = self
            .graph
            .execute(query(cypher).param("key", self.key.clone()))
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get("n").unwrap()
    }
    async fn notices(&self) -> i64 {
        self.count("MATCH(n:SchoolNotice) WHERE n.id STARTS WITH 'broadcast:' AND n.id CONTAINS $key RETURN count(n) AS n").await
    }
    async fn broadcasts(&self) -> i64 {
        self.count("MATCH(b:SchoolBroadcast) WHERE b.id ENDS WITH $key RETURN count(b) AS n")
            .await
    }
    async fn clean(self) {
        self.graph.run(query("MATCH(n) WHERE n.test_key=$key OR ((n:SchoolNotice OR n:SchoolBroadcast) AND n.id CONTAINS $key) DETACH DELETE n").param("key", self.key.clone())).await.unwrap();
    }
}

fn status(
    result: Result<crate::school_broadcasts::Prepared, (StatusCode, axum::Json<Value>)>,
) -> StatusCode {
    result.err().map(|(s, _)| s).unwrap_or(StatusCode::OK)
}

#[tokio::test]
#[ignore = "requires a disposable loopback SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:<port> graph"]
async fn every_audience_resolves_inside_its_school_only() {
    let f = Fixture::connect().await;
    let p = |who: &str| format!("parent:{}", f.id(who));
    assert_eq!(f.reached(Audience::School {}).await, vec![p("p7"), p("p8")]);
    assert_eq!(
        f.reached(Audience::Grade {
            ids: vec!["Y7".into()]
        })
        .await,
        vec![p("p7")]
    );
    assert_eq!(
        f.reached(Audience::Grade {
            ids: vec!["Year 8".into()]
        })
        .await,
        vec![p("p8")]
    );
    assert_eq!(
        f.reached(Audience::Class {
            ids: vec![f.id("8a")]
        })
        .await,
        vec![p("p8")]
    );
    assert_eq!(
        f.reached(Audience::Students {
            ids: vec![format!("child-{}", f.id("p7"))]
        })
        .await,
        vec![p("p7")]
    );
    assert_eq!(
        f.reached(Audience::Staff {}).await,
        vec![
            format!("staff:{}", f.id("admin")),
            format!("staff:{}", f.id("teacher"))
        ]
    );
    // Ids of another school are refused as a whole, even mixed with this school's own.
    for foreign in [
        Audience::Class {
            ids: vec![f.id("x")],
        },
        Audience::Class {
            ids: vec![f.id("7a"), f.id("x")],
        },
        Audience::Students {
            ids: vec![format!("child-{}", f.id("px"))],
        },
        Audience::Grade {
            ids: vec!["Y9".into()],
        },
    ] {
        assert!(
            !known(&f.graph, &f.key, &f.key, &foreign).await.unwrap(),
            "{foreign:?}"
        );
        let v = f.broadcast("foreign", foreign, false);
        assert_eq!(
            status(
                prepare(
                    &f.graph,
                    Actor {
                        id: &f.id("admin"),
                        capable: true
                    },
                    &v
                )
                .await
            ),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert!(known(
        &f.graph,
        &f.key,
        &f.key,
        &Audience::Grade {
            ids: vec!["Y7".into(), "Year 8".into()]
        }
    )
    .await
    .unwrap());
    assert_eq!(f.broadcasts().await, 0);
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires a disposable loopback SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:<port> graph"]
async fn dry_run_counts_and_writes_nothing() {
    let f = Fixture::connect().await;
    let v = f.broadcast("dry", Audience::School {}, true);
    let out = prepare(
        &f.graph,
        Actor {
            id: &f.id("admin"),
            capable: true,
        },
        &v,
    )
    .await
    .unwrap();
    assert_eq!(out.answer["recipients"], 2);
    assert_eq!(out.answer["dryRun"], true);
    assert!(out.push.is_empty());
    assert_eq!((f.broadcasts().await, f.notices().await), (0, 0));
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires a disposable loopback SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:<port> graph"]
async fn only_this_schools_administrator_sends_and_a_retry_sends_nothing() {
    let f = Fixture::connect().await;
    let v = f.broadcast("send", Audience::School {}, false);
    // A teacher of the school, and another school's administrator, are refused before anything is written.
    let teacher = f.id("teacher");
    assert_eq!(
        status(
            prepare(
                &f.graph,
                Actor {
                    id: &teacher,
                    capable: false
                },
                &v
            )
            .await
        ),
        StatusCode::FORBIDDEN
    );
    let stranger = f.id("other-admin");
    assert_eq!(
        status(
            prepare(
                &f.graph,
                Actor {
                    id: &stranger,
                    capable: false
                },
                &v
            )
            .await
        ),
        StatusCode::FORBIDDEN
    );
    assert_eq!((f.broadcasts().await, f.notices().await), (0, 0));

    let admin = f.id("admin");
    let first = prepare(
        &f.graph,
        Actor {
            id: &admin,
            capable: false,
        },
        &v,
    )
    .await
    .unwrap();
    assert_eq!(first.answer["duplicate"], false);
    assert_eq!(first.answer["recipients"], 2);
    // Both Parents get the board notice; the one who turned announcements off gets no push.
    assert_eq!(
        first.push.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        vec![format!("parent:{}", f.id("p7"))]
    );
    assert_eq!(f.notices().await, 2);

    let again = prepare(
        &f.graph,
        Actor {
            id: &admin,
            capable: false,
        },
        &v,
    )
    .await
    .unwrap();
    assert_eq!(again.answer["duplicate"], true);
    assert!(again.push.is_empty());
    assert_eq!((f.broadcasts().await, f.notices().await), (1, 2));

    // The same id from another school's administrator is a conflict, not a second send.
    let mut theirs = f.broadcast("send", Audience::School {}, false);
    theirs.school_id = format!("other-{}", f.key);
    assert_eq!(
        status(
            prepare(
                &f.graph,
                Actor {
                    id: &stranger,
                    capable: false
                },
                &theirs
            )
            .await
        ),
        StatusCode::CONFLICT
    );
    assert_eq!((f.broadcasts().await, f.notices().await), (1, 2));

    finish(&f.graph, &v.id, 1, 0).await.unwrap();
    let done = prepare(
        &f.graph,
        Actor {
            id: &admin,
            capable: false,
        },
        &v,
    )
    .await
    .unwrap();
    assert_eq!(done.answer["status"], "sent");
    f.clean().await;
}
