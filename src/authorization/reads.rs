//! Reads for Admin and for services. A reader sees only schools where they hold
//! `authorization.read` or `authorization.manage`; nothing about other schools is revealed.
use super::facts::{self, get, Db};
use super::model::{
    covers, decide, grade_number, holds_tenant, readable_schools, reference, Assignment,
    CheckRequest, Facts, Holding, Scope, CACHE_SECONDS, MANAGE, MASTERY_READ, READ, SCHOOL_SUBJECT,
    SECTION_SUBJECT, TENANT,
};
use crate::academic_staffing::repository::{refuse, unavailable, Failure};
use axum::http::StatusCode;
use neo4rs::{query, Graph};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Who reads, with the schools their Auth session covers (`None`: an unscoped Owner).
pub struct Reader {
    pub actor: String,
    pub schools: Option<Vec<String>>,
}

/// The reader's holdings and the schools they may read, or 403 when there are none.
pub struct View {
    pub facts: Facts,
    pub readable: BTreeSet<String>,
    /// Reads the whole tenant, so tenant-wide holdings are shown as such.
    pub tenant_wide: bool,
    /// Readable schools where the reader may also change assignments; whether they may change
    /// templates (manage across the tenant), and as an Owner (sensitive capabilities). Hints for
    /// Admin menus only: every write checks again.
    pub managed: BTreeSet<String>,
    pub edits_templates: bool,
    pub owner_templates: bool,
    pub version: i64,
}
pub async fn view(
    graph: &Graph,
    reader: &Reader,
    tenant: &str,
    staff: &[String],
    sections: &[String],
) -> Result<View, Failure> {
    let mut db = Db::Graph(graph);
    let mut people: Vec<String> = staff.to_vec();
    if !people.is_empty() {
        people.push(reader.actor.clone());
    }
    let facts = facts::load(&mut db, tenant, &people, sections).await?;
    let mine = facts.holdings(&reader.actor);
    let mut readable = readable_schools(&facts, &mine);
    if let Some(session) = &reader.schools {
        readable.retain(|s| session.contains(s));
    }
    if readable.is_empty() {
        return Err(refuse(StatusCode::FORBIDDEN, "FORBIDDEN", json!({})));
    }
    let tenant_wide = reader.schools.is_none()
        && (holds_tenant(&facts, &mine, READ) || holds_tenant(&facts, &mine, MANAGE));
    let version = facts::version(&mut db, tenant).await?;
    let managed = readable
        .iter()
        .filter(|s| decide(&facts, &mine, MANAGE, &Scope::school(s)).is_some())
        .cloned()
        .collect();
    let edits_templates = reader.schools.is_none() && holds_tenant(&facts, &mine, MANAGE);
    let owner_templates =
        edits_templates && super::model::owner_at(&facts, &mine, &Scope::tenant());
    Ok(View {
        facts,
        readable,
        tenant_wide,
        managed,
        edits_templates,
        owner_templates,
        version,
    })
}
impl View {
    fn sees(&self, scope: &Scope) -> bool {
        match scope.school_id.as_deref() {
            None => self.tenant_wide,
            Some(s) => self.readable.contains(s),
        }
    }
    /// Holdings as the reader may see them: a tenant-wide holding is shown per readable school
    /// to a reader of only some schools.
    fn visible(&self, holdings: Vec<Holding>) -> Vec<Holding> {
        let mut out = vec![];
        for h in holdings {
            if h.scope.kind == TENANT && !self.tenant_wide {
                for school in &self.readable {
                    out.push(Holding {
                        scope: Scope::school(school),
                        ..h.clone()
                    });
                }
            } else if self.sees(&h.scope) {
                out.push(h);
            }
        }
        out
    }
    fn label(&self, scope: &Scope) -> Value {
        let school = scope
            .school_id
            .as_deref()
            .and_then(|s| self.facts.schools.get(s));
        let section = scope
            .school_id
            .as_deref()
            .zip(scope.section_id.as_deref())
            .and_then(|(school, id)| self.facts.section(school, id));
        json!({
            "schoolName": school,
            "sectionName": section.map(|s| &s.name),
            "grade": section.map(|s| &s.grade),
            "academicYear": section.map(|s| &s.year),
        })
    }
    fn template_name(&self, code: Option<&String>) -> Value {
        code.and_then(|c| self.facts.template(c))
            .map(|t| json!(t.name))
            .unwrap_or(Value::Null)
    }
    fn holding(&self, h: &Holding) -> Value {
        json!({
            "source": h.source,
            "scope": h.scope,
            "scopeLabel": self.label(&h.scope),
            "templateName": self.template_name(h.source.template_code.as_ref()),
            "capabilities": h.capabilities,
            "derived": h.derived,
        })
    }
    fn envelope(&self, mut data: Value) -> Value {
        data["authorizationVersion"] = json!(self.version);
        data["cacheSeconds"] = json!(CACHE_SECONDS);
        data
    }
    /// Someone the reader may look at: a member of a readable school, or an Owner over one.
    fn person(&self, id: &str) -> Option<&super::model::Staff> {
        self.facts.person(id).filter(|p| {
            self.readable.iter().any(|s| {
                p.school_ids.iter().any(|x| x == s)
                    || (p.owner() && p.school_ids.is_empty() && p.tenant_ids.is_empty())
            })
        })
    }
}

