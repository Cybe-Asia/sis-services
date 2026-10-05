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
