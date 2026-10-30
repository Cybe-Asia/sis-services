//! Reads only. Every query is scoped by school and tenant; the authority check runs in the graph
//! as well as on the introspected session.
use super::model::{Activity, Assignment, Grant, Lesson, PlannedSubject, Section, Snapshot, Staff};
use crate::school_portal::model::identifier;
use neo4rs::{query, Graph, Query, Row};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// The rule before ADR 0018 slice 6: active school administrators of the school and Owners whose
/// scope covers it. The authorization graph test keeps it as the day-one reference.
#[cfg(test)]
pub(crate) fn authorized(school: &str, tenant: &str) -> String {
    format!(
        "{} AND {}",
        known(school, tenant),
        crate::school_portal::staff_capability::authority("a", "'school_admin'", school, tenant)
    )
}
/// The caller is one active Staff Member whose own lists cover the school (an unscoped Owner covers
/// every school), and the school one School node. Who may read is then decided by capability
/// (`academic.staffing.read` at the school, ADR 0018 slice 6).
fn known(school: &str, tenant: &str) -> String {
    format!(
        "MATCH(a:StaffMember {{id:$actor}}) WHERE a.membershipStatus='ACTIVE' AND (({school} IN coalesce(a.schoolIds,[]) AND {tenant} IN coalesce(a.tenantIds,[])) OR ('owner' IN coalesce(a.roles,[]) AND size(coalesce(a.schoolIds,[]))=0 AND size(coalesce(a.tenantIds,[]))=0)) AND NOT EXISTS {{MATCH(other:StaffMember {{id:a.id}}) WHERE other<>a}} AND EXISTS {{MATCH(:School {{school_id:{school},tenant_id:{tenant}}})}} AND NOT EXISTS {{MATCH(one:School {{school_id:{school},tenant_id:{tenant}}}),(two:School {{school_id:{school},tenant_id:{tenant}}}) WHERE one<>two}}"
    )
}
const SECTIONS: &str = "MATCH(sec:Section {school_id:$school,tenant_id:$tenant,academic_year:$year,status:'active'}) WHERE NOT EXISTS {MATCH(o:Section {section_id:sec.section_id}) WHERE o<>sec} RETURN sec.section_id AS id,coalesce(sec.name,sec.section_id) AS name,coalesce(sec.year_group,'') AS grade,sec.homeroom_staff_member_id AS homeroom ORDER BY id LIMIT 201";
const LESSONS: &str = "MATCH(m:LearningClassMeeting {school_id:$school,tenant_id:$tenant}) WHERE coalesce(m.status,'published')='published' AND m.class_id IN $sections AND coalesce(m.academic_year,$year)=$year AND m.starts_at<$to AND m.ends_at>$from RETURN m.id AS id,m.class_id AS section,m.teacher_id AS teacher,m.starts_at AS starts,m.ends_at AS ends,coalesce(m.room_id,'') AS room,m.payload AS payload ORDER BY starts,id LIMIT 4001";
const GRANTS: &str = "MATCH(g:LearningTeachingGrant {school_id:$school,tenant_id:$tenant,status:'ACTIVE'}) WHERE g.section_id IN $sections RETURN g.staff_member_id AS teacher,g.section_id AS section ORDER BY section,teacher LIMIT 4001";
const ACTIVITIES: &str = "MATCH(x:ExtraCurricularActivity {school_id:$school,tenant_id:$tenant,academic_year:$year}) WHERE coalesce(x.archived,false)=false AND coalesce(x.coach_id,'')<>'' RETURN x.id AS id,x.coach_id AS coach,x.payload AS payload ORDER BY id LIMIT 501";
const STAFF: &str = "MATCH(t:StaffMember) WHERE ('teacher' IN coalesce(t.roles,[]) AND $school IN coalesce(t.schoolIds,[]) AND $tenant IN coalesce(t.tenantIds,[])) OR t.id IN $referenced RETURN t.id AS id,coalesce(t.displayName,t.fullName,t.name,t.id) AS name,coalesce(t.membershipStatus,'')='ACTIVE' AND 'teacher' IN coalesce(t.roles,[]) AND $school IN coalesce(t.schoolIds,[]) AND $tenant IN coalesce(t.tenantIds,[]) AS active ORDER BY id LIMIT 1001";