pub fn catalogue() -> Value {
    let r = reference();
    json!({ "version": r.catalogue.version, "capabilities": r.catalogue.capabilities })
}

pub async fn effective(
    graph: &Graph,
    reader: &Reader,
    tenant: &str,
    staff: &str,
) -> Result<Value, Failure> {
    let v = view(graph, reader, tenant, &[staff.to_string()], &[]).await?;
    let person = v
        .person(staff)
        .ok_or_else(|| refuse(StatusCode::NOT_FOUND, "NOT_FOUND", json!({})))?;
    let holdings = v.visible(v.facts.holdings(staff));
    let mut by_capability: BTreeMap<&str, Vec<&Scope>> = BTreeMap::new();
    for h in &holdings {
        for c in &h.capabilities {
            by_capability.entry(c).or_default().push(&h.scope);
        }
    }
    Ok(v.envelope(json!({
        "person": {
            "staffMemberId": person.id, "name": person.name, "active": person.active(),
            "roles": person.roles, "owner": person.owner(),
        },
        "holdings": holdings.iter().map(|h| v.holding(h)).collect::<Vec<_>>(),
        "capabilities": by_capability,
    })))
}

pub async fn check(
    graph: &Graph,
    reader: &Reader,
    request: &CheckRequest,
) -> Result<Value, Failure> {
    let target = request
        .actor
        .clone()
        .unwrap_or_else(|| reader.actor.clone());
    let sections: Vec<String> = request.scope.section_id.iter().cloned().collect();
    let v = view(
        graph,
        reader,
        &request.tenant_id,
        std::slice::from_ref(&target),
        &sections,
    )
    .await?;
    // Asking about someone else needs read access where the question is asked.
    if target != reader.actor && (!v.sees(&request.scope) || v.person(&target).is_none()) {
        return Err(refuse(StatusCode::FORBIDDEN, "FORBIDDEN", json!({})));
    }
    let holdings = v.facts.holdings(&target);
    let known = reference().capability(&request.capability).is_some();
    if !known {
        // A code is not personal data; it is logged so a caller's typo is found.
        tracing::info!(capability = %request.capability, "authorization check for an unknown capability");
    }
    let decision = decide(&v.facts, &holdings, &request.capability, &request.scope);
    let reason = match (
        known,
        v.facts.exists(&request.scope),
        decision,
        holdings.is_empty(),
    ) {
        (false, ..) => "UNKNOWN_CAPABILITY",
        (_, false, ..) => "SCOPE_NOT_FOUND",
        (_, _, Some(_), _) => "ALLOWED",
        (_, _, None, true) => "NO_ACCESS",
        (_, _, None, false) => "NOT_IN_SCOPE",
    };
    let held_at: Vec<Value> = v
        .visible(
            holdings
                .iter()
                .filter(|h| h.capabilities.contains(&request.capability))
                .cloned()
                .collect(),
        )
        .iter()
        .map(|h| v.holding(h))
        .collect();
    Ok(v.envelope(json!({
        "allowed": decision.is_some(),
        "reason": reason,
        "actor": target,
        "capability": request.capability,
        "scope": request.scope,
        "scopeLabel": v.label(&request.scope),
        "source": decision.map(|h| &h.source),
        "templateVersion": decision.and_then(|h| h.source.template_version),
        "heldAt": held_at,
    })))
}

pub async fn templates(graph: &Graph, reader: &Reader, tenant: &str) -> Result<Value, Failure> {
    let v = view(graph, reader, tenant, &[], &[]).await?;
    let people = people_by_template(&v.facts);
    let rows: Vec<Value> = v
        .facts
        .templates
        .iter()
        .map(|t| {
            let mut row = json!(t);
            row["people"] = json!(people.get(t.code.as_str()).map_or(0, BTreeSet::len));
            row
        })
        .collect();
    Ok(v.envelope(json!({
        "templates": rows,
        "canEditTemplates": v.edits_templates,
        "canGrantSensitive": v.owner_templates,
    })))
}

