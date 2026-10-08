use super::*;
fn slot(key: &str, weekday: u8, start: &str, end: &str, course: &str) -> Slot {
    Slot {
        key: key.into(),
        weekday,
        start: start.into(),
        end: end.into(),
        course_id: course.into(),
        subject: "mathematics".into(),
        title: "Mathematics · Year 7A".into(),
        teacher_id: "teacher".into(),
        room_id: "Room-7A".into(),
    }
}
fn weekly() -> Weekly {
    Weekly {
        school_id: "school".into(),
        tenant_id: "tenant".into(),
        class_id: "class".into(),
        academic_year: "2026".into(),
        from: "2026-11-13".into(),
        to: "2026-11-20".into(),
        slots: vec![
            slot("mon-p1", 1, "07:30", "08:15", "math"),
            slot("fri-p1", 5, "07:30", "08:15", "math"),
            slot("fri-p2", 5, "08:15", "09:00", "science"),
        ],
        materials: BTreeMap::new(),
        sequence_from: None,
        operation_id: "8e44e7e1-ae65-4eee-8881-5d6110d2736a".into(),
    }
}
#[test]
fn validation_bounds_range_slots_and_identifiers() {
    assert!(weekly().valid());
    for bad in [
        Weekly {
            to: "2026-12-14".into(),
            ..weekly()
        },
        Weekly {
            to: "2026-11-12".into(),
            ..weekly()
        },
        Weekly {
            slots: vec![slot("Mon P1", 1, "07:30", "08:15", "math")],
            ..weekly()
        },
        Weekly {
            slots: vec![slot("sat", 6, "07:30", "08:15", "math")],
            ..weekly()
        },
        Weekly {
            slots: vec![slot("late", 1, "09:00", "08:15", "math")],
            ..weekly()
        },
        Weekly {
            slots: vec![slot("a", 1, "7:30", "08:15", "math")],
            ..weekly()
        },
        Weekly {
            slots: vec![
                slot("a", 1, "07:30", "08:15", "math"),
                slot("a", 2, "07:30", "08:15", "math"),
            ],
            ..weekly()
        },
        Weekly {
            slots: vec![],
            ..weekly()
        },
        Weekly {
            sequence_from: Some("2026-11-14".into()),
            ..weekly()
        },
        Weekly {
            operation_id: "not-a-uuid".into(),
            ..weekly()
        },
    ] {
        assert!(!bad.valid(), "{bad:?}");
    }
}
#[test]
fn occurrences_are_dated_wib_school_weekdays_with_stable_ids() {
    let all = weekly().occurrences();
    // Fri 13, Mon 16, Fri 20 November 2026 (no weekend occurrences).
    let ids: Vec<_> = all.iter().map(|(_, m)| m.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "wk-fri-p1-20261113",
            "wk-fri-p2-20261113",
            "wk-mon-p1-20261116",
            "wk-fri-p1-20261120",
            "wk-fri-p2-20261120"
        ]
    );
    let first = &all[0].1;
    // 07:30 WIB = 00:30 UTC.
    assert_eq!(
        first.starts_at,
        chrono::DateTime::parse_from_rfc3339("2026-11-13T00:30:00Z")
            .unwrap()
            .timestamp_millis() as u64
    );
    assert_eq!(first.ends_at - first.starts_at, 45 * 60_000);
    assert_eq!(first.room_id, "room-7a");
    assert!(all
        .iter()
        .all(|(_, m)| m.valid() && m.time_issues(0).is_empty()));
}
#[test]
fn materials_continue_after_earlier_course_meetings_and_cycle() {
    let mut all = weekly().occurrences();
    let materials = BTreeMap::from([(
        "math".to_string(),
        vec![
            Material {
                chapter_id: "c1".into(),
                lesson_id: Some("l1".into()),
            },
            Material {
                chapter_id: "c1".into(),
                lesson_id: Some("l2".into()),
            },
            Material {
                chapter_id: "c2".into(),
                lesson_id: None,
            },
        ],
    )]);
    // Two earlier math meetings since the sequence start: this range starts at entry 3 (c2).
    let earlier = BTreeMap::from([("math".to_string(), vec![1, 2])]);
    assign_materials(&mut all, &materials, &earlier);
    let math: Vec<_> = all
        .iter()
        .filter(|(_, m)| m.course_id == "math")
        .map(|(_, m)| (m.chapter_id.clone().unwrap(), m.lesson_id.clone()))
        .collect();
    assert_eq!(
        math,
        [
            ("c2".into(), None),
            ("c1".into(), Some("l1".into())),
            ("c1".into(), Some("l2".into()))
        ]
    );
    assert!(all
        .iter()
        .filter(|(_, m)| m.course_id == "science")
        .all(|(_, m)| m.chapter_id.is_none()));
}
