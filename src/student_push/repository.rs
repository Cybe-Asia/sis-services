//! Graph reads and writes for Student push: who of a class may receive one, and the once-per-fact claim.
use crate::school_broadcasts::repository::Recipient;
use crate::school_portal::model::StudentNotificationCategories;
use neo4rs::{query, Graph};

type Error = Box<dyn std::error::Error + Send + Sync>;

pub async fn init(graph: &Graph) -> Result<(), Error> {
    graph
        .run(query(
            "CREATE CONSTRAINT student_push_key IF NOT EXISTS FOR(n:StudentPush) REQUIRE n.key IS UNIQUE",
        ))
        .await?;
    Ok(())
}

/// A Student account that may hold a push device: the role is only `student`, no staff identity, and an active
/// binding to a Student enrolled in an active class of the school (as for Student preferences).
const STUDENT_ACCOUNT: &str = "u.role='student' AND coalesce(u.staffMemberId,'')='' AND all(role IN coalesce(u.roles,[]) WHERE role='student') AND NOT (u)-[:STAFF_PROFILE]->()";
const ENROLLED: &str = "(s:Student)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant})-[:ENROLLED_IN]->(sec:Section {status:'active',school_id:$school,tenant_id:$tenant})";

/// True when `user` is a Student account with an active binding and class in the school.
pub async fn student_in_school(
    graph: &Graph,
    user: &str,
    school: &str,
    tenant: &str,
) -> Result<bool, Error> {
    let mut rows = graph
        .execute(
            query(&format!("MATCH(u:User {{id:$user}}) WHERE {STUDENT_ACCOUNT} MATCH(b:LearningStudentBinding {{user_id:u.id,status:'ACTIVE',school_id:$school,tenant_id:$tenant}}) MATCH{ENROLLED} WHERE s.studentId=b.student_id RETURN count(*)>0 AS ok"))
                .param("user", user)
                .param("school", school)
                .param("tenant", tenant),
        )
        .await?;
    Ok(match rows.next().await? {
        Some(row) => row.get::<bool>("ok")?,
        None => false,
    })
}

/// True when `user` is a Student account with an active binding and class in any school: the app's device
/// belongs to the Student's account, and the app does not know its school's ids.
pub async fn student_enrolled(graph: &Graph, user: &str) -> Result<bool, Error> {
    let enrolled = ENROLLED
        .replace("$school", "b.school_id")
        .replace("$tenant", "b.tenant_id");
    let mut rows = graph
        .execute(
            query(&format!("MATCH(u:User {{id:$user}}) WHERE {STUDENT_ACCOUNT} MATCH(b:LearningStudentBinding {{user_id:u.id,status:'ACTIVE'}}) MATCH{enrolled} WHERE s.studentId=b.student_id RETURN count(*)>0 AS ok"))
                .param("user", user),
        )
        .await?;
    Ok(match rows.next().await? {
        Some(row) => row.get::<bool>("ok")?,
        None => false,
    })
}

/// The Student ids of `students` that are not enrolled in the class (empty when all are).
pub async fn outside_class(
    graph: &Graph,
    class: &str,
    school: &str,
    tenant: &str,
    students: &[String],
) -> Result<Vec<String>, Error> {
    let mut rows = graph
        .execute(
            query(&format!("OPTIONAL MATCH{ENROLLED} WHERE sec.section_id=$class AND s.studentId IN $students WITH collect(DISTINCT s.studentId) AS inside RETURN [id IN $students WHERE NOT id IN inside] AS outside"))
                .param("class", class)
                .param("school", school)
                .param("tenant", tenant)
                .param("students", students.to_vec()),
        )
        .await?;
    Ok(match rows.next().await? {
        Some(row) => row.get::<Vec<String>>("outside")?,
        None => students.to_vec(),
    })
}