fn people_by_template(facts: &Facts) -> BTreeMap<&str, BTreeSet<&str>> {
    let mut out: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for s in &facts.staff {
        for h in facts.holdings(&s.id) {
            if let Some(code) = facts
                .templates
                .iter()
                .find(|t| Some(&t.code) == h.source.template_code.as_ref())
            {
                out.entry(code.code.as_str()).or_default().insert(&s.id);
            }
        }
    }
    out
}

pub async fn history(
    graph: &Graph,
    reader: &Reader,
    tenant: &str,
    code: &str,
) -> Result<Value, Failure> {
    let v = view(graph, reader, tenant, &[], &[]).await?;
    let template = v
        .facts
        .template(code)
        .ok_or_else(|| refuse(StatusCode::NOT_FOUND, "NOT_FOUND", json!({})))?;
    let mut versions = vec![];
    for r in Db::Graph(graph).rows(query("MATCH(t:AuthorizationRoleTemplateVersion {tenant_id:$tenant,code:$code}) OPTIONAL MATCH(s:StaffMember {id:t.actor_id}) WITH t,collect(s)[0] AS s RETURN t.version AS version,t.name_id AS name_id,t.name_en AS name_en,t.capabilities AS capabilities,t.allowed_scopes AS scopes,t.status AS status,t.actor_id AS actor,coalesce(s.displayName,s.fullName,s.name) AS actor_name,toString(t.created_at) AS at ORDER BY version DESC LIMIT 501").param("tenant", tenant).param("code", code), 500).await? {
        versions.push(json!({
            "version": get::<i64>(&r, "version")?,
            "name": {"id": get::<String>(&r, "name_id")?, "en": get::<String>(&r, "name_en")?},
            "capabilities": get::<Vec<String>>(&r, "capabilities")?,
            "allowedScopes": get::<Vec<String>>(&r, "scopes")?,
            "status": get::<String>(&r, "status")?,
            "seed": get::<String>(&r, "actor")? == "seed",
            "actorName": get::<Option<String>>(&r, "actor_name")?,
            "at": get::<String>(&r, "at")?,
        }));
    }
    if versions.is_empty() && template.seeded {
        versions.push(json!({
            "version": 1, "name": template.name, "capabilities": template.capabilities,
            "allowedScopes": template.allowed_scopes, "status": template.status,
            "seed": true, "actorName": null, "at": null,
        }));
    }
    Ok(v.envelope(json!({ "code": code, "versions": versions })))
}

/// Who a change to the template reaches today: explicit and derived holders, by source.
pub async fn impact(
    graph: &Graph,
    reader: &Reader,
    tenant: &str,
    code: &str,
) -> Result<Value, Failure> {
    let v = view(graph, reader, tenant, &[], &[]).await?;
    let template = v
        .facts
        .template(code)
        .ok_or_else(|| refuse(StatusCode::NOT_FOUND, "NOT_FOUND", json!({})))?;
    let mut people = BTreeSet::new();
    let mut sources: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    let mut schools = BTreeSet::new();
    for s in &v.facts.staff {
        for h in v.facts.holdings(&s.id) {
            if h.source.template_code.as_deref() != Some(code) {
                continue;
            }
            people.insert(s.id.as_str());
            sources
                .entry(h.source.kind.clone())
                .or_default()
                .insert(&s.id);
            match h.scope.school_id {
                Some(school) => {
                    schools.insert(school);
                }
                None => schools.extend(v.facts.schools.keys().cloned()),
            }
        }
    }
    Ok(v.envelope(json!({
        "code": code,
        "version": template.version,
        "people": people.len(),
        "sources": sources.iter().map(|(k, p)| (k.clone(), p.len())).collect::<BTreeMap<_, _>>(),
        "schools": schools.len(),
    })))
}

#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Filter {
    pub tenant_id: String,
    pub school_id: Option<String>,
    pub template_code: Option<String>,
    pub staff_member_id: Option<String>,
    pub section_id: Option<String>,
    pub subject: Option<String>,
    /// `active` (default), `revoked` or `all`; derived rows are always active.
    pub status: Option<String>,
    pub include_derived: Option<bool>,
}

