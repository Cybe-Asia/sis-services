//! Who a school broadcast reaches, read from SIS within one school, and the one-time claim that
//! makes a send idempotent. Every query names `$school` and `$tenant`, so ids of another school
//! match nothing.
use super::Audience;
use crate::school_articles::repository::{manager, Actor};
use crate::school_portal::repository::PARENT_ELIGIBLE;
use neo4rs::{query, Graph, Query};
use serde_json::{json, Value};
pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// One push recipient: `parent:<User.id>` or `staff:<StaffMember.id>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipient {
    pub id: String,
    /// The Parent's User id, for the board notice; none for staff.
    pub user: Option<String>,
    pub locale: String,
    /// False when the Parent turned announcement notifications off: the notice is still posted.
    pub notify: bool,
}

pub async fn init(graph: &Graph) -> Result<(), Error> {
    graph
        .run(query("CREATE CONSTRAINT school_broadcast_id IF NOT EXISTS FOR(n:SchoolBroadcast) REQUIRE n.id IS UNIQUE"))
        .await?;
    Ok(())
}

const SECTION: &str = "(sec:Section {status:'active',school_id:$school,tenant_id:$tenant})";
const GRADE: &str = "coalesce(sec.grade_code,sec.year_group)";

fn enrolled() -> String {
    format!("MATCH(s:Student)-[:ENROLLED_AS]->(:EnrolledStudent {{status:'active',school_id:$school,tenant_id:$tenant}})-[:ENROLLED_IN]->{SECTION}")
}

fn scoped(cypher: &str, school: &str, tenant: &str) -> Query {
    query(cypher)
        .param("school", school)
        .param("tenant", tenant)
}

/// How many of the requested ids exist in this school. Fewer than requested means at least one
/// belongs elsewhere (or nowhere), and the caller refuses the whole audience.
pub async fn known(
    graph: &Graph,
    school: &str,
    tenant: &str,
    audience: &Audience,
) -> Result<bool, Error> {
    let cypher = match audience {
        Audience::School {} | Audience::Staff {} => return Ok(true),
        Audience::Grade { .. } => format!("MATCH{SECTION} WITH DISTINCT {GRADE} AS id WHERE id IN $ids RETURN count(id) AS known"),
        Audience::Class { .. } => format!("MATCH{SECTION} WHERE sec.section_id IN $ids RETURN count(DISTINCT sec.section_id) AS known"),
        Audience::Students { .. } => format!("{} WHERE s.studentId IN $ids RETURN count(DISTINCT s.studentId) AS known", enrolled()),
    };
    let ids = audience.ids();
    let mut rows = graph
        .execute(scoped(&cypher, school, tenant).param("ids", ids.to_vec()))
        .await?;
    let found: i64 = match rows.next().await? {
        Some(row) => row.get("known")?,
        None => 0,
    };
    Ok(usize::try_from(found).is_ok_and(|n| n == ids.len()))
}

/// Everyone the audience reaches today, without writing anything (also the dry-run count).
pub async fn recipients(
    graph: &Graph,
    school: &str,
    tenant: &str,
    audience: &Audience,
) -> Result<Vec<Recipient>, Error> {
    let filter = match audience {
        Audience::Staff {} => {
            let cypher = "MATCH(m:StaffMember {membershipStatus:'ACTIVE'}) WHERE $school IN coalesce(m.schoolIds,[]) AND $tenant IN coalesce(m.tenantIds,[]) RETURN DISTINCT 'staff:'+m.id AS id,null AS user,'id' AS locale,true AS notify ORDER BY id";
            return collect(graph, scoped(cypher, school, tenant)).await;
        }
        Audience::School {} => "true".to_string(),
        Audience::Grade { .. } => format!("{GRADE} IN $ids"),
        Audience::Class { .. } => "sec.section_id IN $ids".into(),
        Audience::Students { .. } => "s.studentId IN $ids".into(),
    };
    let cypher = format!(
        "{} MATCH(u:User)-[:HAS_APPLICATION]->(l:Lead)-[:HAS_STUDENT]->(s) WHERE {filter} AND {PARENT_ELIGIBLE} \
         WITH DISTINCT u RETURN 'parent:'+u.id AS id,u.id AS user,coalesce(u.school_locale,'en') AS locale, \
         coalesce(u.school_notifications,true) AND coalesce(u.school_notify_announcements,true) AS notify ORDER BY id",
        enrolled()
    );
    collect(
        graph,
        scoped(&cypher, school, tenant).param("ids", audience.ids().to_vec()),
    )
    .await
}

async fn collect(graph: &Graph, q: Query) -> Result<Vec<Recipient>, Error> {
    let mut rows = graph.execute(q).await?;
    let mut out = vec![];
    while let Some(r) = rows.next().await? {
        out.push(Recipient {
            id: r.get("id")?,
            user: r.get::<Option<String>>("user")?,
            locale: r.get("locale")?,
            notify: r.get("notify")?,
        });
    }
    Ok(out)
}

pub struct Outgoing<'a> {
    pub id: &'a str,
    pub school: &'a str,
    pub tenant: &'a str,
    pub audience: &'a Audience,
    pub title: &'a str,
    pub body: &'a str,
}

