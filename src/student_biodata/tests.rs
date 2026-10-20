use super::model::*;
use super::repository::{canonical, GENDER_WORDS, RELATIONSHIP_WORDS};
use serde_json::{json, Value};

const GUARDIAN_ID: &str = "0b7c5a52-6f0e-4a8e-9d55-2f5b8d7c1a10";

fn body() -> Value {
    json!({
        "tenantId": "TENANT-001", "schoolId": "SCH-IISS", "studentId": "APP-STU-1",
        "expectedVersion": 0,
        "biodata": {"nisn": "1328909120", "birthPlace": " Jakarta ", "gender": "female", "emergencyContact": "+62 812 1111 2222"},
        "admissionsGuardianRelationship": "mother",
        "guardians": [{"id": null, "fullName": "Budi Santoso", "relationship": "father", "phonePrimary": "0812-3456-789", "phoneSecondary": null, "email": "budi@example.com"}]
    })
}
fn parse(v: Value) -> Result<Change, serde_json::Error> {
    serde_json::from_value(v)
}
fn clean(v: Value) -> Option<Clean> {
    parse(v).ok()?.clean()
}
fn with(path: &str, value: Value) -> Value {
    let mut b = body();
    *b.pointer_mut(path).unwrap() = value;
    b
}

#[test]
fn a_valid_change_is_trimmed_and_kept_in_order() {
    let c = clean(body()).unwrap();
    assert_eq!(c.biodata.birth_place.as_deref(), Some("Jakarta"));
    assert_eq!(c.biodata.nisn.as_deref(), Some("1328909120"));
    assert_eq!(c.relationship.as_deref(), Some("mother"));
    assert_eq!(c.guardians.len(), 1);
    assert_eq!(c.guardians[0].0, None);
    assert_eq!(c.guardians[0].1.relationship, "father");
}

#[test]
fn blank_values_clear_the_field() {
    let mut b = body();
    b["biodata"] = json!({"nisn": "", "birthPlace": "   ", "gender": null, "emergencyContact": null});
    b["admissionsGuardianRelationship"] = Value::Null;
    b["guardians"] = json!([]);
    let c = clean(b).unwrap();
    assert_eq!(c.biodata, Biodata::default());
    assert_eq!(c.relationship, None);
    assert!(c.guardians.is_empty());
}

#[test]
fn every_field_must_be_present_and_known() {
    let mut missing = body();
    missing["biodata"].as_object_mut().unwrap().remove("nisn");
    assert!(parse(missing).is_err());
    let mut missing = body();
    missing.as_object_mut().unwrap().remove("admissionsGuardianRelationship");
    assert!(parse(missing).is_err());
    let mut missing = body();
    missing["guardians"][0].as_object_mut().unwrap().remove("phoneSecondary");
    assert!(parse(missing).is_err());
    let mut extra = body();
    extra["biodata"]["religion"] = json!("x");
    assert!(parse(extra).is_err());
    let mut extra = body();
    extra["studentName"] = json!("x");
    assert!(parse(extra).is_err());
}

#[test]
fn malformed_values_are_refused() {
    for (path, value) in [
        ("/biodata/nisn", json!("132890912")),
        ("/biodata/nisn", json!("13289091201")),
        ("/biodata/nisn", json!("13289O9120")),
        ("/biodata/gender", json!("L")),
        ("/biodata/gender", json!("Male")),
        ("/biodata/birthPlace", json!("x".repeat(129))),
        ("/biodata/birthPlace", json!("Jak\u{7}arta")),
        ("/biodata/emergencyContact", json!("62+812345")),
        ("/biodata/emergencyContact", json!("12345")),
        ("/biodata/emergencyContact", json!("call me")),
        ("/admissionsGuardianRelationship", json!("ayah")),
        ("/guardians/0/fullName", json!("  ")),
        ("/guardians/0/relationship", json!("uncle")),
        ("/guardians/0/phonePrimary", json!("")),
        ("/guardians/0/phoneSecondary", json!("0812 3456 789")),
        ("/guardians/0/email", json!("a@b@c.com")),
        ("/guardians/0/email", json!("budi@localhost")),
        ("/guardians/0/id", json!("not-a-uuid")),
        ("/tenantId", json!("TENANT|1")),
        ("/expectedVersion", json!(-1)),
    ] {
        assert!(clean(with(path, value.clone())).is_none(), "{path} = {value}");
    }
}

#[test]
fn at_most_three_guardians_with_distinct_ids() {
    let g = body()["guardians"][0].clone();
    assert!(clean(with("/guardians", json!([g, g, g]))).is_some());
    assert!(clean(with("/guardians", json!([g, g, g, g]))).is_none());
    let mut a = g.clone();
    a["id"] = json!(GUARDIAN_ID);
    assert!(clean(with("/guardians", json!([a, a]))).is_none());
}

fn guardian(name: &str) -> Guardian {
    Guardian {
        full_name: name.into(),
        relationship: "father".into(),
        phone_primary: "0812-3456-789".into(),
        phone_secondary: None,
        email: None,
    }
}

