//! Teacher load and warnings computed from what SIS stores (`docs/TEACHER_LIST.md`, and the
//! planned hours of `docs/TEACHING_ASSIGNMENTS.md`). Pure: the repository hands over one school
//! year and one school week.
use chrono::{Datelike, Duration, NaiveDate, Timelike};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Default teaching load in lesson periods per week until the school decides its own limits.
pub const LOWER_PERIODS: f64 = 18.0;
pub const UPPER_PERIODS: f64 = 24.0;
/// One lesson period: 40 minutes in Years 7–9 (SMP), 45 minutes from Year 10 (SMA).
pub const JUNIOR_PERIOD_MINUTES: u64 = 40;
pub const SENIOR_PERIOD_MINUTES: u64 = 45;
pub const MAX_TEACHERS: usize = 300;
/// Scheduled and planned periods closer than this are the same load (lessons rarely fill a
/// whole number of periods exactly).
pub const PERIOD_TOLERANCE: f64 = 0.25;
const WIB_MS: i64 = 25_200_000;

pub struct Section {
    pub id: String,
    pub name: String,
    pub year_group: String,
    pub homeroom: Option<String>,
}
pub struct Staff {
    pub id: String,
    pub name: String,
    /// An active teacher of this school and tenant, held by exactly one StaffMember node.
    pub active: bool,
}
pub struct Lesson {
    pub id: String,
    pub section_id: String,
    pub teacher_id: String,
    pub subject: String,
    pub title: String,
    pub starts_at: u64,
    pub ends_at: u64,
    pub room_id: String,
}
pub struct Activity {
    pub id: String,
    pub title: Value,
    pub coach_id: String,
}
pub struct Grant {
    pub teacher_id: String,
    pub section_id: String,
}
/// One active row of a grade's subject plan for the year.
pub struct PlannedSubject {
    pub grade: String,
    pub subject: String,
    pub periods: u32,
    pub required: bool,
}
/// One active teaching assignment of the year.
pub struct Assignment {
    pub section_id: String,
    pub subject: String,
    pub teacher_id: String,
    pub periods: u32,
}
#[derive(Default)]
pub struct Snapshot {
    pub sections: Vec<Section>,
    pub staff: Vec<Staff>,
    pub lessons: Vec<Lesson>,
    pub activities: Vec<Activity>,
    pub grants: Vec<Grant>,
    pub plans: Vec<PlannedSubject>,
    pub assignments: Vec<Assignment>,
}

/// The school week (Monday to Friday) that contains `anchor`; a Saturday or Sunday looks ahead.
pub fn school_week(anchor: NaiveDate) -> (NaiveDate, NaiveDate) {
    let weekday = anchor.weekday().num_days_from_monday() as i64;
    let monday = if weekday >= 5 {
        anchor + Duration::days(7 - weekday)
    } else {
        anchor - Duration::days(weekday)
    };
    (monday, monday + Duration::days(4))
}
/// Half-open epoch window in milliseconds from Monday 00:00 to Saturday 00:00 school time (WIB).
pub fn window_ms(monday: NaiveDate) -> (i64, i64) {
    let start = monday
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis()
        - WIB_MS;
    (start, start + 5 * 86_400_000)
}
pub fn today_wib(now_ms: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp_millis(now_ms + WIB_MS)
        .map(|t| t.date_naive())
        .unwrap_or_default()
}
/// Year 10 and above use the senior period; anything without a number uses the junior one.
pub fn period_minutes(year_group: &str) -> u64 {
    let digits: String = year_group
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    match digits.parse::<u32>() {
        Ok(year) if year >= 10 => SENIOR_PERIOD_MINUTES,
        _ => JUNIOR_PERIOD_MINUTES,
    }
}
fn periods(minutes: f64) -> f64 {
    (minutes * 10.0).round() / 10.0
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClassRef {
    pub section_id: String,
    pub name: String,
    pub year_group: String,
}
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaughtClass {
    #[serde(flatten)]
    pub class: ClassRef,
    pub subjects: Vec<String>,
    pub lessons: usize,
    pub minutes: u64,
    pub periods: f64,
}
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssignedClass {
    #[serde(flatten)]
    pub class: ClassRef,
    pub subject: String,
    pub periods: u32,
    /// This week's lessons of this teacher in this class and subject, in periods.
    pub scheduled_periods: f64,
}
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Coached {
    pub activity_id: String,
    pub title: Value,
}
#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledLesson {
    pub id: String,
    pub section_id: String,
    pub class_name: String,
    pub subject: String,
    pub title: String,
    /// ISO weekday, Monday = 1.
    pub weekday: u32,
    /// School time (WIB) "HH:MM".
    pub start: String,
    pub end: String,
    pub room_id: String,
    pub clash: bool,
}
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Teacher {
    pub staff_member_id: String,
    pub name: String,
    pub active: bool,
    pub subjects: Vec<String>,
    pub classes: Vec<TaughtClass>,
    pub homeroom_of: Vec<ClassRef>,
    pub coach_of: Vec<Coached>,
    pub learning_access: Vec<ClassRef>,
    pub assignments: Vec<AssignedClass>,
    pub lessons: usize,
    pub scheduled_minutes: u64,
    pub scheduled_periods: f64,
    pub planned_periods: u32,
    pub warnings: Vec<Warning>,
    #[serde(skip)]
    pub schedule: Vec<ScheduledLesson>,
    /// The warnings above, one per class and subject they concern.
    #[serde(skip)]
    pub issues: Vec<Issue>,
}
/// One warning of spec §5 with what it is about, for the staffing summary.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub code: &'static str,
    pub level: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staff_member_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub teacher_name: Option<String>,
    #[serde(flatten)]
    pub class: Option<ClassRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planned_periods: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_periods: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_periods: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}
