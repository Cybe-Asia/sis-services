//! School nomenclature (`docs/ACADEMIC_NOMENCLATURE.md`, spec
//! `digital-school/docs/admin/academic-nomenclature-spec.md`): kinds, the rules a saved list keeps,
//! the default labels of spec §2 and the fixed grade mappings of §4. Pure.
use crate::school_portal::model::identifier;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Every kind, in the order Admin shows them.
pub const KINDS: [&str; 11] = [
    "unit",
    "grade",
    "phase",
    "stage",
    "academic_year",
    "term",
    "subject",
    "section_pattern",
    "assessment_kind",
    "mastery_level",
    "role_title",
];
/// Kinds whose codes come from the seed and the curriculum rules only: a save may relabel,
/// reorder and archive them, never add a code.
pub const CLOSED: [&str; 7] = [
    "unit",
    "grade",
    "phase",
    "stage",
    "section_pattern",
    "assessment_kind",
    "mastery_level",
];
/// Kinds that always keep one active row: a school is always one unit and names its classes one way.
pub const ONE_ACTIVE: [&str; 2] = ["unit", "section_pattern"];
pub const MAX_ROWS: usize = 100;
pub const MAX_ORDER: u32 = 999;
pub const LABEL_CHARS: usize = 80;
pub const SHORT_CHARS: usize = 24;
pub const CONTRACT_VERSION: u32 = 1;
pub const CACHE_SECONDS: u32 = 300;
pub const ACTIVE: &str = "active";
pub const ARCHIVED: &str = "archived";
/// The placeholders a section pattern may use; `{letter}` is required.
pub const PATTERN_PARTS: [&str; 3] = ["{grade}", "{grade_short}", "{letter}"];

pub fn kind(value: &str) -> bool {
    KINDS.contains(&value)
}
pub fn closed(kind: &str) -> bool {
    CLOSED.contains(&kind)
}

fn label(value: &str, max: usize) -> bool {
    let chars = value.chars().count();
    (1..=max).contains(&chars) && value.trim() == value && !value.chars().any(char::is_control)
}
/// `YYYY/YYYY` with consecutive years.
pub fn academic_year(value: &str) -> bool {
    let Some((a, b)) = value.split_once('/') else {
        return false;
    };
    let year = |s: &str| {
        (s.len() == 4 && s.bytes().all(|c| c.is_ascii_digit()))
            .then(|| s.parse::<u32>().ok())
            .flatten()
    };
    matches!((year(a), year(b)), (Some(a), Some(b)) if (1990..=2200).contains(&a) && b == a + 1)
}
/// The code rule of each kind. Closed kinds also have to exist already, which the repository checks.
pub fn code(kind: &str, value: &str) -> bool {
    let bytes = value.as_bytes();
    match kind {
        "academic_year" => academic_year(value),
        "subject" => crate::academic_staffing::model::subject_code(value),
        "role_title" => {
            (1..=64).contains(&bytes.len())
                && bytes[0].is_ascii_lowercase()
                && bytes
                    .iter()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
        }
        "term" => {
            (1..=8).contains(&bytes.len())
                && bytes[0].is_ascii_uppercase()
                && bytes
                    .iter()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        }
        _ => {
            (1..=32).contains(&bytes.len())
                && bytes[0].is_ascii_alphanumeric()
                && bytes
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
        }
    }
}
/// A section pattern names the letter and otherwise only known placeholders.
pub fn pattern(value: &str) -> bool {
    if !value.contains("{letter}") {
        return false;
    }
    let mut rest = value;
    while let Some(start) = rest.find('{') {
        let tail = &rest[start..];
        let Some(end) = tail.find('}') else {
            return false;
        };
        if !PATTERN_PARTS.contains(&&tail[..=end]) {
            return false;
        }
        rest = &tail[end + 1..];
    }
    !rest.contains('}')
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Row {
    pub code: String,
    pub label_id: String,
    pub label_en: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_en: Option<String>,
    pub order: u32,
    pub status: String,
}
impl Row {
    pub fn valid(&self, kind: &str) -> bool {
        let short = |s: &Option<String>| s.as_deref().is_none_or(|s| label(s, SHORT_CHARS));
        code(kind, &self.code)
            && label(&self.label_id, LABEL_CHARS)
            && label(&self.label_en, LABEL_CHARS)
            && short(&self.short_id)
            && short(&self.short_en)
            && (1..=MAX_ORDER).contains(&self.order)
            && [ACTIVE, ARCHIVED].contains(&self.status.as_str())
            && (kind != "section_pattern" || (pattern(&self.label_id) && pattern(&self.label_en)))
    }
    pub fn active(&self) -> bool {
        self.status == ACTIVE
    }
}

/// Valid rows, one per code and one per order position.
pub fn valid_rows(kind: &str, rows: &[Row]) -> bool {
    let codes: BTreeSet<&str> = rows.iter().map(|r| r.code.as_str()).collect();
    let orders: BTreeSet<u32> = rows.iter().map(|r| r.order).collect();
    !rows.is_empty()
        && rows.len() <= MAX_ROWS
        && codes.len() == rows.len()
        && orders.len() == rows.len()
        && rows.iter().all(|r| r.valid(kind))
        && (!ONE_ACTIVE.contains(&kind) || rows.iter().any(Row::active))
}

/// Replaces the whole list of one kind (`expectedVersion` is the version the caller read).
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub tenant_id: String,
    pub school_id: String,
    pub expected_version: u32,
    pub rows: Vec<Row>,
}
impl Change {
    pub fn valid(&self, kind: &str) -> bool {
        identifier(&self.tenant_id)
            && identifier(&self.school_id)
            && (1..i32::MAX as u32).contains(&self.expected_version)
            && valid_rows(kind, &self.rows)
    }
}