fn get<T: serde::de::DeserializeOwned>(row: &Row, name: &str) -> Result<T, Error> {
    Ok(row.get(name)?)
}
async fn rows(graph: &Graph, q: Query, max: usize) -> Result<Vec<Row>, Error> {
    let mut stream = graph.execute(q).await?;
    let mut out = vec![];
    while let Some(row) = stream.next().await? {
        if out.len() == max {
            return Err("Bound exceeded".into());
        }
        out.push(row);
    }
    Ok(out)
}
fn text(value: &str, max: usize) -> Result<String, Error> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err("Invalid text".into());
    }
    Ok(value.to_string())
}
fn id(value: String) -> Result<String, Error> {
    identifier(&value)
        .then_some(value)
        .ok_or_else(|| "Invalid id".into())
}

/// Who reads the list: one active Staff Member, one School node, and `academic.staffing.read` at
/// the school from the caller's explicit and derived holdings.
pub async fn authorize(
    graph: &Graph,
    actor: &str,
    school: &str,
    tenant: &str,
) -> Result<bool, Error> {
    Ok(known_school(graph, actor, school, tenant).await?
        && reads_staffing(graph, actor, school, tenant).await?)
}
pub async fn reads_staffing(
    graph: &Graph,
    actor: &str,
    school: &str,
    tenant: &str,
) -> Result<bool, Error> {
    crate::authorization::reads::permits(
        &mut crate::authorization::facts::Db::Graph(graph),
        tenant,
        school,
        actor,
        crate::academic_staffing::repository::STAFFING_READ,
    )
    .await
    .map_err(|_| "Authorization read failed".into())
}
async fn known_school(
    graph: &Graph,
    actor: &str,
    school: &str,
    tenant: &str,
) -> Result<bool, Error> {
    let q = query(&format!(
        "{} RETURN count(a) AS count",
        known("$school", "$tenant")
    ))
    .param("actor", actor)
    .param("school", school)
    .param("tenant", tenant);
    let found = rows(graph, q, 1).await?;
    Ok(found.first().map(|r| get::<i64>(r, "count")).transpose()? == Some(1))
}

