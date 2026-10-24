use super::model::*;
use super::Lookup;
use chrono::NaiveDate;
use serde_json::json;

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}
/// Epoch ms of a school-time (WIB) date and "HH:MM".
fn at(date: &str, time: &str) -> u64 {
    let (h, m) = time.split_once(':').unwrap();
    let (from, _) = window_ms(day(date));
    (from + (h.parse::<i64>().unwrap() * 60 + m.parse::<i64>().unwrap()) * 60_000) as u64
}
fn section(id: &str, grade: &str, homeroom: Option<&str>) -> Section {
    Section {
        id: id.into(),
        name: format!("Class {id}"),
        year_group: grade.into(),
        homeroom: homeroom.map(Into::into),
    }
}
fn staff(id: &str, active: bool) -> Staff {
    Staff {
        id: id.into(),
        name: format!("Teacher {id}"),
        active,
    }
}
fn lesson(
    id: &str,
    class: &str,
    teacher: &str,
    subject: &str,
    date: &str,
    start: &str,
    end: &str,
) -> Lesson {
    Lesson {
        id: id.into(),
        section_id: class.into(),
        teacher_id: teacher.into(),
        subject: subject.into(),
        title: subject.into(),
        starts_at: at(date, start),
        ends_at: at(date, end),
        room_id: "r-1".into(),
    }
}
fn find<'a>(r: &'a Report, id: &str) -> &'a Teacher {
    r.teachers.iter().find(|t| t.staff_member_id == id).unwrap()
}
fn codes(t: &Teacher) -> Vec<&'static str> {
    t.warnings.iter().map(|w| w.code).collect()
}

#[test]
fn the_school_week_runs_monday_to_friday_and_a_weekend_looks_ahead() {
    assert_eq!(
        school_week(day("2026-10-28")),
        (day("2026-10-26"), day("2026-10-30"))
    );
    assert_eq!(
        school_week(day("2026-10-26")),
        (day("2026-10-26"), day("2026-10-30"))
    );
    assert_eq!(
        school_week(day("2026-10-30")),
        (day("2026-10-26"), day("2026-10-30"))
    );
    assert_eq!(
        school_week(day("2026-10-24")),
        (day("2026-10-26"), day("2026-10-30"))
    );
    assert_eq!(
        school_week(day("2026-10-25")),
        (day("2026-10-26"), day("2026-10-30"))
    );
}

#[test]
fn the_window_is_monday_midnight_to_saturday_midnight_school_time() {
    let (from, to) = window_ms(day("2026-10-26"));
    assert_eq!(
        from,
        chrono::DateTime::parse_from_rfc3339("2026-10-25T17:00:00Z")
            .unwrap()
            .timestamp_millis()
    );
    assert_eq!(to - from, 5 * 86_400_000);
    assert_eq!(today_wib(from), day("2026-10-26"));
    assert_eq!(today_wib(from - 1), day("2026-10-25"));
}

#[test]
fn a_period_is_forty_minutes_until_year_ten() {
    assert_eq!(period_minutes("Year 7"), 40);
    assert_eq!(period_minutes("Grade 9"), 40);
    assert_eq!(period_minutes("Grade 10"), 45);
    assert_eq!(period_minutes("12"), 45);
    assert_eq!(period_minutes(""), 40);
}

#[test]
fn who_teaches_what_and_the_scheduled_load_come_from_the_week() {
    let s = Snapshot {
        sections: vec![
            section("7A", "Year 7", Some("T1")),
            section("10A", "Year 10", Some("T2")),
        ],
        staff: vec![staff("T1", true), staff("T2", true)],
        lessons: vec![
            lesson(
                "a",
                "7A",
                "T1",
                "mathematics",
                "2026-10-26",
                "07:30",
                "08:50",
            ),
            lesson(
                "b",
                "7A",
                "T1",
                "mathematics",
                "2026-10-28",
                "07:30",
                "08:10",
            ),
            lesson("c", "10A", "T1", "science", "2026-10-27", "09:00", "10:30"),
            lesson("d", "10A", "T2", "english", "2026-10-26", "07:30", "08:15"),
        ],
        ..Default::default()
    };
    let r = report(&s).unwrap();
    let t1 = find(&r, "T1");
    assert_eq!(t1.subjects, vec!["mathematics", "science"]);
    assert_eq!(t1.lessons, 3);
    assert_eq!(t1.scheduled_minutes, 80 + 40 + 90);
    // 120 min / 40 in Year 7 + 90 min / 45 in Year 10.
    assert_eq!(t1.scheduled_periods, 5.0);
    let names: Vec<_> = t1
        .classes
        .iter()
        .map(|c| c.class.section_id.as_str())
        .collect();
    assert_eq!(names, vec!["10A", "7A"]);
    assert_eq!(t1.classes[1].periods, 3.0);
    assert_eq!(t1.homeroom_of.len(), 1);
    assert_eq!(t1.schedule[0].weekday, 1);
    assert_eq!(t1.schedule[0].start, "07:30");
    assert_eq!(t1.schedule[0].end, "08:50");
    assert_eq!(
        t1.schedule
            .iter()
            .map(|l| l.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "c", "b"]
    );
    assert_eq!(codes(t1), vec!["UNDERLOAD"]);
    assert_eq!(r.lessons, 4);
    assert!(r.class_warnings.is_empty());
}