#[test]
fn diff_names_fields_and_counts_guardians() {
    let old = Biodata { nisn: Some("1328909120".into()), gender: Some("L".into()), ..Biodata::default() };
    let second = "6a1f1c1e-1111-4c4c-9c9c-111111111111".to_string();
    let olds = vec![(GUARDIAN_ID.to_string(), guardian("Budi")), (second.clone(), guardian("Ani"))];
    let new = Clean {
        biodata: Biodata { nisn: Some("1328909120".into()), gender: Some("male".into()), ..Biodata::default() },
        relationship: Some("mother".into()),
        guardians: vec![(Some(GUARDIAN_ID.into()), guardian("Budi S.")), (None, guardian("Sri"))],
    };
    let d = diff(&old, &None, &olds, &new).unwrap();
    assert_eq!(d.fields, vec!["gender", "admissionsGuardianRelationship"]);
    assert_eq!((d.added, d.updated.clone(), d.removed.clone(), d.reordered), (1, vec![GUARDIAN_ID.to_string()], vec![second.clone()], false));

    let same = Clean { biodata: old.clone(), relationship: None, guardians: vec![(Some(GUARDIAN_ID.into()), guardian("Budi")), (Some(second.clone()), guardian("Ani"))] };
    assert!(diff(&old, &None, &olds, &same).unwrap().is_empty());

    let swapped = Clean { guardians: vec![(Some(second.clone()), guardian("Ani")), (Some(GUARDIAN_ID.into()), guardian("Budi"))], ..same };
    assert!(diff(&old, &None, &olds, &swapped).unwrap().reordered);

    let stale = Clean { biodata: old.clone(), relationship: None, guardians: vec![(Some("7d1f1c1e-1111-4c4c-9c9c-111111111111".into()), guardian("Budi"))] };
    assert!(diff(&old, &None, &olds, &stale).is_err());
}

#[test]
fn older_admissions_words_read_as_canonical_values() {
    assert_eq!(canonical(Some("Laki-laki".into()), &GENDER_WORDS).as_deref(), Some("male"));
    assert_eq!(canonical(Some(" P ".into()), &GENDER_WORDS).as_deref(), Some("female"));
    assert_eq!(canonical(Some("other".into()), &GENDER_WORDS), None);
    assert_eq!(canonical(Some("Ayah".into()), &RELATIONSHIP_WORDS).as_deref(), Some("father"));
    assert_eq!(canonical(Some("wali".into()), &RELATIONSHIP_WORDS).as_deref(), Some("guardian"));
    assert_eq!(canonical(None, &RELATIONSHIP_WORDS), None);
}

