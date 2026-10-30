//! Template and assignment commands. One transaction per request, in this order: caller lock,
//! tenant lock, the Auth session again, the idempotency receipt, the caller's holdings from the
//! locked graph, every guardrail, the change and its audit, the last-manager check on the result,
//! the tenant version and the receipt.
use super::facts::{self, get, Db};
use super::model::{
    decide, holds_tenant, owner_at, parse_date, reference, Assignment, AssignmentCommand,
    AssignmentCommands, Facts, Holding, Scope, Template, TemplateCommand, TemplateCommands,
    ARCHIVE, ASSIGN, CHANGE_VALIDITY, CREATE, MANAGE, PRINCIPAL, REGULAR_TEACHER, REVISE, REVOKE,
    SCHOOL_ADMIN, TENANT,
};
use crate::academic_staffing::repository::{refuse, unavailable, Failure};
use axum::http::StatusCode;
use neo4rs::{query, Graph, Query, Txn};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// How the caller's Auth session is checked again once the locks are held.
pub enum Session<'a> {
    Headers(&'a axum::http::HeaderMap),
    /// Graph tests only: they have no Auth fixture and check the graph rules alone.
    #[cfg(test)]
    Unchecked,
}
pub struct Writer<'a> {
    pub session: Session<'a>,
    pub actor: &'a str,
    pub role: &'a str,
    pub tenant: &'a str,
    pub command: &'a str,
}
impl Writer<'_> {
    fn receipt(&self) -> String {
        [self.actor, self.tenant, self.command].join("|")
    }
}

fn forbidden(detail: Value) -> Failure {
    refuse(StatusCode::FORBIDDEN, "FORBIDDEN", detail)
}
fn at(status: StatusCode, code: &str, index: usize) -> Failure {
    refuse(status, code, json!({ "index": index }))
}
fn not_held(index: usize, capability: &str) -> Failure {
    refuse(
        StatusCode::FORBIDDEN,
        "CAPABILITY_NOT_HELD",
        json!({ "index": index, "capability": capability }),
    )
}
async fn run(tx: &mut Txn, q: Query) -> Result<(), Failure> {
    tx.run(q).await.map_err(unavailable)
}

/// The session still names the same person and role, and still covers the tenant.
pub async fn live_session(
    headers: &axum::http::HeaderMap,
    tenant: &str,
) -> Result<(String, String), Failure> {
    let (actor, role, schools, tenants) = crate::learning_access::any_staff_session(headers)
        .await?
        .staff_scope();
    let unscoped = role == "owner" && schools.is_empty() && tenants.is_empty();
    if !unscoped && !tenants.iter().any(|t| t == tenant) {
        return Err(forbidden(json!({})));
    }
    Ok((actor, role))
}

async fn guard(tx: &mut Txn, w: &Writer<'_>, fingerprint: &str) -> Result<Option<Value>, Failure> {
    let mut db = Db::Tx(tx);
    db.one(query("MATCH(actor:StaffMember {id:$actor,membershipStatus:'ACTIVE'}) WITH actor LIMIT 1 SET actor.owner_write_lock=coalesce(actor.owner_write_lock,0)+1 RETURN actor.id AS id").param("actor", w.actor))
        .await?
        .ok_or_else(|| forbidden(json!({})))?;
    db.one(query("MERGE(s:AuthorizationTenantState {tenant_id:$tenant}) ON CREATE SET s.version=0 SET s.lock=coalesce(s.lock,0)+1 RETURN s.tenant_id AS id").param("tenant", w.tenant))
        .await?
        .ok_or_else(|| unavailable("No tenant lock"))?;
    // A role revoked while this request waited for the locks does not write.
    match w.session {
        Session::Headers(headers) => {
            if live_session(headers, w.tenant).await? != (w.actor.into(), w.role.into()) {
                return Err(forbidden(json!({})));
            }
        }
        #[cfg(test)]
        Session::Unchecked => {}
    }
    let stored = db
        .one(
            query("MATCH(r:AuthorizationReceipt {key:$key}) RETURN r.fingerprint AS fingerprint,r.result AS result")
                .param("key", w.receipt()),
        )
        .await?;
    match stored {
        None => Ok(None),
        Some(r) if get::<String>(&r, "fingerprint")? == fingerprint => Ok(Some(
            serde_json::from_str(&get::<String>(&r, "result")?).map_err(unavailable)?,
        )),
        Some(_) => Err(refuse(
            StatusCode::CONFLICT,
            "IDEMPOTENCY_KEY_REUSED",
            json!({}),
        )),
    }
}

