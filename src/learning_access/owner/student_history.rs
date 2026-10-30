use super::*;
use axum::extract::Path;

/// One Student's enrolments across the schools of the tenant, for the Learning pathway (Learning ADR 0017 §7): the
/// years a pathway shows come from SIS identity, never from a shared Learning scope. Each enrolment carries its entry
/// year and grade and the class it is in now (or was in last); `years` adds the class of each academic year recorded
/// since roster moves keep one (`enrolment_history`). The caller is pinned to one school as for `authorize` and may
/// read only a Student they already see there: their own record, their child, a class roster, or a
/// `learning.mastery.read` roster.
const ENROLMENTS: &str = "MATCH (s:Student {studentId:$student})-[:ENROLLED_AS]->(e:EnrolledStudent {tenant_id:$tenant}) WHERE coalesce(e.school_id,'')<>'' OPTIONAL MATCH (e)-[:ENROLLED_IN]->(sec:Section {tenant_id:$tenant}) WHERE sec.school_id=e.school_id OPTIONAL MATCH (school:School {school_id:e.school_id,tenant_id:$tenant}) WITH e,school,collect(DISTINCT sec) AS sections RETURN e.school_id AS school_id,coalesce(school.school_name,school.name) AS school_name,e.status AS status,e.entry_year AS entry_year,e.year_group AS entry_grade,e.graduation_year AS graduation_year,size(sections) AS classes,head(sections).section_id AS class_id,head(sections).name AS class_name,head(sections).year_group AS class_grade,head(sections).academic_year AS class_year ORDER BY entry_year,school_id LIMIT 9";
const MAX_ENROLMENTS: usize = 8;

fn unavailable<E>(_: E) -> Failure {
    failure(StatusCode::SERVICE_UNAVAILABLE)
}
fn text(row: &neo4rs::Row, name: &str, max: usize) -> Option<String> {
    row.get::<Option<String>>(name)
        .ok()
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v.chars().count() <= max && !v.chars().any(char::is_control))
}
fn year(row: &neo4rs::Row, name: &str) -> Option<i64> {
    row.get::<Option<i64>>(name)
        .ok()
        .flatten()
        .filter(|y| (1990..=2100).contains(y))
}

/// Whether the caller sees `student` in this school: in their projection (Student: only themselves), or, for staff,
/// in the classes they may read mastery of (ADR 0018).
async fn visible(s: &AppState, a: &Actor, scope: &Scope, student: &str) -> Result<(), Failure> {
    let projected = projection(s, a, scope).await?;
    let ids: BTreeSet<&str> = projected
        .iter()
        .flat_map(|c| c["students"].as_array().into_iter().flatten())
        .filter_map(|s| s["id"].as_str())
        .collect();
    let seen = match a.role.as_str() {
        "student" => ids.len() == 1 && ids.contains(student),
        "parent" => ids.contains(student),
        "teacher" | "owner" => {
            ids.contains(student)
                || crate::authorization::reads::mastery_roster(
                    &s.graph,
                    &scope.tenant_id,
                    &scope.school_id,
                    &a.id,
                    None,
                    None,
                )
                .await
                .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
                .is_some_and(|(classes, _)| {
                    classes
                        .iter()
                        .any(|c| c.students.iter().any(|m| m.id == student))
                })
        }
        _ => false,
    };
    if seen {
        Ok(())
    } else {
        Err(failure(StatusCode::FORBIDDEN))
    }
}

pub(super) async fn read(
    s: &AppState,
    a: &Actor,
    scope: &Scope,
    student: &str,
) -> Result<(Vec<Value>, Vec<Value>), Failure> {
    visible(s, a, scope, student).await?;
    let mut rows = s
        .graph
        .execute(
            query(ENROLMENTS)
                .param("student", student)
                .param("tenant", scope.tenant_id.clone()),
        )
        .await
        .map_err(unavailable)?;
    let mut enrolments = vec![];
    while let Some(row) = rows.next().await.map_err(unavailable)? {
        let school: String = row.get("school_id").map_err(unavailable)?;
        let classes: i64 = row.get("classes").map_err(unavailable)?;
        if enrolments.len() == MAX_ENROLMENTS || !valid_id(&school) || classes > 1 {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        let class = match text(&row, "class_id", 128).filter(|id| valid_id(id)) {
            Some(id) => {
                json!({"id":id,"name":text(&row,"class_name",256).unwrap_or_else(|| id.clone()),
                "yearGroup":text(&row,"class_grade",64),"academicYear":text(&row,"class_year",32)})
            }
            None => Value::Null,
        };
        enrolments.push(json!({
            "schoolId": school,
            "schoolName": text(&row, "school_name", 256),
            "status": text(&row, "status", 32).unwrap_or_else(|| "unknown".into()),
            "entryYear": year(&row, "entry_year"),
            "entryYearGroup": text(&row, "entry_grade", 64),
            "graduationYear": year(&row, "graduation_year"),
            "class": class,
        }));
    }
    // The exact years recorded on roster moves, handovers and the backfill (`enrolment_history`).
    let mut rows = s
        .graph
        .execute(
            query(crate::enrolment_history::READ)
                .param("student", student)
                .param("tenant", scope.tenant_id.clone()),
        )
        .await
        .map_err(unavailable)?;
    let mut years = vec![];
    while let Some(row) = rows.next().await.map_err(unavailable)? {
        let school: String = row.get("school_id").map_err(unavailable)?;
        let Some(code) = text(&row, "year_code", 16) else {
            continue;
        };
        if years.len() == crate::enrolment_history::MAX_YEARS || !valid_id(&school) {
            return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
        }
        let class = match text(&row, "class_id", 128).filter(|id| valid_id(id)) {
            Some(id) => {
                json!({"id":id,"name":text(&row,"class_name",256).unwrap_or_else(|| id.clone())})
            }
            None => Value::Null,
        };
        years.push(json!({
            "schoolId": school,
            "schoolName": text(&row, "school_name", 256),
            "academicYearCode": code,
            "academicYear": text(&row, "year", 32),
            "gradeCode": text(&row, "grade_code", 16),
            "yearGroup": text(&row, "year_group", 64),
            "class": class,
            "source": text(&row, "source", 32).unwrap_or_else(|| "roster".into()),
        }));
    }
    // A grant or binding removed while reading answers nothing, as Learning's own reads do.
    visible(s, a, scope, student).await?;
    Ok((enrolments, years))
}

pub(super) async fn handler(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(student): Path<String>,
    Query(q): Query<Scope>,
) -> Result<axum::response::Response, Failure> {
    use axum::response::IntoResponse;
    if !valid_id(&q.school_id)
        || !valid_id(&q.tenant_id)
        || !valid_id(&student)
        || q.class_id.is_some()
        || q.student_id.is_some()
        || q.access.is_some()
    {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let a = actor(&h, &q).await?;
    super::super::ensure_schema(&s.graph).await?;
    let (enrolments, years) = read(&s, &a, &q, &student).await?;
    let mut response = Json(
        json!({"data":{"contractVersion":1,"databaseId":identity(&s.graph).await?,
        "actor":{"id":a.id,"role":a.role},"schoolId":q.school_id,"tenantId":q.tenant_id,
        "studentId":student,"enrolments":enrolments,"years":years}}),
    )
    .into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