/// Disposable graph only: STUDENT_BIODATA_TEST_NEO4J_URI (+ _USER/_PASSWORD); data carries a `biodata-test-<uuid>` mark.
#[tokio::test]
#[ignore = "explicit isolated STUDENT_BIODATA_TEST_NEO4J_URI only"]
async fn admin_saves_biodata_with_version_audit_and_scope() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use super::repository::{enrolment, read, save, Saved};
    use neo4rs::{query, Graph};
    let graph = Graph::new(
        std::env::var("STUDENT_BIODATA_TEST_NEO4J_URI")?,
        std::env::var("STUDENT_BIODATA_TEST_NEO4J_USER")?,
        std::env::var("STUDENT_BIODATA_TEST_NEO4J_PASSWORD")?,
    )
    .await?;
    super::init(&graph).await?;
    let m = format!("biodata-test-{}", uuid::Uuid::new_v4());
    let (admin, outsider, student) = (format!("{m}-admin"), format!("{m}-outsider"), format!("{m}-stu"));
    graph.run(query("CREATE (:School {school_id:$m,tenant_id:$m,mark:$m}) CREATE (:School {school_id:$m+'-other',tenant_id:$m,mark:$m})
        CREATE (:StaffMember {id:$admin,membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:[$m],tenantIds:[$m],mark:$m})
        CREATE (:StaffMember {id:$outsider,membershipStatus:'ACTIVE',roles:['school_admin'],schoolIds:[$m+'-other'],tenantIds:[$m],mark:$m})
        CREATE (s:Student {studentId:$student,fullName:'Aisha Putri',dateOfBirth:'2013-04-02',gender:'P',mark:$m})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:$m,tenant_id:$m,mark:$m})
        CREATE (:Lead {lead_id:$m+'-lead',parent_name:'Siti Rahma',relationship:'Ibu',mobile:'0812 1111 2222',whatsapp:'0812 1111 2222',email:'siti@example.com',mark:$m})-[:HAS_STUDENT]->(s)
        CREATE (:Student {studentId:$m+'-taken',nisn:'9999999999',mark:$m})")
        .param("m", m.clone()).param("admin", admin.clone()).param("outsider", outsider.clone()).param("student", student.clone())).await?;
    let scope = Scope { tenant_id: m.clone(), school_id: m.clone(), student_id: student.clone() };
    let outcome = |s: Result<Saved, super::Failure>| match s {
        Ok(Saved::Version(v)) => format!("v{v}"),
        Ok(Saved::Conflict) => "conflict".into(),
        Ok(Saved::NisnInUse) => "nisn".into(),
        Ok(Saved::NotFound) => "missing".into(),
        Ok(Saved::Invalid) => "invalid".into(),
        Err(e) => format!("failure {}", e.0),
    };
    let result = async {
        let found = enrolment(&graph, &student).await.map_err(|e| format!("enrolment {}", e.0))?.ok_or("no enrolment")?;
        assert_eq!((found.school_id.as_str(), found.tenant_id.as_str()), (m.as_str(), m.as_str()));
        assert!(enrolment(&graph, &format!("{m}-taken")).await.map_err(|e| format!("enrolment {}", e.0))?.is_none());
        let first = read(&graph, &admin, &scope).await.map_err(|e| format!("read {}", e.0))?.ok_or("not readable")?;
        assert_eq!(first["version"], 0);
        assert_eq!(first["biodata"]["gender"], "female");
        assert_eq!(first["admissionsGuardian"]["relationship"], "mother");
        assert_eq!(first["admissionsGuardian"]["phones"], json!(["0812 1111 2222"]));
        assert_eq!(first["guardians"], json!([]));

        let change = clean(with("/studentId", json!(student.clone()))).unwrap();
        assert_eq!(outcome(save(&graph, &admin, &change, &scope, 0).await), "v1");
        assert_eq!(outcome(save(&graph, &admin, &change, &scope, 0).await), "conflict");
        let taken = clean(with("/biodata/nisn", json!("9999999999"))).unwrap();
        assert_eq!(outcome(save(&graph, &admin, &taken, &scope, 1).await), "nisn");

        let saved = read(&graph, &admin, &scope).await.map_err(|e| format!("read {}", e.0))?.ok_or("gone")?;
        assert_eq!(saved["biodata"], json!({"nisn":"1328909120","birthPlace":"Jakarta","gender":"female","emergencyContact":"+62 812 1111 2222"}));
        let id = saved["guardians"][0]["id"].as_str().ok_or("no guardian id")?.to_string();
        assert_eq!(saved["guardians"][0]["fullName"], "Budi Santoso");
        let unchanged = clean(with("/guardians/0/id", json!(id.clone()))).unwrap();
        assert_eq!(outcome(save(&graph, &admin, &unchanged, &scope, 1).await), "v1", "an unchanged save writes nothing");

        let mut next = with("/guardians", json!([{"id": id, "fullName": "Budi Santoso", "relationship": "guardian", "phonePrimary": "0812-3456-789", "phoneSecondary": null, "email": null}]));
        next["admissionsGuardianRelationship"] = json!("father");
        let next = clean(next).unwrap();
        assert_eq!(outcome(save(&graph, &admin, &next, &scope, 1).await), "v2");
        let cleared = clean(with("/guardians", json!([]))).unwrap();
        assert_eq!(outcome(save(&graph, &admin, &cleared, &scope, 2).await), "v3");
        let after = read(&graph, &admin, &scope).await.map_err(|e| format!("read {}", e.0))?.ok_or("gone")?;
        assert_eq!(after["guardians"], json!([]));
        assert_eq!(after["admissionsGuardian"]["relationship"], "mother");

        assert!(read(&graph, &outsider, &scope).await.map_err(|e| format!("read {}", e.0))?.is_none());
        let other = Scope { tenant_id: m.clone(), school_id: format!("{m}-other"), student_id: student.clone() };
        assert!(read(&graph, &outsider, &other).await.map_err(|e| format!("read {}", e.0))?.is_none(), "not enrolled at the other school");
        assert_eq!(outcome(save(&graph, &outsider, &change, &other, 3).await), "missing");

        let mut rows = graph.execute(query("MATCH (a:StudentBiodataAudit {student_id:$student}) RETURN a ORDER BY a.version").param("student", student.clone())).await?;
        let mut audits = vec![];
        while let Some(row) = rows.next().await? {
            let node: neo4rs::Node = row.get("a")?;
            let mut keys: Vec<String> = node.keys().into_iter().map(String::from).collect();
            keys.sort();
            audits.push((node.get::<i64>("version")?, node.get::<Vec<String>>("changed_fields")?, keys));
        }
        assert_eq!(audits.len(), 3);
        assert_eq!(audits[0].1, vec!["nisn", "birthPlace", "gender", "emergencyContact", "admissionsGuardianRelationship"]);
        assert_eq!(audits[1].1, vec!["admissionsGuardianRelationship"]);
        assert_eq!(audits[0].2, vec!["actor_id", "actor_role", "changed_fields", "created_at", "guardians_added", "guardians_removed", "guardians_reordered", "guardians_updated", "key", "school_id", "student_id", "tenant_id", "version"]);
        let mut rows = graph.execute(query("MATCH (l:Lead {lead_id:$lead}) RETURN l.relationship AS r").param("lead", format!("{m}-lead"))).await?;
        assert_eq!(rows.next().await?.ok_or("lead")?.get::<String>("r")?, "mother");
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;
    graph.run(query("MATCH (n) WHERE n.mark=$m OR (n:Guardian AND n.school_id=$m) OR (n:StudentBiodataAudit AND n.school_id=$m) DETACH DELETE n").param("m", m)).await?;
    result
}
