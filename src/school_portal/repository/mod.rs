use super::auth::Teacher;
use neo4rs::{query, Graph, Query};
use serde_json::Value;
type Error = Box<dyn std::error::Error + Send + Sync>;
const HOMEROOM: &str = "coalesce(sec.homeroom_staff_member_id=t.id,false)";
const TEACHER_BASE: &str = "MATCH(t:StaffMember {id:$actor}),(sec:Section)";
const GRANT: &str = "EXISTS { MATCH(g:LearningTeachingGrant {staff_member_id:t.id,section_id:sec.section_id,school_id:sec.school_id,tenant_id:sec.tenant_id,status:'ACTIVE'}) }";
pub(super) const ENROLLED: &str = "MATCH(s:Student)-[:ENROLLED_AS]->(e:EnrolledStudent {status:'active',school_id:sec.school_id,tenant_id:sec.tenant_id})-[:ENROLLED_IN]->(sec)";
pub(crate) const PARENT: &str = "MATCH(u:User)-[:HAS_APPLICATION]->(l:Lead)-[:HAS_STUDENT]->(s:Student) WHERE (u.id=$sub OR l.lead_id=$sub) AND coalesce(u.role,'parent')='parent' AND coalesce(u.staffMemberId,'')='' AND all(role IN coalesce(u.roles,[]) WHERE role='parent') AND all(role IN coalesce(u.marketingRoles,[]) WHERE role='parent') AND NOT (u)-[:STAFF_PROFILE]->() AND NOT EXISTS { MATCH(staff:StaffMember) WHERE toLower(staff.email)=toLower(u.email) } AND l.status IN ['verified','paid'] AND toLower(u.email)=toLower(l.email)";
fn parent_write_prefix() -> String {
    format!("{PARENT} WITH collect(DISTINCT u) AS owners WHERE size(owners)=1 UNWIND owners AS owner SET owner.school_portal_lock=coalesce(owner.school_portal_lock,0)+1 WITH owner {PARENT} AND u=owner")
}
pub(super) const PARENT_SECTION: &str = "MATCH(s)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active'})-[:ENROLLED_IN]->(sec:Section {status:'active'}) WHERE coalesce(sec.school_id,'')<>'' AND coalesce(sec.tenant_id,'')<>'' AND EXISTS { MATCH(s)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:sec.school_id,tenant_id:sec.tenant_id})-[:ENROLLED_IN]->(sec) }";
pub(super) fn teacher_query(
    member: &Teacher,
    body: &str,
    section: Option<&str>,
    write: bool,
    homeroom_only: bool,
) -> Query {
    let lock = if write {
        " SET t.school_portal_lock=coalesce(t.school_portal_lock,0)+1,sec.school_portal_lock=coalesce(sec.school_portal_lock,0)+1 WITH t,sec"
    } else {
        " WITH t,sec"
    };
    let selected = if section.is_some() {
        " AND sec.section_id=$section"
    } else {
        ""
    };
    let owner =
        crate::school_portal::staff_capability::owner("t", "sec.school_id", "sec.tenant_id");
    let current=format!("t.membershipStatus='ACTIVE' AND sec.status='active' AND sec.school_id IN $schools AND sec.tenant_id IN $tenants AND ((NOT $owner AND NOT 'owner' IN coalesce(t.roles,[]) AND 'teacher' IN coalesce(t.roles,[]) AND sec.school_id IN coalesce(t.schoolIds,[]) AND sec.tenant_id IN coalesce(t.tenantIds,[])) OR ($owner AND {owner} AND size($schools)=1 AND size($tenants)=1))");
    let scope = if homeroom_only {
        HOMEROOM.to_string()
    } else {
        format!("({HOMEROOM} OR {GRANT})")
    };
    let scope = format!("({scope} OR ($owner AND {owner}))");
    query(&format!("{TEACHER_BASE} WHERE sec.section_id IS NOT NULL{selected} AND {current} AND {scope}{lock} WHERE {current} AND {scope} WITH t,sec,{HOMEROOM} AS homeroom,($owner AND {owner}) AS management {body}"))
        .param("owner",member.is_owner()).param("actor_role",member.role()).param("actor",member.staff_member_id.clone()).param("schools",member.school_ids.clone()).param("tenants",member.tenant_ids.clone()).param("section",section.unwrap_or(""))
}
async fn values(graph: &Graph, q: Query) -> Result<Vec<Value>, Error> {
    let mut rows = graph.execute(q).await?;
    let mut out = vec![];
    while let Some(row) = rows.next().await? {
        if out.len() >= 1000 {
            return Err("school record limit exceeded".into());
        }
        let raw: String = row.get("payload")?;
        out.push(serde_json::from_str(&raw)?);
    }
    Ok(out)
}
pub async fn init(graph: &Graph) -> Result<(), Error> {
    super::assessment_grades::init(graph).await?;
    for (name, label, field) in [
        ("school_record_key", "SchoolPublishedRecord", "key"),
        ("school_audit_id", "SchoolPortalAudit", "id"),
        ("school_rsvp_key", "SchoolRsvp", "key"),
        ("school_notice_id", "SchoolNotice", "id"),
    ] {
        graph
            .run(query(&format!(
                "CREATE CONSTRAINT {name} IF NOT EXISTS FOR(n:{label}) REQUIRE n.{field} IS UNIQUE"
            )))
            .await?;
    }
    graph
        .run(query(
            "MATCH(r:ParentSchoolRequest) WHERE r.public_id IS NULL SET r.public_id=randomUUID()",
        ))
        .await?;
    graph.run(query("CREATE CONSTRAINT school_request_public_id IF NOT EXISTS FOR(r:ParentSchoolRequest) REQUIRE r.public_id IS UNIQUE")).await?;
    Ok(())
}

mod parent;
mod teacher;
pub use parent::{mark_read, parent_calendar, parent_snapshot, preferences, rsvp};
#[cfg(test)]
pub use teacher::{attendance, publish, review};
pub use teacher::{
    attendance_live, classroom, context, publish_live, review_live, teacher_calendar,
};

mod growth;
pub use growth::parent_growth;
