// SIS persistence (spec §3.4 — school structure, SIS side).
//
// Graph edges maintained here:
//   (EnrolledStudent)-[:ENROLLED_IN]->(Section)
//
// Constraints:
//   Section.section_id unique
//   A given EnrolledStudent can be ENROLLED_IN exactly one Section at a
//   time — the assign fn removes any existing edge before creating the
//   new one. Mid-year section moves are a later feature; for v0.1 the
//   straight-line case (one section per kid per year) is enough.

use chrono::Utc;
use neo4rs::{query, Graph, Node};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::models::section_model::{is_valid_section_status, Section, SECTION_STATUS_ACTIVE};
use crate::repositories::lead_repository::RepositoryError;

pub async fn init_section_indexes(graph: &Graph) -> Result<(), RepositoryError> {
    graph
        .run(query(
            "CREATE CONSTRAINT section_id_unique IF NOT EXISTS \
             FOR (s:Section) REQUIRE s.section_id IS UNIQUE",
        ))
        .await
        .map_err(|e| RepositoryError::DbError(e.to_string()))?;
    Ok(())
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSectionInput {
    pub school_id: String,
    pub name: String,
    pub year_group: String,
    pub academic_year: String,
    /// Optional; defaults to "TENANT-001" so MVP callers don't need to
    /// pass tenant context. Kept explicit for future multi-tenant work.
    pub tenant_id: Option<String>,
}

pub async fn create_section(
    graph: &Graph,
    input: &CreateSectionInput,
) -> Result<Section, RepositoryError> {
    if input.school_id.trim().is_empty()
        || input.name.trim().is_empty()
        || input.year_group.trim().is_empty()
        || input.academic_year.trim().is_empty()
    {
        return Err(RepositoryError::DbError("all fields required".into()));
    }
    let section_id = format!("SEC-{}", Uuid::new_v4());
    let now = Utc::now();
    let tenant = input.tenant_id.clone().unwrap_or_else(|| "TENANT-001".to_string());
    let q = query(
        "CREATE (s:Section { \
            section_id: $id, tenant_id: $tenant, school_id: $school, \
            name: $name, year_group: $year_group, academic_year: $academic_year, \
            status: $status, \
            created_at: datetime($now), updated_at: datetime($now) \
         }) RETURN s"
    )
    .param("id", section_id.clone())
    .param("tenant", tenant.clone())
    .param("school", input.school_id.clone())
    .param("name", input.name.clone())
    .param("year_group", input.year_group.clone())
    .param("academic_year", input.academic_year.clone())
    .param("status", SECTION_STATUS_ACTIVE.to_string())
    .param("now", now.to_rfc3339());
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    if let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        let node: Node = row.get("s").map_err(|e| RepositoryError::DbError(e.to_string()))?;
        return Ok(map_node_to_section(node, 0));
    }
    Err(RepositoryError::DbError("failed to create section".into()))
}

#[derive(Debug, Default, Deserialize)]
pub struct ListSectionFilters {
    pub school: Option<String>,
    pub academic_year: Option<String>,
    pub year_group: Option<String>,
    pub status: Option<String>,
}

pub async fn list_sections(
    graph: &Graph,
    filters: &ListSectionFilters,
) -> Result<Vec<Section>, RepositoryError> {
    let mut wheres: Vec<String> = Vec::new();
    if filters.school.is_some() { wheres.push("s.school_id = $school".to_string()); }
    if filters.academic_year.is_some() { wheres.push("s.academic_year = $academic_year".to_string()); }
    if filters.year_group.is_some() { wheres.push("s.year_group = $year_group".to_string()); }
    if filters.status.is_some() { wheres.push("s.status = $status".to_string()); }
    let where_sql = if wheres.is_empty() { String::new() } else { format!("WHERE {}", wheres.join(" AND ")) };

    let cy = format!(
        "MATCH (s:Section) {where_sql} \
         OPTIONAL MATCH (e:EnrolledStudent)-[:ENROLLED_IN]->(s) \
         WITH s, count(DISTINCT e) AS enrolledCount \
         RETURN s, enrolledCount \
         ORDER BY s.academic_year DESC, s.school_id, s.year_group, s.name"
    );
    let mut q = query(&cy);
    if let Some(v) = &filters.school { q = q.param("school", v.clone()); }
    if let Some(v) = &filters.academic_year { q = q.param("academic_year", v.clone()); }
    if let Some(v) = &filters.year_group { q = q.param("year_group", v.clone()); }
    if let Some(v) = &filters.status { q = q.param("status", v.clone()); }
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        let node: Node = row.get("s").map_err(|e| RepositoryError::DbError(e.to_string()))?;
        let cnt = row.get::<i64>("enrolledCount").unwrap_or(0);
        out.push(map_node_to_section(node, cnt));
    }
    Ok(out)
}

