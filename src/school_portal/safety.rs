//! ADR 0025 and `docs/surveillance/anti-bullying-spec.md` phase 1. SIS owns the human reports
//! ("Lapor") and the camera alerts; the cameras never name a person. A report is read by its
//! reporter, the homeroom of the student's class and school Owners; every staff read is audited.
use super::{
    auth, envelope, failure,
    model::{self, identifier},
    repository::{self, PARENT, PARENT_SECTION},
    staff_capability::Selection,
    student_identity, Failure,
};
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use neo4rs::{query, Graph, Query as GraphQuery, Row};
use serde::Deserialize;
use serde_json::{json, Value};
type Error = Box<dyn std::error::Error + Send + Sync>;

pub const PLACES: [&str; 8] = [
    "classroom", "corridor", "toilet", "canteen", "yard", "outside", "online", "other",
];
const OUTCOMES: [&str; 4] = ["handled", "unproven", "referred_to_parents", "other"];
const RATINGS: [&str; 3] = ["true", "false_alarm", "unchecked"];
/// Camera event types that page the roster; the rest are only listed.
const ALERTED: [&str; 6] = [
    "crowd", "restricted", "loitering", "after_hours", "camera_tampered", "camera_offline",
];
/// The signed-in Student's own active enrolment, from the account alone (no caller-chosen school): the Student
/// client knows its class, not its school ids. Same account rules as `student_preferences::OWNED`.
pub(super) const OWNED: &str = "MATCH(u:User {id:$actor,role:'student'}) MATCH(b:LearningStudentBinding {user_id:u.id,status:'ACTIVE'}) MATCH(s:Student {studentId:b.student_id})-[:ENROLLED_AS]->(en:EnrolledStudent {status:'active',school_id:b.school_id,tenant_id:b.tenant_id})-[:ENROLLED_IN]->(sec:Section {status:'active',school_id:b.school_id,tenant_id:b.tenant_id}) WHERE coalesce(u.staffMemberId,'')='' AND all(role IN coalesce(u.roles,[]) WHERE role='student') AND all(role IN coalesce(u.marketingRoles,[]) WHERE role='student') AND NOT (u)-[:STAFF_PROFILE]->() AND NOT EXISTS {MATCH(staff:StaffMember) WHERE toLower(staff.email)=toLower(u.email)}";
const PHOTO_PREFIX: &str = "data:image/jpeg;base64,";
// shortcut: the photo is stored base64 on the node (<= 400 KB, like LearningCheckIn); move to the
// MinIO attachment path of ADR 0024 §9 when reports carry documents.
const PHOTO_MAX: usize = 400_000;

const REPORT: &str = "r.id AS id,r.public_id AS publicId,r.student_id AS studentId,coalesce(s.fullName,'') AS studentName,r.section_id AS sectionId,r.reporter_kind AS reporterKind,r.about AS about,r.happened_on AS happenedOn,coalesce(r.happened_at,'') AS happenedAt,r.place AS place,coalesce(r.people,'') AS people,r.text AS text,r.hide_name AS hideName,r.photo IS NOT NULL AS hasPhoto,r.status AS status,coalesce(r.outcome,'') AS outcome,toString(r.created_at) AS createdAt,toString(r.updated_at) AS updatedAt";
const CREATE: &str = "MERGE(r:SafetyReport {id:$id}) ON CREATE SET r.public_id=randomUUID(),r.school_id=sec.school_id,r.tenant_id=sec.tenant_id,r.section_id=sec.section_id,r.student_id=s.studentId,r.reporter_kind=$kind,r.reporter_id=u.id,r.reporter_name=reporter,r.about=$about,r.happened_on=$date,r.happened_at=$time,r.place=$place,r.people=$people,r.text=$text,r.hide_name=$hide,r.photo=CASE WHEN $photo='' THEN null ELSE $photo END,r.status='new',r.created_at=datetime(),r.updated_at=datetime() MERGE(r)-[:FOR_STUDENT]->(s)";

