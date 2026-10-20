use super::*;

/** The signed-in Student's own biodata, classes and guardians. The caller never names a student:
 * the bearer, its active binding and the current enrollment decide whose record is returned.
 * Fields the school has not recorded yet are null, never invented. */
async fn read(s: &AppState, h: &HeaderMap, q: &Scope) -> Result<Value, Failure> {
    if !valid_id(&q.school_id)
        || !valid_id(&q.tenant_id)
        || q.class_id.is_some()
        || q.student_id.is_some()
        || q.access.is_some()
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let a = actor(h, q).await?;
    if a.role != "student" {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let projected = projection(s, &a, q).await?;
    let students = projected
        .iter()
        .flat_map(|c| c["students"].as_array().into_iter().flatten())
        .filter_map(|s| s["id"].as_str())
        .collect::<BTreeSet<_>>();
    if students.len() != 1 {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let student = (*students.first().unwrap()).to_string();
    let query_for = |body: &str| {
        query(&format!("{STUDENT} WITH DISTINCT p,s,c {body}"))
            .param("principal", a.id.clone())
            .param("school", q.school_id.clone())
            .param("tenant", q.tenant_id.clone())
    };

    let mut rows = s.graph.execute(query_for("OPTIONAL MATCH (school:School {school_id:$school,tenant_id:$tenant}) RETURN DISTINCT p.email AS email,s.fullName AS name,s.nisn AS nisn,s.birthPlace AS birth_place,s.dateOfBirth AS birth_date,s.gender AS gender,s.emergencyContact AS emergency,school.name AS school_name LIMIT 2")).await.map_err(unavailable)?;
    let row = rows
        .next()
        .await
        .map_err(unavailable)?
        .ok_or_else(|| failure(StatusCode::FORBIDDEN))?;
    if rows.next().await.map_err(unavailable)?.is_some() {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    let field = |name: &str| row.get::<Option<String>>(name).ok().flatten();
    let name =
        bounded(field("name"), 256).ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let profile = json!({
        "fullName": name,
        "email": email(field("email")),
        "nisn": nisn(field("nisn")),
        "birthPlace": bounded(field("birth_place"), 128),
        "dateOfBirth": day(field("birth_date")),
        "gender": gender(field("gender")),
        "emergencyContact": phone(field("emergency")),
    });
    let school = json!({"id": q.school_id, "name": bounded(field("school_name"), 256)});

    let mut classes = vec![];
    let mut rows = s.graph.execute(query_for("RETURN DISTINCT c.section_id AS id,c.name AS name,c.year_group AS grade,c.academic_year AS year ORDER BY id LIMIT 51")).await.map_err(unavailable)?;
    while let Some(row) = rows.next().await.map_err(unavailable)? {
        let id: String = row.get("id").map_err(unavailable)?;
        let name = bounded(row.get::<Option<String>>("name").ok().flatten(), 256);
        if !valid_id(&id) || name.is_none() || classes.len() == 50 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        classes.push(json!({"id":id,"name":name,"grade":bounded(row.get::<Option<String>>("grade").ok().flatten(),64),"academicYear":bounded(row.get::<Option<String>>("year").ok().flatten(),32)}));
    }

    // The admissions account holder is the first guardian; guardians the school records later
    // (`HAS_GUARDIAN`) follow. A row without a name is not shown.
    let mut guardians = vec![];
    let mut rows = s.graph.execute(query_for("WITH DISTINCT s CALL { WITH s OPTIONAL MATCH (l:Lead)-[:HAS_STUDENT]->(s) RETURN 0 AS source,l.lead_id AS key,l.parent_name AS name,l.relationship AS relationship,l.mobile AS phone1,l.whatsapp AS phone2,l.email AS email UNION ALL WITH s OPTIONAL MATCH (s)-[:HAS_GUARDIAN]->(g:Guardian) RETURN 1 AS source,g.guardian_id AS key,g.full_name AS name,g.relationship AS relationship,g.phone_primary AS phone1,g.phone_secondary AS phone2,g.email AS email } RETURN source,key,name,relationship,phone1,phone2,email ORDER BY source,key LIMIT 6")).await.map_err(unavailable)?;
    while let Some(row) = rows.next().await.map_err(unavailable)? {
        let get = |name: &str| row.get::<Option<String>>(name).ok().flatten();
        if let Some(guardian) = guardian(
            get("name"),
            get("relationship"),
            get("phone1"),
            get("phone2"),
            get("email"),
        ) {
            if guardians.len() < 4 {
                guardians.push(guardian);
            }
        }
    }
    Ok(
        json!({"contractVersion":1,"actor":{"id":a.id,"role":"student"},"schoolId":q.school_id,"tenantId":q.tenant_id,"studentId":student,"profile":profile,"school":school,"classes":classes,"guardians":guardians}),
    )
}

fn unavailable<E>(_: E) -> Failure {
    failure(StatusCode::SERVICE_UNAVAILABLE)
}
fn bounded(value: Option<String>, max: usize) -> Option<String> {
    let value = value?.trim().to_string();
    (!value.is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control))
        .then_some(value)
}
fn email(value: Option<String>) -> Option<String> {
    bounded(value, 254).filter(|v| {
        let mut parts = v.split('@');
        matches!((parts.next(), parts.next(), parts.next()), (Some(l), Some(d), None) if !l.is_empty() && d.contains('.'))
            && !v.contains(char::is_whitespace)
    })
}
fn phone(value: Option<String>) -> Option<String> {
    bounded(value, 32).filter(|v| {
        let digits = v.chars().filter(char::is_ascii_digit).count();
        (6..=15).contains(&digits)
            && v.chars().enumerate().all(|(i, c)| {
                c.is_ascii_digit() || matches!(c, ' ' | '-' | '(' | ')') || (i == 0 && c == '+')
            })
    })
}
fn nisn(value: Option<String>) -> Option<String> {
    bounded(value, 10).filter(|v| v.len() == 10 && v.chars().all(|c| c.is_ascii_digit()))
}
fn day(value: Option<String>) -> Option<String> {
    let value = bounded(value, 10)?;
    chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d")
        .ok()
        .filter(|d| d.format("%Y-%m-%d").to_string() == value)
        .map(|_| value)
}
fn gender(value: Option<String>) -> Option<&'static str> {
    match bounded(value, 16)?.to_ascii_lowercase().as_str() {
        "male" | "m" | "laki-laki" | "l" => Some("male"),
        "female" | "f" | "perempuan" | "p" => Some("female"),
        _ => None,
    }
}
fn relationship(value: Option<String>) -> Option<&'static str> {
    match bounded(value, 32)?.to_ascii_lowercase().as_str() {
        "father" | "ayah" => Some("father"),
        "mother" | "ibu" => Some("mother"),
        "guardian" | "wali" => Some("guardian"),
        _ => None,
    }
}
fn guardian(
    name: Option<String>,
    kind: Option<String>,
    first: Option<String>,
    second: Option<String>,
    mail: Option<String>,
) -> Option<Value> {
    let name = bounded(name, 256)?;
    let first = phone(first);
    let second = phone(second).filter(|v| Some(v) != first.as_ref());
    let (first, second) = match first {
        Some(first) => (Some(first), second),
        None => (second, None),
    };
    let phones = [first, second].into_iter().flatten().collect::<Vec<_>>();
    Some(json!({"name":name,"relationship":relationship(kind),"phones":phones,"email":email(mail)}))
}

