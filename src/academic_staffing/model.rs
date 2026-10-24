//! Subject plans and teaching assignments (`docs/TEACHING_ASSIGNMENTS.md`): request shapes,
//! validation, record keys and the revision rule of each command. Pure.
use crate::school_portal::model::identifier;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Curriculum frameworks of the combined school curriculum (spec §3.1 and §7 #6).
pub const FRAMEWORKS: [&str; 3] = ["merdeka", "cambridge", "iiec"];
pub const MAX_ROWS: usize = 40;
pub const MAX_PERIODS: u32 = 40;
pub const MAX_COMMANDS: usize = 20;

fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.trim() == value
        && value.len() <= max
        && !value.chars().any(char::is_control)
}
/// Academic years and grades are parts of record keys, so they never contain the separator.
pub fn key_part(value: &str, max: usize) -> bool {
    text(value, max) && !value.contains('|')
}
/// The LMS subject code: lower case letters, digits, `_` and `-`.
pub fn subject_code(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}
fn scope(tenant: &str, school: &str, year: &str) -> bool {
    identifier(tenant) && identifier(school) && key_part(year, 32)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanRow {
    pub subject: String,
    pub name: String,
    pub framework: String,
    pub periods_per_week: u32,
    pub required: bool,
    pub status: String,
}
impl PlanRow {
    pub fn valid(&self) -> bool {
        subject_code(&self.subject)
            && text(&self.name, 120)
            && FRAMEWORKS.contains(&self.framework.as_str())
            && (1..=MAX_PERIODS).contains(&self.periods_per_week)
            && ["active", "archived"].contains(&self.status.as_str())
    }
    pub fn active(&self) -> bool {
        self.status == "active"
    }
}
/// At most `MAX_ROWS` valid rows, one per subject code. A code belongs to one framework per grade
/// because lessons and assignments record the code only.
pub fn valid_rows(rows: &[PlanRow]) -> bool {
    let codes: BTreeSet<&str> = rows.iter().map(|r| r.subject.as_str()).collect();
    rows.len() <= MAX_ROWS && codes.len() == rows.len() && rows.iter().all(PlanRow::valid)
}
pub fn plan_key(tenant: &str, school: &str, year: &str, grade: &str) -> String {
    [tenant, school, year, grade].join("|")
}

/// Replaces one grade's subject plan (`expectedVersion` 0 creates it).
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanChange {
    pub tenant_id: String,
    pub school_id: String,
    pub academic_year: String,
    pub grade: String,
    pub expected_version: u32,
    pub rows: Vec<PlanRow>,
}
impl PlanChange {
    pub fn valid(&self) -> bool {
        scope(&self.tenant_id, &self.school_id, &self.academic_year)
            && key_part(&self.grade, 128)
            && self.expected_version < i32::MAX as u32
            && valid_rows(&self.rows)
    }
}
/// Fills one grade's plan with the active rows of the same grade in another academic year.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanCopy {
    pub tenant_id: String,
    pub school_id: String,
    pub academic_year: String,
    pub grade: String,
    pub from_year: String,
    pub expected_version: u32,
}
impl PlanCopy {
    pub fn valid(&self) -> bool {
        scope(&self.tenant_id, &self.school_id, &self.academic_year)
            && key_part(&self.grade, 128)
            && key_part(&self.from_year, 32)
            && self.from_year != self.academic_year
            && self.expected_version < i32::MAX as u32
    }
}

pub const ASSIGN: &str = "assign";
pub const CHANGE_HOURS: &str = "change_hours";
pub const END: &str = "end";
pub const ACTIVE: &str = "active";
pub const ENDED: &str = "ended";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Command {
    pub action: String,
    pub section_id: String,
    pub subject: String,
    pub staff_member_id: String,
    pub periods: Option<u32>,
    pub expected_version: u32,
}
impl Command {
    pub fn valid(&self) -> bool {
        let periods = match self.action.as_str() {
            ASSIGN | CHANGE_HOURS => self.periods.is_some_and(|p| (1..=MAX_PERIODS).contains(&p)),
            END => self.periods.is_none(),
            _ => false,
        };
        periods
            && identifier(&self.section_id)
            && subject_code(&self.subject)
            && identifier(&self.staff_member_id)
            && self.expected_version < i32::MAX as u32
    }
}
/// Teaching-assignment commands of one school year, applied together or not at all.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Commands {
    pub tenant_id: String,
    pub school_id: String,
    pub academic_year: String,
    pub commands: Vec<Command>,
}
impl Commands {
    /// One to `MAX_COMMANDS` valid commands, never two on the same class, subject and teacher.
    pub fn valid(&self) -> bool {
        let targets: BTreeSet<(&str, &str, &str)> = self
            .commands
            .iter()
            .map(|c| {
                (
                    c.section_id.as_str(),
                    c.subject.as_str(),
                    c.staff_member_id.as_str(),
                )
            })
            .collect();
        scope(&self.tenant_id, &self.school_id, &self.academic_year)
            && (1..=MAX_COMMANDS).contains(&self.commands.len())
            && targets.len() == self.commands.len()
            && self.commands.iter().all(Command::valid)
    }
    pub fn key(&self, c: &Command) -> String {
        [
            self.tenant_id.as_str(),
            &self.school_id,
            &self.academic_year,
            &c.section_id,
            &c.subject,
            &c.staff_member_id,
        ]
        .join("|")
    }
}
/// The stored state of one assignment (class, subject and teacher).
#[derive(Clone, Debug, PartialEq)]
pub struct Current {
    pub status: String,
    pub periods: u32,
    pub version: u32,
}
/// Status and periods after the command, or `None` when the revision or state does not allow it.
/// `assign` creates (`expectedVersion` 0) or reopens an ended assignment at its version.
pub fn next(c: &Command, current: Option<&Current>) -> Option<(&'static str, u32)> {
    match (c.action.as_str(), current) {
        (ASSIGN, None) if c.expected_version == 0 => Some((ACTIVE, c.periods?)),
        (ASSIGN, Some(cur)) if cur.status == ENDED && cur.version == c.expected_version => {
            Some((ACTIVE, c.periods?))
        }
        (CHANGE_HOURS, Some(cur)) if cur.status == ACTIVE && cur.version == c.expected_version => {
            Some((ACTIVE, c.periods?))
        }
        (END, Some(cur)) if cur.status == ACTIVE && cur.version == c.expected_version => {
            Some((ENDED, cur.periods))
        }
        _ => None,
    }
}
/// `Idempotency-Key` header value; the receipt is per caller and school.
pub fn command_key(value: &str) -> bool {
    identifier(value) && value.len() >= 8
}
/// Natural order for grades: by the first number, then by text ("Year 7" before "Year 10").
pub fn grade_order(a: &str, b: &str) -> std::cmp::Ordering {
    let number = |g: &str| -> Option<u32> {
        g.chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok()
    };
    match (number(a), number(b)) {
        (Some(x), Some(y)) if x != y => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        _ => a.cmp(b),
    }
}
