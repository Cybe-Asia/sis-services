//! Grade weights (gradebook spec slice 2, `docs/GRADE_WEIGHTS.md`): the weight of each grade
//! category and the pass mark (KKM), as a school default per academic year and as an override
//! per grade, subject and term. Request shapes, validation, keys and resolution. Pure.
use super::model::{key_part, subject_code};
use crate::school_portal::model::identifier;
use serde::{Deserialize, Serialize};

/// The two semesters of an academic year; the term of a weight set.
pub const TERMS: [&str; 2] = ["semester_1", "semester_2"];
/// The school default before an administrator saves one (spec §7 #1 and #4).
pub const BUILTIN: Weights = Weights {
    chapter_quiz: 20,
    quiz: 20,
    assignment: 20,
    pts: 15,
    pas: 25,
};
pub const BUILTIN_PASS_MARK: u32 = 75;
pub const ACTIVE: &str = "active";
pub const CLEARED: &str = "cleared";

/// Percent per category: Kuis bab, Kuis (pop quizzes included), Tugas, PTS and PAS.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Weights {
    pub chapter_quiz: u32,
    pub quiz: u32,
    pub assignment: u32,
    pub pts: u32,
    pub pas: u32,
}
impl Weights {
    /// Whole percents that add up to 100.
    pub fn valid(&self) -> bool {
        let all = [
            self.chapter_quiz,
            self.quiz,
            self.assignment,
            self.pts,
            self.pas,
        ];
        all.iter().all(|w| *w <= 100) && all.iter().sum::<u32>() == 100
    }
}
/// What a record stores: the weights and the pass mark.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeightSet {
    pub weights: Weights,
    pub pass_mark: u32,
}
impl WeightSet {
    pub fn valid(&self) -> bool {
        self.weights.valid() && self.pass_mark <= 100
    }
}
pub const BUILTIN_SET: WeightSet = WeightSet {
    weights: BUILTIN,
    pass_mark: BUILTIN_PASS_MARK,
};

/// The school default of the year, or one grade, subject and term of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target<'a> {
    School,
    Subject {
        grade: &'a str,
        subject: &'a str,
        term: &'a str,
    },
}
pub fn key(tenant: &str, school: &str, year: &str, target: &Target) -> String {
    match target {
        Target::School => [tenant, school, year, "school"].join("|"),
        Target::Subject {
            grade,
            subject,
            term,
        } => [tenant, school, year, "subject", grade, subject, term].join("|"),
    }
}

/// Saves (or, with `clear`, removes) one weight set. `expectedVersion` 0 creates it; a cleared
/// record keeps its version, so saving it again names that version.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Change {
    pub tenant_id: String,
    pub school_id: String,
    pub academic_year: String,
    #[serde(default)]
    pub grade: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub term: Option<String>,
    pub expected_version: u32,
    #[serde(default)]
    pub set: Option<WeightSet>,
    #[serde(default)]
    pub clear: bool,
}
impl Change {
    /// Grade, subject and term together, or none of them for the school default.
    pub fn target(&self) -> Option<Target<'_>> {
        match (&self.grade, &self.subject, &self.term) {
            (None, None, None) => Some(Target::School),
            (Some(grade), Some(subject), Some(term))
                if key_part(grade, 128)
                    && subject_code(subject)
                    && TERMS.contains(&term.as_str()) =>
            {
                Some(Target::Subject {
                    grade,
                    subject,
                    term,
                })
            }
            _ => None,
        }
    }
    pub fn valid(&self) -> bool {
        identifier(&self.tenant_id)
            && identifier(&self.school_id)
            && key_part(&self.academic_year, 32)
            && self.target().is_some()
            && self.expected_version < i32::MAX as u32
            && match (&self.set, self.clear) {
                (Some(set), false) => set.valid(),
                (None, true) => self.expected_version > 0,
                _ => false,
            }
    }
}

/// Where the weights of one class, subject and term come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Subject,
    School,
    Default,
}
impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subject => "subject",
            Self::School => "school",
            Self::Default => "default",
        }
    }
}
/// The subject's active override, else the school default of the year, else the built-in one.
pub fn resolve(
    subject: Option<(WeightSet, u32)>,
    school: Option<(WeightSet, u32)>,
) -> (Source, WeightSet, u32) {
    match (subject, school) {
        (Some((set, version)), _) => (Source::Subject, set, version),
        (None, Some((set, version))) => (Source::School, set, version),
        (None, None) => (Source::Default, BUILTIN_SET, 0),
    }
}