fn report(r: &Row) -> Result<Value, Error> {
    let s = |name: &str| r.get::<String>(name);
    Ok(json!({
        "id": s("id")?, "publicId": s("publicId")?, "studentId": s("studentId")?, "studentName": s("studentName")?,
        "sectionId": s("sectionId")?, "reporterKind": s("reporterKind")?, "about": s("about")?,
        "happenedOn": s("happenedOn")?, "happenedAt": s("happenedAt")?, "place": s("place")?, "people": s("people")?,
        "text": s("text")?, "hideName": r.get::<bool>("hideName")?, "hasPhoto": r.get::<bool>("hasPhoto")?,
        "status": s("status")?, "outcome": s("outcome")?, "createdAt": s("createdAt")?, "updatedAt": s("updatedAt")?,
    }))
}
async fn reports(graph: &Graph, q: GraphQuery) -> Result<Vec<Value>, Error> {
    let mut rows = graph.execute(q).await?;
    let mut out = vec![];
    while let Some(r) = rows.next().await? {
        let v = report(&r)?;
        if !out.iter().any(|x: &Value| x["id"] == v["id"]) {
            out.push(v);
        }
    }
    Ok(out)
}
fn unavailable(_: Error) -> Failure {
    failure(StatusCode::SERVICE_UNAVAILABLE)
}
fn body<T>(input: Result<Json<T>, JsonRejection>) -> Result<T, Failure> {
    input.map(|Json(v)| v).map_err(|_| failure(StatusCode::BAD_REQUEST))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NewReport {
    /// Client idempotency key; a repeated send returns the stored report.
    id: String,
    /// Parent only: which child.
    student_id: Option<String>,
    about: String,
    happened_on: String,
    #[serde(default)]
    happened_at: Option<String>,
    place: String,
    #[serde(default)]
    people: String,
    text: String,
    #[serde(default = "default_true")]
    hide_name: bool,
    #[serde(default)]
    photo: Option<String>,
}
fn default_true() -> bool {
    true
}
impl NewReport {
    fn valid(&self) -> bool {
        let photo_ok = self.photo.as_deref().is_none_or(|p| {
            p.len() <= PHOTO_MAX
                && p.strip_prefix(PHOTO_PREFIX).is_some_and(|b| {
                    !b.is_empty() && b.bytes().all(|c| c.is_ascii_alphanumeric() || b"+/=".contains(&c))
                })
        });
        identifier(&self.id)
            && self.student_id.as_deref().is_none_or(identifier)
            && ["self", "witness"].contains(&self.about.as_str())
            && model::date(&self.happened_on)
            // A report is about something that already happened; ISO dates compare as text.
            && self.happened_on <= model::school_today().to_string()
            && self.happened_at.as_deref().is_none_or(|t| {
                chrono::NaiveTime::parse_from_str(t, "%H:%M").is_ok_and(|p| p.format("%H:%M").to_string() == t)
            })
            && PLACES.contains(&self.place.as_str())
            && self.people.len() <= 300
            && !self.text.trim().is_empty()
            && self.text.len() <= 2000
            && photo_ok
    }
    fn params(&self, q: GraphQuery, kind: &str) -> GraphQuery {
        q.param("id", self.id.as_str())
            .param("kind", kind)
            .param("about", self.about.as_str())
            .param("date", self.happened_on.as_str())
            .param("time", self.happened_at.as_deref().unwrap_or(""))
            .param("place", self.place.as_str())
            .param("people", self.people.trim())
            .param("text", self.text.trim())
            .param("hide", self.hide_name)
            .param("photo", self.photo.as_deref().unwrap_or(""))
    }
}

fn student_list_query(actor: &str) -> GraphQuery {
    query(&format!("MATCH(r:SafetyReport {{reporter_id:$actor,reporter_kind:'student'}}) OPTIONAL MATCH(r)-[:FOR_STUDENT]->(s:Student) RETURN DISTINCT r.created_at AS at,{REPORT} ORDER BY at DESC LIMIT 100")).param("actor", actor)
}
/// A report id another account already used is not reused: the MERGE would hand that report over.
fn student_create_query(v: &NewReport, actor: &str) -> GraphQuery {
    let q = format!("{OWNED} WITH DISTINCT u,s,sec WHERE NOT EXISTS {{ MATCH(x:SafetyReport {{id:$id}}) WHERE x.reporter_id<>u.id }} WITH u,s,sec,coalesce(s.fullName,'') AS reporter {CREATE} RETURN {REPORT}");
    v.params(query(&q), "student").param("actor", actor)
}
fn parent_list_query(sub: &str) -> GraphQuery {
    query(&format!("{PARENT} WITH DISTINCT u MATCH(r:SafetyReport {{reporter_id:u.id,reporter_kind:'parent'}}) OPTIONAL MATCH(r)-[:FOR_STUDENT]->(s:Student) RETURN DISTINCT r.created_at AS at,{REPORT} ORDER BY at DESC LIMIT 100")).param("sub", sub)
}
fn parent_create_query(v: &NewReport, sub: &str, student: &str) -> GraphQuery {
    let q = format!("{PARENT} AND s.studentId=$studentId WITH DISTINCT u,s,collect(l.parent_name) AS names {PARENT_SECTION} WITH DISTINCT u,s,sec,names WHERE NOT EXISTS {{ MATCH(x:SafetyReport {{id:$id}}) WHERE x.reporter_id<>u.id }} WITH u,s,sec,coalesce(u.fullName,names[0],'') AS reporter {CREATE} RETURN {REPORT}");
    v.params(query(&q), "parent").param("sub", sub).param("studentId", student)
}
fn staff_list_query(m: &auth::Teacher) -> GraphQuery {
    let body = format!("MATCH(r:SafetyReport {{section_id:sec.section_id,school_id:sec.school_id,tenant_id:sec.tenant_id}}) OPTIONAL MATCH(r)-[:FOR_STUDENT]->(s:Student) RETURN DISTINCT r.created_at AS at,{REPORT} ORDER BY at DESC LIMIT 200");
    repository::teacher_query(m, &body, None, false, true)
}
fn ingest_query(e: &CameraEvent) -> GraphQuery {
    query("MATCH(:School {school_id:$school,tenant_id:$tenant}) MERGE(e:SafetyEvent {id:$id}) ON CREATE SET e.fresh=true,e.school_id=$school,e.tenant_id=$tenant,e.type=$type,e.camera=$camera,e.zone=$zone,e.count=$count,e.started_at=$started,e.ended_at=$ended,e.media_id=$media,e.created_at=datetime() ON MATCH SET e.fresh=false,e.ended_at=coalesce($ended,e.ended_at) WITH e OPTIONAL MATCH(ro:SafetyRoster {school_id:$school,tenant_id:$tenant}) RETURN e.fresh AS fresh,coalesce(ro.phones,[]) AS phones LIMIT 1")
        .param("id", e.id.as_str()).param("school", e.school_id.as_str()).param("tenant", e.tenant_id.as_str())
        .param("type", e.kind.as_str()).param("camera", e.camera.trim()).param("zone", e.zone.trim()).param("count", e.count)
        .param("started", e.started_at.as_str()).param("ended", e.ended_at.clone()).param("media", e.media_id.clone())
}

// Student: own reports, the single active enrolment of the signed-in account.
pub(super) async fn student_list(State(s): State<AppState>, h: HeaderMap) -> Result<Json<Value>, Failure> {
    let actor = student_identity::actor(&s.config, &h).await?;
    Ok(envelope(json!({"reports": reports(&s.graph, student_list_query(&actor)).await.map_err(unavailable)?})))
}
pub(super) async fn student_create(
    State(s): State<AppState>,
    h: HeaderMap,
    input: Result<Json<NewReport>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let v = body(input)?;
    if !v.valid() || v.student_id.is_some() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let actor = student_identity::actor(&s.config, &h).await?;
    created(&s, student_create_query(&v, &actor)).await
}
async fn created(s: &AppState, q: GraphQuery) -> Result<Json<Value>, Failure> {
    let rows = reports(&s.graph, q).await.map_err(unavailable)?;
    let report = rows.into_iter().next().ok_or_else(|| failure(StatusCode::FORBIDDEN))?;
    // A repeated send returns the stored report; notification-services drops the repeated delivery by its key.
    let (state, id) = (s.clone(), report["id"].as_str().unwrap_or_default().to_string());
    tokio::spawn(async move { notify_staff(&state, &id).await });
    Ok(envelope(report))
}

/// The class's homeroom and the school's Owners, by email: staff records hold no phone number.
fn recipients_query(id: &str) -> GraphQuery {
    query("MATCH(r:SafetyReport {id:$id}) MATCH(sec:Section {section_id:r.section_id,school_id:r.school_id,tenant_id:r.tenant_id}) OPTIONAL MATCH(h:StaffMember {id:sec.homeroom_staff_member_id,membershipStatus:'ACTIVE'}) OPTIONAL MATCH(o:StaffMember {membershipStatus:'ACTIVE'}) WHERE 'owner' IN coalesce(o.roles,[]) AND ((size(coalesce(o.schoolIds,[]))=0 AND size(coalesce(o.tenantIds,[]))=0) OR (r.school_id IN coalesce(o.schoolIds,[]) AND r.tenant_id IN coalesce(o.tenantIds,[]))) WITH r,sec,collect(DISTINCT toLower(coalesce(h.primaryEmail,h.email)))+collect(DISTINCT toLower(coalesce(o.primaryEmail,o.email))) AS emails RETURN r.public_id AS ref,coalesce(sec.name,'') AS class,r.place AS place,r.happened_on AS day,r.reporter_kind AS kind,[e IN emails WHERE e CONTAINS '@'] AS emails")
        .param("id", id)
}
fn place_label(place: &str) -> &'static str {
    match place {
        "classroom" => "kelas", "corridor" => "koridor", "toilet" => "toilet", "canteen" => "kantin",
        "yard" => "lapangan", "outside" => "luar sekolah", "online" => "online", _ => "lainnya",
    }
}
/// Class, place and day only: no student name and no report text leave SIS by email.
fn report_email(class: &str, place: &str, day: &str, kind: &str, link: Option<&str>) -> (String, String) {
    let from = if kind == "parent" { "orang tua" } else { "siswa" };
    let open = link.map(|l| format!("\nBuka Keamanan untuk membacanya: {l}")).unwrap_or_default();
    (
        format!("Laporan keamanan baru · {class}"),
        format!("Ada laporan keamanan baru dari {from} kelas {class}: tempat {}, tanggal {day}.{open}\n\nA new safety report from a {} in {class} is waiting in Safety.", place_label(place), if kind == "parent" { "parent" } else { "student" }),
    )
}
async fn notify_staff(s: &AppState, id: &str) {
    let (Some(origin), Some(token)) = (s.config.notification_service_url.as_deref(), s.config.notification_internal_token.as_deref()) else {
        tracing::warn!("safety report {id}: no notification-services configured, staff not emailed");
        return;
    };
    let row = match s.graph.execute(recipients_query(id)).await {
        Ok(mut rows) => rows.next().await.ok().flatten(),
        Err(_) => None,
    };
    let Some(r) = row else { tracing::warn!("safety report {id}: recipients unreadable"); return };
    let text = |k: &str| r.get::<String>(k).unwrap_or_default();
    let mut emails: Vec<String> = r.get("emails").unwrap_or_default();
    emails.sort();
    emails.dedup();
    let (subject, body) = report_email(&text("class"), &text("place"), &text("day"), &text("kind"), s.config.safety_alert_link.as_deref());
    for email in emails {
        let payload = json!({"idempotencyKey": format!("safety-report-{}-{email}", text("ref")), "email": email, "subject": subject, "body": body});
        let ok = s.http_client.post(format!("{}/api/email/v1/send", origin.trim_end_matches('/')))
            .header("X-Internal-Token", token).json(&payload)
            .timeout(std::time::Duration::from_secs(10)).send().await
            .map(|r| r.status().is_success()).unwrap_or(false);
        if !ok { tracing::warn!("safety report {id}: one staff email failed") }
    }
}

