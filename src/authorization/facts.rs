//! Loads the `Facts` of one tenant from the graph, inside or outside a write transaction. Every
//! list is bounded; past a bound the answer is 503, never a partial decision.
use super::model::{reference, Assignment, Facts, Scope, Section, Staff, Teaching, Template, Text};
use crate::academic_staffing::repository::{unavailable, Failure};
use neo4rs::{query, Graph, Query, Row, Txn};

pub enum Db<'a> {
    Graph(&'a Graph),
    Tx(&'a mut Txn),
}
impl Db<'_> {
    pub async fn rows(&mut self, q: Query, max: usize) -> Result<Vec<Row>, Failure> {
        let mut out = vec![];
        match self {
            Db::Graph(g) => {
                let mut stream = g.execute(q).await.map_err(unavailable)?;
                while let Some(row) = stream.next().await.map_err(unavailable)? {
                    if out.len() == max {
                        return Err(unavailable("Bound exceeded"));
                    }
                    out.push(row);
                }
            }
            Db::Tx(tx) => {
                let mut stream = tx.execute(q).await.map_err(unavailable)?;
                while let Some(row) = stream.next(tx.handle()).await.map_err(unavailable)? {
                    if out.len() == max {
                        return Err(unavailable("Bound exceeded"));
                    }
                    out.push(row);
                }
            }
        }
        Ok(out)
    }
    /// Exactly one row or none; more is an error.
    pub async fn one(&mut self, q: Query) -> Result<Option<Row>, Failure> {
        Ok(self.rows(q, 1).await?.into_iter().next())
    }
}
pub fn get<T: serde::de::DeserializeOwned>(row: &Row, name: &str) -> Result<T, Failure> {
    row.get(name).map_err(unavailable)
}
fn number(row: &Row, name: &str) -> Result<u32, Failure> {
    get::<i64>(row, name)?.try_into().map_err(unavailable)
}

const STAFF: &str = "MATCH(s:StaffMember) WHERE (size($staff)=0 OR s.id IN $staff) AND ($tenant IN coalesce(s.tenantIds,[]) OR ('owner' IN coalesce(s.roles,[]) AND size(coalesce(s.schoolIds,[]))=0 AND size(coalesce(s.tenantIds,[]))=0)) WITH s.id AS sid,collect(s)[0] AS s RETURN sid AS id,COUNT {MATCH(o:StaffMember {id:sid})} AS copies,coalesce(s.displayName,s.fullName,s.name,sid) AS name,coalesce(s.membershipStatus,'') AS status,coalesce(s.roles,[]) AS roles,coalesce(s.schoolIds,[]) AS schools,coalesce(s.tenantIds,[]) AS tenants,coalesce(s.teamIds,[]) AS teams ORDER BY id LIMIT 3001";
const SCHOOLS: &str = "MATCH(sc:School {tenant_id:$tenant}) WITH sc.school_id AS sid,collect(sc) AS ss WHERE sid IS NOT NULL AND size(ss)=1 RETURN sid AS id,coalesce(ss[0].name,sid) AS name ORDER BY id LIMIT 501";
const SECTIONS: &str = "MATCH(c:Section {tenant_id:$tenant,status:'active'}) WHERE size($staff)=0 OR c.section_id IN $sections OR c.homeroom_staff_member_id IN $staff OR EXISTS {MATCH(g:LearningTeachingGrant {section_id:c.section_id,status:'ACTIVE',tenant_id:$tenant}) WHERE g.staff_member_id IN $staff} OR EXISTS {MATCH(a:AuthorizationAssignment {section_id:c.section_id,tenant_id:$tenant}) WHERE a.staff_member_id IN $staff} RETURN c.section_id AS id,c.school_id AS school,coalesce(c.name,c.section_id) AS name,coalesce(c.year_group,'') AS grade,coalesce(c.academic_year,'') AS year,c.homeroom_staff_member_id AS homeroom ORDER BY id,school LIMIT 5001";
const GRANTS: &str = "MATCH(g:LearningTeachingGrant {tenant_id:$tenant,status:'ACTIVE'}) WHERE size($staff)=0 OR g.staff_member_id IN $staff RETURN g.staff_member_id AS staff,g.section_id AS section,g.school_id AS school LIMIT 10001";
const TEACHING: &str = "MATCH(a:TeachingAssignment {tenant_id:$tenant,status:'active'}) WHERE size($staff)=0 OR a.staff_member_id IN $staff RETURN a.id AS id,a.staff_member_id AS staff,a.section_id AS section,a.school_id AS school,a.subject AS subject ORDER BY section,subject,staff LIMIT 10001";
pub const ASSIGNMENT_FIELDS: &str = "a.id AS id,a.staff_member_id AS staff,a.template_code AS template,a.scope_type AS kind,a.school_id AS school,a.section_id AS section,a.subject AS subject,a.grade_from AS grade_from,a.grade_to AS grade_to,a.academic_year AS year,a.valid_from AS valid_from,a.valid_until AS valid_until,a.status AS status,a.version AS version,toString(a.updated_at) AS updated";
const TEMPLATES: &str = "MATCH(t:AuthorizationRoleTemplate {tenant_id:$tenant}) RETURN t.code AS code,t.name_id AS name_id,t.name_en AS name_en,t.version AS version,t.capabilities AS capabilities,t.allowed_scopes AS scopes,t.status AS status,toString(t.updated_at) AS updated ORDER BY code LIMIT 201";