pub(super) async fn handler(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<Scope>,
) -> Result<Json<Value>, Failure> {
    tokio::time::timeout(std::time::Duration::from_secs(5), read(&s, &h, &q))
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .map(|data| Json(json!({"data":data})))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn some(v: &str) -> Option<String> {
        Some(v.into())
    }
    #[test]
    fn unrecorded_or_malformed_fields_are_null() {
        assert_eq!(nisn(some("1328909120")), some("1328909120"));
        assert_eq!(nisn(some("13289091201")), None);
        assert_eq!(nisn(some("13289O9120")), None);
        assert_eq!(day(some("2006-10-12")), some("2006-10-12"));
        assert_eq!(day(some("2006-02-30")), None);
        assert_eq!(day(some("12/10/2006")), None);
        assert_eq!(gender(some("Laki-laki")), Some("male"));
        assert_eq!(gender(some("P")), Some("female"));
        assert_eq!(gender(some("other")), None);
        assert_eq!(
            phone(some("+62 21 180 190 1111")),
            some("+62 21 180 190 1111")
        );
        assert_eq!(phone(some("0812-3456-789")), some("0812-3456-789"));
        assert_eq!(phone(some("62+812")), None);
        assert_eq!(phone(some("call me")), None);
        assert_eq!(email(some("sultan@benua.com")), some("sultan@benua.com"));
        assert_eq!(email(some("a@b@c.com")), None);
        assert_eq!(bounded(some("   "), 10), None);
        assert_eq!(bounded(some("Jakarta\n"), 10), some("Jakarta"));
        assert_eq!(bounded(some("Jak\u{0}arta"), 10), None);
    }
    #[test]
    fn guardian_phones_skip_duplicates_and_nameless_rows() {
        let g = guardian(
            some("Mr. Sultan Benua"),
            some("Ayah"),
            some("+62 21 180 190 1111"),
            some("+62 21 180 190 1111"),
            some("sultan@benua.com"),
        )
        .unwrap();
        assert_eq!(
            g,
            json!({"name":"Mr. Sultan Benua","relationship":"father","phones":["+62 21 180 190 1111"],"email":"sultan@benua.com"})
        );
        let g = guardian(some("Wali"), None, None, some("0812 1111 2222"), None).unwrap();
        assert_eq!(g["phones"], json!(["0812 1111 2222"]));
        assert_eq!(g["relationship"], Value::Null);
        assert!(guardian(None, some("mother"), some("0812 1111 2222"), None, None).is_none());
    }
}