pub async fn find_section_by_id(
    graph: &Graph,
    section_id: &str,
) -> Result<Option<Section>, RepositoryError> {
    let q = query(
        "MATCH (s:Section {section_id: $id}) \
         OPTIONAL MATCH (e:EnrolledStudent)-[:ENROLLED_IN]->(s) \
         RETURN s, count(DISTINCT e) AS enrolledCount"
    )
    .param("id", section_id.to_string());
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    if let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        let node: Node = row.get("s").map_err(|e| RepositoryError::DbError(e.to_string()))?;
        let cnt = row.get::<i64>("enrolledCount").unwrap_or(0);
        return Ok(Some(map_node_to_section(node, cnt)));
    }
    Ok(None)
}

/// Admin can update the free-text homeroom teacher fields. Either field
/// can be None to clear it. Leaves other Section properties alone.
pub async fn set_homeroom_teacher(
    graph: &Graph,
    section_id: &str,
    name: Option<&str>,
    email: Option<&str>,
) -> Result<Section, RepositoryError> {
    let now = Utc::now();
    let q = query(
        "MATCH (s:Section {section_id: $id}) \
         SET s.homeroom_teacher_name = $name, \
             s.homeroom_teacher_email = $email, \
             s.updated_at = datetime($now) \
         OPTIONAL MATCH (e:EnrolledStudent)-[:ENROLLED_IN]->(s) \
         RETURN s, count(DISTINCT e) AS enrolledCount"
    )
    .param("id", section_id.to_string())
    .param("name", name.unwrap_or(""))
    .param("email", email.unwrap_or(""))
    .param("now", now.to_rfc3339());
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    if let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        let node: Node = row.get("s").map_err(|e| RepositoryError::DbError(e.to_string()))?;
        let cnt = row.get::<i64>("enrolledCount").unwrap_or(0);
        return Ok(map_node_to_section(node, cnt));
    }
    Err(RepositoryError::NotFound)
}

pub async fn set_section_status(
    graph: &Graph,
    section_id: &str,
    status: &str,
) -> Result<Section, RepositoryError> {
    if !is_valid_section_status(status) {
        return Err(RepositoryError::DbError(format!("invalid section status: {}", status)));
    }
    let now = Utc::now();
    let q = query(
        "MATCH (s:Section {section_id: $id}) \
         SET s.status = $status, s.updated_at = datetime($now) \
         RETURN s"
    )
    .param("id", section_id.to_string())
    .param("status", status.to_string())
    .param("now", now.to_rfc3339());
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    if let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        let node: Node = row.get("s").map_err(|e| RepositoryError::DbError(e.to_string()))?;
        return Ok(map_node_to_section(node, 0));
    }
    Err(RepositoryError::NotFound)
}

/// Assigns a set of EnrolledStudents to a Section. Removes any existing
/// `[:ENROLLED_IN]` edges from each student first so a kid is never in
/// two sections at once. Returns the number of successfully assigned
/// students (students that don't exist as EnrolledStudent are silently
/// skipped — caller can diff the response against the input to know).
pub async fn assign_students(
    graph: &Graph,
    section_id: &str,
    applicant_student_ids: &[String],
) -> Result<i64, RepositoryError> {
    let now = Utc::now();
    let q = query(
        "MATCH (sec:Section {section_id: $section_id}) \
         UNWIND $ids AS appId \
         MATCH (s:Student {studentId: appId})-[:ENROLLED_AS]->(e:EnrolledStudent) \
         OPTIONAL MATCH (e)-[old:ENROLLED_IN]->(:Section) \
         DELETE old \
         MERGE (e)-[r:ENROLLED_IN]->(sec) \
         ON CREATE SET r.assigned_at = datetime($now) \
         SET sec.updated_at = datetime($now) \
         RETURN count(DISTINCT e) AS assigned"
    )
    .param("section_id", section_id.to_string())
    .param("ids", applicant_student_ids.to_vec())
    .param("now", now.to_rfc3339());
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    if let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        return Ok(row.get::<i64>("assigned").unwrap_or(0));
    }
    Ok(0)
}