/// Schools of the tenant where at least one person holds `authorization.manage`.
fn managed(facts: &Facts) -> BTreeSet<String> {
    let holdings: Vec<Vec<Holding>> = facts.staff.iter().map(|s| facts.holdings(&s.id)).collect();
    facts
        .schools
        .keys()
        .filter(|school| {
            holdings
                .iter()
                .any(|h| decide(facts, h, MANAGE, &Scope::school(school)).is_some())
        })
        .cloned()
        .collect()
}

/// Runs `$work` with the tenant's facts read under the locks, then the last-manager check, the
/// tenant version and the receipt; commits only when all of them pass.
macro_rules! transact {
    ($graph:expr, $writer:expr, $fingerprint:expr, |$tx:ident, $facts:ident| $work:expr) => {{
        let w: &Writer = $writer;
        let fingerprint: &str = $fingerprint;
        let mut tx = $graph.start_txn().await.map_err(unavailable)?;
        let outcome: Result<(Value, bool), Failure> = async {
            if let Some(replayed) = guard(&mut tx, w, fingerprint).await? {
                return Ok((replayed, true));
            }
            let $facts = facts::load(&mut Db::Tx(&mut tx), w.tenant, &[], &[]).await?;
            let had = managed(&$facts);
            let $tx = &mut tx;
            let mut value: Value = $work.await?;
            // The last holder of `authorization.manage` in a school cannot lose it.
            let after = facts::load(&mut Db::Tx(&mut tx), w.tenant, &[], &[]).await?;
            let kept = managed(&after);
            if let Some(school) = had.difference(&kept).next() {
                return Err(refuse(
                    StatusCode::CONFLICT,
                    "LAST_AUTHORIZATION_MANAGER",
                    json!({ "schoolId": school }),
                ));
            }
            let version = Db::Tx(&mut tx)
                .one(query("MATCH(s:AuthorizationTenantState {tenant_id:$tenant}) SET s.version=s.version+1 RETURN s.version AS version").param("tenant", w.tenant))
                .await?
                .ok_or_else(|| unavailable("No tenant state"))?;
            value["authorizationVersion"] = json!(get::<i64>(&version, "version")?);
            run(&mut tx, query("CREATE(:AuthorizationReceipt {key:$key,fingerprint:$fingerprint,result:$result,actor_id:$actor,created_at:datetime()})")
                .param("key", w.receipt())
                .param("fingerprint", fingerprint)
                .param("result", value.to_string())
                .param("actor", w.actor))
                .await?;
            Ok((value, false))
        }
        .await;
        match outcome {
            Ok((value, false)) => {
                tx.commit().await.map_err(unavailable)?;
                Ok(value)
            }
            Ok((value, true)) => {
                let _ = tx.rollback().await;
                Ok(value)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }};
}

struct Audit<'a> {
    kind: &'a str,
    action: &'a str,
    record: &'a str,
    school: Option<&'a str>,
    template: &'a str,
    template_version: Option<u32>,
    before: Value,
    after: Value,
    authorized_by: Value,
}
fn audit(w: &Writer<'_>, a: Audit<'_>) -> Query {
    query("CREATE(:AuthorizationAudit {key:$key,kind:$kind,action:$action,record_key:$record,actor_id:$actor,actor_role:$role,tenant_id:$tenant,school_id:$school,template_code:$template,template_version:$template_version,before:$before,after:$after,authorized_by:$authorized_by,command_key:$command,created_at:datetime()})")
        .param("key", uuid::Uuid::new_v4().to_string())
        .param("kind", a.kind)
        .param("action", a.action)
        .param("record", a.record)
        .param("actor", w.actor)
        .param("role", w.role)
        .param("tenant", w.tenant)
        .param("school", a.school.map(str::to_string))
        .param("template", a.template)
        .param("template_version", a.template_version.map(i64::from))
        .param("before", a.before.to_string())
        .param("after", a.after.to_string())
        .param("authorized_by", a.authorized_by.to_string())
        .param("command", w.command)
}

/// Which of the caller's holdings answers `authorization.manage` at `scope`, for the audit.
fn manager(facts: &Facts, mine: &[Holding], scope: &Scope) -> Option<Value> {
    if scope.kind == TENANT {
        return holds_tenant(facts, mine, MANAGE).then(|| {
            let sources: Vec<_> = mine
                .iter()
                .filter(|h| h.capabilities.contains(MANAGE))
                .map(|h| &h.source)
                .collect();
            json!(sources)
        });
    }
    decide(facts, mine, MANAGE, scope).map(|h| json!(h.source))
}
fn held(facts: &Facts, mine: &[Holding], capability: &str, scope: &Scope) -> bool {
    if scope.kind == TENANT {
        holds_tenant(facts, mine, capability)
    } else {
        decide(facts, mine, capability, scope).is_some()
    }
}

// ---------------------------------------------------------------------------------------------
// Templates

fn template_json(t: &Template) -> Value {
    json!({
        "code": t.code, "name": t.name, "version": t.version, "capabilities": t.capabilities,
        "allowedScopes": t.allowed_scopes, "status": t.status,
    })
}
async fn store_version(
    tx: &mut Txn,
    w: &Writer<'_>,
    t: &Template,
    actor: &str,
) -> Result<(), Failure> {
    run(tx, query("CREATE(:AuthorizationRoleTemplateVersion {key:$key,tenant_id:$tenant,code:$code,version:$version,name_id:$name_id,name_en:$name_en,capabilities:$capabilities,allowed_scopes:$scopes,status:$status,actor_id:$actor,created_at:datetime()})")
        .param("key", format!("{}|{}|{}", w.tenant, t.code, t.version))
        .param("tenant", w.tenant).param("code", t.code.clone()).param("version", i64::from(t.version))
        .param("name_id", t.name.id.clone()).param("name_en", t.name.en.clone())
        .param("capabilities", t.capabilities.clone()).param("scopes", t.allowed_scopes.clone())
        .param("status", t.status.clone()).param("actor", actor)).await
}

async fn template_command(
    tx: &mut Txn,
    w: &Writer<'_>,
    facts: &Facts,
    mine: &[Holding],
    index: usize,
    c: &TemplateCommand,
) -> Result<Value, Failure> {
    let reference = reference();
    let current = facts.template(&c.code).cloned();
    let conflict = |code: &str| at(StatusCode::CONFLICT, code, index);
    match (c.action.as_str(), &current) {
        (CREATE, None) if c.expected_version == 0 => {}
        (REVISE | ARCHIVE, Some(t)) if t.version == c.expected_version => {
            if t.status != "active" {
                return Err(conflict("TEMPLATE_ARCHIVED"));
            }
        }
        _ => return Err(conflict("REVISION_CONFLICT")),
    }
    let in_use: Vec<&Assignment> = facts
        .assignments
        .iter()
        .filter(|a| a.template_code == c.code && a.status == "active")
        .collect();
    let before: Option<&Template> = current.as_ref();
    let next = if c.action == ARCHIVE {
        if [REGULAR_TEACHER, SCHOOL_ADMIN].contains(&c.code.as_str()) {
            return Err(conflict("TEMPLATE_REQUIRED"));
        }
        if !in_use.is_empty() {
            return Err(conflict("TEMPLATE_IN_USE"));
        }
        let mut t = current.clone().ok_or_else(|| unavailable("No template"))?;
        t.status = "archived".into();
        t
    } else {
        let capabilities = c.capabilities.clone().unwrap_or_default();
        if let Some(unknown) = capabilities
            .iter()
            .find(|x| reference.capability(x).is_none())
        {
            return Err(refuse(
                StatusCode::UNPROCESSABLE_ENTITY,
                "UNKNOWN_CAPABILITY",
                json!({ "index": index, "capability": unknown }),
            ));
        }
        let old: BTreeSet<&String> = before
            .map(|t| t.capabilities.iter().collect())
            .unwrap_or_default();
        let added: Vec<&String> = capabilities.iter().filter(|x| !old.contains(x)).collect();
        // Nobody adds a capability they do not hold for the whole tenant; a sensitive one needs
        // an Owner of every school of the tenant.
        if let Some(missing) = added.iter().find(|x| !holds_tenant(facts, mine, x)) {
            return Err(not_held(index, missing));
        }
        if added.iter().any(|x| reference.sensitive(x)) && !owner_at(facts, mine, &Scope::tenant())
        {
            return Err(at(StatusCode::FORBIDDEN, "SENSITIVE_NEEDS_OWNER", index));
        }
        let scopes = c.allowed_scopes.clone().unwrap_or_default();
        if in_use.iter().any(|a| !scopes.contains(&a.scope.kind)) {
            return Err(conflict("SCOPE_IN_USE"));
        }
        let mut sorted = capabilities;
        sorted.sort();
        Template {
            code: c.code.clone(),
            name: c.name.clone().ok_or_else(|| unavailable("No name"))?,
            version: 0,
            capabilities: sorted,
            allowed_scopes: scopes,
            status: "active".into(),
            seeded: false,
            updated_at: None,
        }
    };
    if let Some(t) = before.filter(|t| t.seeded) {
        // The first save of a repository default keeps that default at its seed version.
        store_version(tx, w, t, "seed").await?;
    }
    let mut next = next;
    next.version = c.expected_version + 1;
    let saved = Db::Tx(tx).one(query("MERGE(t:AuthorizationRoleTemplate {key:$key}) ON CREATE SET t.tenant_id=$tenant,t.code=$code,t.created_at=datetime() SET t.version=$version,t.name_id=$name_id,t.name_en=$name_en,t.capabilities=$capabilities,t.allowed_scopes=$scopes,t.status=$status,t.updated_by=$actor,t.updated_at=datetime() RETURN toString(t.updated_at) AS updated")
        .param("key", format!("{}|{}", w.tenant, next.code))
        .param("tenant", w.tenant).param("code", next.code.clone()).param("version", i64::from(next.version))
        .param("name_id", next.name.id.clone()).param("name_en", next.name.en.clone())
        .param("capabilities", next.capabilities.clone()).param("scopes", next.allowed_scopes.clone())
        .param("status", next.status.clone()).param("actor", w.actor)).await?
        .ok_or_else(|| unavailable("No template"))?;
    store_version(tx, w, &next, w.actor).await?;
    let after = template_json(&next);
    let record = format!("{}|{}", w.tenant, next.code);
    run(
        tx,
        audit(
            w,
            Audit {
                kind: "template",
                action: &c.action,
                record: &record,
                school: None,
                template: &next.code,
                template_version: Some(next.version),
                before: before.map(template_json).unwrap_or(Value::Null),
                after: after.clone(),
                authorized_by: manager(facts, mine, &Scope::tenant()).unwrap_or(Value::Null),
            },
        ),
    )
    .await?;
    let mut result = after;
    result["updatedAt"] = json!(get::<String>(&saved, "updated")?);
    Ok(result)
}

pub async fn templates(
    graph: &Graph,
    w: &Writer<'_>,
    batch: &TemplateCommands,
) -> Result<Value, Failure> {
    let fingerprint = serde_json::to_string(batch).map_err(unavailable)?;
    transact!(graph, w, &fingerprint, |tx, facts| async {
        let mine = facts.holdings(w.actor);
        // Templates are shared by every school of the tenant.
        if !holds_tenant(&facts, &mine, MANAGE) {
            return Err(forbidden(json!({})));
        }
        let mut out = vec![];
        for (index, c) in batch.commands.iter().enumerate() {
            out.push(template_command(tx, w, &facts, &mine, index, c).await?);
        }
        Ok::<Value, Failure>(json!({ "templates": out }))
    })
}

// ---------------------------------------------------------------------------------------------
// Assignments

fn assignment_json(a: &Assignment) -> Value {
    serde_json::to_value(a).unwrap_or(Value::Null)
}
async fn by_id(tx: &mut Txn, tenant: &str, id: &str) -> Result<Option<Assignment>, Failure> {
    Db::Tx(tx)
        .one(
            query(&format!(
                "MATCH(a:AuthorizationAssignment {{id:$id,tenant_id:$tenant}}) RETURN {}",
                facts::ASSIGNMENT_FIELDS
            ))
            .param("id", id)
            .param("tenant", tenant),
        )
        .await?
        .map(|r| facts::assignment(&r))
        .transpose()
}
async fn by_key(tx: &mut Txn, key: &str) -> Result<Option<Assignment>, Failure> {
    Db::Tx(tx)
        .one(
            query(&format!(
                "MATCH(a:AuthorizationAssignment {{key:$key}}) RETURN {}",
                facts::ASSIGNMENT_FIELDS
            ))
            .param("key", key),
        )
        .await?
        .map(|r| facts::assignment(&r))
        .transpose()
}

/// The caller may hand out `template` at `scope`: they manage access there, hold every one of its
/// capabilities there, and are an Owner there when one of them is sensitive. The Principal
/// template is given by an Owner only (spec §10 #8.3), whatever it carries.
pub(super) fn may_grant(
    facts: &Facts,
    mine: &[Holding],
    template: &Template,
    scope: &Scope,
    index: usize,
) -> Result<Value, Failure> {
    let authorized =
        manager(facts, mine, scope).ok_or_else(|| forbidden(json!({ "index": index })))?;
    if template.code == PRINCIPAL && !owner_at(facts, mine, scope) {
        return Err(at(StatusCode::FORBIDDEN, "PRINCIPAL_NEEDS_OWNER", index));
    }
    if let Some(missing) = template
        .capabilities
        .iter()
        .find(|c| !held(facts, mine, c, scope))
    {
        return Err(not_held(index, missing));
    }
    let r = reference();
    if template.capabilities.iter().any(|c| r.sensitive(c)) && !owner_at(facts, mine, scope) {
        return Err(at(StatusCode::FORBIDDEN, "SENSITIVE_NEEDS_OWNER", index));
    }
    Ok(authorized)
}

/// A renewal (master-thread default 2026-10-30, under the user's "Principal can renew"; the school
/// may revisit it): a holder of `authorization.manage` over the assignment's scope extends its end
/// without holding the template's capabilities, when all of these hold:
/// - the assignment is active and current today (revoked or expired ones go through `assign`);
/// - only the end moves, and later: `from` is not earlier than today's start, `until` is after the
///   current end and no later than the end of the academic year after it (the Perpanjang suggestion);
/// - the template carries no sensitive capability (those stay with Owners).
///
/// `Some` is the audit's `authorized_by`; `None` leaves the ordinary grant rules to decide.
pub(super) fn renewal(
    facts: &Facts,
    mine: &[Holding],
    template: &Template,
    current: &Assignment,
    from: Option<&str>,
    until: Option<&str>,
) -> Option<Value> {
    let today = facts.today();
    if !current.current(today)
        || template
            .capabilities
            .iter()
            .any(|c| reference().sensitive(c))
    {
        return None;
    }
    let end = parse_date(current.valid_until.as_deref()?)?;
    let next = parse_date(until?)?;
    let (_, limit, _) = facts.academic_year_on(
        current.scope.school_id.as_deref(),
        end + chrono::Duration::days(1),
    );
    let start_kept = match (current.valid_from.as_deref(), from) {
        (_, None) => current.valid_from.is_none(),
        (None, Some(f)) => parse_date(f).is_some(),
        (Some(old), Some(f)) => {
            matches!((parse_date(old), parse_date(f)), (Some(o), Some(n)) if n >= o)
        }
    };
    if !start_kept || next <= end || next > limit {
        return None;
    }
    let source = manager(facts, mine, &current.scope)?;
    Some(json!({
        "rule": "renewal",
        "source": source,
        "previousEnd": current.valid_until,
        "newEnd": until,
    }))
}

/// `assign` creates or reopens by key; every other command changes the record it named by id.
async fn save(
    tx: &mut Txn,
    w: &Writer<'_>,
    a: &Assignment,
    key: Option<&str>,
) -> Result<Assignment, Failure> {
    let s = &a.scope;
    let target = match key {
        Some(_) => "MERGE(a:AuthorizationAssignment {key:$key}) ON CREATE SET a.id=$id,a.tenant_id=$tenant,a.staff_member_id=$staff,a.template_code=$template,a.scope_type=$kind,a.school_id=$school,a.section_id=$section,a.subject=$subject,a.grade_from=$grade_from,a.grade_to=$grade_to,a.academic_year=$year,a.version=0,a.created_at=datetime(),a.created_by=$actor",
        None => "MATCH(a:AuthorizationAssignment {id:$id,tenant_id:$tenant})",
    };
    let r = Db::Tx(tx).one(query(&format!("{target} SET a.version=a.version+1,a.status=$status,a.valid_from=$valid_from,a.valid_until=$valid_until,a.updated_by=$actor,a.updated_at=datetime(),a.revoked_at=CASE WHEN $status='revoked' THEN datetime() ELSE null END RETURN {}", facts::ASSIGNMENT_FIELDS))
        .param("key", key.unwrap_or_default()).param("id", a.id.clone()).param("tenant", w.tenant)
        .param("staff", a.staff_member_id.clone()).param("template", a.template_code.clone())
        .param("kind", s.kind.clone()).param("school", s.school_id.clone()).param("section", s.section_id.clone())
        .param("subject", s.subject.clone()).param("grade_from", s.grade_from.clone()).param("grade_to", s.grade_to.clone())
        .param("year", a.academic_year.clone()).param("status", a.status.clone())
        .param("valid_from", a.valid_from.clone()).param("valid_until", a.valid_until.clone())
        .param("actor", w.actor)).await?
        .ok_or_else(|| unavailable("No assignment"))?;
    facts::assignment(&r)
}

async fn assignment_command(
    tx: &mut Txn,
    w: &Writer<'_>,
    facts: &Facts,
    mine: &[Holding],
    index: usize,
    c: &AssignmentCommand,
) -> Result<Value, Failure> {
    let unprocessable = |code: &str| at(StatusCode::UNPROCESSABLE_ENTITY, code, index);
    let conflict = || at(StatusCode::CONFLICT, "REVISION_CONFLICT", index);
    let (current, key) = if c.action == ASSIGN {
        let key = c
            .key(w.tenant)
            .ok_or_else(|| unprocessable("INVALID_INPUT"))?;
        (by_key(tx, &key).await?, Some(key))
    } else {
        let id = c.assignment_id.as_deref().unwrap_or_default();
        let found = by_id(tx, w.tenant, id)
            .await?
            .ok_or_else(|| at(StatusCode::NOT_FOUND, "NOT_FOUND", index))?;
        (Some(found), None)
    };
    let mut next = match (c.action.as_str(), &current) {
        (ASSIGN, None) if c.expected_version == 0 => Assignment {
            id: uuid::Uuid::new_v4().to_string(),
            staff_member_id: c.staff_member_id.clone().unwrap_or_default(),
            template_code: c.template_code.clone().unwrap_or_default(),
            scope: c.scope.clone().ok_or_else(conflict)?,
            academic_year: c.academic_year.clone(),
            ..Assignment::default()
        },
        // A revoked key reopens at its version, or from an "add" that sends 0 because it never read
        // the revoked record. An active key still takes 0 as a conflict.
        (ASSIGN, Some(a))
            if a.status == "revoked"
                && (c.expected_version == 0 || a.version == c.expected_version) =>
        {
            a.clone()
        }
        (REVOKE | CHANGE_VALIDITY, Some(a))
            if a.status == "active" && a.version == c.expected_version =>
        {
            a.clone()
        }
        _ => return Err(conflict()),
    };
    let template = facts
        .template(&next.template_code)
        .filter(|t| t.status == "active")
        .cloned();
    let (authorized, validity) = if c.action == REVOKE {
        next.status = "revoked".into();
        let authorized = manager(facts, mine, &next.scope)
            .ok_or_else(|| forbidden(json!({ "index": index })))?;
        (authorized, None)
    } else {
        let template = template.ok_or_else(|| unprocessable("TEMPLATE_NOT_FOUND"))?;
        if !template.allowed_scopes.contains(&next.scope.kind) {
            return Err(unprocessable("SCOPE_NOT_ALLOWED"));
        }
        let place = facts.exists(&next.scope)
            && match (
                &next.academic_year,
                next.scope.school_id.as_deref(),
                next.scope.section_id.as_deref(),
            ) {
                (Some(year), Some(school), Some(section)) => facts
                    .section(school, section)
                    .is_some_and(|s| &s.year == year),
                _ => true,
            };
        if !place {
            return Err(unprocessable("SCOPE_NOT_FOUND"));
        }
        let eligible = facts.person(&next.staff_member_id).is_some_and(|p| {
            !p.owner()
                && match next.scope.school_id.as_deref() {
                    Some(school) => p.in_school(w.tenant, school),
                    None => p.active() && p.tenant_ids.iter().any(|t| t == w.tenant),
                }
        });
        if !eligible {
            return Err(unprocessable("TARGET_NOT_ELIGIBLE"));
        }
        let renewed = (c.action == CHANGE_VALIDITY)
            .then(|| {
                renewal(
                    facts,
                    mine,
                    &template,
                    &next,
                    c.valid_from.as_deref(),
                    c.valid_until.as_deref(),
                )
            })
            .flatten();
        let authorized = match renewed {
            Some(by) => by,
            None => may_grant(facts, mine, &template, &next.scope, index)?,
        };
        next.status = "active".into();
        // Without an end, the assignment ends with the academic year (spec §10 #8.4).
        let validity = facts.validity(
            &next.template_code,
            &next.scope,
            next.academic_year.as_deref(),
            c.valid_from.as_deref(),
            c.valid_until.as_deref(),
        );
        next.valid_from = c.valid_from.clone();
        next.valid_until = validity.valid_until.clone();
        (authorized, Some(validity))
    };
    let saved = save(tx, w, &next, key.as_deref()).await?;
    let template_version = facts.template(&saved.template_code).map(|t| t.version);
    run(
        tx,
        audit(
            w,
            Audit {
                kind: "assignment",
                action: &c.action,
                record: &saved.id,
                school: saved.scope.school_id.as_deref(),
                template: &saved.template_code,
                template_version,
                before: current.as_ref().map(assignment_json).unwrap_or(Value::Null),
                after: assignment_json(&saved),
                authorized_by: authorized,
            },
        ),
    )
    .await?;
    let mut out = assignment_json(&saved);
    if let Some(validity) = validity {
        out["validity"] = json!(validity);
    }
    Ok(out)
}

pub async fn assignments(
    graph: &Graph,
    w: &Writer<'_>,
    batch: &AssignmentCommands,
) -> Result<Value, Failure> {
    let fingerprint = serde_json::to_string(batch).map_err(unavailable)?;
    transact!(graph, w, &fingerprint, |tx, facts| async {
        let mine = facts.holdings(w.actor);
        let mut out = vec![];
        for (index, c) in batch.commands.iter().enumerate() {
            out.push(assignment_command(tx, w, &facts, &mine, index, c).await?);
        }
        Ok::<Value, Failure>(json!({ "assignments": out }))
    })
}
