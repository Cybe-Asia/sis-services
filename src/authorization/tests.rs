use super::model::*;
use std::collections::BTreeMap;

fn staff(id: &str, roles: &[&str], schools: &[&str]) -> Staff {
    Staff {
        id: id.into(),
        name: id.into(),
        copies: 1,
        status: "ACTIVE".into(),
        roles: roles.iter().map(|r| r.to_string()).collect(),
        school_ids: schools.iter().map(|s| s.to_string()).collect(),
        tenant_ids: if schools.is_empty() {
            vec![]
        } else {
            vec!["tenant".into()]
        },
        team_ids: vec![],
    }
}
fn section(id: &str, school: &str, grade: &str, homeroom: Option<&str>) -> Section {
    Section {
        id: id.into(),
        school: school.into(),
        name: id.into(),
        grade: grade.into(),
        year: "2026/2027".into(),
        homeroom: homeroom.map(Into::into),
    }
}
fn facts() -> Facts {
    Facts {
        tenant: "tenant".into(),
        today: chrono::NaiveDate::from_ymd_opt(2026, 10, 28),
        staff: vec![
            staff("owner", &["owner"], &[]),
            staff("owner-a", &["owner"], &["school"]),
            staff("admin", &["school_admin"], &["school"]),
            staff("homeroom", &["teacher"], &["school"]),
            staff("assigned", &["teacher"], &["school"]),
            staff("granted", &["teacher"], &["school"]),
            staff("revoked", &["teacher"], &["school"]),
        ],
        schools: BTreeMap::from([
            ("school".to_string(), "IISS".to_string()),
            ("school-2".to_string(), "IIHS".to_string()),
        ]),
        sections: vec![
            section("7A", "school", "Year 7", Some("homeroom")),
            section("7B", "school", "Year 7", None),
            section("9A", "school", "Year 9", None),
            section("10A", "school-2", "Year 10", None),
        ],
        grants: vec![
            ("assigned".into(), "7B".into(), "school".into()),
            ("granted".into(), "9A".into(), "school".into()),
        ],
        teaching: vec![
            Teaching {
                id: "ta-1".into(),
                staff: "assigned".into(),
                section: "7B".into(),
                school: "school".into(),
                subject: "mathematics".into(),
            },
            // An assignment whose grant was revoked by hand: no access, as today.
            Teaching {
                id: "ta-2".into(),
                staff: "revoked".into(),
                section: "7B".into(),
                school: "school".into(),
                subject: "science".into(),
            },
        ],
        assignments: vec![],
        templates: reference().seeds.iter().map(Template::from_seed).collect(),
    }
}
fn allowed(f: &Facts, who: &str, capability: &str, scope: &Scope) -> bool {
    decide(f, &f.holdings(who), capability, scope).is_some()
}

#[test]
fn authorization_catalogue_and_seeds_load_and_keep_manage_with_owners() {
    let r = load().expect("catalogue and seeds are valid");
    assert!(r.capability(MANAGE).is_some());
    assert!(r.sensitive(MANAGE));
    assert!(r.sensitive("student.biodata.manage"));
    assert!(!r.sensitive(READ));
    assert!(
        r.sensitive("not.a.capability"),
        "unknown counts as sensitive"
    );
    assert!(r.class_wide("learning.class.teach") && r.class_wide(HOMEROOM));
    assert!(!r.class_wide("learning.module.review"));
    let codes: Vec<&str> = r.seeds.iter().map(|s| s.code.as_str()).collect();
    assert_eq!(
        codes,
        [
            "regular_teacher",
            "admin_teacher",
            "subject_lead",
            "curriculum_coordinator",
            "school_admin",
            "principal"
        ]
    );
    // `authorization.manage` is carried by the Principal seed alone, at `SCHOOL` only (spec §10
    // #8.3); the school administrator reads access in their schools.
    for seed in &r.seeds {
        assert_eq!(
            seed.capabilities.iter().any(|c| c == MANAGE),
            seed.code == PRINCIPAL,
            "{}",
            seed.code
        );
    }
    let principal = r.seeds.iter().find(|s| s.code == PRINCIPAL).unwrap();
    assert_eq!(principal.allowed_scopes, [SCHOOL]);
    for code in [
        MANAGE,
        READ,
        "academic.staffing.read",
        "learning.curriculum.read",
        MASTERY_READ,
    ] {
        assert!(principal.capabilities.iter().any(|c| c == code), "{code}");
    }
    assert!(r
        .seeds
        .iter()
        .find(|s| s.code == SCHOOL_ADMIN)
        .unwrap()
        .capabilities
        .iter()
        .any(|c| c == READ));
    // Admin teacher is Regular teacher plus review, manage-any and the staffing read (spec §3).
    let regular: Vec<&String> = r.seeds[0].capabilities.iter().collect();
    let admin = &r.seeds[1].capabilities;
    assert!(regular.iter().all(|c| admin.contains(c)));
    for extra in [
        "learning.module.review",
        "learning.module.manage_any",
        "academic.staffing.read",
    ] {
        assert!(admin.iter().any(|c| c == extra));
    }
}

