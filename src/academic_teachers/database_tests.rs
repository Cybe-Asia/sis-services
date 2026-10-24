use super::{model, repository as repo};
use neo4rs::{query, Graph};

#[tokio::test]
#[ignore = "requires disposable SCHOOL_PORTAL_TEST_BOLT graph"]
async fn teacher_list_reads_one_school_year_and_week_for_its_administrators() {
    let uri = std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap();
    assert_eq!(uri, "127.0.0.1:3223");
    let g = Graph::new(uri, "neo4j", "parent-otp-fixture-password")
        .await
        .unwrap();
    g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
    let (from, to) = model::window_ms(chrono::NaiveDate::from_ymd_opt(2026, 10, 26).unwrap());
    let meeting = |id: &str, class: &str, teacher: &str, starts: i64, status: &str| {
        format!("CREATE(:LearningClassMeeting {{key:'{id}',id:'{id}',school_id:'school',tenant_id:'tenant',class_id:'{class}',teacher_id:'{teacher}',academic_year:'2026/2027',room_id:'r-1',starts_at:{starts},ends_at:{},status:'{status}',payload:'{{\"subject\":\"mathematics\",\"title\":\"Mathematics\"}}'}})", starts + 2_400_000)
    };
    let in_week = from + 7 * 3_600_000 + 30 * 60_000;
    let seed = [
        "CREATE(:School {school_id:'school',tenant_id:'tenant'}) CREATE(:School {school_id:'school-2',tenant_id:'tenant'})".to_string(),
        "CREATE(:StaffMember {id:'admin',membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'owner',membershipStatus:'ACTIVE',roles:['owner'],schoolIds:[],tenantIds:[]}) CREATE(:StaffMember {id:'elsewhere',membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:['school-2'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'suspended',membershipStatus:'SUSPENDED',roles:['school_admin'],schoolIds:['school'],tenantIds:['tenant']})".into(),
        "CREATE(:StaffMember {id:'t1',displayName:'Siti Rahma',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t2',displayName:'Budi',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t3',displayName:'Former',membershipStatus:'DISABLED',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) CREATE(:StaffMember {id:'t4',displayName:'Other school',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school-2'],tenantIds:['tenant']})".into(),
        "CREATE(:Section {section_id:'7A',name:'7A',year_group:'Year 7',academic_year:'2026/2027',school_id:'school',tenant_id:'tenant',status:'active',homeroom_staff_member_id:'t1'}) CREATE(:Section {section_id:'8A',name:'8A',year_group:'Year 8',academic_year:'2026/2027',school_id:'school',tenant_id:'tenant',status:'active'}) CREATE(:Section {section_id:'9A',name:'9A',year_group:'Year 9',academic_year:'2026/2027',school_id:'school',tenant_id:'tenant',status:'archived',homeroom_staff_member_id:'t2'}) CREATE(:Section {section_id:'OLD',name:'7A',year_group:'Year 7',academic_year:'2025/2026',school_id:'school',tenant_id:'tenant',status:'active'})".into(),
        meeting("m-week", "7A", "t1", in_week, "published"),
        meeting("m-next-week", "7A", "t1", to + 3_600_000, "published"),
        meeting("m-archived", "7A", "t1", in_week + 3_600_000, "archived"),
        meeting("m-old-class", "OLD", "t2", in_week, "published"),
        meeting("m-archived-class", "9A", "t2", in_week, "published"),
        "CREATE(:LearningTeachingGrant {key:'t3|8A',staff_member_id:'t3',section_id:'8A',school_id:'school',tenant_id:'tenant',status:'ACTIVE'}) CREATE(:LearningTeachingGrant {key:'t2|8A',staff_member_id:'t2',section_id:'8A',school_id:'school',tenant_id:'tenant',status:'REVOKED'})".into(),
        "CREATE(:ExtraCurricularActivity {key:'a1',id:'a1',school_id:'school',tenant_id:'tenant',academic_year:'2026/2027',coach_id:'t2',archived:false,payload:'{\"title\":{\"en\":\"Football\",\"id\":\"Sepak bola\"}}'}) CREATE(:ExtraCurricularActivity {key:'a2',id:'a2',school_id:'school',tenant_id:'tenant',academic_year:'2026/2027',coach_id:'t4',archived:true,payload:'{\"title\":{\"en\":\"Chess\",\"id\":\"Catur\"}}'})".into(),
    ];
    for statement in seed {
        g.run(query(&statement)).await.unwrap();
    }

    for (actor, allowed) in [
        ("admin", true),
        ("owner", true),
        ("elsewhere", false),
        ("suspended", false),
        ("t1", false),
    ] {
        assert_eq!(
            repo::authorize(&g, actor, "school", "tenant")
                .await
                .unwrap(),
            allowed,
            "{actor}"
        );
    }

    let s = repo::snapshot(&g, "school", "tenant", "2026/2027", from, to)
        .await
        .unwrap();
    assert_eq!(
        s.sections.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
        vec!["7A", "8A"]
    );
    assert_eq!(
        s.lessons.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
        vec!["m-week"]
    );
    assert_eq!(s.lessons[0].subject, "mathematics");
    assert_eq!(s.grants.len(), 1);
    assert_eq!(s.activities.len(), 1);
    assert_eq!(s.activities[0].title["id"], "Sepak bola");
    let staff: Vec<_> = s.staff.iter().map(|p| (p.id.as_str(), p.active)).collect();
    assert_eq!(staff, vec![("t1", true), ("t2", true), ("t3", false)]);
    let r = model::report(&s).unwrap();
    assert_eq!(
        r.teachers
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Budi", "Former", "Siti Rahma"]
    );
    assert_eq!(r.class_warnings.len(), 1);
    assert_eq!(r.class_warnings[0].class.section_id, "8A");

    // A duplicated StaffMember id is never an active teacher.
    g.run(query("CREATE(:StaffMember {id:'t1',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']})")).await.unwrap();
    let s = repo::snapshot(&g, "school", "tenant", "2026/2027", from, to)
        .await
        .unwrap();
    assert!(!s.staff.iter().find(|p| p.id == "t1").unwrap().active);

    let years = |rows: Vec<repo::School>| {
        rows.into_iter()
            .map(|r| format!("{}:{}:{}", r.school_id, r.academic_year, r.classes))
            .collect::<Vec<_>>()
    };
    let mine = repo::schools(
        &g,
        "admin",
        vec!["school".into()],
        vec!["tenant".into()],
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        years(mine),
        vec!["school:2026/2027:2", "school:2025/2026:1"]
    );
    let owner = repo::schools(&g, "owner", vec![], vec![], true)
        .await
        .unwrap();
    assert_eq!(
        years(owner),
        vec!["school:2026/2027:2", "school:2025/2026:1"]
    );
    let other = repo::schools(
        &g,
        "elsewhere",
        vec!["school-2".into()],
        vec!["tenant".into()],
        false,
    )
    .await
    .unwrap();
    assert!(other.is_empty());
    // A school administrator cannot widen the lists past their own scope.
    let widened = repo::schools(&g, "elsewhere", vec![], vec![], true)
        .await
        .unwrap();
    assert!(widened.is_empty());
}
