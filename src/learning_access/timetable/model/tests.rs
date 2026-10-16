use super::*;
fn meeting() -> Meeting {
    Meeting {
        id: "one".into(),
        school_id: "school".into(),
        tenant_id: "tenant".into(),
        class_id: "class".into(),
        teacher_id: "teacher".into(),
        course_id: "course".into(),
        subject: "math".into(),
        title: "Mathematics".into(),
        starts_at: chrono::DateTime::parse_from_rfc3339("2030-01-07T02:00:00Z")
            .unwrap()
            .timestamp_millis() as u64,
        ends_at: chrono::DateTime::parse_from_rfc3339("2030-01-07T03:00:00Z")
            .unwrap()
            .timestamp_millis() as u64,
        academic_year: "2030/2031".into(),
        room_id: "room-a".into(),
        chapter_id: None,
        lesson_id: None,
    }
}
#[test]
fn explicit_scopes_room_and_resource_ids_are_required() {
    let original = meeting();
    assert!(original.valid());
    let mut m = original.clone();
    m.room_id.clear();
    assert!(!m.valid());
    m = original.clone();
    m.teacher_id = "teacher / injected".into();
    assert!(!m.valid());
    m = original.clone();
    m.academic_year = "".into();
    assert!(!m.valid());
    m = original.clone();
    m.ends_at = m.starts_at;
    assert!(!m.valid());
    m = original.clone();
    m.ends_at = m.starts_at + 12 * 3_600_000 + 1;
    assert!(!m.valid());
    m = original.clone();
    m.academic_year = "year\n".into();
    assert!(!m.valid());
}
#[test]
fn school_day_and_future_use_jakarta_dates() {
    let mut m = meeting();
    assert!(m.time_issues(1).is_empty());
    assert_eq!(m.time_issues(m.starts_at), vec!["PAST_MEETING"]);
    m.starts_at = chrono::DateTime::parse_from_rfc3339("2030-01-11T16:30:00Z")
        .unwrap()
        .timestamp_millis() as u64;
    m.ends_at = m.starts_at + 3_600_000;
    assert_eq!(
        m.time_issues(1),
        vec!["CROSSES_SCHOOL_DAY", "NON_SCHOOL_DAY"]
    );
    m = meeting();
    m.starts_at += 5 * 86_400_000;
    m.ends_at += 5 * 86_400_000;
    assert_eq!(m.time_issues(1), vec!["NON_SCHOOL_DAY"]);
}
#[test]
fn new_drafts_and_immutable_operation_ids_are_bounded() {
    let mut c = Change {
        meeting: meeting(),
        expected_version: 0,
        expected_published_version: 0,
        action: Action::SaveDraft,
        operation_id: uuid::Uuid::new_v4().to_string(),
        ad_hoc: false,
    };
    assert!(c.valid());
    c.action = Action::Publish;
    assert!(!c.valid());
    c.expected_version = 2;
    assert!(c.valid());
    c.expected_published_version = i32::MAX as u32;
    assert!(!c.valid());
    c.expected_published_version = 0;
    c.operation_id = "another op".into();
    assert!(!c.valid());
}
#[test]
fn meeting_key_matches_existing_owner_and_scope_is_unambiguous() {
    let mut m = meeting();
    assert_eq!(m.key(), "6:school6:tenant5:class3:one");
    let old = m.key();
    m.id = "another".into();
    assert_ne!(old, m.key());
    let old = m.school_key();
    m.school_id = "other".into();
    assert_ne!(old, m.school_key());
}
#[test]
fn material_link_is_optional_and_lesson_needs_its_chapter() {
    let mut m = meeting();
    m.chapter_id = Some("chapter-1".into());
    assert!(m.valid());
    m.lesson_id = Some("lesson-1".into());
    assert!(m.valid());
    m.lesson_id = Some("lesson / injected".into());
    assert!(!m.valid());
    m.chapter_id = None;
    m.lesson_id = Some("lesson-1".into());
    assert!(!m.valid());
    let legacy = serde_json::to_value(meeting()).unwrap();
    assert!(legacy.get("chapterId").is_none() && legacy.get("lessonId").is_none());
    let linked: Meeting = serde_json::from_value(
        serde_json::json!({"chapterId":"chapter-1","lessonId":"lesson-1"})
            .as_object()
            .unwrap()
            .iter()
            .fold(legacy, |mut v, (k, x)| {
                v[k] = x.clone();
                v
            }),
    )
    .unwrap();
    assert_eq!(linked.lesson_id.as_deref(), Some("lesson-1"));
}
#[test]
fn a_class_started_now_may_fall_on_a_weekend_or_holiday_but_only_within_minutes() {
    let mut m = meeting();
    // Saturday 2030-01-12 09:00-09:45 WIB.
    m.starts_at = chrono::DateTime::parse_from_rfc3339("2030-01-12T02:00:00Z")
        .unwrap()
        .timestamp_millis() as u64;
    m.ends_at = m.starts_at + 45 * 60_000;
    let now = m.starts_at - 60_000;
    assert_eq!(m.time_issues(now), vec!["NON_SCHOOL_DAY"]);
    assert!(m.ad_hoc_issues(m.time_issues(now), now).is_empty());
    assert!(m
        .ad_hoc_issues(vec!["NON_SCHOOL_DAY", "SCHOOL_HOLIDAY"], now)
        .is_empty());
    // The Teacher's own other meeting and the inherited room do not block a class started now;
    // every other rule stays: a second meeting of the class, a past start, crossing midnight.
    assert_eq!(
        m.ad_hoc_issues(
            vec![
                "SCHOOL_HOLIDAY",
                "TEACHER_CONFLICT",
                "ROOM_CONFLICT",
                "CLASS_CONFLICT",
                "PAST_MEETING"
            ],
            now
        ),
        vec!["CLASS_CONFLICT", "PAST_MEETING"]
    );
    assert_eq!(
        m.ad_hoc_issues(vec!["CROSSES_SCHOOL_DAY", "OUTSIDE_ACADEMIC_YEAR"], now),
        vec!["CROSSES_SCHOOL_DAY", "OUTSIDE_ACADEMIC_YEAR"]
    );
    // Not a way to plan ahead or to book a long slot.
    assert_eq!(
        m.ad_hoc_issues(vec![], m.starts_at - 16 * 60_000),
        vec!["AD_HOC_WINDOW"]
    );
    m.ends_at = m.starts_at + 3 * 3_600_000 + 1;
    assert_eq!(m.ad_hoc_issues(vec![], now), vec!["AD_HOC_WINDOW"]);
}
#[test]
fn an_ordinary_change_keeps_its_stored_request_and_the_flag_defaults_to_false() {
    let raw = serde_json::json!({"meeting":meeting(),"expectedVersion":0,"expectedPublishedVersion":0,"action":"save_draft","operationId":"8e44e7e1-ae65-4eee-8881-5d6110d2736a"});
    let change: Change = serde_json::from_value(raw.clone()).unwrap();
    assert!(!change.ad_hoc);
    assert_eq!(serde_json::to_value(&change).unwrap(), raw);
    let mut flagged = raw;
    flagged["adHoc"] = serde_json::json!(true);
    let change: Change = serde_json::from_value(flagged.clone()).unwrap();
    assert!(change.ad_hoc);
    assert_eq!(serde_json::to_value(&change).unwrap(), flagged);
}
