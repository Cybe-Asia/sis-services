use super::model::*;
use super::Lookup;
use serde_json::json;

fn row(subject: &str) -> PlanRow {
    PlanRow {
        subject: subject.into(),
        name: "Matematika".into(),
        framework: "merdeka".into(),
        periods_per_week: 4,
        required: true,
        status: "active".into(),
    }
}
fn command(action: &str, periods: Option<u32>, expected: u32) -> Command {
    Command {
        action: action.into(),
        section_id: "SEC-7A".into(),
        subject: "mathematics".into(),
        staff_member_id: "staff-1".into(),
        periods,
        expected_version: expected,
    }
}
fn current(status: &str, periods: u32, version: u32) -> Current {
    Current {
        status: status.into(),
        periods,
        version,
    }
}

#[test]
fn subject_codes_are_lms_codes() {
    assert!(subject_code("mathematics"));
    assert!(subject_code("cambridge_english-2"));
    assert!(!subject_code("Mathematics"));
    assert!(!subject_code("_x"));
    assert!(!subject_code("a b"));
    assert!(!subject_code(""));
    assert!(!subject_code(&"a".repeat(65)));
}

#[test]
fn a_plan_row_needs_a_framework_periods_and_a_clean_name() {
    assert!(row("mathematics").valid());
    for framework in FRAMEWORKS {
        assert!(PlanRow {
            framework: framework.into(),
            ..row("x")
        }
        .valid());
    }
    assert!(!PlanRow {
        framework: "ib".into(),
        ..row("x")
    }
    .valid());
    assert!(!PlanRow {
        periods_per_week: 0,
        ..row("x")
    }
    .valid());
    assert!(!PlanRow {
        periods_per_week: MAX_PERIODS + 1,
        ..row("x")
    }
    .valid());
    assert!(!PlanRow {
        name: " Matematika".into(),
        ..row("x")
    }
    .valid());
    assert!(!PlanRow {
        name: "Mate\nmatika".into(),
        ..row("x")
    }
    .valid());
    assert!(!PlanRow {
        status: "deleted".into(),
        ..row("x")
    }
    .valid());
    assert!(PlanRow {
        status: "archived".into(),
        ..row("x")
    }
    .valid());
}

#[test]
fn a_subject_appears_once_per_grade_and_a_plan_is_bounded() {
    assert!(valid_rows(&[row("mathematics"), row("science")]));
    assert!(!valid_rows(&[
        row("english"),
        PlanRow {
            framework: "cambridge".into(),
            ..row("english")
        }
    ]));
    let many: Vec<PlanRow> = (0..=MAX_ROWS).map(|n| row(&format!("s{n}"))).collect();
    assert!(!valid_rows(&many));
    assert!(valid_rows(&[]));
}

#[test]
fn plan_requests_accept_only_their_fields_and_clean_key_parts() {
    let change = |v| serde_json::from_value::<PlanChange>(v);
    let ok = change(json!({"tenantId":"TENANT-001","schoolId":"SCH-IISS","academicYear":"2026/2027","grade":"Year 7","expectedVersion":0,"rows":[{"subject":"mathematics","name":"Matematika","framework":"merdeka","periodsPerWeek":5,"required":true,"status":"active"}]})).unwrap();
    assert!(ok.valid());
    assert!(change(json!({"tenantId":"TENANT-001","schoolId":"SCH-IISS","academicYear":"2026/2027","grade":"Year 7","expectedVersion":0,"rows":[],"extra":1})).is_err());
    let piped = PlanChange {
        grade: "Year|7".into(),
        ..ok
    };
    assert!(!piped.valid());
    let copy = |from: &str| PlanCopy {
        tenant_id: "TENANT-001".into(),
        school_id: "SCH-IISS".into(),
        academic_year: "2026/2027".into(),
        grade: "Year 7".into(),
        from_year: from.into(),
        expected_version: 0,
    };
    assert!(copy("2025/2026").valid());
    assert!(!copy("2026/2027").valid());
    assert!(!copy(" ").valid());
    assert_eq!(
        plan_key("TENANT-001", "SCH-IISS", "2026/2027", "Year 7"),
        "TENANT-001|SCH-IISS|2026/2027|Year 7"
    );
}