#[derive(Clone, serde::Serialize, ToSchema)]
#[allow(non_snake_case)]
pub struct SectionMemberRow {
    pub applicantStudentId: String,
    pub studentNumber: String,
    pub fullName: String,
    pub yearGroup: Option<String>,
    pub parentName: String,
    pub parentEmail: String,
}

pub async fn list_section_members(
    graph: &Graph,
    section_id: &str,
) -> Result<Vec<SectionMemberRow>, RepositoryError> {
    let q = query(
        "MATCH (e:EnrolledStudent)-[:ENROLLED_IN]->(sec:Section {section_id: $id}) \
         MATCH (s:Student)-[:ENROLLED_AS]->(e) \
         OPTIONAL MATCH (l:Lead)-[:HAS_STUDENT]->(s) \
         RETURN \
           s.studentId AS applicantStudentId, \
           e.student_number AS studentNumber, \
           s.fullName AS fullName, \
           e.year_group AS yearGroup, \
           coalesce(l.parent_name, '') AS parentName, \
           coalesce(l.email, '') AS parentEmail \
         ORDER BY s.fullName"
    )
    .param("id", section_id.to_string());
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        out.push(SectionMemberRow {
            applicantStudentId: row.get::<String>("applicantStudentId").unwrap_or_default(),
            studentNumber: row.get::<String>("studentNumber").unwrap_or_default(),
            fullName: row.get::<String>("fullName").unwrap_or_default(),
            yearGroup: row.get::<String>("yearGroup").ok().filter(|s| !s.is_empty()),
            parentName: row.get::<String>("parentName").unwrap_or_default(),
            parentEmail: row.get::<String>("parentEmail").unwrap_or_default(),
        });
    }
    Ok(out)
}

/// Parent-facing: find Sections (plus parent-friendly context) for every
/// kid an authed parent owns. Returns one row per kid that IS assigned;
/// kids without an ENROLLED_IN edge are not in the response.
#[derive(Clone, serde::Serialize, ToSchema)]
#[allow(non_snake_case)]
pub struct ParentSectionRow {
    pub applicantStudentId: String,
    pub studentName: String,
    pub studentNumber: String,
    pub sectionId: String,
    pub sectionName: String,
    pub yearGroup: String,
    pub academicYear: String,
    pub schoolId: String,
    pub homeroomTeacherName: Option<String>,
    pub homeroomTeacherEmail: Option<String>,
}

pub async fn list_parent_sections(
    graph: &Graph,
    lead_ids: &[String],
) -> Result<Vec<ParentSectionRow>, RepositoryError> {
    let q = query(
        "MATCH (l:Lead)-[:HAS_STUDENT]->(s:Student)-[:ENROLLED_AS]->(e:EnrolledStudent)-[:ENROLLED_IN]->(sec:Section) \
         WHERE l.lead_id IN $lead_ids \
         RETURN \
           s.studentId AS applicantStudentId, \
           s.fullName AS studentName, \
           e.student_number AS studentNumber, \
           sec.section_id AS sectionId, \
           sec.name AS sectionName, \
           sec.year_group AS yearGroup, \
           sec.academic_year AS academicYear, \
           sec.school_id AS schoolId, \
           sec.homeroom_teacher_name AS homeroomTeacherName, \
           sec.homeroom_teacher_email AS homeroomTeacherEmail \
         ORDER BY s.fullName"
    )
    .param("lead_ids", lead_ids.to_vec());
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        out.push(ParentSectionRow {
            applicantStudentId: row.get::<String>("applicantStudentId").unwrap_or_default(),
            studentName: row.get::<String>("studentName").unwrap_or_default(),
            studentNumber: row.get::<String>("studentNumber").unwrap_or_default(),
            sectionId: row.get::<String>("sectionId").unwrap_or_default(),
            sectionName: row.get::<String>("sectionName").unwrap_or_default(),
            yearGroup: row.get::<String>("yearGroup").unwrap_or_default(),
            academicYear: row.get::<String>("academicYear").unwrap_or_default(),
            schoolId: row.get::<String>("schoolId").unwrap_or_default(),
            homeroomTeacherName: row.get::<String>("homeroomTeacherName").ok().filter(|s| !s.is_empty()),
            homeroomTeacherEmail: row.get::<String>("homeroomTeacherEmail").ok().filter(|s| !s.is_empty()),
        });
    }
    Ok(out)
}