#[test]
fn overlapping_lessons_in_two_classes_clash_but_back_to_back_or_one_class_do_not() {
    let s = Snapshot {
        sections: vec![
            section("7A", "Year 7", Some("T1")),
            section("8A", "Year 8", Some("T1")),
        ],
        staff: vec![staff("T1", true)],
        lessons: vec![
            lesson(
                "a",
                "7A",
                "T1",
                "mathematics",
                "2026-10-26",
                "07:30",
                "08:50",
            ),
            lesson(
                "b",
                "8A",
                "T1",
                "mathematics",
                "2026-10-26",
                "08:10",
                "09:00",
            ),
            lesson(
                "c",
                "8A",
                "T1",
                "mathematics",
                "2026-10-26",
                "08:50",
                "09:30",
            ),
            lesson(
                "d",
                "7A",
                "T1",
                "mathematics",
                "2026-10-27",
                "07:30",
                "08:10",
            ),
            lesson(
                "e",
                "7A",
                "T1",
                "mathematics",
                "2026-10-27",
                "07:50",
                "08:30",
            ),
        ],
        ..Default::default()
    };
    let r = report(&s).unwrap();
    let t = find(&r, "T1");
    assert_eq!(
        t.warnings[0],
        Warning {
            code: "TEACHER_CLASH",
            count: Some(1)
        }
    );
    let clashing: Vec<_> = t
        .schedule
        .iter()
        .filter(|l| l.clash)
        .map(|l| l.id.as_str())
        .collect();
    assert_eq!(clashing, vec!["a", "b"]);
    assert_eq!(r.summary.clashes, 1);
}

#[test]
fn load_outside_the_default_limits_is_flagged_and_an_empty_week_is_not() {
    let mut lessons = vec![];
    // 25 lessons of 40 minutes in Year 7 = 25 periods.
    for (n, date) in [
        "2026-10-26",
        "2026-10-27",
        "2026-10-28",
        "2026-10-29",
        "2026-10-30",
    ]
    .iter()
    .enumerate()
    {
        for (k, start) in ["07:00", "08:00", "09:00", "10:00", "11:00"]
            .iter()
            .enumerate()
        {
            let end = format!("{}:40", &start[..2]);
            lessons.push(lesson(
                &format!("x{n}{k}"),
                "7A",
                "BUSY",
                "mathematics",
                date,
                start,
                &end,
            ));
        }
    }
    // 18 periods exactly is within the limits.
    for k in 0..18 {
        let date = ["2026-10-26", "2026-10-27", "2026-10-28"][k / 6];
        let start = format!("{:02}:00", 7 + k % 6);
        let end = format!("{:02}:40", 7 + k % 6);
        lessons.push(lesson(
            &format!("y{k}"),
            "8A",
            "EVEN",
            "science",
            date,
            &start,
            &end,
        ));
    }
    let s = Snapshot {
        sections: vec![
            section("7A", "Year 7", Some("BUSY")),
            section("8A", "Year 8", Some("EVEN")),
        ],
        staff: vec![
            staff("BUSY", true),
            staff("EVEN", true),
            staff("IDLE", true),
        ],
        lessons,
        ..Default::default()
    };
    let r = report(&s).unwrap();
    assert_eq!(find(&r, "BUSY").scheduled_periods, 25.0);
    assert_eq!(codes(find(&r, "BUSY")), vec!["OVERLOAD"]);
    assert_eq!(find(&r, "EVEN").scheduled_periods, 18.0);
    assert!(codes(find(&r, "EVEN")).is_empty());
    assert_eq!(codes(find(&r, "IDLE")), vec!["UNDERLOAD"]);
    assert_eq!((r.summary.overloaded, r.summary.underloaded), (1, 1));

    let quiet = Snapshot {
        lessons: vec![],
        ..s
    };
    let r = report(&quiet).unwrap();
    assert!(r.teachers.iter().all(|t| t.warnings.is_empty()));
    assert_eq!(r.lessons, 0);
}