#[test]
fn authorization_codes_and_scopes_are_validated() {
    assert!(capability_code("learning.module.review"));
    assert!(capability_code("authorization.manage"));
    assert!(!capability_code("learning"));
    assert!(!capability_code("Learning.module.review"));
    assert!(!capability_code("learning.module.review.extra"));
    assert!(template_code("admin_teacher"));
    for bad in ["owner", "student", "parent", "homeroom", "A", "1x", "x-y"] {
        assert!(!template_code(bad), "{bad}");
    }
    assert!(Scope::tenant().valid());
    assert!(Scope::school("school").valid());
    assert!(Scope::section_subject("school", "7A", "mathematics").valid());
    let mut subject = Scope::school("school");
    subject.kind = SCHOOL_SUBJECT.into();
    assert!(!subject.valid(), "a subject scope names its subject");
    subject.subject = Some("mathematics".into());
    subject.grade_from = Some("Year 7".into());
    subject.grade_to = Some("Year 9".into());
    assert!(subject.valid());
    subject.grade_from = Some("Year 10".into());
    assert!(!subject.valid(), "from after to");
    subject.grade_from = Some("Kelas".into());
    assert!(!subject.valid(), "a grade bound has a number");
    let mut graded_section = Scope::section("school", "7A");
    graded_section.grade_from = Some("Year 7".into());
    assert!(
        !graded_section.valid(),
        "grade ranges only on SCHOOL_SUBJECT"
    );
    let mut tenant_with_school = Scope::tenant();
    tenant_with_school.school_id = Some("school".into());
    assert!(!tenant_with_school.valid());
}

#[test]
fn authorization_scope_containment() {
    let school = Scope::school("school");
    let class = Scope::section("school", "7A");
    let math_7a = Scope::section_subject("school", "7A", "mathematics");
    let science_7a = Scope::section_subject("school", "7A", "science");
    let mut math = Scope::school("school");
    math.kind = SCHOOL_SUBJECT.into();
    math.subject = Some("mathematics".into());
    let mut math_7_8 = math.clone();
    math_7_8.grade_from = Some("Year 7".into());
    math_7_8.grade_to = Some("Year 8".into());
    let y7 = Some("Year 7");
    let y9 = Some("Year 9");

    assert!(covers(&Scope::tenant(), &school, None, false));
    assert!(covers(&school, &math_7a, y7, false));
    assert!(!covers(&school, &Scope::school("school-2"), None, false));
    assert!(
        !covers(&school, &Scope::tenant(), None, false),
        "never wider than held"
    );
    assert!(covers(&class, &math_7a, y7, false));
    assert!(!covers(&class, &school, None, false));
    assert!(covers(&math_7a, &math_7a, y7, false));
    assert!(!covers(&math_7a, &science_7a, y7, false));
    assert!(
        !covers(&math_7a, &class, y7, false),
        "one subject is not the class"
    );
    assert!(
        covers(&math_7a, &class, y7, true),
        "unless the capability is class-wide"
    );
    assert!(covers(&math, &math_7a, y9, false));
    assert!(covers(&math_7_8, &math_7a, y7, false));
    assert!(!covers(&math_7_8, &math_7a, y9, false));
    assert!(
        !covers(&math_7_8, &math_7a, None, false),
        "an unknown grade is outside a range"
    );
    assert!(covers(&math, &math_7_8, None, false));
    assert!(!covers(&math_7_8, &math, None, false));
    assert!(!covers(&math, &class, y7, false));
    assert!(covers(&math, &class, y7, true));
    assert!(!covers(&math, &school, None, true));
}