// ---- Attendance ------------------------------------------------------------

use crate::models::attendance_model::{is_valid_attendance_status, AttendanceRecord};

#[derive(Clone, Debug, Deserialize)]
pub struct BulkAttendanceEntry {
    pub applicant_student_id: String,
    pub status: String,
    pub notes: Option<String>,
}

/// Upserts attendance for a set of students on one date. Idempotent —
/// MERGE keyed on (section_id, applicant_student_id, date) so replaying
/// the same call corrects instead of duplicating. Invalid status values
/// are silently skipped and counted; admin UI can diff requested vs
/// written to surface the drop.
pub async fn upsert_attendance_batch(
    graph: &Graph,
    section_id: &str,
    date: &str,
    entries: &[BulkAttendanceEntry],
    recorded_by: &str,
) -> Result<i64, RepositoryError> {
    if entries.is_empty() {
        return Ok(0);
    }
    // Cypher-side: UNWIND over the entries and MERGE per row. Cheaper
    // than round-trip-per-student and stays atomic under neo4j's
    // transaction boundary.
    let valid: Vec<_> = entries.iter()
        .filter(|e| is_valid_attendance_status(&e.status) && !e.applicant_student_id.trim().is_empty())
        .collect();
    if valid.is_empty() {
        return Ok(0);
    }
    // Flatten into parallel arrays so neo4rs can pass them as params
    // without needing a custom parameter type.
    let ids: Vec<String> = valid.iter().map(|e| e.applicant_student_id.clone()).collect();
    let statuses: Vec<String> = valid.iter().map(|e| e.status.clone()).collect();
    let notes: Vec<String> = valid.iter().map(|e| e.notes.clone().unwrap_or_default()).collect();

    let q = query(
        "MATCH (sec:Section {section_id: $section_id}) \
         UNWIND range(0, size($ids) - 1) AS i \
         WITH sec, $ids[i] AS sid, $statuses[i] AS st, $notes[i] AS nt \
         MATCH (s:Student {studentId: sid})-[:ENROLLED_AS]->(e:EnrolledStudent)-[:ENROLLED_IN]->(sec) \
         MERGE (r:AttendanceRecord { \
            section_id: $section_id, \
            applicant_student_id: sid, \
            date: $date \
         }) \
         ON CREATE SET r.record_id = 'ATT-' + randomUUID() \
         SET r.status = st, \
             r.notes = nt, \
             r.recorded_at = datetime(), \
             r.recorded_by = $by \
         MERGE (s)-[:HAS_ATTENDANCE]->(r) \
         RETURN count(DISTINCT r) AS n"
    )
    .param("section_id", section_id.to_string())
    .param("date", date.to_string())
    .param("ids", ids)
    .param("statuses", statuses)
    .param("notes", notes)
    .param("by", recorded_by.to_string());

    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    if let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        return Ok(row.get::<i64>("n").unwrap_or(0));
    }
    Ok(0)
}

/// Roster view for one date. Returns one row per student currently
/// assigned to the section, with their attendance status if recorded
/// (None if not yet). Lets the admin UI render the full section as a
/// checklist and pre-fill known statuses.
#[derive(Clone, Serialize, utoipa::ToSchema)]
#[allow(non_snake_case)]
pub struct AttendanceRosterRow {
    pub applicantStudentId: String,
    pub studentNumber: String,
    pub fullName: String,
    pub status: Option<String>,
    pub notes: Option<String>,
    pub recordedAt: Option<String>,
    pub recordedBy: Option<String>,
}