pub fn row_key(tenant: &str, school: &str, kind: &str, code: &str) -> String {
    [tenant, school, kind, code].join("|")
}
pub fn kind_key(tenant: &str, school: &str, kind: &str) -> String {
    [tenant, school, kind].join("|")
}

// ---------------------------------------------------------------------------------------------
// Grades and their fixed mappings (spec §4: curriculum rules, kept in code, changed by ADR)

/// `(code, number, roman)` for every grade the schools teach.
pub const GRADES: [(&str, u32, &str); 6] = [
    ("Y7", 7, "VII"),
    ("Y8", 8, "VIII"),
    ("Y9", 9, "IX"),
    ("Y10", 10, "X"),
    ("Y11", 11, "XI"),
    ("Y12", 12, "XII"),
];
pub fn unit_of(grade: u32) -> &'static str {
    if grade <= 9 {
        "IISS"
    } else {
        "IIHS"
    }
}
pub fn phase_of(grade: u32) -> &'static str {
    match grade {
        ..=9 => "D",
        10 => "E",
        _ => "F",
    }
}
pub fn stage_of(grade: u32) -> &'static str {
    match grade {
        7 => "CLS7",
        8 => "CLS8",
        9 => "CLS9",
        10 | 11 => "IGCSE",
        _ => "AL",
    }
}
/// The grade code of a free-text year group, for the forms SIS and admissions store today:
/// "Year 7", "Grade 7", "Kelas 7", "Kelas VII", "VII", "Y7", "7". `None` for anything else.
/// The normalisation Job of spec §5 slice 5 uses it; Admin carries the same rule.
#[allow(dead_code)]
pub fn grade_code(value: &str) -> Option<&'static str> {
    let text = value.trim().to_ascii_lowercase();
    let rest = ["year", "grade", "kelas", "y"]
        .iter()
        .find_map(|p| text.strip_prefix(p))
        .unwrap_or(&text)
        .trim();
    GRADES
        .iter()
        .find(|(_, n, roman)| rest == n.to_string() || rest == roman.to_ascii_lowercase())
        .map(|(code, _, _)| *code)
}
/// The unit code of a school id (`SCH-IISS` → `IISS`).
pub fn unit_code(school: &str) -> Option<String> {
    let code = school
        .strip_prefix("SCH-")
        .unwrap_or(school)
        .to_ascii_uppercase();
    (2..=16)
        .contains(&code.len())
        .then_some(code)
        .filter(|c| c.bytes().all(|b| b.is_ascii_alphanumeric()))
}
/// The grades a school teaches: by its unit when known, every grade otherwise.
pub fn grades_of(unit: Option<&str>) -> Vec<(&'static str, u32, &'static str)> {
    GRADES
        .iter()
        .copied()
        .filter(|(_, n, _)| match unit {
            Some("IISS") | Some("IIHS") => Some(unit_of(*n)) == unit,
            _ => true,
        })
        .collect()
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GradeMapping {
    pub code: &'static str,
    pub unit: &'static str,
    pub phase: &'static str,
    pub stage: &'static str,
}
pub fn mappings() -> Vec<GradeMapping> {
    GRADES
        .iter()
        .map(|(code, n, _)| GradeMapping {
            code,
            unit: unit_of(*n),
            phase: phase_of(*n),
            stage: stage_of(*n),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Defaults (spec §2), seeded per school on first read

/// What the seed needs to know about one school.
pub struct SchoolFacts {
    pub school_id: String,
    pub school_name: String,
    /// Academic years of the school's classes, in any form; only `YYYY/YYYY` ones are seeded.
    pub years: Vec<String>,
    /// `(code, name)` of the subjects in the school's plans, latest plan first.
    pub subjects: Vec<(String, String)>,
    /// Today in WIB, for the current academic year.
    pub today: chrono::NaiveDate,
}

fn row(code: &str, id: &str, en: &str, short: Option<(&str, &str)>, order: u32) -> Row {
    Row {
        code: code.into(),
        label_id: id.into(),
        label_en: en.into(),
        short_id: short.map(|s| s.0.into()),
        short_en: short.map(|s| s.1.into()),
        order,
        status: ACTIVE.into(),
    }
}
fn numbered<'a>(
    items: impl IntoIterator<Item = (&'a str, &'a str, &'a str, Option<(&'a str, &'a str)>)>,
) -> Vec<Row> {
    items
        .into_iter()
        .zip(1..)
        .map(|((code, id, en, short), order)| row(code, id, en, short, order))
        .collect()
}

/// Bilingual names of the LMS subject codes, for subjects whose plan gives one name only.
pub const SUBJECT_NAMES: [(&str, &str, &str); 15] = [
    ("mathematics", "Matematika", "Mathematics"),
    ("science", "IPA", "Science"),
    ("physics", "Fisika", "Physics"),
    ("chemistry", "Kimia", "Chemistry"),
    ("biology", "Biologi", "Biology"),
    ("computing", "Informatika", "Computing"),
    ("english", "Bahasa Inggris", "English"),
    ("indonesian", "Bahasa Indonesia", "Indonesian"),
    ("humanities", "IPS", "Humanities"),
    ("geography", "Geografi", "Geography"),
    ("economics", "Ekonomi", "Economics"),
    ("sociology", "Sosiologi", "Sociology"),
    ("history", "Sejarah", "History"),
    ("civics", "Pendidikan Pancasila", "Civics"),
    ("religion", "Pendidikan Agama Islam", "Islamic Education"),
];

/// The academic year a date falls in: July starts the year.
pub fn current_year(today: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    let start = if today.month() >= 7 {
        today.year()
    } else {
        today.year() - 1
    };
    format!("{}/{}", start, start + 1)
}

fn clip(value: &str, max: usize) -> String {
    value
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// The default rows of one kind for one school.
pub fn defaults(kind: &str, school: &SchoolFacts) -> Vec<Row> {
    let unit = unit_code(&school.school_id);
    let grades = grades_of(unit.as_deref());
    match kind {
        "unit" => {
            let (code, id, en, short) = match unit.as_deref() {
                Some("IISS") => (
                    "IISS".to_string(),
                    "SMP · International Islamic Secondary School".to_string(),
                    "International Islamic Secondary School".to_string(),
                    "IISS".to_string(),
                ),
                Some("IIHS") => (
                    "IIHS".to_string(),
                    "SMA · International Islamic High School".to_string(),
                    "International Islamic High School".to_string(),
                    "IIHS".to_string(),
                ),
                other => {
                    let code = other.map(str::to_string).unwrap_or_else(|| "SCHOOL".into());
                    let name = Some(clip(&school.school_name, LABEL_CHARS))
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| code.clone());
                    (code.clone(), name.clone(), name, clip(&code, SHORT_CHARS))
                }
            };
            vec![Row {
                code,
                label_id: id,
                label_en: en,
                short_id: Some(short.clone()),
                short_en: Some(short),
                order: 1,
                status: ACTIVE.into(),
            }]
        }
        "grade" => grades
            .iter()
            .zip(1..)
            .map(|((code, n, roman), order)| Row {
                code: (*code).into(),
                label_id: format!("Kelas {roman}"),
                label_en: format!("Year {n}"),
                short_id: Some((*roman).into()),
                short_en: Some(format!("Y{n}")),
                order,
                status: ACTIVE.into(),
            })
            .collect(),
        "phase" => {
            let used: BTreeSet<&str> = grades.iter().map(|g| phase_of(g.1)).collect();
            let all = [
                (
                    "D",
                    "Fase D (Kelas VII–IX)",
                    "Phase D (Years 7–9)",
                    Some(("Fase D", "Phase D")),
                ),
                (
                    "E",
                    "Fase E (Kelas X)",
                    "Phase E (Year 10)",
                    Some(("Fase E", "Phase E")),
                ),
                (
                    "F",
                    "Fase F (Kelas XI–XII)",
                    "Phase F (Years 11–12)",
                    Some(("Fase F", "Phase F")),
                ),
            ];
            numbered(all.into_iter().filter(|p| used.contains(p.0)))
        }
        "stage" => {
            let used: BTreeSet<&str> = grades.iter().map(|g| stage_of(g.1)).collect();
            let all = [
                (
                    "CLS7",
                    "Cambridge Lower Secondary Stage 7",
                    "Cambridge Lower Secondary Stage 7",
                    Some(("Stage 7", "Stage 7")),
                ),
                (
                    "CLS8",
                    "Cambridge Lower Secondary Stage 8",
                    "Cambridge Lower Secondary Stage 8",
                    Some(("Stage 8", "Stage 8")),
                ),
                (
                    "CLS9",
                    "Cambridge Lower Secondary Stage 9",
                    "Cambridge Lower Secondary Stage 9",
                    Some(("Stage 9", "Stage 9")),
                ),
                (
                    "IGCSE",
                    "Cambridge IGCSE (Kelas X–XI)",
                    "Cambridge IGCSE (Years 10–11)",
                    Some(("IGCSE", "IGCSE")),
                ),
                (
                    "AL",
                    "Cambridge AS & A Level (Kelas XII)",
                    "Cambridge AS & A Level (Year 12)",
                    Some(("A Level", "A Level")),
                ),
            ];
            numbered(all.into_iter().filter(|s| used.contains(s.0)))
        }
        "academic_year" => {
            let mut years: BTreeSet<String> = school
                .years
                .iter()
                .map(|y| y.trim().to_string())
                .filter(|y| academic_year(y))
                .collect();
            years.insert(current_year(school.today));
            years
                .into_iter()
                .rev()
                .take(MAX_ROWS)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .zip(1..)
                .map(|(y, order)| Row {
                    label_id: format!("Tahun Ajaran {y}"),
                    label_en: format!("Academic Year {y}"),
                    short_id: Some(y.clone()),
                    short_en: Some(y.clone()),
                    code: y,
                    order,
                    status: ACTIVE.into(),
                })
                .collect()
        }
        "term" => numbered([
            (
                "S1",
                "Semester 1 (Ganjil)",
                "Semester 1 (Odd)",
                Some(("Sem 1", "Sem 1")),
            ),
            (
                "S2",
                "Semester 2 (Genap)",
                "Semester 2 (Even)",
                Some(("Sem 2", "Sem 2")),
            ),
        ]),
        "subject" => {
            let mut seen = BTreeSet::new();
            let mut out = vec![];
            for (code, name) in &school.subjects {
                if out.len() == MAX_ROWS
                    || !crate::academic_staffing::model::subject_code(code)
                    || !seen.insert(code.clone())
                {
                    continue;
                }
                let known = SUBJECT_NAMES.iter().find(|s| s.0 == code);
                let name = clip(name, LABEL_CHARS);
                let (id, en) = match (known, name.is_empty()) {
                    // The plan's own name is what the school typed; English falls back to the LMS name.
                    (Some(k), false) => (
                        name.clone(),
                        if name == k.1 {
                            k.2.to_string()
                        } else {
                            name.clone()
                        },
                    ),
                    (Some(k), true) => (k.1.to_string(), k.2.to_string()),
                    (None, false) => (name.clone(), name.clone()),
                    (None, true) => (code.clone(), code.clone()),
                };
                out.push(Row {
                    code: code.clone(),
                    label_id: id,
                    label_en: en,
                    short_id: None,
                    short_en: None,
                    order: out.len() as u32 + 1,
                    status: ACTIVE.into(),
                });
            }
            out
        }
        "section_pattern" => vec![row(
            "default",
            "Kelas {grade_short}-{letter}",
            "{grade}{letter}",
            None,
            1,
        )],
        "assessment_kind" => numbered([
            ("quiz", "Kuis", "Quiz", None),
            (
                "pts",
                "Penilaian Tengah Semester",
                "Mid-term assessment",
                Some(("PTS", "Mid-term")),
            ),
            (
                "pas",
                "Penilaian Akhir Semester",
                "End-of-term assessment",
                Some(("PAS", "End-of-term")),
            ),
            ("assignment", "Tugas", "Assignment", None),
            ("checkpoint", "Checkpoint", "Checkpoint", None),
            ("practice", "Latihan", "Practice", None),
        ]),
        "mastery_level" => numbered([
            (
                "insufficient",
                "Belum cukup bukti",
                "Not enough evidence",
                None,
            ),
            ("beginning", "Mulai berkembang", "Beginning", None),
            ("developing", "Berkembang", "Developing", None),
            ("proficient", "Cakap", "Proficient", None),
            ("advanced", "Mahir", "Advanced", None),
        ]),
        "role_title" => numbered([
            ("principal", "Kepala Sekolah", "Principal", None),
            (
                "curriculum_coordinator",
                "Wakasek Kurikulum",
                "Curriculum Coordinator",
                Some(("Wakasek", "Coordinator")),
            ),
            ("subject_lead", "Koordinator Mapel", "Subject Lead", None),
            ("homeroom", "Wali Kelas", "Homeroom Teacher", None),
            ("school_admin", "Admin Sekolah", "School Admin", None),
            ("admin_teacher", "Guru Admin", "Admin Teacher", None),
            ("regular_teacher", "Guru", "Teacher", None),
        ]),
        _ => vec![],
    }
}
