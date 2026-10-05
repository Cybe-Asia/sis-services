use super::*;

/** Student bearer and current governed enrollment determine every returned record. */
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
    let classes = projection(s, &a, q).await?;
    let students = classes
        .iter()
        .flat_map(|c| c["students"].as_array().into_iter().flatten())
        .filter_map(|s| s["id"].as_str())
        .collect::<BTreeSet<_>>();
    if students.len() != 1 {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let student = *students.first().unwrap();
    let query_for = |body: &str| {
        query(&format!("{STUDENT} WITH DISTINCT s,c {body}"))
            .param("principal", a.id.clone())
            .param("school", q.school_id.clone())
            .param("tenant", q.tenant_id.clone())
    };
    let mut attendance = vec![];
    let mut rows=s.graph.execute(query_for("MATCH(r:AttendanceRecord {section_id:c.section_id,applicant_student_id:s.studentId}) RETURN DISTINCT r.date AS date,r.status AS status,r.arrived_at AS arrived,r.dismissed_at AS dismissed ORDER BY date DESC LIMIT 1000")).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    {
        attendance.push(json!({"studentId":student,"date":row.get::<String>("date").map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?,"status":row.get::<String>("status").map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?,"arrivedAt":row.get::<String>("arrived").ok().filter(|v|!v.is_empty()),"dismissedAt":row.get::<String>("dismissed").ok().filter(|v|!v.is_empty())}));
    }
    let education_calendar=crate::education_calendar::repository::published(&s.graph,query_for(&format!("WITH s,c AS sec {} RETURN DISTINCT r.payload AS payload,r.version AS version,s.studentId AS student,r.start_date AS calendar_start,r.id AS calendar_id ORDER BY calendar_start,calendar_id LIMIT 101",crate::education_calendar::repository::PUBLISHED))).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let class_meetings=crate::learning_access::timetable::reads::published(&s.graph,query_for(&format!("WITH s,c AS sec {} RETURN DISTINCT m.payload AS payload,s.studentId AS student,m.starts_at AS starts,m.id AS id ORDER BY starts,id LIMIT 201",crate::learning_access::timetable::reads::PUBLISHED))).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
    // Reserve the response's fixed owner/roster/attendance bytes before sharing
    // the remaining budget across record kinds. Valid history never disables this read.
    let base_bytes = serde_json::to_vec(
        &json!({"classes":classes,"attendance":attendance,"educationCalendar":education_calendar,"classMeetings":class_meetings}),
    )
    .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
    .len();
    let per_kind_budget = 1_900_000usize.saturating_sub(base_bytes) / 4;
    let mut records = vec![];
    let mut has_more = false;
    for kind in ["event", "project", "grade", "report"] {
        let mut used = 0;
        let mut count = 0;
        let mut rows = s.graph.execute(query_for("MATCH(r:SchoolPublishedRecord)-[:FOR_SECTION]->(c) WHERE r.kind=$kind AND (r.student_id='' OR r.student_id=s.studentId) RETURN DISTINCT s.studentId AS student,r.payload AS payload,toString(r.created_at) AS created ORDER BY created DESC LIMIT 101").param("kind",kind)).await.map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?;
        while let Some(row) = rows
            .next()
            .await
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        {
            count += 1;
            if count > 100 {
                has_more = true;
                break;
            }
            let raw: String = row
                .get("payload")
                .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
            if raw.len() > 524_288 {
                return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
            }
            let content: Value =
                serde_json::from_str(&raw).map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
            let entry = json!({"studentId":student,"content":content,"createdAt":row.get::<String>("created").map_err(|_|failure(StatusCode::SERVICE_UNAVAILABLE))?});
            let bytes = serde_json::to_vec(&entry)
                .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
                .len()
                + 1;
            if used + bytes > per_kind_budget {
                has_more = true;
                break;
            }
            used += bytes;
            records.push(entry);
        }
    }
    let data = json!({"contractVersion":1,"actor":{"id":a.id,"role":"student"},"schoolId":q.school_id,"tenantId":q.tenant_id,"studentId":student,"classes":classes,"records":records,"educationCalendar":education_calendar,"classMeetings":class_meetings,"attendance":attendance,"recordWindow":{"perKindLimit":100,"hasMore":has_more}});
    if serde_json::to_vec(&data)
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?
        .len()
        > 2_000_000
    {
        return Err(failure(StatusCode::SERVICE_UNAVAILABLE));
    }
    Ok(data)
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
    #[tokio::test]
    #[ignore = "requires dedicated SCHOOL_PORTAL_TEST_BOLT=127.0.0.1:3223"]
    async fn escaped_history_remains_available_and_current_student_bound() {
        assert_eq!(
            std::env::var("SCHOOL_PORTAL_TEST_BOLT").unwrap(),
            "127.0.0.1:3223"
        );
        let graph = neo4rs::Graph::new("127.0.0.1:3223", "neo4j", "parent-otp-fixture-password")
            .await
            .unwrap();
        let secret = "student-read-test-key-at-least-32-bytes";
        std::env::set_var("JWT_SECRET", secret);
        let s = AppState {
            graph: graph.clone(),
            config: crate::config::config::AppConfig {
                server_port: 0,
                neo4j_uri: "127.0.0.1:3223".into(),
                neo4j_user: "neo4j".into(),
                neo4j_password: "parent-otp-fixture-password".into(),
                jwt_secret: secret.into(),
            },
            http_client: reqwest::Client::new(),
        };
        let key = uuid::Uuid::new_v4().to_string();
        graph.run(query("CREATE(:User {id:$key,role:'student',test_key:$key}) CREATE(:LearningStudentBinding {user_id:$key,student_id:$key,status:'ACTIVE',school_id:'student-school',tenant_id:'student-tenant',test_key:$key}) CREATE(:Student {studentId:$key,fullName:'Student',test_key:$key})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'student-school',tenant_id:'student-tenant',test_key:$key})-[:ENROLLED_IN]->(:Section {section_id:$key,name:'Class',status:'active',school_id:'student-school',tenant_id:'student-tenant',test_key:$key})").param("key",key.clone())).await.unwrap();
        for index in 0..100 {
            let record = crate::school_portal::model::PublishedRecord::Project {
                id: format!("p-{index}"),
                student_id: key.clone(),
                title: "Project".into(),
                subject: "Science".into(),
                summary: "\u{1}".repeat(4000),
                state: "completed".into(),
            };
            assert!(record.valid());
            graph.run(query("MATCH(c:Section {section_id:$key}) CREATE(:SchoolPublishedRecord {id:$id,kind:'project',student_id:$key,payload:$payload,created_at:datetime(),test_key:$key})-[:FOR_SECTION]->(c)").param("key",key.clone()).param("id",record.id().to_string()).param("payload",serde_json::to_string(&record).unwrap())).await.unwrap();
        }
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &json!({"sub":key,"role":"student","exp":chrono::Utc::now().timestamp()+300}),
            &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();
        let mut h = HeaderMap::new();
        h.insert("Authorization", format!("Bearer {token}").parse().unwrap());
        let mut q = Scope {
            school_id: "student-school".into(),
            tenant_id: "student-tenant".into(),
            class_id: None,
            student_id: None,
            access: None,
        };
        let result = read(&s, &h, &q).await.unwrap();
        assert_eq!(result["studentId"], key);
        assert_eq!(result["recordWindow"]["hasMore"], true);
        let rows = result["records"].as_array().unwrap();
        assert!(!rows.is_empty() && rows.len() < 100);
        assert!(serde_json::to_vec(&json!({"data":result})).unwrap().len() < 2_000_000);
        q.tenant_id = "foreign".into();
        assert!(read(&s, &h, &q).await.is_err());
        q.tenant_id = "student-tenant".into();
        graph
            .run(
                query("MATCH(b:LearningStudentBinding {user_id:$key}) SET b.status='REVOKED'")
                    .param("key", key.clone()),
            )
            .await
            .unwrap();
        assert!(read(&s, &h, &q).await.is_err());
        graph
            .run(query("MATCH(n {test_key:$key}) DETACH DELETE n").param("key", key))
            .await
            .unwrap();
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StudentWindow {
    school_id: String,
    tenant_id: String,
    from: String,
    to: String,
    after: Option<String>,
}
pub(super) async fn calendar(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<StudentWindow>,
) -> Result<Json<Value>, Failure> {
    let scope = Scope {
        school_id: q.school_id.clone(),
        tenant_id: q.tenant_id.clone(),
        class_id: None,
        student_id: None,
        access: None,
    };
    let w = crate::education_calendar::window::Window {
        from: q.from,
        to: q.to,
        after: q.after,
        student_id: None,
    };
    if !w.valid() || !valid_id(&q.school_id) || !valid_id(&q.tenant_id) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let a = actor(&h, &scope).await?;
    if a.role != "student" {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let classes = projection(&s, &a, &scope).await?;
    let students = classes
        .iter()
        .flat_map(|c| c["students"].as_array().into_iter().flatten())
        .filter_map(|s| s["id"].as_str())
        .collect::<BTreeSet<_>>();
    if students.len() != 1 {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let q = query(&format!(
        "{STUDENT} WITH DISTINCT s,c AS sec {} {}",
        crate::education_calendar::repository::PUBLISHED,
        w.clause()
    ))
    .param("principal", a.id.clone())
    .param("school", scope.school_id.clone())
    .param("tenant", scope.tenant_id.clone())
    .param("from", w.from.clone())
    .param("to", w.to.clone())
    .param(
        "after",
        w.cursor().map_err(|_| failure(StatusCode::BAD_REQUEST))?,
    );
    let data = crate::education_calendar::window::page(&s.graph, q)
        .await
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let (start, end) = crate::learning_access::timetable::reads::window_ms(&w.from, &w.to)
        .map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    let query=query(&format!("{STUDENT} WITH DISTINCT s,c AS sec {} AND m.starts_at<$end AND m.starts_at>=$earliest RETURN DISTINCT m.payload AS payload,s.studentId AS student,m.starts_at AS starts,m.id AS id ORDER BY starts,id LIMIT 201",crate::learning_access::timetable::reads::PUBLISHED)).param("principal",a.id.clone()).param("school",scope.school_id.clone()).param("tenant",scope.tenant_id.clone()).param("end",end as i64).param("earliest",start.saturating_sub(12*3_600_000) as i64);
    let class_meetings =
        crate::learning_access::timetable::reads::published_in_window(&s.graph, query, start, end)
            .await
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    Ok(Json(
        json!({"data":{"contractVersion":1,"actor":{"id":a.id,"role":a.role},"studentId":students.first(),"schoolId":scope.school_id,"tenantId":scope.tenant_id,"educationCalendar":data,"classMeetings":class_meetings}}),
    ))
}