#[test]
fn inactive_holders_are_listed_and_idle_inactive_staff_are_not() {
    let s = Snapshot {
        sections: vec![
            section("7A", "Year 7", Some("GONE")),
            section("8A", "Year 8", None),
            section("9A", "Year 9", Some("NO-SUCH-STAFF")),
        ],
        staff: vec![
            staff("GONE", false),
            staff("COACH", false),
            staff("GRANTED", false),
            staff("IDLE", false),
        ],
        activities: vec![Activity {
            id: "x-1".into(),
            title: json!({"en":"Football","id":"Sepak bola"}),
            coach_id: "COACH".into(),
        }],
        grants: vec![
            Grant {
                teacher_id: "GRANTED".into(),
                section_id: "8A".into(),
            },
            Grant {
                teacher_id: "IDLE".into(),
                section_id: "OTHER-YEAR".into(),
            },
        ],
        ..Default::default()
    };
    let r = report(&s).unwrap();
    let ids: Vec<_> = r
        .teachers
        .iter()
        .map(|t| t.staff_member_id.as_str())
        .collect();
    assert_eq!(ids, vec!["COACH", "GONE", "GRANTED"]);
    assert!(r
        .teachers
        .iter()
        .all(|t| codes(t) == vec!["INACTIVE_TEACHER"]));
    assert_eq!(find(&r, "COACH").coach_of[0].activity_id, "x-1");
    assert_eq!(find(&r, "GRANTED").learning_access[0].section_id, "8A");
    let missing: Vec<_> = r
        .class_warnings
        .iter()
        .map(|w| (w.code, w.class.section_id.as_str()))
        .collect();
    assert_eq!(missing, vec![("NO_HOMEROOM", "8A"), ("NO_HOMEROOM", "9A")]);
    assert_eq!(r.summary.inactive, 3);
    assert_eq!(r.summary.no_homeroom, 2);
}

#[test]
fn lessons_of_classes_outside_the_year_are_ignored() {
    let s = Snapshot {
        sections: vec![section("7A", "Year 7", Some("T1"))],
        staff: vec![staff("T1", true)],
        lessons: vec![lesson(
            "a",
            "OLD",
            "T1",
            "mathematics",
            "2026-10-26",
            "07:30",
            "08:10",
        )],
        ..Default::default()
    };
    let r = report(&s).unwrap();
    assert_eq!(find(&r, "T1").lessons, 0);
    assert_eq!(r.lessons, 0);
}

#[test]
fn more_than_the_bound_of_teachers_is_refused() {
    let s = Snapshot {
        staff: (0..=MAX_TEACHERS)
            .map(|n| staff(&format!("T{n}"), true))
            .collect(),
        ..Default::default()
    };
    assert!(report(&s).is_none());
}

#[test]
fn the_lookup_accepts_only_its_own_fields_and_a_real_date() {
    let parse = |v| serde_json::from_value::<Lookup>(v);
    let ok = parse(json!({"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026/2027","week":"2026-10-28"})).unwrap();
    assert!(ok.valid());
    assert_eq!(ok.week(), Some((day("2026-10-26"), day("2026-10-30"))));
    assert!(parse(
        json!({"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026","staff":"x"})
    )
    .is_err());
    let bad_week = parse(json!({"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026","week":"2026-13-01"})).unwrap();
    assert_eq!(bad_week.week(), None);
    let short = parse(json!({"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026","week":"2026-1-1"})).unwrap();
    assert_eq!(short.week(), None);
    let bad_school =
        parse(json!({"schoolId":"SCH IISS","tenantId":"TENANT-001","academicYear":"2026"}))
            .unwrap();
    assert!(!bad_school.valid());
    let blank_year =
        parse(json!({"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"  "})).unwrap();
    assert!(!blank_year.valid());
}

#[test]
fn the_wire_shape_matches_the_documented_contract() {
    let s = Snapshot {
        sections: vec![
            section("7A", "Year 7", Some("T1")),
            section("8A", "Year 8", None),
        ],
        staff: vec![staff("T1", true)],
        lessons: vec![lesson(
            "a",
            "7A",
            "T1",
            "mathematics",
            "2026-10-26",
            "07:30",
            "08:10",
        )],
        ..Default::default()
    };
    let r = report(&s).unwrap();
    let teacher = serde_json::to_value(&r.teachers[0]).unwrap();
    let mut keys: Vec<_> = teacher.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "active",
            "classes",
            "coachOf",
            "homeroomOf",
            "learningAccess",
            "lessons",
            "name",
            "scheduledMinutes",
            "scheduledPeriods",
            "staffMemberId",
            "subjects",
            "warnings"
        ]
    );
    assert_eq!(
        teacher["classes"][0],
        json!({"sectionId":"7A","name":"Class 7A","yearGroup":"Year 7","subjects":["mathematics"],"lessons":1,"minutes":40,"periods":1.0})
    );
    assert_eq!(teacher["warnings"], json!([{"code":"UNDERLOAD"}]));
    assert_eq!(
        serde_json::to_value(&r.class_warnings).unwrap(),
        json!([{"code":"NO_HOMEROOM","sectionId":"8A","name":"Class 8A","yearGroup":"Year 8"}])
    );
    assert_eq!(
        serde_json::to_value(&r.teachers[0].schedule[0]).unwrap(),
        json!({"id":"a","sectionId":"7A","className":"Class 7A","subject":"mathematics","title":"mathematics","weekday":1,"start":"07:30","end":"08:10","roomId":"r-1","clash":false})
    );
}