/// Day one: the holdings built from today's records give exactly today's answers.
#[test]
fn authorization_day_one_access_equals_todays_rules() {
    let f = facts();
    let teach = "learning.class.teach";
    let class = |id: &str| Scope::section("school", id);
    // Owner: everything within its schools; an unscoped Owner within every school.
    for owner in ["owner", "owner-a"] {
        assert!(allowed(&f, owner, MANAGE, &Scope::school("school")));
        assert!(allowed(&f, owner, teach, &class("9A")));
        assert!(allowed(&f, owner, HOMEROOM, &class("7B")));
        assert!(allowed(
            &f,
            owner,
            "student.biodata.manage",
            &Scope::school("school")
        ));
    }
    assert!(allowed(
        &f,
        "owner",
        teach,
        &Scope::section("school-2", "10A")
    ));
    assert!(!allowed(&f, "owner-a", MANAGE, &Scope::school("school-2")));
    // School admin: the School admin template at its school, no teaching, no access management.
    assert!(allowed(
        &f,
        "admin",
        "academic.staffing.manage",
        &Scope::school("school")
    ));
    assert!(allowed(
        &f,
        "admin",
        "student.biodata.manage",
        &Scope::school("school")
    ));
    assert!(allowed(&f, "admin", READ, &Scope::school("school")));
    assert!(!allowed(&f, "admin", MANAGE, &Scope::school("school")));
    assert!(!allowed(
        &f,
        "admin",
        "academic.staffing.manage",
        &Scope::school("school-2")
    ));
    assert!(!allowed(&f, "admin", teach, &class("7A")));
    // Teachers: teach where the grant is, homeroom only as the homeroom, nothing else.
    assert!(allowed(&f, "assigned", teach, &class("7B")));
    assert!(allowed(
        &f,
        "assigned",
        "learning.gradebook.read",
        &Scope::section_subject("school", "7B", "mathematics")
    ));
    assert!(!allowed(
        &f,
        "assigned",
        "learning.gradebook.read",
        &Scope::section_subject("school", "7B", "science")
    ));
    assert!(!allowed(&f, "assigned", HOMEROOM, &class("7B")));
    assert!(!allowed(&f, "assigned", teach, &class("7A")));
    assert!(allowed(&f, "granted", teach, &class("9A")));
    assert!(allowed(
        &f,
        "granted",
        "learning.gradebook.read",
        &Scope::section_subject("school", "9A", "art")
    ));
    assert!(!allowed(&f, "granted", HOMEROOM, &class("9A")));
    assert!(allowed(&f, "homeroom", teach, &class("7A")));
    assert!(allowed(&f, "homeroom", HOMEROOM, &class("7A")));
    assert!(!allowed(&f, "homeroom", teach, &class("7B")));
    assert!(
        !allowed(&f, "revoked", teach, &class("7B")),
        "revoked grant, open assignment"
    );
    for teacher in ["assigned", "granted", "homeroom"] {
        assert!(!allowed(
            &f,
            teacher,
            "academic.staffing.manage",
            &Scope::school("school")
        ));
        assert!(!allowed(&f, teacher, READ, &Scope::school("school")));
    }
    // Unknown capabilities, scopes that do not exist and unknown people are denied.
    assert!(!allowed(
        &f,
        "owner",
        "learning.unknown.thing",
        &class("7A")
    ));
    assert!(!allowed(&f, "owner", teach, &class("NOPE")));
    assert!(!allowed(&f, "nobody", teach, &class("7A")));
}

#[test]
fn authorization_inactive_duplicated_or_teamed_people_hold_nothing() {
    let mut f = facts();
    f.staff[3].status = "DISABLED".into();
    f.staff[4].copies = 2;
    f.staff[1].team_ids = vec!["team".into()];
    assert!(f.holdings("homeroom").is_empty());
    assert!(f.holdings("assigned").is_empty());
    assert!(f.holdings("owner-a").is_empty());
}

fn explicit(id: &str, staff: &str, template: &str, scope: Scope) -> Assignment {
    Assignment {
        id: id.into(),
        staff_member_id: staff.into(),
        template_code: template.into(),
        scope,
        status: "active".into(),
        version: 1,
        ..Assignment::default()
    }
}

