use super::model::*;
use chrono::NaiveDate;

fn school(id: &str) -> SchoolFacts {
    SchoolFacts {
        school_id: id.into(),
        school_name: "Sekolah Contoh".into(),
        years: vec!["2025/2026".into(), "2026".into(), " 2024/2025 ".into()],
        subjects: vec![
            ("mathematics".into(), "Matematika".into()),
            ("computing".into(), "Cambridge Computing".into()),
            ("mathematics".into(), "Math (older plan)".into()),
            ("Bad Code".into(), "x".into()),
        ],
        today: NaiveDate::from_ymd_opt(2026, 10, 29).unwrap(),
    }
}
fn row(code: &str, order: u32) -> Row {
    Row {
        code: code.into(),
        label_id: "Kelas VII".into(),
        label_en: "Year 7".into(),
        short_id: None,
        short_en: None,
        order,
        status: ACTIVE.into(),
    }
}
fn codes(rows: &[Row]) -> Vec<&str> {
    rows.iter().map(|r| r.code.as_str()).collect()
}

#[test]
fn every_kind_seeds_valid_rows_for_both_units() {
    for id in ["SCH-IISS", "SCH-IIHS", "SCH-OTHER", "school"] {
        let facts = school(id);
        for kind in KINDS {
            let rows = defaults(kind, &facts);
            assert!(valid_rows(kind, &rows), "{id} {kind}: {rows:?}");
        }
    }
}

#[test]
fn grades_phases_and_stages_follow_the_unit() {
    let iiss = school("SCH-IISS");
    assert_eq!(codes(&defaults("grade", &iiss)), ["Y7", "Y8", "Y9"]);
    assert_eq!(codes(&defaults("phase", &iiss)), ["D"]);
    assert_eq!(codes(&defaults("stage", &iiss)), ["CLS7", "CLS8", "CLS9"]);
    let iihs = school("SCH-IIHS");
    assert_eq!(codes(&defaults("grade", &iihs)), ["Y10", "Y11", "Y12"]);
    assert_eq!(codes(&defaults("phase", &iihs)), ["E", "F"]);
    assert_eq!(codes(&defaults("stage", &iihs)), ["IGCSE", "AL"]);
    assert_eq!(defaults("grade", &school("SCH-OTHER")).len(), 6);
    let y7 = &defaults("grade", &iiss)[0];
    assert_eq!(
        (y7.label_id.as_str(), y7.label_en.as_str()),
        ("Kelas VII", "Year 7")
    );
    assert_eq!(
        (y7.short_id.as_deref(), y7.short_en.as_deref()),
        (Some("VII"), Some("Y7"))
    );
    let unit = &defaults("unit", &iihs)[0];
    assert_eq!(unit.code, "IIHS");
    assert_eq!(unit.short_en.as_deref(), Some("IIHS"));
    let other = &defaults("unit", &school("SCH-OTHER"))[0];
    assert_eq!(
        (other.code.as_str(), other.label_en.as_str()),
        ("OTHER", "Sekolah Contoh")
    );
}

#[test]
fn years_subjects_and_shared_kinds_seed_from_school_data() {
    let facts = school("SCH-IISS");
    assert_eq!(
        codes(&defaults("academic_year", &facts)),
        ["2024/2025", "2025/2026", "2026/2027"]
    );
    let subjects = defaults("subject", &facts);
    assert_eq!(codes(&subjects), ["mathematics", "computing"]);
    assert_eq!(
        (subjects[0].label_id.as_str(), subjects[0].label_en.as_str()),
        ("Matematika", "Mathematics")
    );
    assert_eq!(
        (subjects[1].label_id.as_str(), subjects[1].label_en.as_str()),
        ("Cambridge Computing", "Cambridge Computing")
    );
    let mut none = school("SCH-IISS");
    none.subjects.clear();
    assert!(defaults("subject", &none).is_empty());
    assert_eq!(codes(&defaults("term", &facts)), ["S1", "S2"]);
    assert_eq!(defaults("assessment_kind", &facts).len(), 6);
    assert_eq!(defaults("mastery_level", &facts)[4].label_id, "Mahir");
    let titles = defaults("role_title", &facts);
    for code in [
        "curriculum_coordinator",
        "subject_lead",
        "homeroom",
        "principal",
    ] {
        assert!(titles.iter().any(|t| t.code == code), "{code}");
    }
    assert_eq!(
        titles
            .iter()
            .find(|t| t.code == "curriculum_coordinator")
            .unwrap()
            .label_id,
        "Wakasek Kurikulum"
    );
}

#[test]
fn current_year_turns_in_july() {
    assert_eq!(
        current_year(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap()),
        "2025/2026"
    );
    assert_eq!(
        current_year(NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()),
        "2026/2027"
    );
}

#[test]
fn codes_follow_the_rule_of_their_kind() {
    assert!(code("academic_year", "2026/2027"));
    assert!(!code("academic_year", "2026/2028"));
    assert!(!code("academic_year", "2026"));
    assert!(code("subject", "mathematics"));
    assert!(!code("subject", "Mathematics"));
    assert!(code("term", "S1"));
    assert!(code("term", "T3"));
    assert!(!code("term", "s1"));
    assert!(code("role_title", "curriculum_coordinator"));
    assert!(!code("role_title", "Wakasek"));
    assert!(code("grade", "Y10"));
    assert!(!code("grade", "Y 10"));
    assert!(!code("grade", ""));
}

