use super::commands::{self, Session, Writer};
use super::facts::{self, Db};
use super::model::*;
use super::reads::{self, Reader};
use crate::academic_staffing::repository::Failure;
use neo4rs::{query, Graph};
use serde_json::Value;

fn writer<'a>(actor: &'a str, role: &'a str, command: &'a str) -> Writer<'a> {
    Writer {
        session: Session::Unchecked,
        actor,
        role,
        tenant: "tenant",
        command,
    }
}
fn code(result: Result<Value, Failure>) -> String {
    let (_, body) = result.expect_err("refused");
    body.0["error"]["code"].as_str().unwrap().to_string()
}
fn reader(actor: &str) -> Reader {
    Reader {
        actor: actor.into(),
        schools: None,
    }
}
fn scoped_reader(actor: &str, schools: &[&str]) -> Reader {
    Reader {
        actor: actor.into(),
        schools: Some(schools.iter().map(|s| s.to_string()).collect()),
    }
}
async fn count(g: &Graph, cypher: &str) -> i64 {
    let mut rows = g.execute(query(cypher)).await.unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>("n").unwrap()
}
async fn today(g: &Graph, cypher: &str, actor: &str, school: &str, class: &str) -> bool {
    let mut rows = g
        .execute(
            query(cypher)
                .param("principal", actor)
                .param("actor", actor)
                .param("school", school)
                .param("tenant", "tenant")
                .param("class", class),
        )
        .await
        .unwrap();
    rows.next().await.unwrap().unwrap().get::<i64>("n").unwrap() > 0
}
async fn new(g: &Graph, actor: &str, capability: &str, scope: &Scope) -> bool {
    let sections: Vec<String> = scope.section_id.iter().cloned().collect();
    let f = facts::load(&mut Db::Graph(g), "tenant", &[actor.to_string()], &sections)
        .await
        .unwrap();
    decide(&f, &f.holdings(actor), capability, scope).is_some()
}
fn subject_scope(subject: &str) -> Scope {
    Scope {
        kind: SCHOOL_SUBJECT.into(),
        school_id: Some("school".into()),
        subject: Some(subject.into()),
        ..Scope::default()
    }
}
fn assign(staff: &str, template: &str, scope: Scope, version: u32) -> AssignmentCommand {
    AssignmentCommand {
        action: ASSIGN.into(),
        assignment_id: None,
        staff_member_id: Some(staff.into()),
        template_code: Some(template.into()),
        scope: Some(scope),
        academic_year: None,
        valid_from: None,
        valid_until: None,
        expected_version: version,
    }
}
fn by_id(action: &str, id: &str, version: u32) -> AssignmentCommand {
    AssignmentCommand {
        action: action.into(),
        assignment_id: Some(id.into()),
        staff_member_id: None,
        template_code: None,
        scope: None,
        academic_year: None,
        valid_from: None,
        valid_until: None,
        expected_version: version,
    }
}
fn batch(commands: Vec<AssignmentCommand>) -> AssignmentCommands {
    AssignmentCommands {
        tenant_id: "tenant".into(),
        commands,
    }
}
fn template(
    action: &str,
    code: &str,
    caps: &[&str],
    scopes: &[&str],
    version: u32,
) -> TemplateCommands {
    TemplateCommands {
        tenant_id: "tenant".into(),
        commands: vec![TemplateCommand {
            action: action.into(),
            code: code.into(),
            name: (action != ARCHIVE).then(|| Text {
                id: format!("Nama {code}"),
                en: format!("Name {code}"),
            }),
            capabilities: (action != ARCHIVE).then(|| caps.iter().map(|c| c.to_string()).collect()),
            allowed_scopes: (action != ARCHIVE)
                .then(|| scopes.iter().map(|c| c.to_string()).collect()),
            expected_version: version,
        }],
    }
}