#[test]
fn authorization_explicit_assignments_follow_dates_templates_and_membership() {
    let mut f = facts();
    let mut lead = Scope::school("school");
    lead.kind = SCHOOL_SUBJECT.into();
    lead.subject = Some("mathematics".into());
    f.assignments = vec![
        explicit("a1", "granted", "subject_lead", lead),
        explicit("a2", "admin", SCHOOL_ADMIN, Scope::school("school")),
    ];
    let math_9a = Scope::section_subject("school", "9A", "mathematics");
    let h = f.holdings("granted");
    let found = decide(&f, &h, "learning.module.review", &math_9a).unwrap();
    assert_eq!(found.source.kind, "assignment");
    assert_eq!(found.source.id.as_deref(), Some("a1"));
    assert_eq!(found.source.template_version, Some(1));
    assert!(!allowed(
        &f,
        "granted",
        "learning.module.review",
        &Scope::section_subject("school", "9A", "art")
    ));
    // An explicit School admin assignment replaces the derived one at that school.
    let admin = f.holdings("admin");
    assert_eq!(admin.len(), 1);
    assert_eq!(admin[0].source.kind, "assignment");
    // Dates, archived templates and leaving the school end an assignment.
    f.assignments[0].valid_until = Some("2026-10-27".into());
    assert!(!allowed(&f, "granted", "learning.module.review", &math_9a));
    f.assignments[0].valid_until = None;
    f.assignments[0].valid_from = Some("2026-10-29".into());
    assert!(!allowed(&f, "granted", "learning.module.review", &math_9a));
    f.assignments[0].valid_from = Some("2026-10-28".into());
    assert!(allowed(&f, "granted", "learning.module.review", &math_9a));
    f.templates
        .iter_mut()
        .find(|t| t.code == "subject_lead")
        .unwrap()
        .status = "archived".into();
    assert!(!allowed(&f, "granted", "learning.module.review", &math_9a));
    f.templates
        .iter_mut()
        .find(|t| t.code == "subject_lead")
        .unwrap()
        .status = "active".into();
    f.staff[5].school_ids = vec!["school-2".into()];
    assert!(!allowed(&f, "granted", "learning.module.review", &math_9a));
    // Explicit assignments never add to an Owner: today's checks never combine roles.
    f.assignments.push(explicit(
        "a3",
        "owner-a",
        "subject_lead",
        Scope::school("school"),
    ));
    assert!(f
        .holdings("owner-a")
        .iter()
        .all(|h| h.source.kind == "owner"));
}

#[test]
fn authorization_edited_templates_change_derived_access() {
    let mut f = facts();
    let t = f
        .templates
        .iter_mut()
        .find(|t| t.code == REGULAR_TEACHER)
        .unwrap();
    t.capabilities.retain(|c| c != "learning.gradebook.read");
    t.version = 2;
    let h = f.holdings("granted");
    assert!(h.iter().all(|h| h.source.template_version == Some(2)));
    assert!(!allowed(
        &f,
        "granted",
        "learning.gradebook.read",
        &Scope::section("school", "9A")
    ));
    assert!(allowed(
        &f,
        "granted",
        "learning.class.teach",
        &Scope::section("school", "9A")
    ));
}

#[test]
fn authorization_tenant_coverage_and_reading() {
    let f = facts();
    assert!(holds_tenant(&f, &f.holdings("owner"), MANAGE));
    assert!(
        !holds_tenant(&f, &f.holdings("owner-a"), MANAGE),
        "one of two schools"
    );
    assert!(owner_at(
        &f,
        &f.holdings("owner-a"),
        &Scope::school("school")
    ));
    assert!(!owner_at(
        &f,
        &f.holdings("admin"),
        &Scope::school("school")
    ));
    assert_eq!(
        readable_schools(&f, &f.holdings("admin"))
            .into_iter()
            .collect::<Vec<_>>(),
        ["school"]
    );
    assert!(readable_schools(&f, &f.holdings("granted")).is_empty());
    assert_eq!(readable_schools(&f, &f.holdings("owner")).len(), 2);
}

