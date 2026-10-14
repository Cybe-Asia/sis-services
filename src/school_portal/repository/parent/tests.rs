//! Parent notification categories against a disposable loopback graph. The test owns the records
//! it creates (marked with `test_key`) and removes nothing else.
use super::*;

fn body(version: u32, notifications: bool, categories: Option<[bool; 3]>) -> ParentPreferences {
    ParentPreferences {
        version,
        locale: "id".into(),
        theme: "dark".into(),
        notifications,
        notification_categories: categories.map(|[class_schedule, announcements, messages]| {
            NotificationCategories {
                class_schedule,
                announcements,
                messages,
            }
        }),
    }
}

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
        let key = format!("parent-preferences-{}", uuid::Uuid::new_v4());
        graph.run(query("CREATE(u:User {id:$key,email:$email,role:'parent',test_key:$key})-[:HAS_APPLICATION]->(:Lead {lead_id:$lead,email:$email,status:'verified',test_key:$key})-[:HAS_STUDENT]->(:Student {studentId:$key,fullName:'Synthetic Child',test_key:$key})").param("key",key.clone()).param("lead",format!("LEAD-{key}")).param("email",format!("{key}@example.test"))).await.unwrap();
        Self { graph, key }
    }
    async fn save(&self, input: ParentPreferences) -> bool {
        preferences(&self.graph, &self.key, &input).await.unwrap()
    }
    async fn shown(&self) -> Value {
        parent_snapshot(&self.graph, &self.key).await.unwrap()["preferences"].clone()
    }
    /// The single switch, the three category properties and the version as stored.
    async fn stored(&self) -> (Option<bool>, [Option<bool>; 3], i64) {
        let mut rows = self.graph.execute(query("MATCH(u:User {id:$key}) RETURN u.school_notifications AS single,u.school_notify_class_schedule AS class_schedule,u.school_notify_announcements AS announcements,u.school_notify_messages AS messages,coalesce(u.school_preferences_version,0) AS version").param("key",self.key.clone())).await.unwrap();
        let r = rows.next().await.unwrap().unwrap();
        (
            r.get::<bool>("single").ok(),
            ["class_schedule", "announcements", "messages"].map(|name| r.get::<bool>(name).ok()),
            r.get::<i64>("version").unwrap(),
        )
    }
    async fn run(&self, statement: &str) {
        self.graph
            .run(query(statement).param("key", self.key.clone()))
            .await
            .unwrap();
    }
    async fn audits(&self) -> (i64, Vec<String>) {
        let mut rows = self.graph.execute(query("MATCH(a:SchoolPortalAudit {actor_id:$key}) WITH a ORDER BY a.created_at RETURN count(a) AS total,reduce(names=[],entry IN collect(keys(a)) | names+[name IN entry WHERE NOT name IN names]) AS fields,collect(DISTINCT a.kind) AS kinds").param("key",self.key.clone())).await.unwrap();
        let r = rows.next().await.unwrap().unwrap();
        assert_eq!(
            r.get::<Vec<String>>("kinds").unwrap_or_default().join(","),
            if r.get::<i64>("total").unwrap() == 0 {
                ""
            } else {
                "preferences"
            }
        );
        let mut fields = r.get::<Vec<String>>("fields").unwrap();
        fields.sort();
        (r.get("total").unwrap(), fields)
    }
}