pub async fn assignments(graph: &Graph, reader: &Reader, f: &Filter) -> Result<Value, Failure> {
    let v = view(graph, reader, &f.tenant_id, &[], &[]).await?;
    let status = f.status.as_deref().unwrap_or("active");
    let mut explicit: Vec<Assignment> = if status == "revoked" {
        vec![]
    } else {
        v.facts.assignments.clone()
    };
    if status != "active" {
        let q = format!("MATCH(a:AuthorizationAssignment {{tenant_id:$tenant,status:'revoked'}}) RETURN {} ORDER BY id LIMIT 5001", facts::ASSIGNMENT_FIELDS);
        for r in Db::Graph(graph)
            .rows(query(&q).param("tenant", f.tenant_id.clone()), 5000)
            .await?
        {
            explicit.push(facts::assignment(&r)?);
        }
    }
    let wanted = |staff: &str, template: Option<&str>, scope: &Scope| {
        v.sees(scope)
            && f.staff_member_id.as_deref().is_none_or(|x| x == staff)
            && f.template_code
                .as_deref()
                .is_none_or(|x| Some(x) == template)
            && f.school_id
                .as_deref()
                .is_none_or(|x| scope.school_id.as_deref() == Some(x))
            && f.section_id
                .as_deref()
                .is_none_or(|x| scope.section_id.as_deref() == Some(x))
            && f.subject
                .as_deref()
                .is_none_or(|x| scope.subject.as_deref() == Some(x))
    };
    let name = |id: &str| v.facts.person(id).map(|p| p.name.clone());
    let today = (chrono::Utc::now() + chrono::Duration::hours(7)).date_naive();
    let mut rows = vec![];
    for a in &explicit {
        if !wanted(&a.staff_member_id, Some(&a.template_code), &a.scope) {
            continue;
        }
        let mut row = json!(a);
        row["name"] = json!(name(&a.staff_member_id));
        row["derived"] = json!(false);
        row["current"] = json!(a.current(today));
        row["templateName"] = v.template_name(Some(&a.template_code));
        row["templateVersion"] = json!(v.facts.template(&a.template_code).map(|t| t.version));
        row["scopeLabel"] = v.label(&a.scope);
        rows.push(row);
    }
    if f.include_derived.unwrap_or(true) && status != "revoked" {
        for s in &v.facts.staff {
            for h in v.visible(v.facts.holdings(&s.id)) {
                if !h.derived || !wanted(&s.id, h.source.template_code.as_deref(), &h.scope) {
                    continue;
                }
                let mut row = v.holding(&h);
                row["staffMemberId"] = json!(s.id);
                row["name"] = json!(s.name);
                row["status"] = json!("active");
                row["current"] = json!(true);
                rows.push(row);
            }
        }
    }
    if rows.len() > 5000 {
        return Err(unavailable("Bound exceeded"));
    }
    let people: Vec<Value> = v
        .facts
        .staff
        .iter()
        .filter(|p| p.active() && !p.owner() && v.readable.iter().any(|s| p.school_ids.contains(s)))
        .map(|p| json!({"staffMemberId": p.id, "name": p.name, "roles": p.roles, "schoolIds": p.school_ids.iter().filter(|s| v.readable.contains(*s)).collect::<Vec<_>>()}))
        .collect();
    let schools: Vec<Value> = v
        .readable
        .iter()
        .map(|s| json!({"schoolId": s, "name": v.facts.schools.get(s)}))
        .collect();
    let sections: Vec<Value> = v
        .facts
        .sections
        .iter()
        .filter(|s| v.readable.contains(&s.school))
        .map(|s| json!({"sectionId": s.id, "schoolId": s.school, "name": s.name, "grade": s.grade, "academicYear": s.year}))
        .collect();
    Ok(v.envelope(json!({
        "assignments": rows, "people": people, "schools": schools, "sections": sections,
        "templates": v.facts.templates, "tenantWide": v.tenant_wide,
        "manageSchools": v.managed,
        "ownerSchools": v.readable.iter().filter(|s| super::model::owner_at(&v.facts, &v.facts.holdings(&reader.actor), &Scope::school(s))).collect::<Vec<_>>(),
    })))
}