// Parent: reports about the caller's own children.
pub(super) async fn parent_list(State(s): State<AppState>, h: HeaderMap) -> Result<Json<Value>, Failure> {
    let sub = auth::parent(&s.config, &h)?;
    let rows = reports(&s.graph, parent_list_query(&sub)).await.map_err(unavailable)?;
    Ok(envelope(json!({"reports": rows})))
}
pub(super) async fn parent_create(
    State(s): State<AppState>,
    h: HeaderMap,
    input: Result<Json<NewReport>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let v = body(input)?;
    let Some(student) = v.student_id.clone().filter(|_| v.valid()) else {
        return Err(failure(StatusCode::BAD_REQUEST));
    };
    let sub = auth::parent(&s.config, &h)?;
    created(&s, parent_create_query(&v, &sub, &student)).await
}

// Staff: the homeroom of the class, or an Owner of the school.
pub(super) async fn staff_list(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(scope): Query<Selection>,
) -> Result<Json<Value>, Failure> {
    let m = auth::teacher(&s.config, &h).await?.select(&scope)?;
    let rows = reports(&s.graph, staff_list_query(&m)).await.map_err(unavailable)?;
    Ok(envelope(json!({"reports": rows})))
}
pub(super) async fn staff_detail(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(scope): Query<Selection>,
) -> Result<Json<Value>, Failure> {
    let m = auth::teacher(&s.config, &h).await?.select(&scope)?;
    if !identifier(&id) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    // Every staff read leaves an audit row (spec §4.3).
    let body = format!("MATCH(r:SafetyReport {{id:$id,section_id:sec.section_id,school_id:sec.school_id,tenant_id:sec.tenant_id}}) OPTIONAL MATCH(r)-[:FOR_STUDENT]->(s:Student) OPTIONAL MATCH(n:SafetyReportNote)-[:ON]->(r) WITH DISTINCT t,r,s,n ORDER BY n.created_at WITH t,r,s,collect(n.author_name) AS authors,collect(n.text) AS texts,collect(toString(n.created_at)) AS times LIMIT 1 CREATE(:SchoolPortalAudit {{id:$audit,actor_id:t.id,kind:'safety_report_read',report_id:r.id,school_id:r.school_id,tenant_id:r.tenant_id,created_at:datetime()}}) RETURN {REPORT},r.reporter_name AS reporterName,coalesce(r.photo,'') AS photo,authors,texts,times");
    let q = repository::teacher_query(&m, &body, None, false, true).param("id", id).param("audit", uuid::Uuid::new_v4().to_string());
    let mut rows = s.graph.execute(q).await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let Some(r) = rows.next().await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))? else {
        return Err(failure(StatusCode::FORBIDDEN));
    };
    let mut v = report(&r).map_err(unavailable)?;
    let authors: Vec<String> = r.get("authors").map_err(|e| unavailable(e.into()))?;
    let texts: Vec<String> = r.get("texts").map_err(|e| unavailable(e.into()))?;
    let times: Vec<String> = r.get("times").map_err(|e| unavailable(e.into()))?;
    v["reporterName"] = json!(r.get::<String>("reporterName").unwrap_or_default());
    v["photo"] = json!(r.get::<String>("photo").ok().filter(|p| !p.is_empty()));
    v["notes"] = json!(authors.into_iter().zip(texts).zip(times).map(|((author, text), at)| json!({"author": author, "text": text, "createdAt": at})).collect::<Vec<_>>());
    Ok(envelope(v))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    #[serde(default)]
    status: String,
    #[serde(default)]
    outcome: String,
    #[serde(default)]
    note: String,
}
impl Update {
    fn valid(&self) -> bool {
        (self.status.is_empty() || ["acknowledged", "done"].contains(&self.status.as_str()))
            && (self.outcome.is_empty() || OUTCOMES.contains(&self.outcome.as_str()))
            && (self.status != "done" || !self.outcome.is_empty())
            && self.note.len() <= 2000
            && (!self.status.is_empty() || !self.note.trim().is_empty())
    }
}
pub(super) async fn staff_update(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(scope): Query<Selection>,
    input: Result<Json<Update>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let m = auth::teacher(&s.config, &h).await?.select(&scope)?;
    let v = body(input)?;
    if !identifier(&id) || !v.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    // The Owner write path locks one class, so the report's class is resolved first.
    let find = repository::teacher_query(&m, "MATCH(r:SafetyReport {id:$id,section_id:sec.section_id,school_id:sec.school_id,tenant_id:sec.tenant_id}) RETURN sec.section_id AS section LIMIT 1", None, false, true).param("id", id.clone());
    let mut rows = s.graph.execute(find).await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let section: String = match rows.next().await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))? {
        Some(r) => r.get("section").map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?,
        None => return Err(failure(StatusCode::FORBIDDEN)),
    };
    // A closed report is only annotated, never reopened or re-closed.
    let q = update_query(&m, &section, &id, &v);
    match repository::execute_write(&s.graph, &m, &section, q, Some((&s.config, &h))).await {
        Ok(true) => Ok(envelope(json!({"saved": true}))),
        Ok(false) => Err(failure(StatusCode::CONFLICT)),
        Err(error) => Err(match error.downcast::<super::staff_capability::SessionFailure>() {
            Ok(error) => error.0,
            Err(_) => failure(StatusCode::SERVICE_UNAVAILABLE),
        }),
    }
}