pub async fn list_attendance_for_date(
    graph: &Graph,
    section_id: &str,
    date: &str,
) -> Result<Vec<AttendanceRosterRow>, RepositoryError> {
    let q = query(
        "MATCH (e:EnrolledStudent)-[:ENROLLED_IN]->(sec:Section {section_id: $section_id}) \
         MATCH (s:Student)-[:ENROLLED_AS]->(e) \
         OPTIONAL MATCH (r:AttendanceRecord {section_id: $section_id, applicant_student_id: s.studentId, date: $date}) \
         RETURN \
           s.studentId AS applicantStudentId, \
           e.student_number AS studentNumber, \
           s.fullName AS fullName, \
           r.status AS status, \
           r.notes AS notes, \
           toString(r.recorded_at) AS recordedAt, \
           r.recorded_by AS recordedBy \
         ORDER BY s.fullName"
    )
    .param("section_id", section_id.to_string())
    .param("date", date.to_string());
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        out.push(AttendanceRosterRow {
            applicantStudentId: row.get::<String>("applicantStudentId").unwrap_or_default(),
            studentNumber: row.get::<String>("studentNumber").unwrap_or_default(),
            fullName: row.get::<String>("fullName").unwrap_or_default(),
            status: row.get::<String>("status").ok().filter(|s| !s.is_empty()),
            notes: row.get::<String>("notes").ok().filter(|s| !s.is_empty()),
            recordedAt: row.get::<String>("recordedAt").ok().filter(|s| !s.is_empty() && s != "null"),
            recordedBy: row.get::<String>("recordedBy").ok().filter(|s| !s.is_empty()),
        });
    }
    Ok(out)
}

/// Parent-scoped attendance window — returns records across all
/// sections the given leads' kids belong to, for a date range.
#[derive(Clone, Serialize, utoipa::ToSchema)]
#[allow(non_snake_case)]
pub struct ParentAttendanceRow {
    pub applicantStudentId: String,
    pub studentName: String,
    pub sectionName: String,
    pub date: String,
    pub status: String,
}

pub async fn list_attendance_for_parent(
    graph: &Graph,
    lead_ids: &[String],
    from_date: &str,
    to_date: &str,
) -> Result<Vec<ParentAttendanceRow>, RepositoryError> {
    let q = query(
        "MATCH (l:Lead)-[:HAS_STUDENT]->(s:Student)-[:ENROLLED_AS]->(e:EnrolledStudent)-[:ENROLLED_IN]->(sec:Section) \
         WHERE l.lead_id IN $lead_ids \
         MATCH (r:AttendanceRecord {section_id: sec.section_id, applicant_student_id: s.studentId}) \
         WHERE r.date >= $from AND r.date <= $to \
         RETURN \
           s.studentId AS applicantStudentId, \
           s.fullName AS studentName, \
           sec.name AS sectionName, \
           r.date AS date, \
           r.status AS status \
         ORDER BY r.date DESC, s.fullName"
    )
    .param("lead_ids", lead_ids.to_vec())
    .param("from", from_date.to_string())
    .param("to", to_date.to_string());
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        out.push(ParentAttendanceRow {
            applicantStudentId: row.get::<String>("applicantStudentId").unwrap_or_default(),
            studentName: row.get::<String>("studentName").unwrap_or_default(),
            sectionName: row.get::<String>("sectionName").unwrap_or_default(),
            date: row.get::<String>("date").unwrap_or_default(),
            status: row.get::<String>("status").unwrap_or_default(),
        });
    }
    Ok(out)
}

// Kept around so AttendanceRecord isn't flagged as unused while we
// grow the domain. Next patch will surface individual records to
// admins for correction after initial roll-call.
#[allow(dead_code)]
fn _keep_attendance_model_import(r: AttendanceRecord) -> AttendanceRecord { r }

// ---- end attendance -------------------------------------------------------