pub async fn audit(graph: &Graph, reader: &Reader, tenant: &str) -> Result<Value, Failure> {
    let v = view(graph, reader, tenant, &[], &[]).await?;
    let readable: Vec<String> = v.readable.iter().cloned().collect();
    let mut entries = vec![];
    for r in Db::Graph(graph).rows(query("MATCH(e:AuthorizationAudit {tenant_id:$tenant}) WHERE e.school_id IN $schools OR (e.school_id IS NULL AND (e.kind='template' OR $tenant_wide)) OPTIONAL MATCH(s:StaffMember {id:e.actor_id}) WITH e,collect(s)[0] AS s RETURN e.kind AS kind,e.action AS action,e.record_key AS record,e.actor_id AS actor,coalesce(s.displayName,s.fullName,s.name) AS actor_name,e.school_id AS school,e.template_code AS template,e.template_version AS template_version,e.before AS before,e.after AS after,toString(e.created_at) AS at ORDER BY e.created_at DESC LIMIT 200").param("tenant", tenant).param("schools", readable).param("tenant_wide", v.tenant_wide), 200).await? {
        let parse = |name: &str| -> Result<Value, Failure> {
            serde_json::from_str(&get::<String>(&r, name)?).map_err(unavailable)
        };
        entries.push(json!({
            "kind": get::<String>(&r, "kind")?, "action": get::<String>(&r, "action")?,
            "recordKey": get::<String>(&r, "record")?, "actorId": get::<String>(&r, "actor")?,
            "actorName": get::<Option<String>>(&r, "actor_name")?, "schoolId": get::<Option<String>>(&r, "school")?,
            "templateCode": get::<String>(&r, "template")?, "templateVersion": get::<Option<i64>>(&r, "template_version")?,
            "before": parse("before")?, "after": parse("after")?, "at": get::<String>(&r, "at")?,
        }));
    }
    Ok(v.envelope(json!({ "entries": entries })))
}

/// Every `learning.*` capability `staff` holds inside one school, by scope, with the tenant's
/// authorization version. Tenant-wide holdings count; another school's never do. `None` when the
/// person is not one active Staff Member (the caller then refuses as the authorize read does).
pub async fn learning_scopes(
    graph: &Graph,
    tenant: &str,
    school: &str,
    staff: &str,
) -> Result<Option<(BTreeMap<String, Vec<Scope>>, i64)>, Failure> {
    let mut db = Db::Graph(graph);
    let facts = facts::load(&mut db, tenant, &[staff.to_string()], &[]).await?;
    if !facts.person(staff).is_some_and(|p| p.active()) {
        return Ok(None);
    }
    let mut out: BTreeMap<String, Vec<Scope>> = BTreeMap::new();
    for h in facts.holdings(staff) {
        if !(h.scope.kind == TENANT || h.scope.school_id.as_deref() == Some(school)) {
            continue;
        }
        for code in h.capabilities.iter().filter(|c| c.starts_with("learning.")) {
            let scopes = out.entry(code.clone()).or_default();
            if !scopes.contains(&h.scope) {
                scopes.push(h.scope.clone());
            }
        }
    }
    Ok(Some((out, facts::version(&mut db, tenant).await?)))
}

/// The `learning.*` capabilities `staff` holds in one class, for the Learning authorize answer.
pub async fn learning_capabilities(
    graph: &Graph,
    tenant: &str,
    school: &str,
    class: &str,
    staff: &str,
) -> Result<Vec<String>, Failure> {
    let facts = facts::load(
        &mut Db::Graph(graph),
        tenant,
        &[staff.to_string()],
        &[class.to_string()],
    )
    .await?;
    let holdings = facts.holdings(staff);
    let scope = Scope::section(school, class);
    Ok(reference()
        .catalogue
        .capabilities
        .iter()
        .filter(|c| c.code.starts_with("learning."))
        .filter(|c| decide(&facts, &holdings, &c.code, &scope).is_some())
        .map(|c| c.code.clone())
        .collect())
}

/// Whether `staff` holds `capability` for the whole of `school` now, for services that moved
/// from role checks (spec §7 slice 6). The decision is the same as `/authorization/check` at
/// `SCHOOL`: Owners and school administrators pass on day one, and a school that is not one
/// unique `School` of the tenant denies. Inside a write transaction the facts are read under the
/// caller's locks.
pub async fn permits(
    db: &mut Db<'_>,
    tenant: &str,
    school: &str,
    staff: &str,
    capability: &str,
) -> Result<bool, Failure> {
    let facts = facts::load(db, tenant, &[staff.to_string()], &[]).await?;
    let holdings = facts.holdings(staff);
    Ok(decide(&facts, &holdings, capability, &Scope::school(school)).is_some())
}

