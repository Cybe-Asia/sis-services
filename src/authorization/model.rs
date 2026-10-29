//! Capability-based access for school staff (ADR 0018, `docs/AUTHORIZATION.md`): the catalogue,
//! role templates, scopes and their containment, and the holdings a staff member has from
//! explicit assignments and from today's records. Pure; the graph only supplies `Facts`.
use crate::academic_staffing::model::{key_part, subject_code};
use crate::school_portal::model::identifier;
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

pub const TENANT: &str = "TENANT";
pub const SCHOOL: &str = "SCHOOL";
pub const SCHOOL_SUBJECT: &str = "SCHOOL_SUBJECT";
pub const SECTION: &str = "SECTION";
pub const SECTION_SUBJECT: &str = "SECTION_SUBJECT";
pub const SCOPE_TYPES: [&str; 5] = [TENANT, SCHOOL, SCHOOL_SUBJECT, SECTION, SECTION_SUBJECT];

pub const MANAGE: &str = "authorization.manage";
pub const READ: &str = "authorization.read";
pub const HOMEROOM: &str = "learning.class.homeroom";
/// Templates that derived sources use: they cannot be archived.
pub const REGULAR_TEACHER: &str = "regular_teacher";
pub const SCHOOL_ADMIN: &str = "school_admin";
/// Kepala Sekolah per unit (spec §10 #8.3): the one seed that carries `authorization.manage`, at
/// `SCHOOL` only, and only an Owner may give it.
pub const PRINCIPAL: &str = "principal";
pub const CURRICULUM_COORDINATOR: &str = "curriculum_coordinator";
/// Templates whose assignments run until revoked; every other assignment ends with the academic
/// year when the command names no end (spec §10 #8.4).
pub const UNTIL_REVOKED: [&str; 2] = [PRINCIPAL, CURRICULUM_COORDINATOR];
pub const MASTERY_READ: &str = "learning.mastery.read";
/// Owner, Student and Parent are system roles; `homeroom` names a derived source.
const RESERVED: [&str; 4] = ["owner", "student", "parent", "homeroom"];
pub const MAX_COMMANDS: usize = 20;
pub const MAX_CAPABILITIES: usize = 64;
/// Callers may keep an answer this long, or until the tenant's authorization version changes.
pub const CACHE_SECONDS: u32 = 60;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub id: String,
    pub en: String,
}
impl Text {
    fn valid(&self, max: usize) -> bool {
        [&self.id, &self.en].iter().all(|t| {
            !t.trim().is_empty()
                && t.trim() == t.as_str()
                && t.chars().count() <= max
                && !t.chars().any(char::is_control)
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capability {
    pub code: String,
    pub service: String,
    pub area: String,
    pub sensitivity: String,
    /// About a class as a whole: holding it for one subject of a class holds it for the class,
    /// as a teaching grant per class does today.
    #[serde(default)]
    pub class_wide: bool,
    pub description: Text,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Catalogue {
    pub version: u32,
    pub capabilities: Vec<Capability>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Seed {
    pub code: String,
    pub name: Text,
    pub allowed_scopes: Vec<String>,
    pub capabilities: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Seeds {
    templates: Vec<Seed>,
}

/// `<area>.<resource>.<action>` or `<area>.<action>`, lower case with `_`.
pub fn capability_code(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    value.len() <= 96
        && (2..=3).contains(&parts.len())
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.as_bytes()[0].is_ascii_lowercase()
                && p.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        })
}
pub fn template_code(value: &str) -> bool {
    (2..=48).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !RESERVED.contains(&value)
}

fn parse_catalogue(text: &str) -> Result<Catalogue, String> {
    let catalogue: Catalogue = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut seen = BTreeSet::new();
    for c in &catalogue.capabilities {
        if !capability_code(&c.code)
            || !seen.insert(c.code.clone())
            || !["normal", "sensitive"].contains(&c.sensitivity.as_str())
            || c.service.is_empty()
            || !c.code.starts_with(&format!("{}.", c.area))
            || !c.description.valid(160)
        {
            return Err(format!("invalid capability {}", c.code));
        }
    }
    Ok(catalogue)
}
fn parse_seeds(text: &str, catalogue: &Catalogue) -> Result<Vec<Seed>, String> {
    let seeds: Seeds = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let known: BTreeSet<&str> = catalogue
        .capabilities
        .iter()
        .map(|c| c.code.as_str())
        .collect();
    let mut seen = BTreeSet::new();
    for s in &seeds.templates {
        let caps: BTreeSet<&String> = s.capabilities.iter().collect();
        if !template_code(&s.code)
            || !seen.insert(s.code.clone())
            || !s.name.valid(80)
            || caps.len() != s.capabilities.len()
            || !s.capabilities.iter().all(|c| known.contains(c.as_str()))
            // `authorization.manage` stays with Owners and the Principal (spec §10 #8.3); the
            // Principal seed is given per school only.
            || (s.code != PRINCIPAL && s.capabilities.iter().any(|c| c == MANAGE))
            || (s.code == PRINCIPAL
                && (s.allowed_scopes != [SCHOOL] || !s.capabilities.iter().any(|c| c == MANAGE)))
            || !valid_scope_types(&s.allowed_scopes)
        {
            return Err(format!("invalid seed template {}", s.code));
        }
    }
    for required in [REGULAR_TEACHER, SCHOOL_ADMIN, PRINCIPAL] {
        if !seen.contains(required) {
            return Err(format!("missing seed template {required}"));
        }
    }
    Ok(seeds.templates)
}
fn valid_scope_types(types: &[String]) -> bool {
    let unique: BTreeSet<&String> = types.iter().collect();
    !types.is_empty()
        && unique.len() == types.len()
        && types.iter().all(|t| SCOPE_TYPES.contains(&t.as_str()))
}

/// The reviewed catalogue and seed templates in this repository, checked once at start-up.
pub struct Reference {
    pub catalogue: Catalogue,
    pub seeds: Vec<Seed>,
}
impl Reference {
    pub fn capability(&self, code: &str) -> Option<&Capability> {
        self.catalogue.capabilities.iter().find(|c| c.code == code)
    }
    pub fn sensitive(&self, code: &str) -> bool {
        self.capability(code)
            .is_none_or(|c| c.sensitivity == "sensitive")
    }
    pub fn class_wide(&self, code: &str) -> bool {
        self.capability(code).is_some_and(|c| c.class_wide)
    }
    pub fn all(&self) -> BTreeSet<String> {
        self.catalogue
            .capabilities
            .iter()
            .map(|c| c.code.clone())
            .collect()
    }
}
pub fn load() -> Result<Reference, String> {
    let catalogue = parse_catalogue(include_str!("capabilities.json"))?;
    let seeds = parse_seeds(include_str!("templates.json"), &catalogue)?;
    Ok(Reference { catalogue, seeds })
}
pub fn reference() -> &'static Reference {
    static REFERENCE: OnceLock<Reference> = OnceLock::new();
    REFERENCE.get_or_init(|| load().expect("authorization catalogue is checked at start-up"))
}

// ---------------------------------------------------------------------------------------------
// Scopes

/// Where a capability applies. The tenant is carried by the request, never by the scope.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub school_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade_to: Option<String>,
}
/// The first number of a grade label ("Year 7" → 7), as subject plans order grades.
pub fn grade_number(grade: &str) -> Option<u32> {
    grade
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}
impl Scope {
    pub fn school(school: &str) -> Self {
        Self::of(SCHOOL, Some(school), None, None)
    }
    pub fn section(school: &str, section: &str) -> Self {
        Self::of(SECTION, Some(school), Some(section), None)
    }
    pub fn section_subject(school: &str, section: &str, subject: &str) -> Self {
        Self::of(SECTION_SUBJECT, Some(school), Some(section), Some(subject))
    }
    pub fn tenant() -> Self {
        Self::of(TENANT, None, None, None)
    }
    fn of(kind: &str, school: Option<&str>, section: Option<&str>, subject: Option<&str>) -> Self {
        Self {
            kind: kind.into(),
            school_id: school.map(Into::into),
            section_id: section.map(Into::into),
            subject: subject.map(Into::into),
            grade_from: None,
            grade_to: None,
        }
    }
    pub fn valid(&self) -> bool {
        let grade = |g: &Option<String>| {
            g.as_deref()
                .is_none_or(|g| key_part(g, 64) && !g.contains('#') && grade_number(g).is_some())
        };
        let range = match (&self.grade_from, &self.grade_to) {
            (Some(a), Some(b)) => grade_number(a) <= grade_number(b),
            _ => true,
        };
        let shape = match self.kind.as_str() {
            TENANT => {
                self.school_id.is_none() && self.section_id.is_none() && self.subject.is_none()
            }
            SCHOOL => self.section_id.is_none() && self.subject.is_none(),
            SCHOOL_SUBJECT => self.section_id.is_none() && self.subject.is_some(),
            SECTION => self.section_id.is_some() && self.subject.is_none(),
            SECTION_SUBJECT => self.section_id.is_some() && self.subject.is_some(),
            _ => false,
        };
        let grades_allowed =
            self.kind == SCHOOL_SUBJECT || (self.grade_from.is_none() && self.grade_to.is_none());
        shape
            && grades_allowed
            && (self.kind == TENANT) == self.school_id.is_none()
            && self.school_id.as_deref().is_none_or(identifier)
            && self.section_id.as_deref().is_none_or(identifier)
            && self.subject.as_deref().is_none_or(subject_code)
            && grade(&self.grade_from)
            && grade(&self.grade_to)
            && range
    }
    /// A stable text form, part of an assignment's key.
    pub fn signature(&self) -> String {
        [
            Some(self.kind.as_str()),
            self.school_id.as_deref(),
            self.section_id.as_deref(),
            self.subject.as_deref(),
            self.grade_from.as_deref(),
            self.grade_to.as_deref(),
        ]
        .iter()
        .map(|p| p.unwrap_or(""))
        .collect::<Vec<_>>()
        .join("|")
    }
}
fn bounds(from: &Option<String>, to: &Option<String>) -> (Option<u32>, Option<u32>) {
    (
        from.as_deref().and_then(grade_number),
        to.as_deref().and_then(grade_number),
    )
}
fn grade_in(grade: Option<&str>, held: &Scope) -> bool {
    let (low, high) = bounds(&held.grade_from, &held.grade_to);
    if low.is_none() && high.is_none() {
        return true;
    }
    grade
        .and_then(grade_number)
        .is_some_and(|g| low.is_none_or(|l| g >= l) && high.is_none_or(|h| g <= h))
}
fn range_within(requested: &Scope, held: &Scope) -> bool {
    let (rl, rh) = bounds(&requested.grade_from, &requested.grade_to);
    let (hl, hh) = bounds(&held.grade_from, &held.grade_to);
    hl.is_none_or(|h| rl.is_some_and(|r| r >= h)) && hh.is_none_or(|h| rh.is_some_and(|r| r <= h))
}
/// Whether holding a capability at `held` holds it at `requested`. `grade` is the requested
/// class's grade. A class-wide capability held for one subject of a class covers the class.
pub fn covers(held: &Scope, requested: &Scope, grade: Option<&str>, class_wide: bool) -> bool {
    if held.kind == TENANT {
        return true;
    }
    if requested.kind == TENANT || held.school_id != requested.school_id {
        return false;
    }
    let same_section = held.section_id == requested.section_id;
    match (held.kind.as_str(), requested.kind.as_str()) {
        (SCHOOL, _) => true,
        (SCHOOL_SUBJECT, SCHOOL_SUBJECT) => {
            held.subject == requested.subject && range_within(requested, held)
        }
        (SCHOOL_SUBJECT, SECTION_SUBJECT) => {
            held.subject == requested.subject && grade_in(grade, held)
        }
        (SCHOOL_SUBJECT, SECTION) => class_wide && grade_in(grade, held),
        (SECTION, SECTION | SECTION_SUBJECT) => same_section,
        (SECTION_SUBJECT, SECTION_SUBJECT) => same_section && held.subject == requested.subject,
        (SECTION_SUBJECT, SECTION) => class_wide && same_section,
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Facts and holdings

/// What the graph knows, for one tenant. `staff` holds the Auth mirror of each person.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub tenant: String,
    pub today: Option<NaiveDate>,
    pub staff: Vec<Staff>,
    /// Unique schools of the tenant: id → name.
    pub schools: BTreeMap<String, String>,
    pub sections: Vec<Section>,
    pub grants: Vec<(String, String, String)>,
    pub teaching: Vec<Teaching>,
    pub assignments: Vec<Assignment>,
    pub templates: Vec<Template>,
}
#[derive(Clone, Debug, Default)]
pub struct Staff {
    pub id: String,
    pub name: String,
    pub copies: i64,
    pub status: String,
    pub roles: Vec<String>,
    pub school_ids: Vec<String>,
    pub tenant_ids: Vec<String>,
    pub team_ids: Vec<String>,
}
#[derive(Clone, Debug, Default)]
pub struct Section {
    pub id: String,
    pub school: String,
    pub name: String,
    pub grade: String,
    pub year: String,
    pub homeroom: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct Teaching {
    pub id: String,
    pub staff: String,
    pub section: String,
    pub school: String,
    pub subject: String,
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    pub id: String,
    pub staff_member_id: String,
    pub template_code: String,
    pub scope: Scope,
    pub academic_year: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    pub status: String,
    pub version: u32,
    pub updated_at: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub code: String,
    pub name: Text,
    pub version: u32,
    pub capabilities: Vec<String>,
    pub allowed_scopes: Vec<String>,
    pub status: String,
    /// Still the repository default: never saved for this tenant.
    pub seeded: bool,
    pub updated_at: Option<String>,
}
impl Template {
    pub fn from_seed(seed: &Seed) -> Self {
        Self {
            code: seed.code.clone(),
            name: seed.name.clone(),
            version: 1,
            capabilities: seed.capabilities.clone(),
            allowed_scopes: seed.allowed_scopes.clone(),
            status: "active".into(),
            seeded: true,
            updated_at: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// `owner`, `assignment`, `teaching_assignment`, `teaching_grant`, `homeroom`, `auth_role`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_version: Option<u32>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Holding {
    pub source: Source,
    pub scope: Scope,
    pub capabilities: BTreeSet<String>,
    /// Derived from another record: read-only in Admin.
    pub derived: bool,
}

impl Staff {
    pub fn active(&self) -> bool {
        self.copies == 1 && self.status == "ACTIVE"
    }
    fn has(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }
    pub fn owner(&self) -> bool {
        self.has("owner")
    }
    /// Today's `staff_capability::owner` fragment: an Owner without teams, unscoped or scoped to
    /// the school and tenant, as one Staff Member node.
    fn owner_of(&self, tenant: &str, school: Option<&str>) -> bool {
        self.active()
            && self.owner()
            && self.team_ids.is_empty()
            && ((self.school_ids.is_empty() && self.tenant_ids.is_empty())
                || school.is_some_and(|s| {
                    self.school_ids.iter().any(|x| x == s)
                        && self.tenant_ids.iter().any(|x| x == tenant)
                }))
    }
    pub fn unscoped_owner(&self) -> bool {
        self.owner_of("", None)
    }
    /// Today's role branch of `staff_capability::authority`: never an Owner.
    pub fn member(&self, role: &str, tenant: &str, school: &str) -> bool {
        self.active()
            && !self.owner()
            && self.has(role)
            && self.school_ids.iter().any(|x| x == school)
            && self.tenant_ids.iter().any(|x| x == tenant)
    }
    /// Belongs to the school in Auth at all, whatever the role.
    pub fn in_school(&self, tenant: &str, school: &str) -> bool {
        self.active()
            && self.school_ids.iter().any(|x| x == school)
            && self.tenant_ids.iter().any(|x| x == tenant)
    }
}

pub fn parse_date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}
/// Today at the schools (UTC+7).
pub fn today() -> NaiveDate {
    (chrono::Utc::now() + chrono::Duration::hours(7)).date_naive()
}
/// The first and last day of an academic year written `YYYY/YYYY` (or `YYYY-YYYY`) with
/// consecutive years: 1 July to 30 June. Other spellings name no span.
pub fn academic_year_span(value: &str) -> Option<(NaiveDate, NaiveDate)> {
    let (a, b) = value.trim().split_once(['/', '-'])?;
    let digits = |x: &str| x.len() == 4 && x.bytes().all(|c| c.is_ascii_digit());
    if !digits(a) || !digits(b) {
        return None;
    }
    let (a, b): (i32, i32) = (a.parse().ok()?, b.parse().ok()?);
    if b != a + 1 {
        return None;
    }
    Some((
        NaiveDate::from_ymd_opt(a, 7, 1)?,
        NaiveDate::from_ymd_opt(b, 6, 30)?,
    ))
}
/// How an assignment's end was set; see `Facts::validity`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Validity {
    #[serde(skip)]
    pub valid_until: Option<String>,
    pub rule: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub academic_year: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
}
impl Assignment {
    /// Active and inside its dates today.
    pub fn current(&self, today: NaiveDate) -> bool {
        self.status == "active"
            && self
                .valid_from
                .as_deref()
                .and_then(parse_date)
                .is_none_or(|d| d <= today)
            && self
                .valid_until
                .as_deref()
                .and_then(parse_date)
                .is_none_or(|d| d >= today)
    }
}

impl Facts {
    pub fn template(&self, code: &str) -> Option<&Template> {
        self.templates.iter().find(|t| t.code == code)
    }
    pub fn section(&self, school: &str, id: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| s.id == id && s.school == school)
    }
    pub fn person(&self, id: &str) -> Option<&Staff> {
        self.staff.iter().find(|s| s.id == id)
    }
    pub fn today(&self) -> NaiveDate {
        self.today.unwrap_or_else(today)
    }
    /// The academic year in force on `on` at `school` (any school of the tenant when `None`): the
    /// year of its active classes whose span holds `on`, else the calendar year from 1 July.
    pub fn academic_year_on(
        &self,
        school: Option<&str>,
        on: NaiveDate,
    ) -> (String, NaiveDate, &'static str) {
        let from_classes = self
            .sections
            .iter()
            .filter(|s| school.is_none_or(|x| x == s.school))
            .filter_map(|s| academic_year_span(&s.year).map(|span| (s.year.as_str(), span)))
            .filter(|(_, (start, end))| *start <= on && on <= *end)
            .map(|(year, (_, end))| (year.to_string(), end))
            .min_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        if let Some((year, end)) = from_classes {
            return (year, end, "classes");
        }
        let first = if on.month() >= 7 {
            on.year()
        } else {
            on.year() - 1
        };
        let year = format!("{first}/{}", first + 1);
        let end = academic_year_span(&year).map_or(on, |(_, end)| end);
        (year, end, "calendar")
    }
    /// The dates an `assign` or `change_validity` command saves, and the rule that set the end:
    /// `explicit` (the command named it), `until_revoked` (Principal and Curriculum coordinator) or
    /// `academic_year_end` (the academic year the command names, else the one in force on
    /// `validFrom` or today, at the scope's school).
    pub fn validity(
        &self,
        template: &str,
        scope: &Scope,
        academic_year: Option<&str>,
        from: Option<&str>,
        until: Option<&str>,
    ) -> Validity {
        if let Some(until) = until {
            return Validity {
                valid_until: Some(until.into()),
                rule: "explicit",
                academic_year: None,
                source: None,
            };
        }
        if UNTIL_REVOKED.contains(&template) {
            return Validity {
                valid_until: None,
                rule: "until_revoked",
                academic_year: None,
                source: None,
            };
        }
        let on = from.and_then(parse_date).unwrap_or_else(|| self.today());
        let (year, end, source) = academic_year
            .and_then(|y| academic_year_span(y).map(|(_, end)| (y.to_string(), end, "command")))
            .filter(|(_, end, _)| *end >= on)
            .unwrap_or_else(|| self.academic_year_on(scope.school_id.as_deref(), on));
        Validity {
            valid_until: Some(end.format("%Y-%m-%d").to_string()),
            rule: "academic_year_end",
            academic_year: Some(year),
            source: Some(source),
        }
    }
    fn caps(&self, code: &str) -> Option<(BTreeSet<String>, u32)> {
        self.template(code)
            .filter(|t| t.status == "active")
            .map(|t| (t.capabilities.iter().cloned().collect(), t.version))
    }
    /// Whether an explicit assignment's scope still exists: the school, and for a class scope
    /// an active class of the school (in the assignment's academic year when it names one).
    fn place_exists(&self, a: &Assignment) -> bool {
        match a.scope.school_id.as_deref() {
            None => !self.schools.is_empty(),
            Some(school) => {
                self.schools.contains_key(school)
                    && a.scope.section_id.as_deref().is_none_or(|id| {
                        self.section(school, id)
                            .is_some_and(|s| a.academic_year.as_deref().is_none_or(|y| y == s.year))
                    })
            }
        }
    }
    /// Every holding of one staff member, Owner first, then explicit assignments, then today's
    /// records. An inactive, duplicated or unknown person holds nothing.
    pub fn holdings(&self, staff_id: &str) -> Vec<Holding> {
        let Some(person) = self.person(staff_id).filter(|p| p.active()) else {
            return vec![];
        };
        let tenant = self.tenant.as_str();
        let derived = |kind: &str, id: Option<&str>, code: &str, version: u32| Source {
            kind: kind.into(),
            id: id.map(Into::into),
            template_code: (!code.is_empty()).then(|| code.into()),
            template_version: (version > 0).then_some(version),
        };
        let mut out = vec![];
        if person.owner() {
            // An Owner holds everything within its schools and nothing through other sources,
            // as today's role checks never combine Owner with another role.
            let all = reference().all();
            if person.unscoped_owner() {
                out.push(Holding {
                    source: derived("owner", None, "", 0),
                    scope: Scope::tenant(),
                    capabilities: all,
                    derived: true,
                });
            } else {
                for school in self.schools.keys() {
                    if person.owner_of(tenant, Some(school)) {
                        out.push(Holding {
                            source: derived("owner", None, "", 0),
                            scope: Scope::school(school),
                            capabilities: all.clone(),
                            derived: true,
                        });
                    }
                }
            }
            return out;
        }
        let today = self.today();
        let mut replaced_admin = BTreeSet::new();
        for a in self
            .assignments
            .iter()
            .filter(|a| a.staff_member_id == staff_id)
        {
            let in_scope = match a.scope.school_id.as_deref() {
                Some(school) => person.in_school(tenant, school),
                None => person.tenant_ids.iter().any(|t| t == tenant),
            };
            if !a.current(today) || !in_scope || !self.place_exists(a) {
                continue;
            }
            let Some((caps, version)) = self.caps(&a.template_code) else {
                continue;
            };
            if a.template_code == SCHOOL_ADMIN && a.scope.kind == SCHOOL {
                replaced_admin.extend(a.scope.school_id.clone());
            }
            out.push(Holding {
                source: derived("assignment", Some(&a.id), &a.template_code, version),
                scope: a.scope.clone(),
                capabilities: caps,
                derived: false,
            });
        }
        // Auth role `school_admin`, until an explicit School admin assignment replaces it.
        if let Some((caps, version)) = self.caps(SCHOOL_ADMIN) {
            for school in self.schools.keys() {
                if person.member("school_admin", tenant, school) && !replaced_admin.contains(school)
                {
                    out.push(Holding {
                        source: derived("auth_role", None, SCHOOL_ADMIN, version),
                        scope: Scope::school(school),
                        capabilities: caps.clone(),
                        derived: true,
                    });
                }
            }
        }
        // Today's teaching records: the `TEACHER` projection of the Learning owner context.
        if let Some((caps, version)) = self.caps(REGULAR_TEACHER) {
            for s in &self.sections {
                if !person.member("teacher", tenant, &s.school) {
                    continue;
                }
                // The class's grant stays the gate, as today: assignments keep it in line, and a
                // grant revoked by hand also ends the access of an assignment still open.
                let granted = self.grants.iter().any(|(staff, section, school)| {
                    staff == staff_id && *section == s.id && *school == s.school
                });
                let taught: Vec<&Teaching> = self
                    .teaching
                    .iter()
                    .filter(|t| t.staff == staff_id && t.section == s.id && t.school == s.school)
                    .collect();
                for t in taught.iter().filter(|_| granted) {
                    out.push(Holding {
                        source: derived(
                            "teaching_assignment",
                            Some(&t.id),
                            REGULAR_TEACHER,
                            version,
                        ),
                        scope: Scope::section_subject(&s.school, &s.id, &t.subject),
                        capabilities: caps.clone(),
                        derived: true,
                    });
                }
                if granted && taught.is_empty() {
                    out.push(Holding {
                        source: derived("teaching_grant", None, REGULAR_TEACHER, version),
                        scope: Scope::section(&s.school, &s.id),
                        capabilities: caps.clone(),
                        derived: true,
                    });
                }
                if s.homeroom.as_deref() == Some(staff_id) {
                    let mut homeroom = caps.clone();
                    homeroom.insert(HOMEROOM.into());
                    out.push(Holding {
                        source: derived("homeroom", None, REGULAR_TEACHER, version),
                        scope: Scope::section(&s.school, &s.id),
                        capabilities: homeroom,
                        derived: true,
                    });
                }
            }
        }
        out
    }
    /// The grade of the class a scope names, when it names one.
    pub fn grade(&self, scope: &Scope) -> Option<&str> {
        let section = self.section(scope.school_id.as_deref()?, scope.section_id.as_deref()?)?;
        Some(section.grade.as_str())
    }
    /// Whether the scope names something that exists now: a school of the tenant and, for class
    /// scopes, an active class of that school. Checks on anything else are denied.
    pub fn exists(&self, scope: &Scope) -> bool {
        match scope.school_id.as_deref() {
            None => !self.schools.is_empty(),
            Some(school) => {
                self.schools.contains_key(school)
                    && scope
                        .section_id
                        .as_deref()
                        .is_none_or(|id| self.section(school, id).is_some())
            }
        }
    }
}

/// The first holding that grants `capability` at `scope`, or `None` (deny).
pub fn decide<'a>(
    facts: &Facts,
    holdings: &'a [Holding],
    capability: &str,
    scope: &Scope,
) -> Option<&'a Holding> {
    let r = reference();
    if r.capability(capability).is_none() || !scope.valid() || !facts.exists(scope) {
        return None;
    }
    let grade = facts.grade(scope);
    let wide = r.class_wide(capability);
    holdings
        .iter()
        .find(|h| h.capabilities.contains(capability) && covers(&h.scope, scope, grade, wide))
}
/// Holds `capability` for the whole tenant: at `TENANT`, or at every school of it.
pub fn holds_tenant(facts: &Facts, holdings: &[Holding], capability: &str) -> bool {
    let tenant = Scope::tenant();
    holdings
        .iter()
        .any(|h| h.capabilities.contains(capability) && h.scope == tenant)
        || (!facts.schools.is_empty()
            && facts
                .schools
                .keys()
                .all(|s| decide(facts, holdings, capability, &Scope::school(s)).is_some()))
}
/// Whether the holding that answers `capability` at `scope` is an Owner's.
pub fn owner_at(facts: &Facts, holdings: &[Holding], scope: &Scope) -> bool {
    let owned: Vec<Holding> = holdings
        .iter()
        .filter(|h| h.source.kind == "owner")
        .cloned()
        .collect();
    if scope.kind == TENANT {
        return holds_tenant(facts, &owned, MANAGE);
    }
    decide(facts, &owned, MANAGE, scope).is_some()
}
/// Schools where the holdings may read authorization data.
pub fn readable_schools(facts: &Facts, holdings: &[Holding]) -> BTreeSet<String> {
    facts
        .schools
        .keys()
        .filter(|s| {
            let scope = Scope::school(s);
            decide(facts, holdings, READ, &scope).is_some()
                || decide(facts, holdings, MANAGE, &scope).is_some()
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Requests

pub fn academic_year(value: &str) -> bool {
    key_part(value, 32)
}
pub fn date(value: &str) -> bool {
    value.len() == 10 && parse_date(value).is_some()
}
fn dates(from: &Option<String>, until: &Option<String>) -> bool {
    from.as_deref().is_none_or(date)
        && until.as_deref().is_none_or(date)
        && match (from.as_deref(), until.as_deref()) {
            (Some(a), Some(b)) => a <= b,
            _ => true,
        }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckRequest {
    pub tenant_id: String,
    /// The staff member asked about; the caller when absent.
    #[serde(default)]
    pub actor: Option<String>,
    pub capability: String,
    pub scope: Scope,
}
impl CheckRequest {
    /// An unknown capability is not invalid input: it is a deny.
    pub fn valid(&self) -> bool {
        identifier(&self.tenant_id)
            && self.actor.as_deref().is_none_or(identifier)
            && capability_code(&self.capability)
            && self.scope.valid()
    }
}

pub const CREATE: &str = "create";
pub const REVISE: &str = "revise";
pub const ARCHIVE: &str = "archive";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateCommand {
    pub action: String,
    pub code: String,
    #[serde(default)]
    pub name: Option<Text>,
    #[serde(default)]
    pub capabilities: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_scopes: Option<Vec<String>>,
    pub expected_version: u32,
}
impl TemplateCommand {
    pub fn valid(&self) -> bool {
        let body = match self.action.as_str() {
            CREATE | REVISE => {
                self.name.as_ref().is_some_and(|n| n.valid(80))
                    && self.capabilities.as_ref().is_some_and(|c| {
                        let unique: BTreeSet<&String> = c.iter().collect();
                        c.len() <= MAX_CAPABILITIES
                            && unique.len() == c.len()
                            && c.iter().all(|c| capability_code(c))
                    })
                    && self
                        .allowed_scopes
                        .as_deref()
                        .is_some_and(valid_scope_types)
            }
            ARCHIVE => {
                self.name.is_none() && self.capabilities.is_none() && self.allowed_scopes.is_none()
            }
            _ => false,
        };
        body && template_code(&self.code) && self.expected_version < i32::MAX as u32
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateCommands {
    pub tenant_id: String,
    pub commands: Vec<TemplateCommand>,
}
impl TemplateCommands {
    pub fn valid(&self) -> bool {
        let codes: BTreeSet<&str> = self.commands.iter().map(|c| c.code.as_str()).collect();
        identifier(&self.tenant_id)
            && (1..=MAX_COMMANDS).contains(&self.commands.len())
            && codes.len() == self.commands.len()
            && self.commands.iter().all(TemplateCommand::valid)
    }
}

pub const ASSIGN: &str = "assign";
pub const REVOKE: &str = "revoke";
pub const CHANGE_VALIDITY: &str = "change_validity";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentCommand {
    pub action: String,
    #[serde(default)]
    pub assignment_id: Option<String>,
    #[serde(default)]
    pub staff_member_id: Option<String>,
    #[serde(default)]
    pub template_code: Option<String>,
    #[serde(default)]
    pub scope: Option<Scope>,
    #[serde(default)]
    pub academic_year: Option<String>,
    #[serde(default)]
    pub valid_from: Option<String>,
    #[serde(default)]
    pub valid_until: Option<String>,
    pub expected_version: u32,
}
impl AssignmentCommand {
    pub fn valid(&self) -> bool {
        let body = match self.action.as_str() {
            ASSIGN => {
                self.assignment_id.is_none()
                    && self.staff_member_id.as_deref().is_some_and(identifier)
                    && self.template_code.as_deref().is_some_and(template_code)
                    && self.scope.as_ref().is_some_and(Scope::valid)
                    && self.academic_year.as_deref().is_none_or(academic_year)
                    && dates(&self.valid_from, &self.valid_until)
            }
            REVOKE => {
                self.assignment_id.as_deref().is_some_and(identifier)
                    && self.staff_member_id.is_none()
                    && self.template_code.is_none()
                    && self.scope.is_none()
                    && self.academic_year.is_none()
                    && self.valid_from.is_none()
                    && self.valid_until.is_none()
            }
            CHANGE_VALIDITY => {
                self.assignment_id.as_deref().is_some_and(identifier)
                    && self.staff_member_id.is_none()
                    && self.template_code.is_none()
                    && self.scope.is_none()
                    && self.academic_year.is_none()
                    && dates(&self.valid_from, &self.valid_until)
            }
            _ => false,
        };
        body && self.expected_version < i32::MAX as u32
    }
    /// Who, which template, where and when: one record per combination.
    pub fn key(&self, tenant: &str) -> Option<String> {
        Some(
            [
                tenant,
                self.staff_member_id.as_deref()?,
                self.template_code.as_deref()?,
                &self.scope.as_ref()?.signature(),
                self.academic_year.as_deref().unwrap_or(""),
            ]
            .join("#"),
        )
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentCommands {
    pub tenant_id: String,
    pub commands: Vec<AssignmentCommand>,
}
impl AssignmentCommands {
    pub fn valid(&self) -> bool {
        let targets: BTreeSet<String> = self
            .commands
            .iter()
            .map(|c| {
                c.assignment_id
                    .clone()
                    .or_else(|| c.key(&self.tenant_id))
                    .unwrap_or_default()
            })
            .collect();
        identifier(&self.tenant_id)
            && (1..=MAX_COMMANDS).contains(&self.commands.len())
            && targets.len() == self.commands.len()
            && self.commands.iter().all(AssignmentCommand::valid)
    }
}