#[test]
fn authorization_commands_are_validated() {
    let assign = |scope: Scope| AssignmentCommand {
        action: ASSIGN.into(),
        assignment_id: None,
        staff_member_id: Some("t1".into()),
        template_code: Some("subject_lead".into()),
        scope: Some(scope),
        academic_year: Some("2026/2027".into()),
        valid_from: Some("2026-07-01".into()),
        valid_until: Some("2027-06-30".into()),
        expected_version: 0,
    };
    let ok = assign(Scope::school("school"));
    assert!(ok.valid());
    let mut backwards = ok.clone();
    backwards.valid_until = Some("2026-01-01".into());
    assert!(!backwards.valid());
    let mut bad_date = ok.clone();
    bad_date.valid_from = Some("2026-13-01".into());
    assert!(!bad_date.valid());
    let revoke = AssignmentCommand {
        action: REVOKE.into(),
        assignment_id: Some("id-1".into()),
        staff_member_id: None,
        template_code: None,
        scope: None,
        academic_year: None,
        valid_from: None,
        valid_until: None,
        expected_version: 1,
    };
    assert!(revoke.valid());
    let mut revoke_with_scope = revoke.clone();
    revoke_with_scope.scope = Some(Scope::school("school"));
    assert!(!revoke_with_scope.valid());
    let batch = |commands: Vec<AssignmentCommand>| AssignmentCommands {
        tenant_id: "tenant".into(),
        commands,
    };
    assert!(batch(vec![ok.clone(), revoke.clone()]).valid());
    assert!(
        !batch(vec![ok.clone(), ok.clone()]).valid(),
        "one command per target"
    );
    assert!(!batch(vec![]).valid());
    assert!(!batch(vec![revoke.clone(); MAX_COMMANDS + 1]).valid());

    let template = TemplateCommand {
        action: CREATE.into(),
        code: "access_manager".into(),
        name: Some(Text {
            id: "Pengelola akses".into(),
            en: "Access manager".into(),
        }),
        capabilities: Some(vec![MANAGE.into(), READ.into()]),
        allowed_scopes: Some(vec![SCHOOL.into()]),
        expected_version: 0,
    };
    assert!(template.valid());
    let mut duplicate = template.clone();
    duplicate.capabilities = Some(vec![READ.into(), READ.into()]);
    assert!(!duplicate.valid());
    let mut no_scopes = template.clone();
    no_scopes.allowed_scopes = Some(vec![]);
    assert!(!no_scopes.valid());
    let mut archive = template.clone();
    archive.action = ARCHIVE.into();
    assert!(!archive.valid(), "archive names only the code and version");
    let mut reserved = template.clone();
    reserved.code = "owner".into();
    assert!(!reserved.valid());

    let check = CheckRequest {
        tenant_id: "tenant".into(),
        actor: None,
        capability: "learning.made.up".into(),
        scope: Scope::section("school", "7A"),
    };
    assert!(
        check.valid(),
        "an unknown capability is answered with a deny"
    );
}

#[test]
fn authorization_validity_defaults_to_the_end_of_the_academic_year() {
    let date = |y, m, d| chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap();
    assert_eq!(
        academic_year_span("2026/2027"),
        Some((date(2026, 7, 1), date(2027, 6, 30)))
    );
    assert_eq!(
        academic_year_span("2026-2027"),
        Some((date(2026, 7, 1), date(2027, 6, 30)))
    );
    for odd in [
        "2026/2028",
        "2026",
        "26/27",
        "TA 2026/2027",
        "abcd/efgh",
        "",
    ] {
        assert_eq!(academic_year_span(odd), None, "{odd}");
    }

    let mut f = facts();
    let school = Scope::school("school");
    let until = |f: &Facts, template: &str, year: Option<&str>, from: Option<&str>| {
        f.validity(template, &school, year, from, None)
    };
    // The year of the school's active classes that holds today.
    let v = until(&f, "subject_lead", None, None);
    assert_eq!(v.valid_until.as_deref(), Some("2027-06-30"));
    assert_eq!(
        (v.rule, v.academic_year.as_deref(), v.source),
        ("academic_year_end", Some("2026/2027"), Some("classes"))
    );
    // A start date in the next year picks that year; there are no classes yet, so the calendar.
    let v = until(&f, "subject_lead", None, Some("2027-08-01"));
    assert_eq!(v.valid_until.as_deref(), Some("2028-06-30"));
    assert_eq!(
        (v.academic_year.as_deref(), v.source),
        (Some("2027/2028"), Some("calendar"))
    );
    // The year the command names, unless it is already over.
    let v = until(&f, "school_admin", Some("2027/2028"), None);
    assert_eq!(
        (v.valid_until.as_deref(), v.source),
        (Some("2028-06-30"), Some("command"))
    );
    let v = until(&f, "school_admin", Some("2025/2026"), None);
    assert_eq!(
        (v.valid_until.as_deref(), v.source),
        (Some("2027-06-30"), Some("classes"))
    );
    // Principal and Curriculum coordinator run until revoked; an explicit end always wins.
    for template in [PRINCIPAL, CURRICULUM_COORDINATOR] {
        let v = until(&f, template, None, None);
        assert_eq!((v.valid_until, v.rule), (None, "until_revoked"));
    }
    let v = f.validity(PRINCIPAL, &school, None, None, Some("2026-12-31"));
    assert_eq!(
        (v.valid_until.as_deref(), v.rule),
        (Some("2026-12-31"), "explicit")
    );
    // Without classes in a parseable year: the calendar year from 1 July.
    for s in &mut f.sections {
        s.year = "TA 26/27".into();
    }
    f.today = Some(date(2027, 3, 1));
    let v = until(&f, "subject_lead", None, None);
    assert_eq!(
        (
            v.valid_until.as_deref(),
            v.academic_year.as_deref(),
            v.source
        ),
        (Some("2027-06-30"), Some("2026/2027"), Some("calendar"))
    );
    let json = serde_json::to_value(&v).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"rule": "academic_year_end", "academicYear": "2026/2027", "source": "calendar"})
    );
}
