use super::*;

fn reader(locale: &str, notify: bool) -> Recipient {
    Recipient {
        id: "student:u1".into(),
        user: Some("u1".into()),
        locale: locale.into(),
        notify,
    }
}
fn notice(v: Value) -> Option<Notice> {
    serde_json::from_value::<Notice>(v)
        .ok()
        .filter(Notice::valid)
}

#[test]
fn the_lock_screen_text_is_fixed_per_kind_and_language_and_links_only_an_id() {
    let a = push_payload(&reader("id", true), Kind::Assignment, "A-1");
    assert_eq!(a["recipientId"], "student:u1");
    assert_eq!(a["title"], "Tugas baru");
    assert_eq!(a["url"], "/lessons?assignment=A-1");
    let q = push_payload(&reader("en", true), Kind::Assessment, "Q-1");
    assert_eq!(q["title"], "New quiz");
    assert_eq!(q["url"], "/tests?assessment=Q-1");
    let r = push_payload(&reader("id-ID", true), Kind::ReportCard, "R-1");
    assert_eq!(r["body"], "Rapormu sudah bisa dilihat.");
    assert_eq!(r["url"], "/report-cards");
    let l = push_payload(&reader("id", true), Kind::LiveStarted, "S-1");
    assert_eq!(l["title"], "Kelas live dimulai");
    assert_eq!(l["url"], "/lessons?mode=live&session=S-1");
    // Exactly the four fields notification-services takes; nothing else rides along.
    for v in [a, q, r, l] {
        assert_eq!(v.as_object().unwrap().len(), 4, "{v}");
    }
}

#[test]
fn each_kind_follows_its_student_category() {
    use crate::school_portal::model::StudentNotificationCategories as C;
    assert_eq!(C::STORED[Kind::Assignment.category()], "assignments");
    assert_eq!(C::STORED[Kind::Assessment.category()], "assignments");
    assert_eq!(C::STORED[Kind::ReportCard.category()], "grades");
    assert_eq!(
        C::STORED[Kind::LiveStarted.category()],
        "learning_reminders"
    );
}

#[test]
fn quiet_hours_are_nine_in_the_evening_to_six_in_the_morning() {
    for h in [21, 23, 0, 5] {
        assert!(quiet(h), "{h}");
    }
    for h in [6, 12, 20] {
        assert!(!quiet(h), "{h}");
    }
}

#[test]
fn only_students_who_want_it_get_it_and_nobody_in_quiet_hours() {
    let all = vec![reader("id", true), reader("en", false)];
    assert_eq!(wanted(all.clone(), false).len(), 1);
    assert!(wanted(all, true).is_empty());
}

#[test]
fn a_notice_names_a_known_kind_an_id_and_a_class_and_nothing_else() {
    assert!(notice(json!({"kind":"assignment.published","ref":"A-1","classId":"7a"})).is_some());
    assert!(notice(json!({"kind":"live_class.started","ref":"S-1","classId":"7a"})).is_some());
    assert!(notice(
        json!({"kind":"report_card.published","ref":"R-1","classId":"7a","studentIds":["s1","s2"]})
    )
    .is_some());
    for bad in [
        json!({"kind":"article.published","ref":"A-1","classId":"7a"}),
        json!({"kind":"assignment.published","ref":"A 1","classId":"7a"}),
        json!({"kind":"assignment.published","ref":"A-1","classId":""}),
        json!({"kind":"assignment.published","ref":"A-1"}),
        json!({"kind":"assignment.published","ref":"A-1","classId":"7a","title":"Fractions"}),
        json!({"kind":"assignment.published","ref":"A-1","classId":"7a","recipientId":"student:x"}),
        json!({"kind":"report_card.published","ref":"R-1","classId":"7a","studentIds":[]}),
        json!({"kind":"report_card.published","ref":"R-1","classId":"7a","studentIds":["s1","s1"]}),
        json!({"kind":"report_card.published","ref":"R-1","classId":"7a","studentIds":(0..61).map(|i| format!("s{i}")).collect::<Vec<_>>()}),
    ] {
        assert!(notice(bad.clone()).is_none(), "{bad}");
    }
}

#[test]
fn a_device_body_cannot_name_the_recipient() {
    let base = json!({"installationId":"3f2b8c1e-9d4a-4c55-8f0e-2a1b3c4d5e6f","token":"t","platform":"ios","appVersion":"1.0.0","locale":"id"});
    assert!(serde_json::from_value::<Device>(base.clone()).is_ok());
    let mut forged = base;
    forged["recipientId"] = json!("student:someone-else");
    assert!(serde_json::from_value::<Device>(forged).is_err());
}