pub fn assignment(row: &Row) -> Result<Assignment, Failure> {
    Ok(Assignment {
        id: get(row, "id")?,
        staff_member_id: get(row, "staff")?,
        template_code: get(row, "template")?,
        scope: Scope {
            kind: get(row, "kind")?,
            school_id: get(row, "school")?,
            section_id: get(row, "section")?,
            subject: get(row, "subject")?,
            grade_from: get(row, "grade_from")?,
            grade_to: get(row, "grade_to")?,
        },
        academic_year: get(row, "year")?,
        valid_from: get(row, "valid_from")?,
        valid_until: get(row, "valid_until")?,
        status: get(row, "status")?,
        version: number(row, "version")?,
        updated_at: get(row, "updated")?,
    })
}

/// Saved templates of the tenant, plus the repository seeds it has not saved yet.
pub async fn templates(db: &mut Db<'_>, tenant: &str) -> Result<Vec<Template>, Failure> {
    let mut out = vec![];
    for r in db
        .rows(query(TEMPLATES).param("tenant", tenant), 200)
        .await?
    {
        out.push(Template {
            code: get(&r, "code")?,
            name: Text {
                id: get(&r, "name_id")?,
                en: get(&r, "name_en")?,
            },
            version: number(&r, "version")?,
            capabilities: get(&r, "capabilities")?,
            allowed_scopes: get(&r, "scopes")?,
            status: get(&r, "status")?,
            seeded: false,
            updated_at: get(&r, "updated")?,
        });
    }
    for seed in &reference().seeds {
        if !out.iter().any(|t| t.code == seed.code) {
            out.push(Template::from_seed(seed));
        }
    }
    out.sort_by(|a, b| a.code.cmp(&b.code));
    Ok(out)
}

/// The facts about `staff` (everyone when empty) in `tenant`. `sections` adds classes a request
/// names, so that a check on a class nobody holds still resolves it.
pub async fn load(
    db: &mut Db<'_>,
    tenant: &str,
    staff: &[String],
    sections: &[String],
) -> Result<Facts, Failure> {
    let scoped = |body: &str| {
        query(body)
            .param("tenant", tenant)
            .param("staff", staff.to_vec())
            .param("sections", sections.to_vec())
    };
    let mut facts = Facts {
        tenant: tenant.into(),
        ..Facts::default()
    };
    for r in db.rows(scoped(STAFF), 3000).await? {
        facts.staff.push(Staff {
            id: get(&r, "id")?,
            name: get(&r, "name")?,
            copies: get(&r, "copies")?,
            status: get(&r, "status")?,
            roles: get(&r, "roles")?,
            school_ids: get(&r, "schools")?,
            tenant_ids: get(&r, "tenants")?,
            team_ids: get(&r, "teams")?,
        });
    }
    for r in db.rows(scoped(SCHOOLS), 500).await? {
        facts.schools.insert(get(&r, "id")?, get(&r, "name")?);
    }
    for r in db.rows(scoped(SECTIONS), 5000).await? {
        facts.sections.push(Section {
            id: get(&r, "id")?,
            school: get(&r, "school")?,
            name: get(&r, "name")?,
            grade: get(&r, "grade")?,
            year: get(&r, "year")?,
            homeroom: get(&r, "homeroom")?,
        });
    }
    for r in db.rows(scoped(GRANTS), 10000).await? {
        facts
            .grants
            .push((get(&r, "staff")?, get(&r, "section")?, get(&r, "school")?));
    }
    for r in db.rows(scoped(TEACHING), 10000).await? {
        facts.teaching.push(Teaching {
            id: get(&r, "id")?,
            staff: get(&r, "staff")?,
            section: get(&r, "section")?,
            school: get(&r, "school")?,
            subject: get(&r, "subject")?,
        });
    }
    let explicit = format!("MATCH(a:AuthorizationAssignment {{tenant_id:$tenant,status:'active'}}) WHERE size($staff)=0 OR a.staff_member_id IN $staff RETURN {ASSIGNMENT_FIELDS} ORDER BY id LIMIT 5001");
    for r in db.rows(scoped(&explicit), 5000).await? {
        facts.assignments.push(assignment(&r)?);
    }
    facts.templates = templates(db, tenant).await?;
    Ok(facts)
}

/// The tenant's authorization version: bumped by every template or assignment change.
pub async fn version(db: &mut Db<'_>, tenant: &str) -> Result<i64, Failure> {
    Ok(db
        .one(
            query(
                "MATCH(s:AuthorizationTenantState {tenant_id:$tenant}) RETURN s.version AS version",
            )
            .param("tenant", tenant),
        )
        .await?
        .map(|r| get::<i64>(&r, "version"))
        .transpose()?
        .unwrap_or(0))
}