/// A closed report is only annotated, never reopened or re-closed. A Parent reporter gets a school notice when the
/// school acknowledges or closes it; a Student sees the status in "Laporan saya".
fn update_query(m: &auth::Teacher, section: &str, id: &str, v: &Update) -> GraphQuery {
    let body = "MATCH(r:SafetyReport {id:$id,section_id:sec.section_id,school_id:sec.school_id,tenant_id:sec.tenant_id}) WHERE $status='' OR r.status<>'done' SET r.status=CASE WHEN $status='' THEN r.status ELSE $status END,r.outcome=CASE WHEN $outcome='' THEN r.outcome ELSE $outcome END,r.updated_at=datetime() FOREACH(_ IN CASE WHEN $note<>'' THEN [1] ELSE [] END | CREATE(:SafetyReportNote {id:randomUUID(),author_id:t.id,author_name:coalesce(t.displayName,t.fullName,''),text:$note,created_at:datetime()})-[:ON]->(r)) CREATE(:SchoolPortalAudit {id:$audit,actor_id:t.id,kind:'safety_report_update',report_id:r.id,status:$status,school_id:r.school_id,tenant_id:r.tenant_id,created_at:datetime()}) WITH r OPTIONAL MATCH(pu:User {id:r.reporter_id}) WHERE r.reporter_kind='parent' AND $status<>'' FOREACH(_ IN CASE WHEN pu IS NULL THEN [] ELSE [1] END | MERGE(n:SchoolNotice {id:'safety-report:'+r.public_id+':'+$status}) ON CREATE SET n.title=$title,n.body=$notice,n.created_at=datetime(),n.is_read=false MERGE(pu)-[:HAS_SCHOOL_NOTICE]->(n)) RETURN r.id AS id";
    let (title, notice) = match v.status.as_str() {
        "done" => ("Laporan selesai ditangani", "Sekolah sudah menindaklanjuti laporan Anda dan menutupnya. · The school has followed up on your report and closed it."),
        _ => ("Laporan diterima sekolah", "Wali kelas sudah menerima laporan Anda dan sedang menanganinya. · The homeroom teacher received your report and is handling it."),
    };
    repository::teacher_query(m, body, Some(section), true, true)
        .param("id", id).param("status", v.status.as_str()).param("outcome", v.outcome.as_str())
        .param("note", v.note.trim()).param("audit", uuid::Uuid::new_v4().to_string())
        .param("title", title).param("notice", notice)
}