impl Issue {
    fn new(code: &'static str) -> Self {
        Issue {
            code,
            level: level(code),
            ..Default::default()
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassWarning {
    pub code: &'static str,
    #[serde(flatten)]
    pub class: ClassRef,
}
/// A class and subject of the plan: no teacher yet, or the teachers' periods do not add up.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubjectWarning {
    pub code: &'static str,
    #[serde(flatten)]
    pub class: ClassRef,
    pub subject: String,
    pub planned_periods: u32,
    pub assigned_periods: u32,
}
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub teachers: usize,
    pub with_warnings: usize,
    pub no_homeroom: usize,
    pub clashes: usize,
    pub overloaded: usize,
    pub underloaded: usize,
    pub inactive: usize,
    pub subject_unassigned: usize,
    pub plan_hours_mismatch: usize,
    pub hours_mismatch: usize,
    pub schedule_without_assignment: usize,
    pub grant_without_assignment: usize,
}
/// How much a warning matters, spec §5: `critical` (Penting), `attention` (Perhatian), `info`.
pub fn level(code: &str) -> &'static str {
    match code {
        "NO_HOMEROOM" | "SUBJECT_UNASSIGNED" | "TEACHER_CLASH" | "INACTIVE_TEACHER" => "critical",
        "GRANT_WITHOUT_ASSIGNMENT" => "info",
        _ => "attention",
    }
}
pub struct Report {
    pub teachers: Vec<Teacher>,
    pub class_warnings: Vec<ClassWarning>,
    pub subject_warnings: Vec<SubjectWarning>,
    pub summary: Summary,
    /// Lessons of the week in this school's active classes, for any teacher.
    pub lessons: usize,
}

fn clock(ms: u64) -> (u32, String) {
    let t = chrono::DateTime::from_timestamp_millis(ms as i64 + WIB_MS).unwrap_or_default();
    (
        t.weekday().number_from_monday(),
        format!("{:02}:{:02}", t.hour(), t.minute()),
    )
}