// ---- Grades --------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
pub struct BulkGradeEntry {
    pub applicant_student_id: String,
    pub score: f64,
    pub max_score: f64,
    pub notes: Option<String>,
}

/// Upsert grades for a set of students against one (subject, term).
/// Same idempotency pattern as attendance — MERGE keyed on (section,
/// student, subject, term). Score range validated at the handler;
/// repository only requires max_score > 0.
pub async fn upsert_grades_batch(
    graph: &Graph,
    section_id: &str,
    subject: &str,
    term: &str,
    entries: &[BulkGradeEntry],
    recorded_by: &str,
) -> Result<i64, RepositoryError> {
    if entries.is_empty() {
        return Ok(0);
    }
    let valid: Vec<_> = entries.iter()
        .filter(|e| e.max_score > 0.0 && e.score >= 0.0 && e.score <= e.max_score && !e.applicant_student_id.trim().is_empty())
        .collect();
    if valid.is_empty() {
        return Ok(0);
    }
    let ids: Vec<String> = valid.iter().map(|e| e.applicant_student_id.clone()).collect();
    let scores: Vec<f64> = valid.iter().map(|e| e.score).collect();
    let maxes: Vec<f64> = valid.iter().map(|e| e.max_score).collect();
    let notes: Vec<String> = valid.iter().map(|e| e.notes.clone().unwrap_or_default()).collect();

    let q = query(
        "MATCH (sec:Section {section_id: $section_id}) \
         UNWIND range(0, size($ids) - 1) AS i \
         WITH sec, $ids[i] AS sid, $scores[i] AS sc, $maxes[i] AS ms, $notes[i] AS nt \
         MATCH (s:Student {studentId: sid})-[:ENROLLED_AS]->(e:EnrolledStudent)-[:ENROLLED_IN]->(sec) \
         MERGE (g:GradeEntry { \
            section_id: $section_id, applicant_student_id: sid, \
            subject: $subject, term: $term \
         }) \
         ON CREATE SET g.entry_id = 'GRD-' + randomUUID() \
         SET g.score = sc, g.max_score = ms, g.notes = nt, \
             g.recorded_at = datetime(), g.recorded_by = $by \
         MERGE (s)-[:HAS_GRADE]->(g) \
         RETURN count(DISTINCT g) AS n"
    )
    .param("section_id", section_id.to_string())
    .param("subject", subject.to_string())
    .param("term", term.to_string())
    .param("ids", ids)
    .param("scores", scores)
    .param("maxes", maxes)
    .param("notes", notes)
    .param("by", recorded_by.to_string());

    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    if let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        return Ok(row.get::<i64>("n").unwrap_or(0));
    }
    Ok(0)
}

#[derive(Clone, Serialize, utoipa::ToSchema)]
#[allow(non_snake_case)]
pub struct GradeRosterRow {
    pub applicantStudentId: String,
    pub studentNumber: String,
    pub fullName: String,
    pub score: Option<f64>,
    pub maxScore: Option<f64>,
    pub recordedAt: Option<String>,
    pub recordedBy: Option<String>,
}

pub async fn list_grades_for_subject_term(
    graph: &Graph,
    section_id: &str,
    subject: &str,
    term: &str,
) -> Result<Vec<GradeRosterRow>, RepositoryError> {
    let q = query(
        "MATCH (e:EnrolledStudent)-[:ENROLLED_IN]->(sec:Section {section_id: $section_id}) \
         MATCH (s:Student)-[:ENROLLED_AS]->(e) \
         OPTIONAL MATCH (g:GradeEntry { \
            section_id: $section_id, applicant_student_id: s.studentId, \
            subject: $subject, term: $term \
         }) \
         RETURN \
           s.studentId AS applicantStudentId, \
           e.student_number AS studentNumber, \
           s.fullName AS fullName, \
           g.score AS score, \
           g.max_score AS maxScore, \
           toString(g.recorded_at) AS recordedAt, \
           g.recorded_by AS recordedBy \
         ORDER BY s.fullName"
    )
    .param("section_id", section_id.to_string())
    .param("subject", subject.to_string())
    .param("term", term.to_string());
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        out.push(GradeRosterRow {
            applicantStudentId: row.get::<String>("applicantStudentId").unwrap_or_default(),
            studentNumber: row.get::<String>("studentNumber").unwrap_or_default(),
            fullName: row.get::<String>("fullName").unwrap_or_default(),
            score: row.get::<f64>("score").ok(),
            maxScore: row.get::<f64>("maxScore").ok(),
            recordedAt: row.get::<String>("recordedAt").ok().filter(|s| !s.is_empty() && s != "null"),
            recordedBy: row.get::<String>("recordedBy").ok().filter(|s| !s.is_empty()),
        });
    }
    Ok(out)
}