/// One class of the mastery roster: the students the caller may read mastery for, and the
/// subjects that holding covers (`None`: every subject of the class).
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterClass {
    pub id: String,
    pub name: String,
    pub grade: String,
    pub academic_year: String,
    pub subjects: Option<Vec<String>>,
    pub students: Vec<RosterStudent>,
}
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterStudent {
    pub id: String,
    pub name: String,
}

const SCHOOL_SECTIONS: &str = "MATCH(c:Section {tenant_id:$tenant,school_id:$school,status:'active'}) RETURN DISTINCT c.section_id AS id ORDER BY id LIMIT 5001";
const ROSTER: &str = "MATCH(c:Section {tenant_id:$tenant,school_id:$school,status:'active'}) WHERE c.section_id IN $sections MATCH(s:Student)-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$school,tenant_id:$tenant})-[:ENROLLED_IN]->(c) RETURN DISTINCT c.section_id AS class,s.studentId AS id,coalesce(s.fullName,s.studentId) AS name ORDER BY class,id LIMIT 20001";

/// The active classes of `school`, with their students, for which `staff` holds
/// `learning.mastery.read` (ADR 0018 remainder): through SCHOOL, SCHOOL_SUBJECT, SECTION or
/// SECTION_SUBJECT holdings, explicit or derived. `subject` keeps classes where that subject is
/// covered; `grade` keeps classes of that grade. `None` when the person is not one active Staff
/// Member, as the capabilities read refuses.
pub async fn mastery_roster(
    graph: &Graph,
    tenant: &str,
    school: &str,
    staff: &str,
    subject: Option<&str>,
    grade: Option<&str>,
) -> Result<Option<(Vec<RosterClass>, i64)>, Failure> {
    let mut db = Db::Graph(graph);
    let mut sections = vec![];
    for r in db
        .rows(
            query(SCHOOL_SECTIONS)
                .param("tenant", tenant)
                .param("school", school),
            5000,
        )
        .await?
    {
        sections.push(get::<String>(&r, "id")?);
    }
    let facts = facts::load(&mut db, tenant, &[staff.to_string()], &sections).await?;
    if !facts.person(staff).is_some_and(|p| p.active()) {
        return Ok(None);
    }
    let holdings = facts.holdings(staff);
    let wanted_grade = grade.and_then(grade_number);
    let mut classes: Vec<RosterClass> = vec![];
    for s in facts.sections.iter().filter(|s| s.school == school) {
        if wanted_grade.is_some() && grade_number(&s.grade) != wanted_grade {
            continue;
        }
        let whole = decide(
            &facts,
            &holdings,
            MASTERY_READ,
            &Scope::section(school, &s.id),
        );
        let subjects: Option<Vec<String>> = match (subject, whole) {
            (Some(subject), _) => {
                let scope = Scope::section_subject(school, &s.id, subject);
                if decide(&facts, &holdings, MASTERY_READ, &scope).is_none() {
                    continue;
                }
                Some(vec![subject.to_string()])
            }
            (None, Some(_)) => None,
            (None, None) => {
                // Subject holdings only: which subjects of this class they cover.
                let mut covered: BTreeSet<String> = BTreeSet::new();
                for h in holdings.iter().filter(|h| {
                    h.capabilities.contains(MASTERY_READ)
                        && (h.scope.kind == SCHOOL_SUBJECT || h.scope.kind == SECTION_SUBJECT)
                }) {
                    if let Some(code) = h.scope.subject.as_deref() {
                        let scope = Scope::section_subject(school, &s.id, code);
                        if covers(&h.scope, &scope, Some(&s.grade), false) {
                            covered.insert(code.to_string());
                        }
                    }
                }
                if covered.is_empty() {
                    continue;
                }
                Some(covered.into_iter().collect())
            }
        };
        classes.push(RosterClass {
            id: s.id.clone(),
            name: s.name.clone(),
            grade: s.grade.clone(),
            academic_year: s.year.clone(),
            subjects,
            students: vec![],
        });
    }
    if !classes.is_empty() {
        let ids: Vec<String> = classes.iter().map(|c| c.id.clone()).collect();
        for r in db
            .rows(
                query(ROSTER)
                    .param("tenant", tenant)
                    .param("school", school)
                    .param("sections", ids),
                20000,
            )
            .await?
        {
            let class: String = get(&r, "class")?;
            if let Some(c) = classes.iter_mut().find(|c| c.id == class) {
                c.students.push(RosterStudent {
                    id: get(&r, "id")?,
                    name: get(&r, "name")?,
                });
            }
        }
    }
    Ok(Some((classes, facts::version(&mut db, tenant).await?)))
}
