use super::*;

/// The school's active classes for a Learning module's class assignments (Learning ADR 0016):
/// grade and academic year of each, and the subjects the calling Teacher is assigned to teach
/// there. Staff only; no student appears in the answer.
pub(super) async fn read(s: &AppState, a: &Actor, q: &Scope) -> Result<Vec<Value>, Failure> {
    super::super::ensure_schema(&s.graph).await?;
    let gate = match a.role.as_str() {
        "owner" => format!(
            "MATCH(p:StaffMember {{id:$principal,membershipStatus:'ACTIVE'}}) WHERE {}",
            crate::school_portal::staff_capability::owner("p", "$school", "$tenant")
        ),
        "teacher" => "MATCH(p:StaffMember {id:$principal,membershipStatus:'ACTIVE'}) WHERE NOT 'owner' IN coalesce(p.roles,[]) AND 'teacher' IN coalesce(p.roles,[]) AND $school IN coalesce(p.schoolIds,[]) AND $tenant IN coalesce(p.tenantIds,[])".into(),
        _ => return Err(failure(StatusCode::FORBIDDEN)),
    };
    let run = |body: String| {
        query(&body)
            .param("principal", a.id.clone())
            .param("school", q.school_id.clone())
            .param("tenant", q.tenant_id.clone())
    };
    let unavailable = |_| failure(StatusCode::SERVICE_UNAVAILABLE);
    let mut rows = s
        .graph
        .execute(run(format!("{gate} RETURN count(p) AS n")))
        .await
        .map_err(unavailable)?;
    let n: i64 = rows
        .next()
        .await
        .map_err(unavailable)?
        .and_then(|r| r.get("n").ok())
        .ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    if n != 1 {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let mut rows = s.graph.execute(run(format!("{gate} WITH p MATCH(sec:Section {{school_id:$school,tenant_id:$tenant,status:'active'}}) WHERE NOT EXISTS {{MATCH(o:Section {{section_id:sec.section_id}}) WHERE o<>sec}} OPTIONAL MATCH(t:TeachingAssignment {{staff_member_id:p.id,section_id:sec.section_id,school_id:$school,tenant_id:$tenant,status:'active'}}) WHERE t.academic_year=sec.academic_year WITH sec,collect(DISTINCT t.subject) AS subjects RETURN sec.section_id AS id,coalesce(sec.name,sec.section_id) AS name,sec.year_group AS grade,sec.academic_year AS year,subjects ORDER BY id LIMIT 201")))
        .await.map_err(unavailable)?;
    let mut classes = vec![];
    while let Some(row) = rows.next().await.map_err(unavailable)? {
        if classes.len() == 200 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        let id: String = row
            .get("id")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let name: String = row
            .get("name")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let text = |key: &str| {
            row.get::<Option<String>>(key)
                .map(|v| v.filter(|v| !v.is_empty() && v.len() <= 64))
                .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))
        };
        let (grade, year) = (text("grade")?, text("year")?);
        let subjects: Vec<String> = row
            .get("subjects")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        if !valid_id(&id)
            || name.is_empty()
            || name.len() > 256
            || subjects.len() > 40
            || subjects.iter().any(|s| !valid_id(s))
        {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        classes.push(json!({"id":id,"name":name,"yearGroup":grade,"academicYear":year,"taughtSubjects":subjects}));
    }
    Ok(classes)
}

pub(super) async fn handler(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<Scope>,
) -> Result<Json<Value>, Failure> {
    if !valid_id(&q.school_id)
        || !valid_id(&q.tenant_id)
        || q.class_id.is_some()
        || q.student_id.is_some()
        || q.access.is_some()
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let a = actor(&h, &q).await?;
    let classes = read(&s, &a, &q).await?;
    Ok(Json(
        json!({"data":{"contractVersion":1,"databaseId":identity(&s.graph).await?,"actor":{"id":a.id,"role":a.role},"schoolId":q.school_id,"tenantId":q.tenant_id,"classes":classes}}),
    ))
}