#[test]
fn labels_orders_and_lists_are_bounded() {
    assert!(valid_rows("grade", &[row("Y7", 1), row("Y8", 2)]));
    // Same order twice, same code twice, empty list.
    assert!(!valid_rows("grade", &[row("Y7", 1), row("Y8", 1)]));
    assert!(!valid_rows("grade", &[row("Y7", 1), row("Y7", 2)]));
    assert!(!valid_rows("grade", &[]));
    let mut r = row("Y7", 1);
    r.label_id = "x".repeat(80);
    assert!(r.valid("grade"));
    r.label_id = "é".repeat(80);
    assert!(r.valid("grade"), "80 characters, not bytes");
    r.label_id = "x".repeat(81);
    assert!(!r.valid("grade"));
    r.label_id = " Kelas VII".into();
    assert!(!r.valid("grade"));
    r.label_id = String::new();
    assert!(!r.valid("grade"));
    let mut r = row("Y7", 1);
    r.label_en = "Year\n7".into();
    assert!(!r.valid("grade"));
    let mut r = row("Y7", 1);
    r.short_en = Some("x".repeat(25));
    assert!(!r.valid("grade"));
    r.short_en = Some(String::new());
    assert!(!r.valid("grade"));
    let mut r = row("Y7", 0);
    assert!(!r.valid("grade"));
    r.order = 1000;
    assert!(!r.valid("grade"));
    let mut r = row("Y7", 1);
    r.status = "deleted".into();
    assert!(!r.valid("grade"));
    assert!(valid_rows(
        "grade",
        &(1..=100)
            .map(|i| row(&format!("G{i}"), i))
            .collect::<Vec<_>>()
    ));
    assert!(!valid_rows(
        "grade",
        &(1..=101)
            .map(|i| row(&format!("G{i}"), i))
            .collect::<Vec<_>>()
    ));
}

#[test]
fn units_and_patterns_keep_one_active_row_and_patterns_name_the_letter() {
    let mut unit = row("IISS", 1);
    unit.status = ARCHIVED.into();
    assert!(!valid_rows("unit", std::slice::from_ref(&unit)));
    assert!(valid_rows(
        "grade",
        std::slice::from_ref(&{
            let mut g = row("Y7", 1);
            g.status = ARCHIVED.into();
            g
        })
    ));
    assert!(pattern("{grade}{letter}"));
    assert!(pattern("Kelas {grade_short}-{letter}"));
    assert!(!pattern("{grade}"));
    assert!(!pattern("{grade}{letter}{room}"));
    assert!(!pattern("{letter"));
    assert!(!pattern("{letter}}"));
    let mut p = row("default", 1);
    p.label_id = "{grade}".into();
    assert!(!p.valid("section_pattern"));
}

#[test]
fn year_groups_map_to_grade_codes() {
    for (text, code) in [
        ("year7", "Y7"),
        ("Year 7", "Y7"),
        ("Grade 7", "Y7"),
        ("Kelas 7", "Y7"),
        ("Kelas VII", "Y7"),
        ("VII", "Y7"),
        ("y7", "Y7"),
        ("12", "Y12"),
        ("Year 10", "Y10"),
        ("XI", "Y11"),
    ] {
        assert_eq!(grade_code(text), Some(code), "{text}");
    }
    for text in ["Year 13", "Kelas", "", "Year 7A", "TK"] {
        assert_eq!(grade_code(text), None, "{text}");
    }
    let m = mappings();
    assert_eq!(
        m.iter()
            .find(|g| g.code == "Y10")
            .map(|g| (g.unit, g.phase, g.stage)),
        Some(("IIHS", "E", "IGCSE"))
    );
    assert_eq!(
        m.iter()
            .find(|g| g.code == "Y12")
            .map(|g| (g.phase, g.stage)),
        Some(("F", "AL"))
    );
    assert_eq!(unit_code("SCH-IISS").as_deref(), Some("IISS"));
    assert_eq!(unit_code("SCH-"), None);
}

#[test]
fn changes_are_checked_whole() {
    let change = Change {
        tenant_id: "tenant".into(),
        school_id: "SCH-IISS".into(),
        expected_version: 1,
        rows: vec![row("Y7", 1)],
    };
    assert!(change.valid("grade"));
    assert!(!Change {
        expected_version: 0,
        ..change
    }
    .valid("grade"));
    assert!(serde_json::from_value::<Change>(
        serde_json::json!({"tenantId":"t","schoolId":"s","expectedVersion":1,"rows":[],"extra":1})
    )
    .is_err());
    assert!(serde_json::from_value::<Row>(serde_json::json!({"code":"Y7","labelId":"a","labelEn":"b","order":1,"status":"active","version":3})).is_err());
    assert!(!kind("teacher"));
    assert!(closed("grade") && !closed("subject") && !closed("term"));
}

#[test]
fn academic_years_read_as_the_school_year_starting_in_july() {
    use super::model::academic_year_code;
    assert_eq!(academic_year_code("2026").as_deref(), Some("2026/2027"));
    assert_eq!(
        academic_year_code(" 2026-2027 ").as_deref(),
        Some("2026/2027")
    );
    assert_eq!(
        academic_year_code("2026/2027").as_deref(),
        Some("2026/2027")
    );
    for unknown in ["2026/2028", "TA 26", "26/27", "2026-27", "", "1989"] {
        assert_eq!(academic_year_code(unknown), None, "{unknown}");
    }
}