#[test]
fn commands_carry_periods_only_when_they_set_them() {
    assert!(command(ASSIGN, Some(4), 0).valid());
    assert!(command(CHANGE_HOURS, Some(2), 1).valid());
    assert!(command(END, None, 1).valid());
    assert!(!command(ASSIGN, None, 0).valid());
    assert!(!command(ASSIGN, Some(0), 0).valid());
    assert!(!command(ASSIGN, Some(MAX_PERIODS + 1), 0).valid());
    assert!(!command(END, Some(4), 1).valid());
    assert!(!command("replace", Some(4), 1).valid());
    assert!(!Command {
        subject: "Math".into(),
        ..command(ASSIGN, Some(4), 0)
    }
    .valid());
    assert!(!Command {
        staff_member_id: "a|b".into(),
        ..command(ASSIGN, Some(4), 0)
    }
    .valid());
}

#[test]
fn a_batch_is_bounded_and_touches_each_assignment_once() {
    let batch = |commands: Vec<Command>| Commands {
        tenant_id: "TENANT-001".into(),
        school_id: "SCH-IISS".into(),
        academic_year: "2026/2027".into(),
        commands,
    };
    let team = Command {
        staff_member_id: "staff-2".into(),
        ..command(ASSIGN, Some(2), 0)
    };
    let ok = batch(vec![command(ASSIGN, Some(2), 0), team.clone()]);
    assert!(ok.valid());
    assert_eq!(
        ok.key(&ok.commands[1]),
        "TENANT-001|SCH-IISS|2026/2027|SEC-7A|mathematics|staff-2"
    );
    assert!(!batch(vec![command(ASSIGN, Some(2), 0), command(END, None, 1)]).valid());
    assert!(!batch(vec![]).valid());
    let many = (0..=MAX_COMMANDS)
        .map(|n| Command {
            staff_member_id: format!("staff-{n}"),
            ..command(ASSIGN, Some(1), 0)
        })
        .collect();
    assert!(!batch(many).valid());
}

#[test]
fn each_command_applies_only_at_its_revision_and_state() {
    let active = current(ACTIVE, 4, 3);
    let ended = current(ENDED, 4, 5);
    assert_eq!(next(&command(ASSIGN, Some(4), 0), None), Some((ACTIVE, 4)));
    assert_eq!(next(&command(ASSIGN, Some(4), 1), None), None);
    assert_eq!(next(&command(ASSIGN, Some(4), 0), Some(&active)), None);
    assert_eq!(
        next(&command(ASSIGN, Some(2), 5), Some(&ended)),
        Some((ACTIVE, 2))
    );
    assert_eq!(next(&command(ASSIGN, Some(2), 4), Some(&ended)), None);
    assert_eq!(
        next(&command(CHANGE_HOURS, Some(2), 3), Some(&active)),
        Some((ACTIVE, 2))
    );
    assert_eq!(
        next(&command(CHANGE_HOURS, Some(2), 2), Some(&active)),
        None
    );
    assert_eq!(next(&command(CHANGE_HOURS, Some(2), 5), Some(&ended)), None);
    assert_eq!(next(&command(CHANGE_HOURS, Some(2), 0), None), None);
    assert_eq!(
        next(&command(END, None, 3), Some(&active)),
        Some((ENDED, 4))
    );
    assert_eq!(next(&command(END, None, 5), Some(&ended)), None);
    assert_eq!(next(&command(END, None, 0), None), None);
}

#[test]
fn idempotency_keys_are_identifiers_of_some_length() {
    assert!(command_key("6f1c2d4e-0a1b-4c5d-9e8f-123456789abc"));
    assert!(!command_key("short"));
    assert!(!command_key("has space here"));
}

#[test]
fn grades_sort_by_their_number() {
    let mut grades = vec!["Year 10", "Year 7", "Kindergarten", "Year 8"];
    grades.sort_by(|a, b| grade_order(a, b));
    assert_eq!(grades, vec!["Year 7", "Year 8", "Year 10", "Kindergarten"]);
}

#[test]
fn reads_name_one_school_year_and_at_most_one_filter() {
    let parse = |v| serde_json::from_value::<Lookup>(v).unwrap();
    assert!(parse(
        json!({"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026/2027"})
    )
    .valid());
    assert!(parse(json!({"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026/2027","sectionId":"SEC-7A"})).valid());
    assert!(!parse(json!({"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026/2027","sectionId":"SEC-7A","staffMemberId":"t-1"})).valid());
    assert!(!parse(
        json!({"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026|2027"})
    )
    .valid());
    assert!(serde_json::from_value::<Lookup>(json!({"schoolId":"SCH-IISS","tenantId":"TENANT-001","academicYear":"2026/2027","grade":"x"})).is_err());
}