async fn fixture() -> Graph {
    let uri = std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap();
    assert_eq!(uri, "127.0.0.1:3223");
    let g = Graph::new(uri, "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
    super::init(&g).await.unwrap();
    super::init(&g).await.unwrap();
    for statement in [
        "CREATE(:School {school_id:'school',tenant_id:'tenant',name:'IISS'}) CREATE(:School {school_id:'school-2',tenant_id:'tenant',name:'IIHS'})",
        "CREATE(:StaffMember {id:'owner',membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[]}) CREATE(:StaffMember {id:'owner-a',membershipStatus:'ACTIVE',roles:['owner'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'owner-team',membershipStatus:'ACTIVE',roles:['owner'],schoolIds:['school'],tenantIds:['tenant'],teamIds:['team']})",
        "CREATE(:StaffMember {id:'admin',displayName:'Ayu Admin',membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'admin-2',membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['school-2'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'admin-off',membershipStatus:'DISABLED',roles:['school_admin'],schoolIds:['school'],tenantIds:['tenant']})",
        "CREATE(:StaffMember {id:'t-home',displayName:'Siti Rahmawati',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t-assigned',displayName:'Budi Santoso',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t-grant',displayName:'Dewi Lestari',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t-revoked',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t-off',membershipStatus:'DISABLED',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t-other',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school-2'],tenantIds:['tenant']})",
        "CREATE(:Section {section_id:'7A',name:'7A',year_group:'Year 7',academic_year:'2026/2027',school_id:'school',tenant_id:'tenant',status:'active',homeroom_staff_member_id:'t-home'}) CREATE(:Section {section_id:'7B',name:'7B',year_group:'Year 7',academic_year:'2026/2027',school_id:'school',tenant_id:'tenant',status:'active'}) CREATE(:Section {section_id:'9A',name:'9A',year_group:'Year 9',academic_year:'2026/2027',school_id:'school',tenant_id:'tenant',status:'active'}) CREATE(:Section {section_id:'OLD',name:'7A',year_group:'Year 7',academic_year:'2025/2026',school_id:'school',tenant_id:'tenant',status:'archived',homeroom_staff_member_id:'t-grant'}) CREATE(:Section {section_id:'10A',name:'10A',year_group:'Year 10',academic_year:'2026/2027',school_id:'school-2',tenant_id:'tenant',status:'active'})",
        "CREATE(:TeachingAssignment {id:'ta-1',key:'k1',staff_member_id:'t-assigned',section_id:'7B',school_id:'school',tenant_id:'tenant',academic_year:'2026/2027',subject:'mathematics',status:'active',periods:4,version:1}) CREATE(:LearningTeachingGrant {key:'t-assigned|7B',staff_member_id:'t-assigned',section_id:'7B',school_id:'school',tenant_id:'tenant',status:'ACTIVE',version:1})",
        "CREATE(:TeachingAssignment {id:'ta-2',key:'k2',staff_member_id:'t-revoked',section_id:'7B',school_id:'school',tenant_id:'tenant',academic_year:'2026/2027',subject:'science',status:'active',periods:2,version:1}) CREATE(:LearningTeachingGrant {key:'t-revoked|7B',staff_member_id:'t-revoked',section_id:'7B',school_id:'school',tenant_id:'tenant',status:'REVOKED',version:2})",
        "CREATE(:LearningTeachingGrant {key:'t-grant|9A',staff_member_id:'t-grant',section_id:'9A',school_id:'school',tenant_id:'tenant',status:'ACTIVE',version:1}) CREATE(:LearningTeachingGrant {key:'t-grant|OLD',staff_member_id:'t-grant',section_id:'OLD',school_id:'school',tenant_id:'tenant',status:'ACTIVE',version:1}) CREATE(:LearningTeachingGrant {key:'t-off|9A',staff_member_id:'t-off',section_id:'9A',school_id:'school',tenant_id:'tenant',status:'ACTIVE',version:1}) CREATE(:LearningTeachingGrant {key:'t-other|7A',staff_member_id:'t-other',section_id:'7A',school_id:'school',tenant_id:'tenant',status:'ACTIVE',version:1})",
    ] {
        g.run(query(statement)).await.unwrap();
    }
    g
}

const PEOPLE: [&str; 12] = [
    "owner",
    "owner-a",
    "owner-team",
    "admin",
    "admin-2",
    "admin-off",
    "t-home",
    "t-assigned",
    "t-grant",
    "t-revoked",
    "t-off",
    "t-other",
];
const CLASSES: [(&str, &str); 5] = [
    ("school", "7A"),
    ("school", "7B"),
    ("school", "9A"),
    ("school", "OLD"),
    ("school-2", "10A"),
];

/// Day one: every new decision equals today's Cypher check, person by person and class by
/// class: Learning `teach` and `homeroom` (owner context) and school administration.
#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn authorization_graph_day_one_equals_todays_checks() {
    let g = fixture().await;
    let owner = crate::school_portal::staff_capability::owner("p", "$school", "$tenant");
    let owner_teach = format!("MATCH(p:StaffMember {{id:$principal,membershipStatus:'ACTIVE'}}) WHERE {owner} MATCH(c:Section {{section_id:$class,school_id:$school,tenant_id:$tenant,status:'active'}}) RETURN count(c) AS n");
    let teacher_teach = format!(
        "{} WITH DISTINCT c,p WHERE c.section_id=$class RETURN count(c) AS n",
        crate::learning_access::owner::TEACHER
    );
    let teacher_homeroom = format!(
        "{} WITH DISTINCT c,p WHERE c.section_id=$class AND c.homeroom_staff_member_id=p.id RETURN count(c) AS n",
        crate::learning_access::owner::TEACHER
    );
    let admin = format!(
        "{} RETURN count(a) AS n",
        crate::academic_teachers::repository::authorized("$school", "$tenant")
    );
    let mut compared = 0;
    for person in PEOPLE {
        for (school, class) in CLASSES {
            let scope = Scope::section(school, class);
            let is_owner = today(&g, &owner_teach, person, school, class).await;
            let teach = is_owner || today(&g, &teacher_teach, person, school, class).await;
            let homeroom = is_owner || today(&g, &teacher_homeroom, person, school, class).await;
            assert_eq!(
                new(&g, person, "learning.class.teach", &scope).await,
                teach,
                "{person} teach {class}"
            );
            assert_eq!(
                new(&g, person, HOMEROOM, &scope).await,
                homeroom,
                "{person} homeroom {class}"
            );
            compared += 2;
        }
        for school in ["school", "school-2"] {
            let scope = Scope::school(school);
            let administers = today(&g, &admin, person, school, "").await;
            for capability in [
                "academic.staffing.manage",
                "academic.subject_plan.manage",
                "student.biodata.manage",
            ] {
                assert_eq!(
                    new(&g, person, capability, &scope).await,
                    administers,
                    "{person} {capability} {school}"
                );
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 12 * 5 * 2 + 12 * 2 * 3);
    // The personas the ADR names, spelled out.
    assert!(new(&g, "owner", MANAGE, &Scope::school("school-2")).await);
    assert!(
        new(
            &g,
            "admin",
            "academic.staffing.manage",
            &Scope::school("school")
        )
        .await
    );
    assert!(
        new(
            &g,
            "t-assigned",
            "learning.class.teach",
            &Scope::section("school", "7B")
        )
        .await
    );
    assert!(
        new(
            &g,
            "t-grant",
            "learning.class.teach",
            &Scope::section("school", "9A")
        )
        .await
    );
    assert!(new(&g, "t-home", HOMEROOM, &Scope::section("school", "7A")).await);
    assert!(!new(&g, "t-home", HOMEROOM, &Scope::section("school", "7B")).await);

    // The Learning authorize answer gains the class's `learning.*` capabilities.
    let caps = reads::learning_capabilities(&g, "tenant", "school", "7A", "t-home")
        .await
        .unwrap();
    assert!(
        caps.contains(&"learning.class.teach".to_string()) && caps.contains(&HOMEROOM.to_string())
    );
    let caps = reads::learning_capabilities(&g, "tenant", "school", "9A", "t-grant")
        .await
        .unwrap();
    assert!(
        caps.contains(&"learning.module.write".to_string())
            && !caps.contains(&HOMEROOM.to_string())
    );
    assert!(!caps.contains(&"learning.module.review".to_string()));
    let caps = reads::learning_capabilities(&g, "tenant", "school", "9A", "owner")
        .await
        .unwrap();
    assert_eq!(
        caps.len(),
        reference()
            .catalogue
            .capabilities
            .iter()
            .filter(|c| c.code.starts_with("learning."))
            .count()
    );
    assert!(
        reads::learning_capabilities(&g, "tenant", "school", "7B", "t-revoked")
            .await
            .unwrap()
            .is_empty()
    );

    // Deliberately narrower than today: a duplicated Staff Member node holds nothing (fail closed,
    // as the Owner and school-admin checks already do).
    g.run(query("CREATE(:StaffMember {id:'t-grant',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']})")).await.unwrap();
    assert!(
        !new(
            &g,
            "t-grant",
            "learning.class.teach",
            &Scope::section("school", "9A")
        )
        .await
    );
}

#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn authorization_graph_commands_guardrails_reads_and_audit() {
    let g = fixture().await;
    let math_7b = Scope::section_subject("school", "7B", "mathematics");

    // Seeded templates read as version 1 with their holders counted; nobody else reads.
    let listed = reads::templates(&g, &reader("owner"), "tenant")
        .await
        .unwrap();
    let regular = listed["templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["code"] == REGULAR_TEACHER)
        .unwrap();
    assert_eq!(
        (regular["version"].as_i64(), regular["seeded"].as_bool()),
        (Some(1), Some(true))
    );
    assert_eq!(
        regular["people"], 3,
        "homeroom, assignment and grant holders"
    );
    assert_eq!(
        code(reads::templates(&g, &reader("t-grant"), "tenant").await),
        "FORBIDDEN"
    );

    // A school admin reads but cannot change access.
    assert_eq!(
        code(
            commands::assignments(
                &g,
                &writer("admin", "school_admin", "admin-assign-01"),
                &batch(vec![assign(
                    "t-grant",
                    "subject_lead",
                    subject_scope("mathematics"),
                    0
                )])
            )
            .await
        ),
        "FORBIDDEN"
    );
    assert_eq!(
        code(
            commands::templates(
                &g,
                &writer("admin", "school_admin", "admin-template-01"),
                &template(CREATE, "helper", &[READ], &[SCHOOL], 0)
            )
            .await
        ),
        "FORBIDDEN"
    );

    // An Owner assigns; replay, key reuse and stale revisions behave as everywhere in SIS.
    let lead = batch(vec![assign(
        "t-grant",
        "subject_lead",
        subject_scope("mathematics"),
        0,
    )]);
    let saved = commands::assignments(&g, &writer("owner", "owner", "owner-lead-0001"), &lead)
        .await
        .unwrap();
    let lead_id = saved["assignments"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(saved["assignments"][0]["version"], 1);
    assert_eq!(saved["authorizationVersion"], 1);
    let replay = commands::assignments(&g, &writer("owner", "owner", "owner-lead-0001"), &lead)
        .await
        .unwrap();
    assert_eq!(replay, saved);
    assert_eq!(
        code(
            commands::assignments(
                &g,
                &writer("owner", "owner", "owner-lead-0001"),
                &batch(vec![assign(
                    "t-home",
                    "subject_lead",
                    subject_scope("science"),
                    0
                )])
            )
            .await
        ),
        "IDEMPOTENCY_KEY_REUSED"
    );
    assert_eq!(
        code(commands::assignments(&g, &writer("owner", "owner", "owner-lead-0002"), &lead).await),
        "REVISION_CONFLICT"
    );
    let check = |actor: Option<&str>, capability: &str, scope: Scope| CheckRequest {
        tenant_id: "tenant".into(),
        actor: actor.map(Into::into),
        capability: capability.into(),
        scope,
    };
    let answer = reads::check(
        &g,
        &reader("owner"),
        &check(Some("t-grant"), "learning.module.review", math_7b.clone()),
    )
    .await
    .unwrap();
    assert_eq!(answer["allowed"], true);
    assert_eq!(answer["source"]["kind"], "assignment");
    assert_eq!(answer["source"]["id"], lead_id.as_str());
    assert_eq!(answer["templateVersion"], 1);
    assert_eq!(answer["authorizationVersion"], 1);
    let science = reads::check(
        &g,
        &reader("owner"),
        &check(
            Some("t-grant"),
            "learning.module.review",
            Scope::section_subject("school", "7B", "science"),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        (science["allowed"].as_bool(), science["reason"].as_str()),
        (Some(false), Some("NOT_IN_SCOPE"))
    );
    assert_eq!(science["heldAt"].as_array().unwrap().len(), 1);
    let unknown = reads::check(
        &g,
        &reader("owner"),
        &check(Some("t-grant"), "learning.made.up", math_7b.clone()),
    )
    .await
    .unwrap();
    assert_eq!(unknown["reason"], "UNKNOWN_CAPABILITY");

    // Each refusal names its command, and nothing of a refused batch is written.
    let before = count(&g, "MATCH(a:AuthorizationAssignment) RETURN count(a) AS n").await;
    for (command, expected) in [
        (
            assign("t-home", "subject_lead", Scope::school("school"), 0),
            "SCOPE_NOT_ALLOWED",
        ),
        (
            assign("t-home", "nope_template", Scope::school("school"), 0),
            "TEMPLATE_NOT_FOUND",
        ),
        (
            assign("t-home", "admin_teacher", Scope::section("school", "8Z"), 0),
            "SCOPE_NOT_FOUND",
        ),
        (
            assign("t-other", "admin_teacher", Scope::school("school"), 0),
            "TARGET_NOT_ELIGIBLE",
        ),
        (
            assign("owner-a", "admin_teacher", Scope::school("school"), 0),
            "TARGET_NOT_ELIGIBLE",
        ),
        (
            assign("t-off", "admin_teacher", Scope::school("school"), 0),
            "TARGET_NOT_ELIGIBLE",
        ),
    ] {
        let refused = commands::assignments(
            &g,
            &writer("owner", "owner", &format!("refused-{expected}")),
            &batch(vec![
                assign("t-home", "admin_teacher", Scope::school("school"), 0),
                command,
            ]),
        )
        .await;
        let (_, body) = refused.expect_err("refused");
        assert_eq!(body.0["error"]["code"], expected);
        assert_eq!(body.0["error"]["index"], 1);
    }
    assert_eq!(
        count(&g, "MATCH(a:AuthorizationAssignment) RETURN count(a) AS n").await,
        before
    );

    // An Owner creates an access-manager template (sensitive: Owner only) and gives it to the
    // school admin of one school.
    let created = commands::templates(
        &g,
        &writer("owner", "owner", "manager-template-01"),
        &template(CREATE, "access_manager", &[MANAGE, READ], &[SCHOOL], 0),
    )
    .await
    .unwrap();
    assert_eq!(created["templates"][0]["version"], 1);
    let given = commands::assignments(
        &g,
        &writer("owner", "owner", "manager-assign-0001"),
        &batch(vec![assign(
            "admin",
            "access_manager",
            Scope::school("school"),
            0,
        )]),
    )
    .await
    .unwrap();
    let manager_id = given["assignments"][0]["id"].as_str().unwrap().to_string();
    // Owner-a holds only one of two schools: it cannot change tenant templates.
    assert_eq!(
        code(
            commands::templates(
                &g,
                &writer("owner-a", "owner", "owner-a-template-1"),
                &template(CREATE, "helper", &[READ], &[SCHOOL], 0)
            )
            .await
        ),
        "FORBIDDEN"
    );

    // Guardrails for a manager who is not an Owner.
    let admin = |key: &'static str| writer("admin", "school_admin", key);
    assert_eq!(
        code(
            commands::assignments(
                &g,
                &admin("admin-wider-0002"),
                &batch(vec![assign(
                    "t-other",
                    "access_manager",
                    Scope::school("school-2"),
                    0
                )])
            )
            .await
        ),
        "FORBIDDEN",
        "no scope wider than their own"
    );
    let (_, body) = commands::assignments(
        &g,
        &admin("admin-notheld-001"),
        &batch(vec![assign(
            "t-home",
            "subject_lead",
            subject_scope("mathematics"),
            0,
        )]),
    )
    .await
    .expect_err("refused");
    assert_eq!(body.0["error"]["code"], "CAPABILITY_NOT_HELD");
    assert_eq!(body.0["error"]["capability"], "learning.module.review");
    assert_eq!(
        code(
            commands::assignments(
                &g,
                &admin("admin-sensitive-01"),
                &batch(vec![assign(
                    "t-home",
                    SCHOOL_ADMIN,
                    Scope::school("school"),
                    0
                )])
            )
            .await
        ),
        "SENSITIVE_NEEDS_OWNER",
        "School admin carries student biodata"
    );
    assert_eq!(
        code(
            commands::assignments(
                &g,
                &admin("admin-sensitive-02"),
                &batch(vec![assign(
                    "t-home",
                    "access_manager",
                    Scope::school("school"),
                    0
                )])
            )
            .await
        ),
        "SENSITIVE_NEEDS_OWNER"
    );
    // What the manager does hold, they may hand on within their school.
    commands::templates(
        &g,
        &writer("owner", "owner", "reader-template-01"),
        &template(CREATE, "access_reader", &[READ], &[SCHOOL, SECTION], 0),
    )
    .await
    .unwrap();
    let handed = commands::assignments(
        &g,
        &admin("admin-handon-0001"),
        &batch(vec![assign(
            "t-home",
            "access_reader",
            Scope::section("school", "7A"),
            0,
        )]),
    )
    .await
    .unwrap();
    let reader_id = handed["assignments"][0]["id"].as_str().unwrap().to_string();
    let validity = AssignmentCommand {
        valid_from: Some("2026-07-01".into()),
        valid_until: Some("2027-06-30".into()),
        ..by_id(CHANGE_VALIDITY, &reader_id, 1)
    };
    let changed = commands::assignments(&g, &admin("admin-validity-01"), &batch(vec![validity]))
        .await
        .unwrap();
    assert_eq!(changed["assignments"][0]["validUntil"], "2027-06-30");
    assert_eq!(changed["assignments"][0]["version"], 2);

    // Reads stay within the reader's schools.
    let theirs = reads::effective(
        &g,
        &scoped_reader("admin-2", &["school-2"]),
        "tenant",
        "t-grant",
    )
    .await;
    assert_eq!(code(theirs), "NOT_FOUND");
    let seen = reads::effective(
        &g,
        &scoped_reader("admin", &["school"]),
        "tenant",
        "t-grant",
    )
    .await
    .unwrap();
    let sources: Vec<&str> = seen["holdings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["source"]["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        sources,
        ["assignment", "teaching_grant"],
        "archived class OLD left out"
    );
    assert_eq!(seen["person"]["name"], "Dewi Lestari");
    let owner_seen = reads::effective(&g, &scoped_reader("admin", &["school"]), "tenant", "owner")
        .await
        .unwrap();
    assert_eq!(
        owner_seen["holdings"][0]["scope"]["type"], SCHOOL,
        "tenant-wide shown per readable school"
    );
    assert_eq!(
        code(
            reads::check(
                &g,
                &scoped_reader("admin", &["school"]),
                &check(
                    Some("t-other"),
                    "learning.class.teach",
                    Scope::section("school-2", "10A")
                )
            )
            .await
        ),
        "FORBIDDEN"
    );
    let listed = reads::assignments(
        &g,
        &scoped_reader("admin", &["school"]),
        &reads::Filter {
            tenant_id: "tenant".into(),
            ..reads::Filter::default()
        },
    )
    .await
    .unwrap();
    let rows = listed["assignments"].as_array().unwrap();
    assert!(rows.iter().all(|r| r["scope"]["schoolId"] == "school"));
    assert!(rows
        .iter()
        .any(|r| r["source"]["kind"] == "homeroom" && r["staffMemberId"] == "t-home"));
    assert!(rows
        .iter()
        .any(|r| r["derived"] == false && r["id"] == lead_id.as_str()));
    assert_eq!(listed["manageSchools"], serde_json::json!(["school"]));
    assert_eq!(listed["ownerSchools"], serde_json::json!([]));
    let hints = reads::templates(&g, &scoped_reader("admin", &["school"]), "tenant")
        .await
        .unwrap();
    assert_eq!(
        (
            hints["canEditTemplates"].as_bool(),
            hints["canGrantSensitive"].as_bool()
        ),
        (Some(false), Some(false))
    );
    let hints = reads::templates(&g, &reader("owner"), "tenant")
        .await
        .unwrap();
    assert_eq!(
        (
            hints["canEditTemplates"].as_bool(),
            hints["canGrantSensitive"].as_bool()
        ),
        (Some(true), Some(true))
    );

    // The last holder of authorization.manage in a school keeps it.
    g.run(query(
        "MATCH(s:StaffMember) WHERE s.id IN ['owner','owner-a'] SET s.membershipStatus='DISABLED'",
    ))
    .await
    .unwrap();
    assert_eq!(
        code(
            commands::assignments(
                &g,
                &admin("admin-last-00001"),
                &batch(vec![by_id(REVOKE, &manager_id, 1)])
            )
            .await
        ),
        "LAST_AUTHORIZATION_MANAGER"
    );
    g.run(query(
        "MATCH(s:StaffMember) WHERE s.id IN ['owner','owner-a'] SET s.membershipStatus='ACTIVE'",
    ))
    .await
    .unwrap();
    let revoked = commands::assignments(
        &g,
        &writer("owner", "owner", "owner-revoke-0001"),
        &batch(vec![by_id(REVOKE, &manager_id, 1)]),
    )
    .await
    .unwrap();
    assert_eq!(revoked["assignments"][0]["status"], "revoked");
    assert!(!new(&g, "admin", MANAGE, &Scope::school("school")).await);
    // Reassigning reopens the revoked record at its version.
    let reopened = commands::assignments(
        &g,
        &writer("owner", "owner", "owner-reopen-0001"),
        &batch(vec![assign(
            "admin",
            "access_manager",
            Scope::school("school"),
            2,
        )]),
    )
    .await
    .unwrap();
    assert_eq!(
        (
            reopened["assignments"][0]["id"].as_str(),
            reopened["assignments"][0]["version"].as_i64()
        ),
        (Some(manager_id.as_str()), Some(3))
    );

    // Templates: a seed's first save keeps the seed as version 1; derived holders follow it.
    let without_gradebook: Vec<&str> = [
        "learning.class.teach",
        "learning.module.write",
        "learning.module.publish",
        "learning.curriculum.read",
        "learning.objective_tag.write",
        "learning.mastery.read",
    ]
    .to_vec();
    let impact = reads::impact(&g, &reader("owner"), "tenant", REGULAR_TEACHER)
        .await
        .unwrap();
    assert_eq!(
        (
            impact["people"].as_i64(),
            impact["sources"]["homeroom"].as_i64()
        ),
        (Some(3), Some(1))
    );
    let revised = commands::templates(
        &g,
        &writer("owner", "owner", "regular-revise-001"),
        &template(
            REVISE,
            REGULAR_TEACHER,
            &without_gradebook,
            &[SECTION_SUBJECT, SECTION, SCHOOL_SUBJECT],
            1,
        ),
    )
    .await
    .unwrap();
    assert_eq!(revised["templates"][0]["version"], 2);
    let history = reads::history(&g, &reader("owner"), "tenant", REGULAR_TEACHER)
        .await
        .unwrap();
    let versions: Vec<(i64, bool)> = history["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["version"].as_i64().unwrap(), v["seed"].as_bool().unwrap()))
        .collect();
    assert_eq!(versions, [(2, false), (1, true)]);
    assert!(
        !new(
            &g,
            "t-grant",
            "learning.gradebook.read",
            &Scope::section("school", "9A")
        )
        .await
    );
    let answer = reads::check(
        &g,
        &reader("owner"),
        &check(
            Some("t-grant"),
            "learning.class.teach",
            Scope::section("school", "9A"),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        (
            answer["source"]["kind"].as_str(),
            answer["templateVersion"].as_i64()
        ),
        (Some("teaching_grant"), Some(2))
    );
    for (cmd, expected) in [
        (
            template(ARCHIVE, REGULAR_TEACHER, &[], &[], 2),
            "TEMPLATE_REQUIRED",
        ),
        (
            template(ARCHIVE, "access_manager", &[], &[], 1),
            "TEMPLATE_IN_USE",
        ),
        (
            template(REVISE, "access_reader", &[READ], &[SCHOOL], 1),
            "SCOPE_IN_USE",
        ),
        (
            template(REVISE, "access_reader", &[READ], &[SCHOOL, SECTION], 7),
            "REVISION_CONFLICT",
        ),
        (
            template(CREATE, "access_reader", &[READ], &[SCHOOL], 0),
            "REVISION_CONFLICT",
        ),
        (
            template(CREATE, "made_up", &["learning.made.up"], &[SCHOOL], 0),
            "UNKNOWN_CAPABILITY",
        ),
    ] {
        assert_eq!(
            code(
                commands::templates(
                    &g,
                    &writer("owner", "owner", &format!("template-{expected}")),
                    &cmd
                )
                .await
            ),
            expected
        );
    }
    let archived = commands::templates(
        &g,
        &writer("owner", "owner", "lead-archive-00"),
        &template(ARCHIVE, "curriculum_coordinator", &[], &[], 1),
    )
    .await
    .unwrap();
    assert_eq!(archived["templates"][0]["status"], "archived");
    assert_eq!(
        code(
            commands::templates(
                &g,
                &writer("owner", "owner", "lead-archive-01"),
                &template(REVISE, "curriculum_coordinator", &[READ], &[SCHOOL], 2)
            )
            .await
        ),
        "TEMPLATE_ARCHIVED"
    );

    // Audit: one entry per change with before, after and the source that allowed it.
    assert_eq!(
        count(
            &g,
            "MATCH(e:AuthorizationAudit {kind:'assignment'}) RETURN count(e) AS n"
        )
        .await,
        6
    );
    assert_eq!(
        count(
            &g,
            "MATCH(e:AuthorizationAudit {kind:'template'}) RETURN count(e) AS n"
        )
        .await,
        4
    );
    assert_eq!(count(&g, "MATCH(e:AuthorizationAudit {action:'revoke'}) WHERE e.before CONTAINS '\"active\"' AND e.after CONTAINS '\"revoked\"' AND e.authorized_by CONTAINS 'owner' RETURN count(e) AS n").await, 1);
    assert_eq!(count(&g, "MATCH(e:AuthorizationAudit {action:'assign',actor_id:'admin'}) WHERE e.authorized_by CONTAINS 'access_manager' RETURN count(e) AS n").await, 1);
    assert_eq!(
        count(
            &g,
            "MATCH(s:AuthorizationTenantState {tenant_id:'tenant'}) RETURN s.version AS n"
        )
        .await,
        10
    );
    let audit = reads::audit(&g, &scoped_reader("admin", &["school"]), "tenant")
        .await
        .unwrap();
    assert_eq!(audit["entries"].as_array().unwrap().len(), 10);
}

/// Slice 5: what Learning reads for school and subject roles, pinned to one school.
#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn authorization_graph_learning_scopes_per_school() {
    let g = fixture().await;
    let mut math_7_9 = subject_scope("mathematics");
    math_7_9.grade_from = Some("Year 7".into());
    math_7_9.grade_to = Some("Year 9".into());
    commands::assignments(
        &g,
        &writer("owner", "owner", "scopes-assign-0001"),
        &batch(vec![
            assign(
                "t-home",
                "curriculum_coordinator",
                Scope::school("school"),
                0,
            ),
            assign("t-grant", "subject_lead", math_7_9.clone(), 0),
            assign(
                "t-other",
                "curriculum_coordinator",
                Scope::school("school-2"),
                0,
            ),
        ]),
    )
    .await
    .unwrap();
    let scopes = |staff: &'static str| reads::learning_scopes(&g, "tenant", "school", staff);
    let json = |s: &Option<(std::collections::BTreeMap<String, Vec<Scope>>, i64)>| {
        serde_json::to_value(&s.as_ref().unwrap().0).unwrap()
    };

    // Owner: every learning capability, tenant-wide.
    let owner = scopes("owner").await.unwrap();
    let codes = reference()
        .catalogue
        .capabilities
        .iter()
        .filter(|c| c.code.starts_with("learning."))
        .count();
    assert_eq!(owner.as_ref().unwrap().0.len(), codes);
    assert_eq!(
        json(&owner)["learning.curriculum.manage"],
        serde_json::json!([{"type": "TENANT"}])
    );
    assert_eq!(
        owner.as_ref().unwrap().1,
        1,
        "the tenant version after one write"
    );

    // Coordinator at SCHOOL, beside their homeroom and teaching rows.
    let coordinator = json(&scopes("t-home").await.unwrap());
    assert_eq!(
        coordinator["learning.curriculum.manage"],
        serde_json::json!([{"type": "SCHOOL", "schoolId": "school"}])
    );
    assert_eq!(
        coordinator["learning.plan.manage"],
        serde_json::json!([{"type": "SCHOOL", "schoolId": "school"}])
    );
    assert_eq!(
        coordinator["learning.class.homeroom"],
        serde_json::json!([{"type": "SECTION", "schoolId": "school", "sectionId": "7A"}])
    );
    assert!(coordinator.get("learning.question_bank.manage").is_none());

    // Subject lead at SCHOOL_SUBJECT mathematics, Year 7–9.
    let lead = json(&scopes("t-grant").await.unwrap());
    let lead_scope = serde_json::json!([{"type": "SCHOOL_SUBJECT", "schoolId": "school", "subject": "mathematics", "gradeFrom": "Year 7", "gradeTo": "Year 9"}]);
    assert_eq!(lead["learning.module.manage_any"], lead_scope);
    assert_eq!(lead["learning.question_bank.manage"], lead_scope);
    assert!(lead.get("learning.curriculum.manage").is_none());

    // Plain teacher: only the class rows of their assignment.
    let plain = json(&scopes("t-assigned").await.unwrap());
    let class = serde_json::json!([{"type": "SECTION_SUBJECT", "schoolId": "school", "sectionId": "7B", "subject": "mathematics"}]);
    assert_eq!(plain["learning.class.teach"], class);
    assert!(plain.as_object().unwrap().values().all(|v| v == &class));
    assert!(plain.get("learning.curriculum.manage").is_none());

    // Another school's coordinator holds nothing in this school; their own school answers.
    assert_eq!(
        json(&scopes("t-other").await.unwrap()),
        serde_json::json!({})
    );
    let theirs = reads::learning_scopes(&g, "tenant", "school-2", "t-other")
        .await
        .unwrap();
    assert_eq!(
        json(&theirs)["learning.curriculum.manage"],
        serde_json::json!([{"type": "SCHOOL", "schoolId": "school-2"}])
    );

    // A disabled or unknown person is refused, not answered with nothing.
    assert!(scopes("t-off").await.unwrap().is_none());
    assert!(scopes("nobody").await.unwrap().is_none());
    // A teacher who holds nothing gets an empty answer.
    g.run(query("CREATE(:StaffMember {id:'t-new',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']})")).await.unwrap();
    assert_eq!(json(&scopes("t-new").await.unwrap()), serde_json::json!({}));
    println!(
        "example: {}",
        serde_json::json!({"data": {"contractVersion": 1, "capabilities": lead, "authorizationVersion": 1, "cacheSeconds": 60}})
    );
}

/// The School nodes store `school_name`; older fixtures and nodes used `name`. The school list
/// shows `school_name`, then `name`, then the id, so Admin never shows a bare id for a named school.
#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn authorization_graph_school_list_names() {
    let g = fixture().await;
    g.run(query("MATCH(s:School) DETACH DELETE s")).await.unwrap();
    g.run(query("CREATE(:School {school_id:'SCH-IIHS',tenant_id:'tenant',school_code:'IIHS',school_name:'International Islamic High School'}) CREATE(:School {school_id:'SCH-OLD',tenant_id:'tenant',name:'Legacy name'}) CREATE(:School {school_id:'SCH-BOTH',tenant_id:'tenant',school_name:'Preferred',name:'Ignored'}) CREATE(:School {school_id:'SCH-BARE',tenant_id:'tenant'})")).await.unwrap();
    let f = facts::load(&mut Db::Graph(&g), "tenant", &[], &[])
        .await
        .unwrap();
    let name = |id: &str| f.schools.get(id).map(String::as_str);
    assert_eq!(name("SCH-IIHS"), Some("International Islamic High School"));
    assert_eq!(name("SCH-OLD"), Some("Legacy name"));
    assert_eq!(name("SCH-BOTH"), Some("Preferred"));
    assert_eq!(name("SCH-BARE"), Some("SCH-BARE"));
}

/// ADR 0018 remainder: the Principal seed is given by an Owner alone, and the mastery roster lists
/// the classes and students a holder of `learning.mastery.read` may read, by SCHOOL,
/// SCHOOL_SUBJECT, SECTION and SECTION_SUBJECT holdings.
#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn authorization_graph_principal_and_mastery_roster() {
    let g = fixture().await;
    for statement in [
        "MATCH(c:Section {section_id:'7B'}) CREATE(s:Student {studentId:'st-1',fullName:'Aisha'})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'school',tenant_id:'tenant'})-[:ENROLLED_IN]->(c)",
        "MATCH(c:Section {section_id:'7B'}) CREATE(s:Student {studentId:'st-2',fullName:'Bilal'})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'school',tenant_id:'tenant'})-[:ENROLLED_IN]->(c)",
        "MATCH(c:Section {section_id:'9A'}) CREATE(s:Student {studentId:'st-3',fullName:'Citra'})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'school',tenant_id:'tenant'})-[:ENROLLED_IN]->(c)",
        // Left the school: not on any roster.
        "MATCH(c:Section {section_id:'9A'}) CREATE(s:Student {studentId:'st-4',fullName:'Dian'})-[:ENROLLED_AS]->(:EnrolledStudent {status:'withdrawn',school_id:'school',tenant_id:'tenant'})-[:ENROLLED_IN]->(c)",
    ] {
        g.run(query(statement)).await.unwrap();
    }
    let principal = |staff: &str| assign(staff, PRINCIPAL, Scope::school("school"), 0);

    // The school administrator reads access but does not manage it.
    assert_eq!(
        code(
            commands::assignments(
                &g,
                &writer("admin", "school_admin", "principal-admin-01"),
                &batch(vec![principal("t-home")]),
            )
            .await
        ),
        "FORBIDDEN"
    );
    // Only at SCHOOL.
    assert_eq!(
        code(
            commands::assignments(
                &g,
                &writer("owner", "owner", "principal-owner-00"),
                &batch(vec![assign(
                    "t-home",
                    PRINCIPAL,
                    subject_scope("mathematics"),
                    0
                )]),
            )
            .await
        ),
        "SCOPE_NOT_ALLOWED"
    );
    // The Owner names the Principal of the school.
    let given = commands::assignments(
        &g,
        &writer("owner", "owner", "principal-owner-01"),
        &batch(vec![principal("t-home")]),
    )
    .await
    .unwrap();
    assert_eq!(given["assignments"][0]["templateCode"], PRINCIPAL);
    assert!(new(&g, "t-home", MANAGE, &Scope::school("school")).await);
    assert!(!new(&g, "t-home", MANAGE, &Scope::school("school-2")).await);
    // The Principal manages access, but may not name another Principal: an Owner does.
    assert_eq!(
        code(
            commands::assignments(
                &g,
                &writer("t-home", "school_admin", "principal-by-principal"),
                &batch(vec![principal("t-grant")]),
            )
            .await
        ),
        "PRINCIPAL_NEEDS_OWNER"
    );
    // Nor a capability they do not hold themselves (the hold-what-you-grant rule).
    assert_eq!(
        code(
            commands::assignments(
                &g,
                &writer("t-home", "school_admin", "coordinator-by-principal"),
                &batch(vec![assign(
                    "t-grant",
                    "curriculum_coordinator",
                    Scope::school("school"),
                    0
                )]),
            )
            .await
        ),
        "CAPABILITY_NOT_HELD"
    );
    // They do revoke, and the school keeps a manager: the Owner.
    let id = given["assignments"][0]["id"].as_str().unwrap().to_string();
    commands::assignments(
        &g,
        &writer("owner", "owner", "principal-revoke-01"),
        &batch(vec![by_id(REVOKE, &id, 1)]),
    )
    .await
    .unwrap();
    assert!(!new(&g, "t-home", MANAGE, &Scope::school("school")).await);
    commands::assignments(
        &g,
        &writer("owner", "owner", "principal-owner-02"),
        // Reopened at the revoked record's version.
        &batch(vec![assign(
            "t-home",
            PRINCIPAL,
            Scope::school("school"),
            2,
        )]),
    )
    .await
    .unwrap();

    // Mastery roster.
    let roster =
        |staff: &'static str, subject: Option<&'static str>, grade: Option<&'static str>| {
            reads::mastery_roster(&g, "tenant", "school", staff, subject, grade)
        };
    let classes = |r: &Option<(Vec<reads::RosterClass>, i64)>| -> Vec<(String, Option<Vec<String>>, Vec<String>)> {
        r.as_ref()
            .unwrap()
            .0
            .iter()
            .map(|c| (c.id.clone(), c.subjects.clone(), c.students.iter().map(|s| s.id.clone()).collect()))
            .collect()
    };
    let all = |ids: &[&str]| -> Vec<(String, Option<Vec<String>>, Vec<String>)> {
        ids.iter()
            .map(|id| {
                (
                    id.to_string(),
                    None,
                    match *id {
                        "7B" => vec!["st-1".into(), "st-2".into()],
                        "9A" => vec!["st-3".into()],
                        _ => vec![],
                    },
                )
            })
            .collect()
    };
    // Owner and Principal (mastery.read at SCHOOL): every active class, every subject.
    assert_eq!(
        classes(&roster("owner", None, None).await.unwrap()),
        all(&["7A", "7B", "9A"])
    );
    assert_eq!(
        classes(&roster("t-home", None, None).await.unwrap()),
        all(&["7A", "7B", "9A"])
    );
    assert_eq!(
        classes(&roster("owner", None, Some("Year 7")).await.unwrap()),
        all(&["7A", "7B"])
    );
    assert_eq!(
        classes(&roster("owner", None, Some("9")).await.unwrap()),
        all(&["9A"])
    );
    assert_eq!(
        classes(&roster("owner", Some("science"), Some("9")).await.unwrap()),
        vec![(
            "9A".to_string(),
            Some(vec!["science".to_string()]),
            vec!["st-3".to_string()]
        )]
    );
    // A teaching assignment: that class, that subject.
    assert_eq!(
        classes(&roster("t-assigned", None, None).await.unwrap()),
        vec![(
            "7B".to_string(),
            Some(vec!["mathematics".to_string()]),
            vec!["st-1".to_string(), "st-2".to_string()]
        )]
    );
    assert!(classes(&roster("t-assigned", Some("science"), None).await.unwrap()).is_empty());
    // A grant to the class: the whole class.
    assert_eq!(
        classes(&roster("t-grant", None, None).await.unwrap()),
        all(&["9A"])
    );
    // A revoked grant: nothing.
    assert!(classes(&roster("t-revoked", None, None).await.unwrap()).is_empty());
    // Another school's teacher: nothing here; disabled or unknown: refused.
    assert!(classes(&roster("t-other", None, None).await.unwrap()).is_empty());
    assert!(roster("t-off", None, None).await.unwrap().is_none());
    assert!(roster("nobody", None, None).await.unwrap().is_none());

    // A coordinator who teaches no class reads every class; a subject lead (whose seed carries no
    // mastery read) none, until the school revises that template.
    let mut math_7_9 = subject_scope("mathematics");
    math_7_9.grade_from = Some("Year 7".into());
    math_7_9.grade_to = Some("Year 9".into());
    g.run(query("CREATE(:StaffMember {id:'t-new',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t-lead',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']})")).await.unwrap();
    commands::assignments(
        &g,
        &writer("owner", "owner", "roster-assign-01"),
        &batch(vec![
            assign(
                "t-new",
                "curriculum_coordinator",
                Scope::school("school"),
                0,
            ),
            assign("t-lead", "subject_lead", math_7_9.clone(), 0),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(
        classes(&roster("t-new", None, None).await.unwrap()),
        all(&["7A", "7B", "9A"])
    );
    assert!(classes(&roster("t-lead", None, None).await.unwrap()).is_empty());
    // A subject role that does carry the read covers that subject in the classes of its grades.
    commands::templates(
        &g,
        &writer("owner", "owner", "roster-revise-lead"),
        &template(
            REVISE,
            "subject_lead",
            &["learning.module.review", MASTERY_READ],
            &[SCHOOL_SUBJECT],
            1,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        classes(&roster("t-lead", None, None).await.unwrap()),
        vec![
            (
                "7A".to_string(),
                Some(vec!["mathematics".to_string()]),
                vec![]
            ),
            (
                "7B".to_string(),
                Some(vec!["mathematics".to_string()]),
                vec!["st-1".to_string(), "st-2".to_string()]
            ),
            (
                "9A".to_string(),
                Some(vec!["mathematics".to_string()]),
                vec!["st-3".to_string()]
            ),
        ]
    );
    assert!(classes(&roster("t-lead", Some("science"), None).await.unwrap()).is_empty());
    let (_, version) = roster("owner", None, None).await.unwrap().unwrap();
    assert_eq!(version, 5, "the tenant version after five writes");
    println!(
        "example: {}",
        serde_json::json!({"data": {"contractVersion": 1, "schoolId": "school", "tenantId": "tenant", "classes": roster("t-assigned", None, None).await.unwrap().unwrap().0, "authorizationVersion": version, "cacheSeconds": 60}})
    );
}

/// Spec §10 #8.4: an assignment without an end ends with the school's academic year, except the
/// Principal and Curriculum coordinator, and the answer says which rule applied; the expiring read
/// lists what ends within the window, in the reader's schools only.
#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn authorization_graph_validity_default_and_expiring() {
    let g = fixture().await;
    let owner = |command: &'static str| writer("owner", "owner", command);
    let today = super::model::today();
    let day = |n: i64| {
        (today + chrono::Duration::days(n))
            .format("%Y-%m-%d")
            .to_string()
    };
    let loaded = facts::load(&mut Db::Graph(&g), "tenant", &[], &[])
        .await
        .unwrap();
    let (year, end, source) = loaded.academic_year_on(Some("school"), today);
    if today >= chrono::NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()
        && today <= chrono::NaiveDate::from_ymd_opt(2027, 6, 30).unwrap()
    {
        assert_eq!(
            (year.as_str(), end.to_string().as_str(), source),
            ("2026/2027", "2027-06-30", "classes"),
            "the fixture's classes are 2026/2027"
        );
    }

    let given = commands::assignments(
        &g,
        &owner("validity-default-01"),
        &batch(vec![
            assign("t-home", "subject_lead", subject_scope("mathematics"), 0),
            assign("t-grant", PRINCIPAL, Scope::school("school"), 0),
            assign(
                "t-assigned",
                CURRICULUM_COORDINATOR,
                Scope::school("school"),
                0,
            ),
            AssignmentCommand {
                valid_until: Some(day(10)),
                ..assign("t-revoked", "admin_teacher", Scope::school("school"), 0)
            },
        ]),
    )
    .await
    .unwrap();
    let rows = given["assignments"].as_array().unwrap();
    assert_eq!(rows[0]["validUntil"], end.to_string());
    assert_eq!(
        rows[0]["validity"],
        serde_json::json!({"rule": "academic_year_end", "academicYear": year, "source": source})
    );
    for row in &rows[1..3] {
        assert!(row["validUntil"].is_null());
        assert_eq!(
            row["validity"],
            serde_json::json!({"rule": "until_revoked"})
        );
    }
    assert_eq!(rows[3]["validUntil"], day(10));
    assert_eq!(rows[3]["validity"], serde_json::json!({"rule": "explicit"}));
    let stored = count(
        &g,
        &format!(
            "MATCH(a:AuthorizationAssignment {{staff_member_id:'t-home',valid_until:'{end}'}}) RETURN count(a) AS n"
        ),
    )
    .await;
    assert_eq!(stored, 1, "the default is saved, not only answered");

    // Changing the dates without an end applies the same rule.
    let lead = rows[0]["id"].as_str().unwrap().to_string();
    let changed = commands::assignments(
        &g,
        &owner("validity-change-01"),
        &batch(vec![AssignmentCommand {
            valid_from: Some(day(-1)),
            ..by_id(CHANGE_VALIDITY, &lead, 1)
        }]),
    )
    .await
    .unwrap();
    assert_eq!(changed["assignments"][0]["validUntil"], end.to_string());
    assert_eq!(changed["assignments"][0]["validFrom"], day(-1));
    assert_eq!(
        changed["assignments"][0]["validity"]["rule"],
        "academic_year_end"
    );

    // More ends: one in the other school, one after the window, one already over.
    commands::assignments(
        &g,
        &owner("validity-default-02"),
        &batch(vec![
            AssignmentCommand {
                valid_until: Some(day(3)),
                ..assign("t-other", "admin_teacher", Scope::school("school-2"), 0)
            },
            AssignmentCommand {
                valid_until: Some(day(45)),
                ..assign("t-home", "admin_teacher", Scope::school("school"), 0)
            },
            AssignmentCommand {
                valid_from: Some(day(-30)),
                valid_until: Some(day(-1)),
                ..assign("t-grant", "admin_teacher", Scope::school("school"), 0)
            },
        ]),
    )
    .await
    .unwrap();
    let ends = |v: &Value| -> Vec<(String, i64)> {
        v["assignments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["staffMemberId"].as_str().unwrap().to_string(),
                    r["daysLeft"].as_i64().unwrap(),
                )
            })
            .collect()
    };
    // The Owner sees both schools, soonest first, and may renew both.
    let all = reads::expiring(&g, &reader("owner"), "tenant", 30, None)
        .await
        .unwrap();
    assert_eq!(
        ends(&all),
        [("t-other".to_string(), 3), ("t-revoked".to_string(), 10)]
    );
    assert_eq!(all["days"], 30);
    assert_eq!(all["today"], today.to_string());
    let first = &all["assignments"][0];
    assert_eq!(first["canRenew"], true);
    assert_eq!(
        first["name"], "t-other",
        "the id stands in for a missing name"
    );
    assert_eq!(first["templateName"]["en"], "Admin teacher");
    let next = loaded
        .academic_year_on(Some("school-2"), today + chrono::Duration::days(4))
        .1;
    assert_eq!(first["renewUntil"], next.to_string());
    // A wider window takes the later one; a school filter keeps that school.
    assert_eq!(
        ends(
            &reads::expiring(&g, &reader("owner"), "tenant", 60, None)
                .await
                .unwrap()
        )
        .len(),
        3
    );
    assert_eq!(
        ends(
            &reads::expiring(&g, &reader("owner"), "tenant", 30, Some("school-2"))
                .await
                .unwrap()
        ),
        [("t-other".to_string(), 3)]
    );
    // The school administrator reads their school only and may not renew (no manage).
    let theirs = reads::expiring(&g, &scoped_reader("admin", &["school"]), "tenant", 30, None)
        .await
        .unwrap();
    assert_eq!(ends(&theirs), [("t-revoked".to_string(), 10)]);
    assert_eq!(theirs["assignments"][0]["canRenew"], false);
    // A Principal of the school manages access there, but renewing runs the grant rules again: they
    // do not hold the Admin teacher's Learning capabilities, so the hint says no, as the write would.
    let principal = reads::expiring(
        &g,
        &scoped_reader("t-grant", &["school"]),
        "tenant",
        30,
        None,
    )
    .await
    .unwrap();
    assert_eq!(principal["assignments"][0]["canRenew"], false);
    let refused = commands::assignments(
        &g,
        &writer("t-grant", "staff", "validity-principal-renew"),
        &batch(vec![AssignmentCommand {
            valid_until: Some(day(200)),
            ..by_id(
                CHANGE_VALIDITY,
                principal["assignments"][0]["id"].as_str().unwrap(),
                1,
            )
        }]),
    )
    .await;
    assert_eq!(code(refused), "CAPABILITY_NOT_HELD");
    // What they do hold, they renew.
    commands::templates(
        &g,
        &owner("validity-reader-template"),
        &template(CREATE, "access_reader", &[READ], &[SCHOOL], 0),
    )
    .await
    .unwrap();
    commands::assignments(
        &g,
        &owner("validity-reader-assign"),
        &batch(vec![AssignmentCommand {
            valid_until: Some(day(20)),
            ..assign("t-assigned", "access_reader", Scope::school("school"), 0)
        }]),
    )
    .await
    .unwrap();
    let principal = reads::expiring(
        &g,
        &scoped_reader("t-grant", &["school"]),
        "tenant",
        30,
        None,
    )
    .await
    .unwrap();
    let reader_row = principal["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["templateCode"] == "access_reader")
        .unwrap();
    assert_eq!(reader_row["canRenew"], true);
    // A plain teacher reads nothing.
    assert_eq!(
        code(
            reads::expiring(
                &g,
                &scoped_reader("t-home", &["school"]),
                "tenant",
                30,
                None
            )
            .await
        ),
        "FORBIDDEN"
    );
}

/// Admin's gate reads the caller's own holdings: no read capability is needed for that, and only
/// the schools of the caller's session count.
#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn authorization_graph_own_holdings_for_admin_gate() {
    let g = fixture().await;
    let caps = |v: &Value| -> Vec<String> {
        v["capabilities"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    };
    // A plain teacher: their class access, readable without authorization.read.
    let teacher = reads::own(&g, &scoped_reader("t-home", &["school"]), "tenant")
        .await
        .unwrap();
    assert!(caps(&teacher).contains(&HOMEROOM.to_string()));
    assert!(!caps(&teacher)
        .iter()
        .any(|c| c.starts_with("authorization.")));
    assert_eq!(
        code(
            reads::effective(
                &g,
                &scoped_reader("t-home", &["school"]),
                "tenant",
                "t-home"
            )
            .await
        ),
        "FORBIDDEN",
        "reading oneself through the reader path still needs the read capability"
    );
    // Named Principal of the school: access management there, and only there.
    commands::assignments(
        &g,
        &writer("owner", "owner", "own-principal-01"),
        &batch(vec![assign(
            "t-home",
            PRINCIPAL,
            Scope::school("school"),
            0,
        )]),
    )
    .await
    .unwrap();
    let principal = reads::own(&g, &scoped_reader("t-home", &["school"]), "tenant")
        .await
        .unwrap();
    assert!(caps(&principal).contains(&MANAGE.to_string()));
    assert_eq!(
        principal["capabilities"][MANAGE],
        serde_json::json!([{"type": "SCHOOL", "schoolId": "school"}])
    );
    assert_eq!(principal["person"]["roles"], serde_json::json!(["teacher"]));
    // A session that covers another school shows none of it.
    let elsewhere = reads::own(&g, &scoped_reader("t-home", &["school-2"]), "tenant")
        .await
        .unwrap();
    assert!(caps(&elsewhere).is_empty());
    // The Principal now reads access in their school only.
    let seen = reads::assignments(
        &g,
        &scoped_reader("t-home", &["school"]),
        &reads::Filter {
            tenant_id: "tenant".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(seen["manageSchools"], serde_json::json!(["school"]));
    assert_eq!(
        seen["schools"].as_array().unwrap().len(),
        1,
        "school-2 stays out of sight"
    );
    // Disabled or unknown people hold nothing.
    assert_eq!(
        code(reads::own(&g, &scoped_reader("t-off", &["school"]), "tenant").await),
        "FORBIDDEN"
    );
}