#[derive(Clone, Serialize, utoipa::ToSchema)]
#[allow(non_snake_case)]
pub struct ParentGradeRow {
    pub applicantStudentId: String,
    pub studentName: String,
    pub sectionName: String,
    pub subject: String,
    pub term: String,
    pub score: f64,
    pub maxScore: f64,
    pub recordedAt: String,
}

pub async fn list_grades_for_parent(
    graph: &Graph,
    lead_ids: &[String],
) -> Result<Vec<ParentGradeRow>, RepositoryError> {
    let q = query(
        "MATCH (l:Lead)-[:HAS_STUDENT]->(s:Student)-[:ENROLLED_AS]->(e:EnrolledStudent)-[:ENROLLED_IN]->(sec:Section) \
         WHERE l.lead_id IN $lead_ids \
         MATCH (g:GradeEntry {section_id: sec.section_id, applicant_student_id: s.studentId}) \
         RETURN \
           s.studentId AS applicantStudentId, \
           s.fullName AS studentName, \
           sec.name AS sectionName, \
           g.subject AS subject, \
           g.term AS term, \
           g.score AS score, \
           g.max_score AS maxScore, \
           toString(g.recorded_at) AS recordedAt \
         ORDER BY g.recorded_at DESC, s.fullName"
    )
    .param("lead_ids", lead_ids.to_vec());
    let mut rs = graph.execute(q).await.map_err(|e| RepositoryError::DbError(e.to_string()))?;
    let mut out = Vec::new();
    while let Some(row) = rs.next().await.map_err(|e| RepositoryError::DbError(e.to_string()))? {
        out.push(ParentGradeRow {
            applicantStudentId: row.get::<String>("applicantStudentId").unwrap_or_default(),
            studentName: row.get::<String>("studentName").unwrap_or_default(),
            sectionName: row.get::<String>("sectionName").unwrap_or_default(),
            subject: row.get::<String>("subject").unwrap_or_default(),
            term: row.get::<String>("term").unwrap_or_default(),
            score: row.get::<f64>("score").unwrap_or(0.0),
            maxScore: row.get::<f64>("maxScore").unwrap_or(0.0),
            recordedAt: row.get::<String>("recordedAt").unwrap_or_default(),
        });
    }
    Ok(out)
}

// ---- end grades -----------------------------------------------------------

fn map_node_to_section(node: Node, enrolled_count: i64) -> Section {
    Section {
        sectionId: node.get::<String>("section_id").unwrap_or_default(),
        tenantId: node.get::<String>("tenant_id").unwrap_or_default(),
        schoolId: node.get::<String>("school_id").unwrap_or_default(),
        name: node.get::<String>("name").unwrap_or_default(),
        yearGroup: node.get::<String>("year_group").unwrap_or_default(),
        academicYear: node.get::<String>("academic_year").unwrap_or_default(),
        status: node.get::<String>("status").unwrap_or_else(|_| SECTION_STATUS_ACTIVE.to_string()),
        enrolledCount: enrolled_count,
        homeroomTeacherName: node.get::<String>("homeroom_teacher_name").ok().filter(|s| !s.is_empty()),
        homeroomTeacherEmail: node.get::<String>("homeroom_teacher_email").ok().filter(|s| !s.is_empty()),
        createdAt: parse_dt(node.get::<String>("created_at").ok()),
        updatedAt: parse_dt(node.get::<String>("updated_at").ok()),
    }
}

fn parse_dt(val: Option<String>) -> chrono::DateTime<chrono::Utc> {
    val.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now)
}