/// `term` of a learning read: one of `TERMS`.
pub fn term(value: &str) -> bool {
    TERMS.contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn change(body: &str) -> Change {
        serde_json::from_str(body).unwrap()
    }
    const BASE: &str =
        r#""tenantId":"TENANT-001","schoolId":"SCH-IISS","academicYear":"2026/2027""#;
    const SET: &str = r#""set":{"weights":{"chapterQuiz":20,"quiz":20,"assignment":20,"pts":15,"pas":25},"passMark":75}"#;

    #[test]
    fn the_built_in_default_is_the_approved_one_and_adds_up() {
        assert!(BUILTIN.valid() && BUILTIN_SET.valid());
        assert_eq!(
            (BUILTIN.chapter_quiz, BUILTIN.quiz, BUILTIN.assignment),
            (20, 20, 20)
        );
        assert_eq!((BUILTIN.pts, BUILTIN.pas, BUILTIN_PASS_MARK), (15, 25, 75));
        let off = Weights { pas: 24, ..BUILTIN };
        assert!(!off.valid());
        let all_on_pas = Weights {
            chapter_quiz: 0,
            quiz: 0,
            assignment: 0,
            pts: 0,
            pas: 100,
        };
        assert!(all_on_pas.valid());
        assert!(!WeightSet {
            weights: BUILTIN,
            pass_mark: 101
        }
        .valid());
    }
    #[test]
    fn a_change_names_the_school_or_a_whole_grade_subject_and_term() {
        let school = change(&format!(r#"{{{BASE},"expectedVersion":0,{SET}}}"#));
        assert!(school.valid());
        assert_eq!(school.target(), Some(Target::School));
        let subject = change(&format!(
            r#"{{{BASE},"grade":"Year 7","subject":"mathematics","term":"semester_1","expectedVersion":0,{SET}}}"#
        ));
        assert!(subject.valid());
        assert_eq!(
            key("T", "S", "2026/2027", &subject.target().unwrap()),
            "T|S|2026/2027|subject|Year 7|mathematics|semester_1"
        );
        assert_eq!(
            key("T", "S", "2026/2027", &Target::School),
            "T|S|2026/2027|school"
        );
        for partial in [
            r#""grade":"Year 7","subject":"mathematics""#,
            r#""grade":"Year 7","subject":"mathematics","term":"semester_3""#,
            r#""grade":"Year|7","subject":"mathematics","term":"semester_1""#,
            r#""grade":"Year 7","subject":"Maths","term":"semester_1""#,
        ] {
            let c = change(&format!(
                r#"{{{BASE},{partial},"expectedVersion":0,{SET}}}"#
            ));
            assert!(!c.valid(), "{partial}");
        }
    }
    #[test]
    fn clearing_needs_an_existing_record_and_no_set() {
        let target = r#""grade":"Year 7","subject":"mathematics","term":"semester_2""#;
        assert!(change(&format!(
            r#"{{{BASE},{target},"expectedVersion":2,"clear":true}}"#
        ))
        .valid());
        assert!(!change(&format!(
            r#"{{{BASE},{target},"expectedVersion":0,"clear":true}}"#
        ))
        .valid());
        assert!(!change(&format!(
            r#"{{{BASE},{target},"expectedVersion":2,"clear":true,{SET}}}"#
        ))
        .valid());
        assert!(!change(&format!(r#"{{{BASE},{target},"expectedVersion":2}}"#)).valid());
        assert!(serde_json::from_str::<Change>(&format!(
            r#"{{{BASE},"expectedVersion":0,{SET},"extra":1}}"#
        ))
        .is_err());
    }
    #[test]
    fn resolution_prefers_the_subject_then_the_school_then_the_built_in_default() {
        let school = WeightSet {
            weights: Weights {
                chapter_quiz: 10,
                quiz: 30,
                assignment: 20,
                pts: 15,
                pas: 25,
            },
            pass_mark: 70,
        };
        let subject = WeightSet {
            pass_mark: 80,
            ..school
        };
        assert_eq!(
            resolve(Some((subject, 3)), Some((school, 1))),
            (Source::Subject, subject, 3)
        );
        assert_eq!(
            resolve(None, Some((school, 1))),
            (Source::School, school, 1)
        );
        assert_eq!(resolve(None, None), (Source::Default, BUILTIN_SET, 0));
        assert!(term("semester_2") && !term("Semester 2"));
    }
}
