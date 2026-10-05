//! Student account settings belong to the current Auth User, not a caller-selected child.
use super::{
    failure,
    model::{identifier, Preferences},
    Failure,
};
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use neo4rs::{query, Graph, Query as GraphQuery};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scope {
    school_id: String,
    tenant_id: String,
}
const OWNED:&str="MATCH(u:User {id:$actor,role:'student'}) MATCH(b:LearningStudentBinding {user_id:u.id,school_id:$school,tenant_id:$tenant,status:'ACTIVE'}) MATCH(s:Student {studentId:b.student_id})-[:ENROLLED_AS]->(en:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant})-[:ENROLLED_IN]->(sec:Section {status:'active',school_id:$school,tenant_id:$tenant}) WHERE coalesce(u.staffMemberId,'')='' AND all(role IN coalesce(u.roles,[]) WHERE role='student') AND all(role IN coalesce(u.marketingRoles,[]) WHERE role='student') AND NOT (u)-[:STAFF_PROFILE]->() AND NOT EXISTS {MATCH(staff:StaffMember) WHERE toLower(staff.email)=toLower(u.email)}";
const CURRENT:&str="u.role='student' AND coalesce(u.staffMemberId,'')='' AND all(role IN coalesce(u.roles,[]) WHERE role='student') AND all(role IN coalesce(u.marketingRoles,[]) WHERE role='student') AND NOT (u)-[:STAFF_PROFILE]->() AND NOT EXISTS {MATCH(staff:StaffMember) WHERE toLower(staff.email)=toLower(u.email)} AND b.status='ACTIVE' AND b.user_id=u.id AND b.school_id=$school AND b.tenant_id=$tenant AND b.student_id=s.studentId AND en.status='active' AND en.school_id=$school AND en.tenant_id=$tenant AND sec.status='active' AND sec.school_id=$school AND sec.tenant_id=$tenant AND EXISTS { MATCH(s)-[:ENROLLED_AS]->(en)-[:ENROLLED_IN]->(sec) }";
const FIELDS:&str="coalesce(u.student_preferences_version,0) AS version,coalesce(u.student_locale,'en') AS locale,coalesce(u.student_theme,'system') AS theme,coalesce(u.student_notifications,true) AS notifications";
fn scoped(body: &str, principal: &str, scope: &Scope) -> GraphQuery {
    query(body)
        .param("actor", principal)
        .param("school", scope.school_id.clone())
        .param("tenant", scope.tenant_id.clone())
}
type Error = Box<dyn std::error::Error + Send + Sync>;
async fn load(graph: &Graph, principal: &str, scope: &Scope) -> Result<Option<Value>, Error> {
    let mut rows=graph.execute(scoped(&format!("{OWNED} WITH u,collect(DISTINCT s.studentId) AS students WHERE size(students)=1 RETURN students[0] AS student,{FIELDS}"),principal,scope)).await?;
    let Some(r) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(
        json!({"contractVersion":1,"actor":{"id":principal,"role":"student"},"schoolId":scope.school_id,"tenantId":scope.tenant_id,"studentId":r.get::<String>("student")?,"preferences":{"version":r.get::<i64>("version")?,"locale":r.get::<String>("locale")?,"theme":r.get::<String>("theme")?,"notifications":r.get::<bool>("notifications")?}}),
    ))
}
#[cfg(test)]
async fn save(
    graph: &Graph,
    principal: &str,
    scope: &Scope,
    input: &Preferences,
) -> Result<Option<Value>, Error> {
    save_inner(graph, principal, scope, input, None).await
}
async fn save_inner(
    graph: &Graph,
    principal: &str,
    scope: &Scope,
    input: &Preferences,
    headers: Option<&HeaderMap>,
) -> Result<Option<Value>, Error> {
    // Lock current binding/enrollment as well as the account before rechecking.
    // Directory, binding and enrollment revocation cannot race a queued save.
    let mut tx = graph.start_txn().await?;
    let result: Result<Option<Value>, Error> = async {
        let lock=format!("{OWNED} WITH DISTINCT u,b,s,en,sec SET u.student_preferences_lock=coalesce(u.student_preferences_lock,0)+1,b.student_preferences_lock=coalesce(b.student_preferences_lock,0)+1,en.student_preferences_lock=coalesce(en.student_preferences_lock,0)+1,sec.student_preferences_lock=coalesce(sec.student_preferences_lock,0)+1 RETURN u.id AS id");
        let mut locked = tx.execute(scoped(&lock, principal, scope)).await?;
        if locked.next(&mut tx).await?.is_none() { return Ok(None); }
        while locked.next(&mut tx).await?.is_some() {}
        if let Some(headers) = headers {
            let current = super::student_identity::actor(headers).await.map_err(|error| {
                Box::new(super::staff_capability::SessionFailure(error)) as Error
            })?;
            if current != principal { return Err(Box::new(super::staff_capability::SessionFailure(failure(StatusCode::FORBIDDEN))) as Error); }
        }
        let body=format!("{OWNED} WITH DISTINCT u,b,s,en,sec WHERE {CURRENT} WITH u,collect(DISTINCT s.studentId) AS students WHERE size(students)=1 WITH u,students,coalesce(u.student_preferences_version,0) AS previous WHERE previous=$version OR (previous=$version+1 AND u.student_locale=$locale AND u.student_theme=$theme AND u.student_notifications=$notifications) FOREACH(ignore IN CASE WHEN previous=$version THEN [1] ELSE [] END | SET u.student_preferences_version=$version+1,u.student_locale=$locale,u.student_theme=$theme,u.student_notifications=$notifications CREATE(:SchoolPortalAudit {{id:$audit,actor_id:u.id,student_id:students[0],school_id:$school,tenant_id:$tenant,kind:'student_preferences',version:$version+1,created_at:datetime()}})) RETURN students[0] AS student,{FIELDS}");
        let mut rows = tx.execute(
            scoped(&body, principal, scope)
                .param("version", i64::from(input.version))
                .param("locale", input.locale.clone())
                .param("theme", input.theme.clone())
                .param("notifications", input.notifications)
                .param("audit", uuid::Uuid::new_v4().to_string()),
        )
        .await?;
    let Some(r) = rows.next(&mut tx).await? else {
        return Ok(None);
    };
    let value = json!({"contractVersion":1,"actor":{"id":principal,"role":"student"},"schoolId":scope.school_id,"tenantId":scope.tenant_id,"studentId":r.get::<String>("student")?,"preferences":{"version":r.get::<i64>("version")?,"locale":r.get::<String>("locale")?,"theme":r.get::<String>("theme")?,"notifications":r.get::<bool>("notifications")?}});
    if rows.next(&mut tx).await?.is_some() { return Err("Ambiguous Student preferences".into()); }
    Ok(Some(value))
    }.await;
    match result {
        Ok(Some(value)) => {
            tx.commit().await?;
            Ok(Some(value))
        }
        Ok(None) => {
            tx.rollback().await?;
            Ok(None)
        }
        Err(error) => {
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}
pub(super) async fn read(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(scope): Query<Scope>,
) -> Result<Json<Value>, Failure> {
    if !identifier(&scope.school_id) || !identifier(&scope.tenant_id) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let principal = super::student_identity::actor(&h).await?;
    load(&s.graph, &principal, &scope)
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .map(|v| Json(json!({"data":v})))
        .ok_or_else(|| failure(StatusCode::FORBIDDEN))
}
pub(super) async fn write(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(scope): Query<Scope>,
    input: Result<Json<Preferences>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(input) = input.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !identifier(&scope.school_id) || !identifier(&scope.tenant_id) || !input.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let principal = super::student_identity::actor(&h).await?;
    if load(&s.graph, &principal, &scope)
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .is_none()
    {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    save_inner(&s.graph, &principal, &scope, &input, Some(&h))
        .await
        .map_err(
            |error| match error.downcast::<super::staff_capability::SessionFailure>() {
                Ok(error) => error.0,
                Err(_) => failure(StatusCode::SERVICE_UNAVAILABLE),
            },
        )?
        .map(|v| Json(json!({"data":v})))
        .ok_or_else(|| failure(StatusCode::CONFLICT))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires explicit local SCHOOL_WORKFLOW_TEST_BOLT; removes its own marked fixture only"]
    async fn owned_preferences_cas_replay_and_queued_revocation() {
        let uri = std::env::var("SCHOOL_WORKFLOW_TEST_BOLT").unwrap();
        assert_eq!(uri, "127.0.0.1:3213");
        let g = Graph::new(
            uri,
            &std::env::var("NEO4J_USER").unwrap(),
            &std::env::var("NEO4J_PASSWORD").unwrap(),
        )
        .await
        .unwrap();
        let mark = format!("school-pref-check-{}", uuid::Uuid::new_v4());
        let scope = Scope {
            school_id: mark.clone(),
            tenant_id: mark.clone(),
        };
        g.run(query("CREATE(u:User {id:$mark,email:$email,role:'student',workflow_test:$mark}) CREATE(b:LearningStudentBinding {user_id:$mark,student_id:$mark,school_id:$mark,tenant_id:$mark,status:'ACTIVE',workflow_test:$mark}) CREATE(:Student {studentId:$mark,fullName:'Isolated preference check',workflow_test:$mark})-[:ENROLLED_AS]->(en:EnrolledStudent {status:'active',school_id:$mark,tenant_id:$mark,workflow_test:$mark})-[:ENROLLED_IN]->(:Section {section_id:$mark,name:'Isolated check',status:'active',school_id:$mark,tenant_id:$mark,workflow_test:$mark})").param("mark",mark.clone()).param("email",format!("{mark}@example.test"))).await.unwrap();
        let mut input = Preferences {
            version: 0,
            locale: "id".into(),
            theme: "system".into(),
            notifications: true,
        };
        assert_eq!(
            load(&g, &mark, &scope).await.unwrap().unwrap()["preferences"]["version"],
            0
        );
        assert!(load(&g, "foreign", &scope).await.unwrap().is_none());
        assert!(load(
            &g,
            &mark,
            &Scope {
                school_id: "foreign".into(),
                tenant_id: mark.clone()
            }
        )
        .await
        .unwrap()
        .is_none());
        assert_eq!(
            save(&g, &mark, &scope, &input).await.unwrap().unwrap()["preferences"]["version"],
            1
        );
        assert_eq!(
            save(&g, &mark, &scope, &input).await.unwrap().unwrap()["preferences"]["version"],
            1
        );
        input.locale = "en".into();
        assert!(save(&g, &mark, &scope, &input).await.unwrap().is_none());
        input.version = 1;
        let a = Preferences {
            version: 1,
            locale: "en".into(),
            theme: "dark".into(),
            notifications: true,
        };
        let b = Preferences {
            version: 1,
            locale: "id".into(),
            theme: "light".into(),
            notifications: false,
        };
        let (a, b) = tokio::join!(save(&g, &mark, &scope, &a), save(&g, &mark, &scope, &b));
        assert_eq!(
            usize::from(a.unwrap().is_some()) + usize::from(b.unwrap().is_some()),
            1
        );
        input.version = 2;
        // Each canonical owner can revoke while a settings save waits for its lock.
        for (label, property, revoked, active) in [
            ("User", "role", "parent", "student"),
            ("LearningStudentBinding", "status", "REVOKED", "ACTIVE"),
            ("EnrolledStudent", "status", "inactive", "active"),
            ("Section", "status", "inactive", "active"),
        ] {
            let mut tx = g.start_txn().await.unwrap();
            tx.run(
                query(&format!(
                    "MATCH(n:{label} {{workflow_test:$mark}}) SET n.{property}=$revoked"
                ))
                .param("mark", mark.clone())
                .param("revoked", revoked),
            )
            .await
            .unwrap();
            let queued = save(&g, &mark, &scope, &input);
            let commit = async {
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                tx.commit().await.unwrap();
            };
            let (saved, ()) = tokio::join!(queued, commit);
            assert!(
                saved.unwrap().is_none(),
                "revoked {label} cannot save preferences"
            );
            assert!(load(&g, &mark, &scope).await.unwrap().is_none());
            g.run(
                query(&format!(
                    "MATCH(n:{label} {{workflow_test:$mark}}) SET n.{property}=$active"
                ))
                .param("mark", mark.clone())
                .param("active", active),
            )
            .await
            .unwrap();
        }
        let mut tx = g.start_txn().await.unwrap();
        tx.run(query("MATCH(:EnrolledStudent {workflow_test:$mark})-[edge:ENROLLED_IN]->(:Section {workflow_test:$mark}) DELETE edge").param("mark",mark.clone())).await.unwrap();
        let queued = save(&g, &mark, &scope, &input);
        let commit = async {
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            tx.commit().await.unwrap();
        };
        let (saved, ()) = tokio::join!(queued, commit);
        assert!(saved.unwrap().is_none());
        assert!(load(&g, &mark, &scope).await.unwrap().is_none());
        g.run(query("MATCH(en:EnrolledStudent {workflow_test:$mark}),(sec:Section {workflow_test:$mark}) CREATE(en)-[:ENROLLED_IN]->(sec)").param("mark",mark.clone())).await.unwrap();
        assert_eq!(
            load(&g, &mark, &scope).await.unwrap().unwrap()["preferences"]["version"],
            2
        );
        let mut rows=g.execute(query("MATCH(a:SchoolPortalAudit {actor_id:$mark,kind:'student_preferences'}) RETURN count(a) AS count").param("mark",mark.clone())).await.unwrap();
        assert_eq!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<i64>("count")
                .unwrap(),
            2
        );
        g.run(query("MATCH(n) WHERE n.workflow_test=$mark OR (n:SchoolPortalAudit AND n.actor_id=$mark AND n.kind='student_preferences') DETACH DELETE n").param("mark",mark.clone())).await.unwrap();
    }
}

#[cfg(test)]
mod session_tests;