// Camera events: posted by the on-site service, listed and rated by school staff.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraEvent {
    id: String,
    school_id: String,
    tenant_id: String,
    #[serde(rename = "type")]
    kind: String,
    camera: String,
    #[serde(default)]
    zone: String,
    #[serde(default)]
    count: i64,
    started_at: String,
    #[serde(default)]
    ended_at: Option<String>,
    #[serde(default)]
    media_id: Option<String>,
}
impl CameraEvent {
    fn valid(&self) -> bool {
        let text = |v: &str, max: usize| v.len() <= max && !v.contains(['\n', '\r']);
        identifier(&self.id)
            && identifier(&self.school_id)
            && identifier(&self.tenant_id)
            && identifier(&self.kind)
            && self.kind.len() <= 40
            && text(&self.camera, 80)
            && !self.camera.trim().is_empty()
            && text(&self.zone, 80)
            && (0..=10_000).contains(&self.count)
            && chrono::DateTime::parse_from_rfc3339(&self.started_at).is_ok()
            && self.ended_at.as_deref().is_none_or(|t| chrono::DateTime::parse_from_rfc3339(t).is_ok())
            && self.media_id.as_deref().is_none_or(identifier)
    }
    fn label(&self) -> &'static str {
        match self.kind.as_str() {
            "crowd" => "Keramaian",
            "restricted" => "Orang di zona terlarang",
            "loitering" => "Orang berdiam lama",
            "after_hours" => "Orang di luar jam sekolah",
            "camera_tampered" => "Kamera tertutup atau gelap",
            "camera_offline" => "Kamera offline",
            _ => "Kejadian kamera",
        }
    }
    /// Place, count and time only: never a person (spec §4.4).
    fn message(&self, link: Option<&str>) -> String {
        let at = chrono::DateTime::parse_from_rfc3339(&self.started_at)
            .map(|t| (t + chrono::Duration::hours(7)).format("%H:%M").to_string())
            .unwrap_or_default();
        let place = if self.zone.is_empty() { self.camera.clone() } else { format!("{} ({})", self.zone, self.camera) };
        let people = if self.count > 0 { format!(", {} orang", self.count) } else { String::new() };
        let link = link.map(|l| format!(" Lihat: {l}")).unwrap_or_default();
        format!("{} di {place}{people}, {at} WIB.{link}", self.label())
    }
}
fn internal(config: &crate::config::config::AppConfig, h: &HeaderMap) -> Result<(), Failure> {
    super::auth::internal_token(config.safety_ingest_token.as_deref(), h)
}
pub(super) async fn ingest(
    State(s): State<AppState>,
    h: HeaderMap,
    input: Result<Json<CameraEvent>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    internal(&s.config, &h)?;
    let e = body(input)?;
    if !e.valid() {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let mut rows = s.graph.execute(ingest_query(&e)).await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let Some(r) = rows.next().await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))? else {
        return Err(failure(StatusCode::FORBIDDEN));
    };
    let fresh: bool = r.get("fresh").unwrap_or(false);
    let phones: Vec<String> = r.get("phones").unwrap_or_default();
    let mut alerted = 0;
    if fresh && ALERTED.contains(&e.kind.as_str()) {
        alerted = alert(&s, &e, &phones).await;
    }
    Ok(envelope(json!({"stored": true, "new": fresh, "alerted": alerted})))
}
/// Pages the roster through notification-services; a failed send never fails the ingest.
// shortcut: the per-recipient cap of spec §4.4 is left to the on-site cooldowns; add it here if the
// box misbehaves.
async fn alert(s: &AppState, e: &CameraEvent, phones: &[String]) -> usize {
    let (Some(origin), Some(token)) = (s.config.notification_service_url.as_deref(), s.config.notification_internal_token.as_deref()) else {
        tracing::warn!("safety alert {} not sent: notification-services is not configured", e.id);
        return 0;
    };
    let text = e.message(s.config.safety_alert_link.as_deref());
    let mut sent = 0;
    for phone in phones.iter().filter(|p| p.len() >= 8 && p.bytes().all(|b| b.is_ascii_digit())) {
        let payload = json!({"idempotencyKey": format!("safety-{}-{phone}", e.id), "to": phone, "body": text});
        let ok = s.http_client.post(format!("{}/api/whatsapp/v1/send", origin.trim_end_matches('/')))
            .header("X-Internal-Token", token).json(&payload)
            .timeout(std::time::Duration::from_secs(5)).send().await
            .map(|r| r.status().is_success()).unwrap_or(false);
        if ok { sent += 1 } else { tracing::warn!("safety alert {} to one recipient failed", e.id) }
    }
    sent
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Day {
    date: Option<String>,
    #[serde(alias = "schoolId")]
    school_id: Option<String>,
    #[serde(alias = "tenantId")]
    tenant_id: Option<String>,
}
pub(super) async fn events(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(day): Query<Day>,
) -> Result<Json<Value>, Failure> {
    let scope = Selection { school_id: day.school_id.clone(), tenant_id: day.tenant_id.clone() };
    let m = auth::teacher(&s.config, &h).await?.select(&scope)?;
    let (Some(school), Some(tenant)) = (m.school_ids.first(), m.tenant_ids.first()) else {
        return Err(failure(StatusCode::BAD_REQUEST));
    };
    let date = day.date.unwrap_or_else(|| model::school_today().to_string());
    if !model::date(&date) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    // started_at is UTC RFC 3339; the day is matched in WIB.
    let q = query("MATCH(e:SafetyEvent {school_id:$school,tenant_id:$tenant}) WHERE toString(date(datetime(e.started_at)+duration('PT7H')))=$date RETURN e.id AS id,e.type AS type,e.camera AS camera,coalesce(e.zone,'') AS zone,coalesce(e.count,0) AS count,e.started_at AS startedAt,e.ended_at AS endedAt,e.media_id AS mediaId,e.rating AS rating ORDER BY e.started_at DESC LIMIT 200")
        .param("school", school.clone()).param("tenant", tenant.clone()).param("date", date);
    let mut rows = s.graph.execute(q).await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    let mut out = vec![];
    while let Some(r) = rows.next().await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))? {
        out.push(json!({
            "id": r.get::<String>("id").unwrap_or_default(), "type": r.get::<String>("type").unwrap_or_default(),
            "camera": r.get::<String>("camera").unwrap_or_default(), "zone": r.get::<String>("zone").unwrap_or_default(),
            "count": r.get::<i64>("count").unwrap_or(0), "startedAt": r.get::<String>("startedAt").unwrap_or_default(),
            "endedAt": r.get::<Option<String>>("endedAt").ok().flatten(), "mediaId": r.get::<Option<String>>("mediaId").ok().flatten(),
            "rating": r.get::<Option<String>>("rating").ok().flatten(),
        }));
    }
    let mut data = json!({"events": out});
    if m.is_owner() {
        let mut rows = s.graph.execute(query("MATCH(ro:SafetyRoster {school_id:$school,tenant_id:$tenant}) RETURN ro.phones AS phones").param("school", school.clone()).param("tenant", tenant.clone())).await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
        let phones: Vec<String> = match rows.next().await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))? { Some(r) => r.get("phones").unwrap_or_default(), None => vec![] };
        data["roster"] = json!({"phones": phones});
    }
    Ok(envelope(data))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rating {
    rating: String,
}
pub(super) async fn rate(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(scope): Query<Selection>,
    input: Result<Json<Rating>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let m = auth::teacher(&s.config, &h).await?.select(&scope)?;
    let v = body(input)?;
    if !identifier(&id) || !RATINGS.contains(&v.rating.as_str()) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (Some(school), Some(tenant)) = (m.school_ids.first(), m.tenant_ids.first()) else {
        return Err(failure(StatusCode::BAD_REQUEST));
    };
    let q = query("MATCH(e:SafetyEvent {id:$id,school_id:$school,tenant_id:$tenant}) SET e.rating=$rating,e.rated_by=$actor,e.rated_at=datetime() RETURN e.id AS id")
        .param("id", id).param("school", school.clone()).param("tenant", tenant.clone()).param("rating", v.rating.as_str()).param("actor", m.staff_member_id.clone());
    let mut rows = s.graph.execute(q).await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    match rows.next().await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))? {
        Some(_) => Ok(envelope(json!({"saved": true}))),
        None => Err(failure(StatusCode::NOT_FOUND)),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roster {
    phones: Vec<String>,
}
/// Owner only: the WhatsApp numbers paged for this school's camera alerts.
pub(super) async fn roster(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(scope): Query<Selection>,
    input: Result<Json<Roster>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let m = auth::teacher(&s.config, &h).await?.select(&scope)?;
    let v = body(input)?;
    if !m.is_owner() {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    if v.phones.len() > 20 || v.phones.iter().any(|p| !(8..=15).contains(&p.len()) || !p.bytes().all(|b| b.is_ascii_digit())) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (Some(school), Some(tenant)) = (m.school_ids.first(), m.tenant_ids.first()) else {
        return Err(failure(StatusCode::BAD_REQUEST));
    };
    let q = query("MATCH(:School {school_id:$school,tenant_id:$tenant}) MERGE(ro:SafetyRoster {school_id:$school,tenant_id:$tenant}) SET ro.phones=$phones,ro.updated_by=$actor,ro.updated_at=datetime() RETURN ro.phones AS phones")
        .param("school", school.clone()).param("tenant", tenant.clone()).param("phones", v.phones.clone()).param("actor", m.staff_member_id.clone());
    let mut rows = s.graph.execute(q).await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))?;
    match rows.next().await.map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE))? {
        Some(_) => Ok(envelope(json!({"roster": {"phones": v.phones}}))),
        None => Err(failure(StatusCode::FORBIDDEN)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn report() -> NewReport {
        NewReport { id: "r-1".into(), student_id: None, about: "self".into(), happened_on: model::school_today().to_string(), happened_at: Some("09:40".into()), place: "corridor".into(), people: String::new(), text: "Didorong di koridor".into(), hide_name: true, photo: None }
    }
    #[test]
    fn report_validation() {
        assert!(report().valid());
        assert!(!NewReport { text: " ".into(), ..report() }.valid());
        assert!(!NewReport { place: "roof".into(), ..report() }.valid());
        assert!(!NewReport { happened_at: Some("9:40".into()), ..report() }.valid());
        let tomorrow = model::school_today().succ_opt().unwrap().to_string();
        assert!(!NewReport { happened_on: tomorrow, ..report() }.valid());
        assert!(!NewReport { id: "r 1".into(), ..report() }.valid());
        assert!(NewReport { photo: Some(format!("{PHOTO_PREFIX}/9j/4AAQ")), ..report() }.valid());
        assert!(!NewReport { photo: Some("data:image/png;base64,iVBOR".into()), ..report() }.valid());
        assert!(!NewReport { photo: Some(format!("{PHOTO_PREFIX}{}", "A".repeat(PHOTO_MAX))), ..report() }.valid());
    }
    #[test]
    fn update_validation() {
        let u = |status: &str, outcome: &str, note: &str| Update { status: status.into(), outcome: outcome.into(), note: note.into() }.valid();
        assert!(u("acknowledged", "", ""));
        assert!(u("", "", "Sudah bicara dengan orang tua"));
        assert!(u("done", "handled", ""));
        assert!(!u("done", "", ""));
        assert!(!u("", "", ""));
        assert!(!u("new", "", ""));
    }
    #[test]
    fn camera_event_message_names_no_person() {
        let e = CameraEvent { id: "evt-1".into(), school_id: "s".into(), tenant_id: "t".into(), kind: "crowd".into(), camera: "DOME-04".into(), zone: "Koridor lantai 2".into(), count: 9, started_at: "2026-11-05T02:42:00Z".into(), ended_at: None, media_id: None };
        assert!(e.valid());
        assert_eq!(e.message(Some("https://x/safety")), "Keramaian di Koridor lantai 2 (DOME-04), 9 orang, 09:42 WIB. Lihat: https://x/safety");
        assert!(!CameraEvent { started_at: "yesterday".into(), ..e }.valid());
    }
    #[tokio::test]
    #[ignore = "requires disposable SAFETY_TEST_BOLT graph"]
    async fn reports_reach_only_the_homeroom_and_owner() {
        let uri = std::env::var("SAFETY_TEST_BOLT").unwrap();
        assert!(uri.starts_with("127.0.0.1:"));
        let g = Graph::new(uri, "neo4j", "safety-fixture-password").await.unwrap();
        g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
        repository::init(&g).await.unwrap();
        g.run(query("CREATE(:School {school_id:'school',tenant_id:'tenant'}) \
            CREATE(t:StaffMember {id:'teacher',email:'teacher@example.test',fullName:'Bu Rina',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) \
            CREATE(:StaffMember {id:'other',email:'other@example.test',membershipStatus:'ACTIVE',roles:['teacher'],schoolIds:['school'],tenantIds:['tenant']}) \
            CREATE(:StaffMember {id:'owner',email:'owner@example.test',membershipStatus:'ACTIVE',roles:['owner'],schoolIds:['school'],tenantIds:['tenant']}) \
            CREATE(sec:Section {section_id:'class',name:'7A',school_id:'school',tenant_id:'tenant',status:'active',homeroom_staff_member_id:'teacher'}) \
            CREATE(s:Student {studentId:'child',fullName:'Aisha'})-[:ENROLLED_AS]->(:EnrolledStudent {status:'active',school_id:'school',tenant_id:'tenant'})-[:ENROLLED_IN]->(sec) \
            CREATE(:User {id:'kid',email:'kid@example.test',role:'student'}) CREATE(:LearningStudentBinding {user_id:'kid',student_id:'child',school_id:'school',tenant_id:'tenant',status:'ACTIVE'}) \
            CREATE(:User {id:'mother',email:'mother@example.test',role:'parent'})-[:HAS_APPLICATION]->(:Lead {lead_id:'LEAD-MOTHER',email:'mother@example.test',status:'verified',parent_name:'Ibu Aisha'})-[:HAS_STUDENT]->(s) \
            CREATE(:User {id:'stranger',email:'stranger@example.test',role:'parent'})-[:HAS_APPLICATION]->(:Lead {lead_id:'LEAD-STRANGER',email:'stranger@example.test',status:'verified'})-[:HAS_STUDENT]->(:Student {studentId:'foreign'})")).await.unwrap();
        let staff = |id: &str, role: &str| auth::Teacher { active: true, staff_member_id: id.into(), roles: vec![role.into()], school_ids: vec!["school".into()], tenant_ids: vec!["tenant".into()] };

        // The student files once; a repeated send returns the same report; a stranger cannot claim the id.
        let first = reports(&g, student_create_query(&report(), "kid")).await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!((first[0]["status"].as_str(), first[0]["reporterKind"].as_str(), first[0]["studentName"].as_str()), (Some("new"), Some("student"), Some("Aisha")));
        assert_eq!(reports(&g, student_create_query(&report(), "kid")).await.unwrap()[0]["publicId"], first[0]["publicId"]);
        let stolen = NewReport { student_id: Some("child".into()), ..report() };
        assert!(reports(&g, parent_create_query(&stolen, "LEAD-MOTHER", "child")).await.unwrap().is_empty());
        assert_eq!(reports(&g, student_list_query("kid")).await.unwrap().len(), 1);

        // The parent files about their own child only.
        let mine = NewReport { id: "r-2".into(), student_id: Some("child".into()), ..report() };
        assert_eq!(reports(&g, parent_create_query(&mine, "LEAD-MOTHER", "child")).await.unwrap()[0]["reporterKind"], "parent");
        assert!(reports(&g, parent_create_query(&NewReport { id: "r-3".into(), ..mine }, "LEAD-STRANGER", "child")).await.unwrap().is_empty());
        assert_eq!(reports(&g, parent_list_query("mother")).await.unwrap().len(), 1);
        assert!(reports(&g, parent_list_query("stranger")).await.unwrap().is_empty());

        // The homeroom and an Owner read both; a subject teacher reads none.
        assert_eq!(reports(&g, staff_list_query(&staff("teacher", "teacher"))).await.unwrap().len(), 2);
        assert_eq!(reports(&g, staff_list_query(&staff("owner", "owner"))).await.unwrap().len(), 2);
        assert!(reports(&g, staff_list_query(&staff("other", "teacher"))).await.unwrap().is_empty());

        // The homeroom and the Owner are emailed about a new report; the subject teacher is not.
        let id = first[0]["id"].as_str().unwrap().to_string();
        let mut rows = g.execute(recipients_query(&id)).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let mut emails: Vec<String> = row.get("emails").unwrap();
        emails.sort();
        assert_eq!(emails, vec!["owner@example.test".to_string(), "teacher@example.test".to_string()]);
        assert_eq!(row.get::<String>("class").unwrap(), "7A");

        // Acknowledging the parent's report leaves the mother one notice; repeating it adds none.
        let ack = Update { status: "acknowledged".into(), outcome: String::new(), note: String::new() };
        for _ in 0..2 {
            let mut rows = g.execute(update_query(&staff("teacher", "teacher"), "class", "r-2", &ack)).await.unwrap();
            while rows.next().await.unwrap().is_some() {}
        }
        let mut rows = g.execute(query("MATCH(:User {id:'mother'})-[:HAS_SCHOOL_NOTICE]->(n:SchoolNotice) RETURN count(n) AS n, collect(n.title)[0] AS title")).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!((row.get::<i64>("n").unwrap(), row.get::<String>("title").unwrap()), (1, "Laporan diterima sekolah".to_string()));

        // A camera event is stored once; the second post is not fresh and never alerts again.
        let e = CameraEvent { id: "evt-1".into(), school_id: "school".into(), tenant_id: "tenant".into(), kind: "crowd".into(), camera: "DOME-04".into(), zone: "Koridor".into(), count: 9, started_at: "2026-11-05T02:42:00Z".into(), ended_at: None, media_id: None };
        let fresh = |g: &Graph, e: &CameraEvent| { let q = ingest_query(e); let g = g.clone(); async move { g.execute(q).await.unwrap().next().await.unwrap().unwrap().get::<bool>("fresh").unwrap() } };
        assert!(fresh(&g, &e).await);
        assert!(!fresh(&g, &CameraEvent { ended_at: Some("2026-11-05T02:44:00Z".into()), ..e }).await);
        let mut rows = g.execute(query("MATCH(e:SafetyEvent {id:'evt-1'}) RETURN e.ended_at AS ended")).await.unwrap();
        assert_eq!(rows.next().await.unwrap().unwrap().get::<String>("ended").unwrap(), "2026-11-05T02:44:00Z");
        g.run(query("MATCH(n) DETACH DELETE n")).await.unwrap();
    }
    #[test]
    fn report_email_names_no_student() {
        let (subject, body) = report_email("7A", "corridor", "2026-11-06", "student", Some("https://t/safety"));
        assert_eq!(subject, "Laporan keamanan baru · 7A");
        assert!(body.starts_with("Ada laporan keamanan baru dari siswa kelas 7A: tempat koridor, tanggal 2026-11-06."));
        assert!(body.contains("https://t/safety"));
        assert!(report_email("7A", "roof", "2026-11-06", "parent", None).1.contains("orang tua kelas 7A: tempat lainnya"));
    }
    #[test]
    fn ingest_token_is_required_and_long() {
        let mut config = crate::config::config::AppConfig::default();
        let mut h = HeaderMap::new();
        h.insert("x-internal-token", "short".parse().unwrap());
        assert_eq!(internal(&config, &h).unwrap_err().0, StatusCode::SERVICE_UNAVAILABLE);
        config.safety_ingest_token = Some("0123456789abcdef0123456789abcdef".into());
        assert_eq!(internal(&config, &h).unwrap_err().0, StatusCode::UNAUTHORIZED);
        h.insert("x-internal-token", "0123456789abcdef0123456789abcdef".parse().unwrap());
        assert!(internal(&config, &h).is_ok());
    }
}