#[derive(Debug, PartialEq)]
pub enum Claim {
    /// First send of this id: notices are posted, the caller pushes.
    Fresh,
    /// Sent before for this school: nothing is posted or pushed again.
    Duplicate { recipients: i64, status: String },
    /// The id belongs to another school's broadcast.
    Conflict,
    /// The actor may not manage this school's content (checked on their node again).
    Forbidden,
}

/// Claims the broadcast id and, only on the first claim, posts one board notice per Parent, in one
/// statement. A retry with the same id (a double click, a proxy retry) finds the claim and stops.
pub async fn claim(
    graph: &Graph,
    actor: Actor<'_>,
    send: &Outgoing<'_>,
    recipients: &[Recipient],
) -> Result<Claim, Error> {
    let manager = manager();
    let users: Vec<String> = recipients.iter().filter_map(|r| r.user.clone()).collect();
    let cypher = format!(
        "{manager} WITH actor LIMIT 1 MERGE(b:SchoolBroadcast {{id:$id}}) ON CREATE SET b.claim=$attempt,b.school_id=$school,b.tenant_id=$tenant,\
         b.actor_id=actor.id,b.audience=$audience,b.audience_ids=$ids,b.title=$title,b.body=$body,b.recipients=$count,b.status='sending',b.created_at=datetime() \
         WITH b,b.claim=$attempt AS fresh \
         CALL {{ WITH b,fresh WITH b WHERE fresh UNWIND $users AS uid MATCH(u:User {{id:uid}}) \
           MERGE(n:SchoolNotice {{id:'broadcast:'+b.id+':'+uid}}) ON CREATE SET n.title=$title,n.body=$body,n.created_at=datetime(),n.is_read=false \
           MERGE(u)-[:HAS_SCHOOL_NOTICE]->(n) RETURN count(n) AS posted }} \
         RETURN fresh,b.school_id=$school AND b.tenant_id=$tenant AS same,b.recipients AS recipients,b.status AS status"
    );
    let q = query(&cypher)
        .param("actor", actor.id)
        .param("capable", actor.capable)
        .param("school", send.school)
        .param("tenant", send.tenant)
        .param("id", send.id)
        .param("attempt", uuid::Uuid::new_v4().to_string())
        .param("audience", send.audience.kind())
        .param("ids", send.audience.ids().to_vec())
        .param("title", send.title)
        .param("body", send.body)
        .param("count", recipients.len() as i64)
        .param("users", users);
    let mut rows = graph.execute(q).await?;
    let Some(row) = rows.next().await? else {
        return Ok(Claim::Forbidden);
    };
    Ok(
        match (row.get::<bool>("fresh")?, row.get::<bool>("same")?) {
            (true, _) => Claim::Fresh,
            (false, false) => Claim::Conflict,
            (false, true) => Claim::Duplicate {
                recipients: row.get("recipients")?,
                status: row.get("status")?,
            },
        },
    )
}

/// Records the push outcome once the fan-out ends.
pub async fn finish(graph: &Graph, id: &str, pushed: i64, failed: i64) -> Result<(), Error> {
    graph
        .run(query("MATCH(b:SchoolBroadcast {id:$id}) SET b.status='sent',b.pushed=$pushed,b.push_failed=$failed,b.finished_at=datetime()")
            .param("id", id)
            .param("pushed", pushed)
            .param("failed", failed))
        .await?;
    Ok(())
}

/// What the admin picks from: the school's grades, active classes and enrolled students.
pub async fn audiences(graph: &Graph, school: &str, tenant: &str) -> Result<Value, Error> {
    let mut grades = vec![];
    let mut rows = graph
        .execute(scoped(&format!("MATCH{SECTION} WITH DISTINCT {GRADE} AS id WHERE id IS NOT NULL RETURN id ORDER BY id LIMIT 100"), school, tenant))
        .await?;
    while let Some(r) = rows.next().await? {
        grades.push(json!({"id": r.get::<String>("id")?}));
    }
    let mut classes = vec![];
    let mut rows = graph
        .execute(scoped(&format!("MATCH{SECTION} RETURN sec.section_id AS id,coalesce(sec.name,sec.section_id) AS name,{GRADE} AS grade ORDER BY name,id LIMIT 500"), school, tenant))
        .await?;
    while let Some(r) = rows.next().await? {
        classes.push(json!({"id": r.get::<String>("id")?, "name": r.get::<String>("name")?, "grade": r.get::<Option<String>>("grade")?}));
    }
    let mut students = vec![];
    let mut rows = graph
        .execute(scoped(&format!("{} RETURN DISTINCT s.studentId AS id,coalesce(s.fullName,'') AS name,sec.section_id AS class ORDER BY name,id LIMIT 3000", enrolled()), school, tenant))
        .await?;
    while let Some(r) = rows.next().await? {
        students.push(json!({"id": r.get::<String>("id")?, "name": r.get::<String>("name")?, "classId": r.get::<String>("class")?}));
    }
    Ok(json!({"grades": grades, "classes": classes, "students": students}))
}

#[cfg(test)]
mod database_tests;