/// The Student accounts of the class (or of `students` in it) as push recipients `student:<User.id>`.
/// `notify` follows the Student's own switch and the category of `category` (a `STORED` name).
pub async fn recipients(
    graph: &Graph,
    class: &str,
    school: &str,
    tenant: &str,
    students: Option<&[String]>,
    category: usize,
) -> Result<Vec<Recipient>, Error> {
    let fields = StudentNotificationCategories::STORED
        .map(|name| format!("u.student_notify_{name} AS {name}"))
        .join(",");
    let mut rows = graph
        .execute(
            query(&format!("MATCH{ENROLLED} WHERE sec.section_id=$class AND ($students IS NULL OR s.studentId IN $students) MATCH(b:LearningStudentBinding {{student_id:s.studentId,status:'ACTIVE',school_id:$school,tenant_id:$tenant}}) MATCH(u:User {{id:b.user_id}}) WHERE {STUDENT_ACCOUNT} RETURN DISTINCT u.id AS id,coalesce(u.student_locale,'en') AS locale,coalesce(u.student_notifications,true) AS notifications,{fields} ORDER BY id LIMIT 1001"))
                .param("class", class)
                .param("school", school)
                .param("tenant", tenant)
                .param("students", students.map(<[String]>::to_vec)),
        )
        .await?;
    let mut found = Vec::new();
    while let Some(row) = rows.next().await? {
        if found.len() == 1000 {
            return Err("more Student accounts than a class holds".into());
        }
        let notifications = row.get::<bool>("notifications")?;
        let saved = StudentNotificationCategories::STORED
            .map(|name| row.get::<Option<bool>>(name).ok().flatten());
        let user = row.get::<String>("id")?;
        found.push(Recipient {
            id: format!("student:{user}"),
            user: Some(user),
            locale: row.get::<String>("locale")?,
            notify: notifications
                && StudentNotificationCategories::stored(notifications, saved).values()[category],
        });
    }
    Ok(found)
}

pub enum Claim {
    Fresh,
    Duplicate,
    /// The class already had its daily number of pushes.
    Capped,
}

/// Claims `key` once. A key seen before is a duplicate; a new key past `cap` claims of the class on `day` is capped.
#[allow(clippy::too_many_arguments)]
pub async fn claim(
    graph: &Graph,
    key: &str,
    kind: &str,
    class: &str,
    school: &str,
    tenant: &str,
    day: &str,
    cap: i64,
    recipients: i64,
) -> Result<Claim, Error> {
    let attempt = uuid::Uuid::new_v4().to_string();
    let mut rows = graph
        .execute(query("OPTIONAL MATCH(o:StudentPush {class_id:$class,school_id:$school,tenant_id:$tenant,day:$day}) WITH count(o) AS today OPTIONAL MATCH(e:StudentPush {key:$key}) WITH today,e WHERE e IS NOT NULL OR today<$cap MERGE(p:StudentPush {key:$key}) ON CREATE SET p.claim=$attempt,p.kind=$kind,p.class_id=$class,p.school_id=$school,p.tenant_id=$tenant,p.day=$day,p.recipients=$recipients,p.status='sending',p.created_at=datetime() RETURN p.claim=$attempt AS fresh")
            .param("key", key)
            .param("kind", kind)
            .param("class", class)
            .param("school", school)
            .param("tenant", tenant)
            .param("day", day)
            .param("cap", cap)
            .param("recipients", recipients)
            .param("attempt", attempt))
        .await?;
    let claim = match rows.next().await? {
        Some(row) if row.get::<bool>("fresh")? => Claim::Fresh,
        Some(_) => Claim::Duplicate,
        None => Claim::Capped,
    };
    while rows.next().await?.is_some() {}
    Ok(claim)
}

/// Records the outcome once the fan-out ends; `skipped` counts opted-out or quiet-hours recipients.
pub async fn finish(
    graph: &Graph,
    key: &str,
    pushed: i64,
    failed: i64,
    skipped: i64,
) -> Result<(), Error> {
    graph
        .run(query("MATCH(p:StudentPush {key:$key}) SET p.status='sent',p.pushed=$pushed,p.push_failed=$failed,p.skipped=$skipped,p.finished_at=datetime()")
            .param("key", key)
            .param("pushed", pushed)
            .param("failed", failed)
            .param("skipped", skipped))
        .await?;
    Ok(())
}