#[tokio::test]
#[ignore = "requires a disposable loopback SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:<port> graph"]
async fn parent_notification_categories_are_stored_derived_and_compatible() {
    let fx = Fixture::connect().await;
    let on = json!({"classSchedule":true,"announcements":true,"messages":true});
    let off = json!({"classSchedule":false,"announcements":false,"messages":false});

    // Never saved: the single switch defaults to on and each category equals it.
    assert_eq!(
        fx.shown().await,
        json!({"version":0,"locale":"en","theme":"system","notifications":true,"notificationCategories":on})
    );
    assert_eq!(fx.stored().await, (None, [None; 3], 0));
    // The snapshot still carries what other features put next to the preferences.
    let snapshot = parent_snapshot(&fx.graph, &fx.key).await.unwrap();
    let mut sections: Vec<&str> = snapshot
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    sections.sort_unstable();
    assert_eq!(
        sections,
        [
            "announcements",
            "articles",
            "attendance",
            "classMeetings",
            "educationCalendar",
            "messages",
            "preferences",
            "records"
        ]
    );
    assert!(snapshot["articles"].as_array().is_some_and(Vec::is_empty));
    assert!(snapshot["announcements"]
        .as_array()
        .is_some_and(Vec::is_empty));

    // Saved earlier by a release without categories: each category equals the single switch.
    fx.run("MATCH(u:User {id:$key}) SET u.school_notifications=false")
        .await;
    let shown = fx.shown().await;
    assert_eq!(shown["notifications"], false);
    assert_eq!(shown["notificationCategories"], off);
    fx.run("MATCH(u:User {id:$key}) REMOVE u.school_notifications")
        .await;

    // The live client's body (no categories) behaves as before and fills all three.
    assert!(fx.save(body(0, false, None)).await);
    assert_eq!(fx.stored().await, (Some(false), [Some(false); 3], 1));
    let shown = fx.shown().await;
    assert_eq!(
        shown,
        json!({"version":1,"locale":"id","theme":"dark","notifications":false,"notificationCategories":off})
    );
    // Same compare-and-set: the same version cannot be saved twice.
    assert!(!fx.save(body(0, true, None)).await);
    assert!(!fx.save(body(0, true, Some([true, true, true]))).await);
    assert_eq!(fx.stored().await, (Some(false), [Some(false); 3], 1));

    // Categories are stored as sent; the single switch becomes their OR whatever was sent for it.
    assert!(fx.save(body(1, false, Some([true, false, false]))).await);
    assert_eq!(
        fx.stored().await,
        (Some(true), [Some(true), Some(false), Some(false)], 2)
    );
    let shown = fx.shown().await;
    assert_eq!(shown["notifications"], true);
    assert_eq!(
        shown["notificationCategories"],
        json!({"classSchedule":true,"announcements":false,"messages":false})
    );
    assert!(fx.save(body(2, true, Some([false, false, false]))).await);
    assert_eq!(fx.stored().await, (Some(false), [Some(false); 3], 3));
    assert!(fx.save(body(3, false, Some([false, true, true]))).await);
    assert_eq!(
        fx.stored().await,
        (Some(true), [Some(false), Some(true), Some(true)], 4)
    );

    // A release without categories saves the single switch only (rollback): it wins on read.
    fx.run(
        "MATCH(u:User {id:$key}) SET u.school_notifications=false,u.school_preferences_version=5",
    )
    .await;
    let shown = fx.shown().await;
    assert_eq!(shown["version"], 5);
    assert_eq!(shown["notifications"], false);
    assert_eq!(shown["notificationCategories"], off);
    // The next save without categories brings all three back in line.
    assert!(fx.save(body(5, true, None)).await);
    assert_eq!(fx.stored().await, (Some(true), [Some(true); 3], 6));
    assert_eq!(fx.shown().await["notificationCategories"], on);

    // One audit record per accepted save, with ids and the kind only.
    assert_eq!(
        fx.audits().await,
        (
            5,
            vec![
                "actor_id".to_string(),
                "created_at".to_string(),
                "id".to_string(),
                "kind".to_string()
            ]
        )
    );

    // A User that is no longer a plain Parent cannot save, with or without categories.
    fx.run("MATCH(u:User {id:$key}) SET u.role='staff'").await;
    assert!(!fx.save(body(6, false, Some([false, false, false]))).await);
    assert!(!fx.save(body(6, false, None)).await);
    assert!(parent_snapshot(&fx.graph, &fx.key).await.is_err());
    assert_eq!(fx.stored().await, (Some(true), [Some(true); 3], 6));
    assert_eq!(fx.audits().await.0, 5);
    // Another Parent's id never reaches this User.
    assert!(
        !preferences(&fx.graph, "someone-else", &body(6, false, None))
            .await
            .unwrap()
    );

    fx.run("MATCH(a:SchoolPortalAudit {actor_id:$key}) DETACH DELETE a")
        .await;
    fx.run("MATCH(n {test_key:$key}) DETACH DELETE n").await;
}
