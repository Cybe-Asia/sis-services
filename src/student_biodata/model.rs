//! Admin-entered Student biodata and guardians. Every write replaces the whole record, so each
//! field must be present in the body (null clears it); values are normalised before storage.
use crate::school_portal::model::identifier;
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, BTreeSet};

/// The admissions account holder is shown first on the Student Profile (max 4 guardians), so the
/// school records at most three more.
pub const MAX_GUARDIANS: usize = 3;

fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

/// Admin opens a Student by id alone; SIS finds the school from the active enrolment.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lookup {
    pub student_id: String,
}

#[derive(Debug)]
pub struct Scope {
    pub tenant_id: String,
    pub school_id: String,
    pub student_id: String,
}
impl Scope {
    pub fn valid(&self) -> bool {
        identifier(&self.tenant_id) && identifier(&self.school_id) && identifier(&self.student_id)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub tenant_id: String,
    pub school_id: String,
    pub student_id: String,
    pub expected_version: u32,
    pub biodata: BiodataInput,
    #[serde(deserialize_with = "present")]
    pub admissions_guardian_relationship: Option<String>,
    pub guardians: Vec<GuardianInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BiodataInput {
    #[serde(deserialize_with = "present")]
    pub nisn: Option<String>,
    #[serde(deserialize_with = "present")]
    pub birth_place: Option<String>,
    #[serde(deserialize_with = "present")]
    pub gender: Option<String>,
    #[serde(deserialize_with = "present")]
    pub emergency_contact: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuardianInput {
    #[serde(deserialize_with = "present")]
    pub id: Option<String>,
    pub full_name: String,
    pub relationship: String,
    pub phone_primary: String,
    #[serde(deserialize_with = "present")]
    pub phone_secondary: Option<String>,
    #[serde(deserialize_with = "present")]
    pub email: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Biodata {
    pub nisn: Option<String>,
    pub birth_place: Option<String>,
    pub gender: Option<String>,
    pub emergency_contact: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guardian {
    pub full_name: String,
    pub relationship: String,
    pub phone_primary: String,
    pub phone_secondary: Option<String>,
    pub email: Option<String>,
}

/// A validated change: `guardians` keeps the submitted order; `None` ids are new guardians.
#[derive(Debug, PartialEq, Eq)]
pub struct Clean {
    pub biodata: Biodata,
    pub relationship: Option<String>,
    pub guardians: Vec<(Option<String>, Guardian)>,
}

/// Blank is "not recorded"; anything else is trimmed and must fit without control characters.
fn text(value: &Option<String>, max: usize) -> Result<Option<String>, ()> {
    let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(());
    }
    Ok(Some(value.to_string()))
}
pub fn nisn(value: &str) -> bool {
    value.len() == 10 && value.bytes().all(|b| b.is_ascii_digit())
}
pub fn phone(value: &str) -> bool {
    let digits = value.chars().filter(char::is_ascii_digit).count();
    value.chars().count() <= 32
        && (6..=15).contains(&digits)
        && value.chars().enumerate().all(|(i, c)| {
            c.is_ascii_digit() || matches!(c, ' ' | '-' | '(' | ')') || (i == 0 && c == '+')
        })
}
pub fn email(value: &str) -> bool {
    let mut parts = value.split('@');
    value.len() <= 254
        && !value.contains(char::is_whitespace)
        && matches!((parts.next(), parts.next(), parts.next()), (Some(l), Some(d), None) if !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.'))
}
pub const GENDERS: [&str; 2] = ["male", "female"];
pub const RELATIONSHIPS: [&str; 3] = ["father", "mother", "guardian"];

fn digits(value: &str) -> String {
    value.chars().filter(char::is_ascii_digit).collect()
}
fn checked(value: Option<String>, rule: fn(&str) -> bool) -> Result<Option<String>, ()> {
    match value {
        Some(v) if !rule(&v) => Err(()),
        v => Ok(v),
    }
}
fn one_of(value: Option<String>, allowed: &[&str]) -> Result<Option<String>, ()> {
    match value {
        Some(v) if !allowed.contains(&v.as_str()) => Err(()),
        v => Ok(v),
    }
}

impl Change {
    pub fn scope(&self) -> Scope {
        Scope {
            tenant_id: self.tenant_id.clone(),
            school_id: self.school_id.clone(),
            student_id: self.student_id.clone(),
        }
    }
    /// `None` when any field breaks the contract in `docs/STUDENT_BIODATA.md`.
    pub fn clean(&self) -> Option<Clean> {
        if !self.scope().valid() || self.expected_version > i32::MAX as u32 - 1 {
            return None;
        }
        let b = &self.biodata;
        let biodata = Biodata {
            nisn: checked(text(&b.nisn, 10).ok()?, nisn).ok()?,
            birth_place: text(&b.birth_place, 128).ok()?,
            gender: one_of(text(&b.gender, 16).ok()?, &GENDERS).ok()?,
            emergency_contact: checked(text(&b.emergency_contact, 32).ok()?, phone).ok()?,
        };
        let relationship =
            one_of(text(&self.admissions_guardian_relationship, 16).ok()?, &RELATIONSHIPS).ok()?;
        if self.guardians.len() > MAX_GUARDIANS {
            return None;
        }
        let mut ids = BTreeSet::new();
        let mut guardians = vec![];
        for g in &self.guardians {
            let id = text(&g.id, 64).ok()?;
            if let Some(id) = &id {
                if uuid::Uuid::parse_str(id).is_err() || !ids.insert(id.clone()) {
                    return None;
                }
            }
            let phone_primary = checked(text(&Some(g.phone_primary.clone()), 32).ok()?, phone).ok()??;
            let phone_secondary = checked(text(&g.phone_secondary, 32).ok()?, phone).ok()?;
            if phone_secondary.as_deref().map(digits) == Some(digits(&phone_primary)) {
                return None;
            }
            guardians.push((
                id,
                Guardian {
                    full_name: text(&Some(g.full_name.clone()), 256).ok()??,
                    relationship: one_of(text(&Some(g.relationship.clone()), 16).ok()?, &RELATIONSHIPS).ok()??,
                    phone_primary,
                    phone_secondary,
                    email: checked(text(&g.email, 254).ok()?, email).ok()?,
                },
            ));
        }
        Some(Clean { biodata, relationship, guardians })
    }
}

/// What a save changes, by field name only: the audit never holds the values themselves.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub fields: Vec<&'static str>,
    pub added: usize,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub reordered: bool,
}
impl Diff {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
            && self.added == 0
            && self.updated.is_empty()
            && self.removed.is_empty()
            && !self.reordered
    }
}
/// `Err` when the change names a guardian the record no longer has (a stale form).
pub fn diff(
    old: &Biodata,
    old_relationship: &Option<String>,
    old_guardians: &[(String, Guardian)],
    new: &Clean,
) -> Result<Diff, ()> {
    let mut d = Diff::default();
    for (name, before, after) in [
        ("nisn", &old.nisn, &new.biodata.nisn),
        ("birthPlace", &old.birth_place, &new.biodata.birth_place),
        ("gender", &old.gender, &new.biodata.gender),
        ("emergencyContact", &old.emergency_contact, &new.biodata.emergency_contact),
        ("admissionsGuardianRelationship", old_relationship, &new.relationship),
    ] {
        if before != after {
            d.fields.push(name);
        }
    }
    let existing: BTreeMap<&str, &Guardian> =
        old_guardians.iter().map(|(id, g)| (id.as_str(), g)).collect();
    let mut kept = vec![];
    for (id, g) in &new.guardians {
        match id {
            None => d.added += 1,
            Some(id) => {
                let before = existing.get(id.as_str()).ok_or(())?;
                if *before != g {
                    d.updated.push(id.clone());
                }
                kept.push(id.as_str());
            }
        }
    }
    d.removed = old_guardians
        .iter()
        .map(|(id, _)| id)
        .filter(|id| !kept.contains(&id.as_str()))
        .cloned()
        .collect();
    let old_order: Vec<&str> = old_guardians
        .iter()
        .map(|(id, _)| id.as_str())
        .filter(|id| kept.contains(id))
        .collect();
    d.reordered = old_order != kept;
    Ok(d)
}