/// One school year, and the lessons of one school week (`from`..`to` epoch ms).
pub async fn snapshot(
    graph: &Graph,
    school: &str,
    tenant: &str,
    year: &str,
    from: i64,
    to: i64,
) -> Result<Snapshot, Error> {
    let scoped = |body: &str| {
        query(body)
            .param("school", school)
            .param("tenant", tenant)
            .param("year", year)
    };
    let mut s = Snapshot::default();
    for r in rows(graph, scoped(SECTIONS), 200).await? {
        s.sections.push(Section {
            id: id(get(&r, "id")?)?,
            name: text(&get::<String>(&r, "name")?, 256)?,
            year_group: get::<String>(&r, "grade")?.chars().take(128).collect(),
            homeroom: get::<Option<String>>(&r, "homeroom")?
                .filter(|h| !h.is_empty())
                .map(id)
                .transpose()?,
        });
    }
    let sections: Vec<String> = s.sections.iter().map(|x| x.id.clone()).collect();
    for r in rows(
        graph,
        scoped(LESSONS)
            .param("sections", sections.clone())
            .param("from", from)
            .param("to", to),
        4000,
    )
    .await?
    {
        let payload: Value = serde_json::from_str(&get::<String>(&r, "payload")?)?;
        let starts: i64 = get(&r, "starts")?;
        let ends: i64 = get(&r, "ends")?;
        s.lessons.push(Lesson {
            id: id(get(&r, "id")?)?,
            section_id: id(get(&r, "section")?)?,
            teacher_id: id(get(&r, "teacher")?)?,
            subject: id(payload["subject"].as_str().unwrap_or_default().to_string())?,
            title: text(payload["title"].as_str().unwrap_or_default(), 200)?,
            starts_at: starts.try_into()?,
            ends_at: ends.try_into()?,
            room_id: get::<String>(&r, "room")?.chars().take(128).collect(),
        });
    }
    for r in rows(
        graph,
        scoped(GRANTS).param("sections", sections.clone()),
        4000,
    )
    .await?
    {
        s.grants.push(Grant {
            teacher_id: id(get(&r, "teacher")?)?,
            section_id: id(get(&r, "section")?)?,
        });
    }
    for r in rows(graph, scoped(ACTIVITIES), 500).await? {
        let payload: Value = serde_json::from_str(&get::<String>(&r, "payload")?)?;
        let title = &payload["title"];
        s.activities.push(Activity {
            id: id(get(&r, "id")?)?,
            coach_id: id(get(&r, "coach")?)?,
            title: json!({
                "en": text(title["en"].as_str().unwrap_or_default(), 200)?,
                "id": text(title["id"].as_str().unwrap_or_default(), 200)?,
            }),
        });
    }
    let (plans, assignments) =
        crate::academic_staffing::repository::planned(graph, school, tenant, year, &sections)
            .await
            .map_err(|_| "Assignments unavailable")?;
    s.plans = plans
        .into_iter()
        .map(|(grade, row)| PlannedSubject {
            grade,
            subject: row.subject,
            periods: row.periods_per_week,
            required: row.required,
        })
        .collect();
    s.assignments = assignments
        .into_iter()
        .map(|(section_id, subject, teacher_id, periods)| Assignment {
            section_id,
            subject,
            teacher_id,
            periods,
        })
        .collect();
    let referenced: BTreeSet<String> = s
        .lessons
        .iter()
        .map(|l| l.teacher_id.clone())
        .chain(s.sections.iter().filter_map(|x| x.homeroom.clone()))
        .chain(s.activities.iter().map(|a| a.coach_id.clone()))
        .chain(s.grants.iter().map(|g| g.teacher_id.clone()))
        .chain(s.assignments.iter().map(|a| a.teacher_id.clone()))
        .collect();
    let mut seen = BTreeSet::new();
    for r in rows(
        graph,
        scoped(STAFF).param("referenced", referenced.into_iter().collect::<Vec<_>>()),
        1000,
    )
    .await?
    {
        let staff_id = id(get(&r, "id")?)?;
        let name: String = get(&r, "name")?;
        let active: bool = get(&r, "active")?;
        // Two StaffMember nodes with one id are never trusted, here or in any write.
        if !seen.insert(staff_id.clone()) {
            if let Some(p) = s.staff.iter_mut().find(|p| p.id == staff_id) {
                p.active = false;
            }
            continue;
        }
        s.staff.push(Staff {
            name: text(&name, 256).unwrap_or_else(|_| staff_id.clone()),
            id: staff_id,
            active,
        });
    }
    Ok(s)
}

pub struct School {
    pub school_id: String,
    pub tenant_id: String,
    pub academic_year: String,
    pub classes: i64,
}
/// The schools (and their academic years with active classes) in the caller's scope that are one
/// School node each; the route keeps those where the caller reads staffing.
/// `all` is an Owner without a school scope: every school with exactly one School node.
pub async fn schools(
    graph: &Graph,
    actor: &str,
    schools: Vec<String>,
    tenants: Vec<String>,
    all: bool,
) -> Result<Vec<School>, Error> {
    let body = format!(
        "MATCH(sec:Section {{status:'active'}}) WHERE ($all OR (sec.school_id IN $schools AND sec.tenant_id IN $tenants)) AND coalesce(sec.academic_year,'')<>'' WITH sec.school_id AS school,sec.tenant_id AS tenant,sec.academic_year AS year,count(sec) AS classes CALL {{ WITH school,tenant {} RETURN count(a) AS allowed }} WITH school,tenant,year,classes WHERE allowed=1 RETURN school,tenant,year,classes ORDER BY school,tenant,year DESC LIMIT 101",
        known("school", "tenant")
    );
    let q = query(&body)
        .param("actor", actor)
        .param("schools", schools)
        .param("tenants", tenants)
        .param("all", all);
    let mut out = vec![];
    for r in rows(graph, q, 100).await? {
        out.push(School {
            school_id: id(get(&r, "school")?)?,
            tenant_id: id(get(&r, "tenant")?)?,
            academic_year: text(&get::<String>(&r, "year")?, 32)?,
            classes: get(&r, "classes")?,
        });
    }
    Ok(out)
}