/// `None` when more than `MAX_TEACHERS` people would be listed.
pub fn report(s: &Snapshot) -> Option<Report> {
    let sections: BTreeMap<&str, &Section> =
        s.sections.iter().map(|x| (x.id.as_str(), x)).collect();
    let class_ref = |id: &str| {
        sections.get(id).map(|x| ClassRef {
            section_id: x.id.clone(),
            name: x.name.clone(),
            year_group: x.year_group.clone(),
        })
    };
    let lessons: Vec<&Lesson> = s
        .lessons
        .iter()
        .filter(|l| sections.contains_key(l.section_id.as_str()) && l.starts_at < l.ends_at)
        .collect();
    let staff: BTreeMap<&str, &Staff> = s.staff.iter().map(|x| (x.id.as_str(), x)).collect();

    // Everyone listed: active teachers, plus anyone (active or not) who holds something here.
    let mut ids: BTreeSet<&str> = s
        .staff
        .iter()
        .filter(|x| x.active)
        .map(|x| x.id.as_str())
        .collect();
    ids.extend(lessons.iter().map(|l| l.teacher_id.as_str()));
    ids.extend(
        s.sections
            .iter()
            .filter_map(|x| x.homeroom.as_deref())
            .filter(|h| staff.contains_key(h)),
    );
    ids.extend(s.activities.iter().map(|a| a.coach_id.as_str()));
    ids.extend(
        s.grants
            .iter()
            .filter(|g| sections.contains_key(g.section_id.as_str()))
            .map(|g| g.teacher_id.as_str()),
    );
    let assignments: Vec<&Assignment> = s
        .assignments
        .iter()
        .filter(|a| sections.contains_key(a.section_id.as_str()))
        .collect();
    ids.extend(assignments.iter().map(|a| a.teacher_id.as_str()));
    // Classes whose grade has a subject plan this year have started on assignments; only there
    // do lessons and grants without an assignment mean something.
    let planned_grades: BTreeSet<&str> = s.plans.iter().map(|p| p.grade.as_str()).collect();
    let planned = |section: &str| {
        sections
            .get(section)
            .is_some_and(|x| planned_grades.contains(x.year_group.as_str()))
    };
    if ids.len() > MAX_TEACHERS {
        return None;
    }
    let mut teachers: Vec<Teacher> = ids
        .into_iter()
        .map(|id| {
            let mine: Vec<&&Lesson> = lessons.iter().filter(|l| l.teacher_id == id).collect();
            let mut by_class: BTreeMap<&str, (BTreeSet<String>, usize, u64, f64)> = BTreeMap::new();
            let mut subjects = BTreeSet::new();
            let mut total_periods = 0.0;
            let mut by_subject: BTreeMap<(&str, &str), f64> = BTreeMap::new();
            for l in &mine {
                let minutes = (l.ends_at - l.starts_at) / 60_000;
                let length = period_minutes(&sections[l.section_id.as_str()].year_group) as f64;
                let entry = by_class.entry(l.section_id.as_str()).or_default();
                entry.0.insert(l.subject.clone());
                entry.1 += 1;
                entry.2 += minutes;
                entry.3 += minutes as f64 / length;
                total_periods += minutes as f64 / length;
                *by_subject
                    .entry((l.section_id.as_str(), l.subject.as_str()))
                    .or_default() += minutes as f64 / length;
                subjects.insert(l.subject.clone());
            }
            // Two lessons of this teacher in different classes that overlap in time.
            let mut clashing = BTreeSet::new();
            let mut pairs = 0;
            for (i, a) in mine.iter().enumerate() {
                for b in &mine[i + 1..] {
                    if a.section_id != b.section_id
                        && a.starts_at < b.ends_at
                        && b.starts_at < a.ends_at
                    {
                        pairs += 1;
                        clashing.insert(a.id.as_str());
                        clashing.insert(b.id.as_str());
                    }
                }
            }
            let mut schedule: Vec<ScheduledLesson> = mine
                .iter()
                .map(|l| {
                    let (weekday, start) = clock(l.starts_at);
                    ScheduledLesson {
                        id: l.id.clone(),
                        section_id: l.section_id.clone(),
                        class_name: sections[l.section_id.as_str()].name.clone(),
                        subject: l.subject.clone(),
                        title: l.title.clone(),
                        weekday,
                        start,
                        end: clock(l.ends_at).1,
                        room_id: l.room_id.clone(),
                        clash: clashing.contains(l.id.as_str()),
                    }
                })
                .collect();
            schedule.sort_by(|a, b| {
                (a.weekday, &a.start, &a.class_name).cmp(&(b.weekday, &b.start, &b.class_name))
            });
            let person = staff.get(id);
            let active = person.is_some_and(|p| p.active);
            let scheduled_minutes = by_class.values().map(|c| c.2).sum();
            let scheduled_periods = periods(total_periods);
            let held: Vec<&&Assignment> =
                assignments.iter().filter(|a| a.teacher_id == id).collect();
            let mut assigned: Vec<AssignedClass> = held
                .iter()
                .filter_map(|a| {
                    Some(AssignedClass {
                        class: class_ref(&a.section_id)?,
                        subject: a.subject.clone(),
                        periods: a.periods,
                        scheduled_periods: periods(
                            by_subject
                                .get(&(a.section_id.as_str(), a.subject.as_str()))
                                .copied()
                                .unwrap_or_default(),
                        ),
                    })
                })
                .collect();
            assigned.sort_by(|a, b| (&a.class.name, &a.subject).cmp(&(&b.class.name, &b.subject)));
            subjects.extend(held.iter().map(|a| a.subject.clone()));
            let mut warnings = vec![];
            if !active {
                warnings.push(Warning {
                    code: "INACTIVE_TEACHER",
                    count: None,
                });
            }
            if pairs > 0 {
                warnings.push(Warning {
                    code: "TEACHER_CLASH",
                    count: Some(pairs),
                });
            }
            // A week without any lesson in the school says nothing about anyone's load.
            if active && !lessons.is_empty() {
                if scheduled_periods > UPPER_PERIODS {
                    warnings.push(Warning {
                        code: "OVERLOAD",
                        count: None,
                    });
                } else if scheduled_periods < LOWER_PERIODS {
                    warnings.push(Warning {
                        code: "UNDERLOAD",
                        count: None,
                    });
                }
            }
            // A week without lessons says nothing about whether the timetable matches the plan.
            let name = person
                .map(|p| p.name.clone())
                .unwrap_or_else(|| id.to_string());
            let about = |code| Issue {
                staff_member_id: Some(id.to_string()),
                teacher_name: Some(name.clone()),
                ..Issue::new(code)
            };
            let mut issues = vec![];
            for w in &warnings {
                issues.push(Issue {
                    count: w.count,
                    scheduled_periods: matches!(w.code, "OVERLOAD" | "UNDERLOAD")
                        .then_some(scheduled_periods),
                    ..about(w.code)
                });
            }
            let mismatched: Vec<&AssignedClass> = if lessons.is_empty() {
                vec![]
            } else {
                assigned
                    .iter()
                    .filter(|a| {
                        (a.scheduled_periods - f64::from(a.periods)).abs() > PERIOD_TOLERANCE
                    })
                    .collect()
            };
            for a in &mismatched {
                issues.push(Issue {
                    class: Some(a.class.clone()),
                    subject: Some(a.subject.clone()),
                    planned_periods: Some(a.periods),
                    scheduled_periods: Some(a.scheduled_periods),
                    ..about("HOURS_MISMATCH")
                });
            }
            let mismatched = mismatched.len();
            if mismatched > 0 {
                warnings.push(Warning {
                    code: "HOURS_MISMATCH",
                    count: Some(mismatched),
                });
            }
            let unassigned: Vec<(&(&str, &str), &f64)> = by_subject
                .iter()
                .filter(|((section, subject), _)| {
                    planned(section)
                        && !held
                            .iter()
                            .any(|a| a.section_id == *section && a.subject == *subject)
                })
                .collect();
            for ((section, subject), p) in &unassigned {
                issues.push(Issue {
                    class: class_ref(section),
                    subject: Some(subject.to_string()),
                    scheduled_periods: Some(periods(**p)),
                    ..about("SCHEDULE_WITHOUT_ASSIGNMENT")
                });
            }
            let unassigned_lessons = unassigned.len();
            if unassigned_lessons > 0 {
                warnings.push(Warning {
                    code: "SCHEDULE_WITHOUT_ASSIGNMENT",
                    count: Some(unassigned_lessons),
                });
            }
            let ungoverned: Vec<&str> = s
                .grants
                .iter()
                .filter(|g| g.teacher_id == id && planned(&g.section_id))
                .map(|g| g.section_id.as_str())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter(|section| !held.iter().any(|a| a.section_id == *section))
                .collect();
            for section in &ungoverned {
                issues.push(Issue {
                    class: class_ref(section),
                    ..about("GRANT_WITHOUT_ASSIGNMENT")
                });
            }
            let ungoverned_grants = ungoverned.len();
            if ungoverned_grants > 0 {
                warnings.push(Warning {
                    code: "GRANT_WITHOUT_ASSIGNMENT",
                    count: Some(ungoverned_grants),
                });
            }
            let mut classes: Vec<TaughtClass> = by_class
                .into_iter()
                .filter_map(|(section, (subjects, count, minutes, p))| {
                    Some(TaughtClass {
                        class: class_ref(section)?,
                        subjects: subjects.into_iter().collect(),
                        lessons: count,
                        minutes,
                        periods: periods(p),
                    })
                })
                .collect();
            classes.sort_by(|a, b| a.class.name.cmp(&b.class.name));
            let mut homeroom_of: Vec<ClassRef> = s
                .sections
                .iter()
                .filter(|x| x.homeroom.as_deref() == Some(id))
                .filter_map(|x| class_ref(&x.id))
                .collect();
            homeroom_of.sort_by(|a, b| a.name.cmp(&b.name));
            let mut learning_access: Vec<ClassRef> = s
                .grants
                .iter()
                .filter(|g| g.teacher_id == id)
                .map(|g| g.section_id.as_str())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter_map(class_ref)
                .collect();
            learning_access.sort_by(|a, b| a.name.cmp(&b.name));
            Teacher {
                staff_member_id: id.to_string(),
                name,
                active,
                subjects: subjects.into_iter().collect(),
                classes,
                homeroom_of,
                coach_of: s
                    .activities
                    .iter()
                    .filter(|a| a.coach_id == id)
                    .map(|a| Coached {
                        activity_id: a.id.clone(),
                        title: a.title.clone(),
                    })
                    .collect(),
                learning_access,
                planned_periods: assigned.iter().map(|a| a.periods).sum(),
                assignments: assigned,
                lessons: mine.len(),
                scheduled_minutes,
                scheduled_periods,
                warnings,
                schedule,
                issues,
            }
        })
        .collect();
    teachers.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.staff_member_id.cmp(&b.staff_member_id))
    });
    // A homeroom link to someone SIS has no StaffMember for is no homeroom at all; an inactive
    // homeroom teacher is reported on the teacher instead.
    let mut class_warnings: Vec<ClassWarning> = s
        .sections
        .iter()
        .filter(|x| x.homeroom.as_deref().is_none_or(|h| !staff.contains_key(h)))
        .filter_map(|x| {
            class_ref(&x.id).map(|class| ClassWarning {
                code: "NO_HOMEROOM",
                class,
            })
        })
        .collect();
    class_warnings.sort_by(|a, b| a.class.name.cmp(&b.class.name));
    let mut subject_warnings = vec![];
    for x in &s.sections {
        for p in s.plans.iter().filter(|p| p.grade == x.year_group) {
            let held: Vec<&&Assignment> = assignments
                .iter()
                .filter(|a| a.section_id == x.id && a.subject == p.subject)
                .collect();
            let assigned_periods = held.iter().map(|a| a.periods).sum();
            let code = if held.is_empty() {
                if !p.required {
                    continue;
                }
                "SUBJECT_UNASSIGNED"
            } else if assigned_periods != p.periods {
                "PLAN_HOURS_MISMATCH"
            } else {
                continue;
            };
            if let Some(class) = class_ref(&x.id) {
                subject_warnings.push(SubjectWarning {
                    code,
                    class,
                    subject: p.subject.clone(),
                    planned_periods: p.periods,
                    assigned_periods,
                });
            }
        }
    }
    subject_warnings.sort_by(|a, b| {
        (&a.class.name, &a.subject, a.code).cmp(&(&b.class.name, &b.subject, b.code))
    });
    let has = |t: &Teacher, code: &str| t.warnings.iter().any(|w| w.code == code);
    let summary = Summary {
        teachers: teachers.len(),
        with_warnings: teachers.iter().filter(|t| !t.warnings.is_empty()).count(),
        no_homeroom: class_warnings.len(),
        clashes: teachers.iter().filter(|t| has(t, "TEACHER_CLASH")).count(),
        overloaded: teachers.iter().filter(|t| has(t, "OVERLOAD")).count(),
        underloaded: teachers.iter().filter(|t| has(t, "UNDERLOAD")).count(),
        inactive: teachers
            .iter()
            .filter(|t| has(t, "INACTIVE_TEACHER"))
            .count(),
        subject_unassigned: subject_warnings
            .iter()
            .filter(|w| w.code == "SUBJECT_UNASSIGNED")
            .count(),
        plan_hours_mismatch: subject_warnings
            .iter()
            .filter(|w| w.code == "PLAN_HOURS_MISMATCH")
            .count(),
        hours_mismatch: teachers.iter().filter(|t| has(t, "HOURS_MISMATCH")).count(),
        schedule_without_assignment: teachers
            .iter()
            .filter(|t| has(t, "SCHEDULE_WITHOUT_ASSIGNMENT"))
            .count(),
        grant_without_assignment: teachers
            .iter()
            .filter(|t| has(t, "GRANT_WITHOUT_ASSIGNMENT"))
            .count(),
    };
    Some(Report {
        teachers,
        class_warnings,
        subject_warnings,
        summary,
        lessons: lessons.len(),
    })
}

/// Every warning of the year and week, classes first and then teachers by name.
pub fn issues(r: &Report) -> Vec<Issue> {
    let mut out: Vec<Issue> = r
        .class_warnings
        .iter()
        .map(|w| Issue {
            class: Some(w.class.clone()),
            ..Issue::new(w.code)
        })
        .collect();
    out.extend(r.subject_warnings.iter().map(|w| Issue {
        class: Some(w.class.clone()),
        subject: Some(w.subject.clone()),
        planned_periods: Some(w.planned_periods),
        assigned_periods: Some(w.assigned_periods),
        ..Issue::new(w.code)
    }));
    out.extend(r.teachers.iter().flat_map(|t| t.issues.iter().cloned()));
    out
}
