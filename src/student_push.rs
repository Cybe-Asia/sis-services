//! Push for the Student app. A signed-in Student registers the app's device here (SIS knows who the
//! Student is; the device itself lives in notification-services), and Learning asks SIS, with the
//! teacher's own token, to tell a class that something was published. SIS picks the recipients, honours
//! the Student's notification settings and writes the lock-screen text; the caller supplies neither.
pub mod repository;
use crate::school_broadcasts::{self, repository::Recipient};
use crate::school_portal::model::identifier;
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, Query, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use chrono::{Timelike, Utc};
use repository::Claim;
use serde::Deserialize;
use serde_json::{json, Value};

type Failure = (StatusCode, Json<Value>);
fn refuse(status: StatusCode, code: &str) -> Failure {
    (status, Json(json!({"error":{"code":code}})))
}
fn invalid() -> Failure {
    refuse(StatusCode::BAD_REQUEST, "INVALID_INPUT")
}
fn unavailable<E>(_: E) -> Failure {
    refuse(StatusCode::SERVICE_UNAVAILABLE, "DEPENDENCY_UNAVAILABLE")
}

/// More Students than this in one notice is not one class.
const MAX_STUDENTS: usize = 60;
/// Pushes one class may get from Learning per school day (Asia/Jakarta).
const DAILY_CAP: i64 = 20;
/// Asia/Jakarta has no daylight saving.
const WIB_OFFSET_HOURS: i64 = 7;

/// What happened. Each maps to one Student notification category and one fixed lock-screen text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Kind {
    #[serde(rename = "assignment.published")]
    Assignment,
    #[serde(rename = "assessment.published")]
    Assessment,
    #[serde(rename = "report_card.published")]
    ReportCard,
    /// A teacher opened a class's live session; `ref` is the session id.
    #[serde(rename = "live_class.started")]
    LiveStarted,
    /// A teacher changed what a class reads; `ref` is `<course id>@<YYYYMMDDHH WIB>`, so a burst of edits
    /// to one course in one hour is one push. The app re-reads its content manifest.
    #[serde(rename = "content.updated")]
    ContentUpdated,
}
impl Kind {
    fn slug(self) -> &'static str {
        match self {
            Self::Assignment => "assignment.published",
            Self::Assessment => "assessment.published",
            Self::ReportCard => "report_card.published",
            Self::LiveStarted => "live_class.started",
            Self::ContentUpdated => "content.updated",
        }
    }
    /// Index into `StudentNotificationCategories::STORED`.
    fn category(self) -> usize {
        match self {
            Self::Assignment | Self::Assessment => 0, // assignments
            Self::ReportCard => 1,                    // grades
            Self::LiveStarted | Self::ContentUpdated => 5, // learning_reminders
        }
    }
    /// A Student web path; the app opens the same screen.
    fn url(self, reference: &str) -> String {
        match self {
            Self::Assignment => format!("/lessons?assignment={reference}"),
            Self::Assessment => format!("/tests?assessment={reference}"),
            Self::ReportCard => "/report-cards".into(),
            Self::LiveStarted => format!("/lessons?mode=live&session={reference}"),
            Self::ContentUpdated => "/lessons".into(),
        }
    }
    fn text(self, indonesian: bool) -> (&'static str, &'static str) {
        match (self, indonesian) {
            (Self::Assignment, true) => ("Tugas baru", "Ada tugas baru di kelasmu."),
            (Self::Assignment, false) => ("New assignment", "You have a new assignment."),
            (Self::Assessment, true) => ("Kuis baru", "Ada kuis atau tes baru untukmu."),
            (Self::Assessment, false) => ("New quiz", "You have a new quiz or test."),
            (Self::ReportCard, true) => ("Rapor tersedia", "Rapormu sudah bisa dilihat."),
            (Self::ReportCard, false) => ("Report card ready", "Your report card is available."),
            (Self::LiveStarted, true) => (
                "Kelas live dimulai",
                "Gurumu sudah memulai kelas live. Ketuk untuk bergabung.",
            ),
            (Self::LiveStarted, false) => (
                "Live class started",
                "Your teacher started a live class. Tap to join.",
            ),
            (Self::ContentUpdated, true) => (
                "Materi diperbarui",
                "Gurumu memperbarui materi. Buka app supaya materi di tablet ikut terbaru.",
            ),
            (Self::ContentUpdated, false) => (
                "Lessons updated",
                "Your teacher updated your lessons. Open the app to get the latest.",
            ),
        }
    }
}

/// Lock screens are not private, and these readers are children: a fixed text in the Student's
/// language, never a title, a name, a subject or a score. The link carries only an opaque id.
fn push_payload(r: &Recipient, kind: Kind, reference: &str) -> Value {
    let (title, body) = kind.text(r.locale.starts_with("id"));
    json!({"recipientId": r.id, "title": title, "body": body, "url": kind.url(reference)})
}

/// 21:00–06:00 WIB: a push is skipped, not delayed; the item is in the app anyway.
fn quiet(hour_wib: u32) -> bool {
    !(6..21).contains(&hour_wib)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    school_id: String,
    tenant_id: String,
}
impl Scope {
    fn valid(&self) -> bool {
        identifier(&self.school_id) && identifier(&self.tenant_id)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Notice {
    pub kind: Kind,
    /// The published record's id in Learning (assignment, assessment or report card publication), or
    /// `<course id>@<YYYYMMDDHH>` for `content.updated`.
    #[serde(rename = "ref")]
    pub reference: String,
    pub class_id: String,
    /// Only these Students of the class (a report card); none means the whole class.
    pub student_ids: Option<Vec<String>>,
}
impl Notice {
    fn valid(&self) -> bool {
        let reference = match self.kind {
            Kind::ContentUpdated => self
                .reference
                .split_once('@')
                .filter(|(_, hour)| hour.len() == 10 && hour.bytes().all(|b| b.is_ascii_digit())),
            _ => Some((self.reference.as_str(), "")),
        };
        reference.is_some_and(|(id, _)| identifier(id))
            && identifier(&self.class_id)
            && self.student_ids.as_ref().is_none_or(|ids| {
                let mut unique: Vec<&String> = ids.iter().collect();
                unique.sort();
                unique.dedup();
                (1..=MAX_STUDENTS).contains(&ids.len())
                    && unique.len() == ids.len()
                    && ids.iter().all(|id| identifier(id))
            })
    }
}

async fn notify(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<Scope>,
    j: Result<Json<Notice>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), Failure> {
    let Json(n) = j.map_err(|_| invalid())?;
    if !q.valid() || !n.valid() {
        return Err(invalid());
    }
    let (_, role) =
        crate::learning_access::owner::class_actor(&s, &h, &q.school_id, &q.tenant_id, &n.class_id)
            .await?;
    if !matches!(role.as_str(), "teacher" | "owner") {
        return Err(refuse(StatusCode::FORBIDDEN, "FORBIDDEN"));
    }
    let students = n.student_ids.as_deref();
    if let Some(ids) = students {
        let outside =
            repository::outside_class(&s.graph, &n.class_id, &q.school_id, &q.tenant_id, ids)
                .await
                .map_err(unavailable)?;
        if !outside.is_empty() {
            return Err(refuse(StatusCode::FORBIDDEN, "FORBIDDEN"));
        }
    }
    let everyone = repository::recipients(
        &s.graph,
        &n.class_id,
        &q.school_id,
        &q.tenant_id,
        students,
        n.kind.category(),
    )
    .await
    .map_err(unavailable)?;
    let now = Utc::now() + chrono::Duration::hours(WIB_OFFSET_HOURS);
    // A published paper can serve several classes: each class is told once.
    let key = format!(
        "{}|{}|{}|{}|{}",
        q.tenant_id,
        q.school_id,
        n.kind.slug(),
        n.class_id,
        n.reference
    );
    let claim = repository::claim(
        &s.graph,
        &key,
        n.kind.slug(),
        &n.class_id,
        &q.school_id,
        &q.tenant_id,
        &now.format("%Y-%m-%d").to_string(),
        DAILY_CAP,
        everyone.len() as i64,
    )
    .await
    .map_err(unavailable)?;
    let count = everyone.len();
    let duplicate = match claim {
        Claim::Capped => return Err(refuse(StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED")),
        Claim::Duplicate => true,
        Claim::Fresh => {
            tokio::spawn(fan_out(s, key, n, everyone, quiet(now.hour())));
            false
        }
    };
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"data":{"recipients":count,"duplicate":duplicate}})),
    ))
}

/// Who of the recipients gets the push: those who want this category, and nobody in quiet hours.
fn wanted(everyone: Vec<Recipient>, quiet: bool) -> Vec<Recipient> {
    if quiet {
        return vec![];
    }
    everyone.into_iter().filter(|r| r.notify).collect()
}

// shortcut: as for school broadcasts, the fan-out runs in this process; a restart mid-way leaves the
// rest unpushed and the claim `sending`. Move to a durable outbox if that matters.
async fn fan_out(s: AppState, key: String, n: Notice, everyone: Vec<Recipient>, quiet: bool) {
    let total = everyone.len() as i64;
    let wanted = wanted(everyone, quiet);
    let skipped = total - wanted.len() as i64;
    let (pushed, failed) =
        match school_broadcasts::deliver(&s, &wanted, |r| push_payload(r, n.kind, &n.reference))
            .await
        {
            Some(outcome) => outcome,
            None => {
                tracing::warn!("student push {key}: notification-services is not configured");
                (0, wanted.len() as i64)
            }
        };
    if repository::finish(&s.graph, &key, pushed, failed, skipped)
        .await
        .is_err()
    {
        tracing::warn!("student push {key}: outcome not recorded");
    }
}

/// The app's install, as the Parent app sends it (messaging-services `push-devices.md`).
/// notification-services validates the fields; the token is never logged.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Device {
    installation_id: String,
    token: String,
    platform: String,
    app_version: String,
    locale: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Installation {
    installation_id: String,
}

/// A device route's optional school: both ids or neither (the app sends neither).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceScope {
    school_id: Option<String>,
    tenant_id: Option<String>,
}

/// The signed-in Student as a push recipient, only with an active binding and class: in the named school,
/// or in any school when none is named.
async fn recipient(s: &AppState, h: &HeaderMap, q: &DeviceScope) -> Result<String, Failure> {
    let school = match (&q.school_id, &q.tenant_id) {
        (Some(school_id), Some(tenant_id)) => {
            let scope = Scope {
                school_id: school_id.clone(),
                tenant_id: tenant_id.clone(),
            };
            if !scope.valid() {
                return Err(invalid());
            }
            Some(scope)
        }
        (None, None) => None,
        _ => return Err(invalid()),
    };
    let user = crate::school_portal::student_identity::actor(&s.config, h).await?;
    let allowed = match &school {
        Some(q) => repository::student_in_school(&s.graph, &user, &q.school_id, &q.tenant_id).await,
        None => repository::student_enrolled(&s.graph, &user).await,
    };
    if !allowed.map_err(unavailable)? {
        return Err(refuse(StatusCode::FORBIDDEN, "FORBIDDEN"));
    }
    Ok(format!("student:{user}"))
}

/// One call to notification-services' device store with the internal token.
async fn devices(
    s: &AppState,
    path: &str,
    method: reqwest::Method,
    body: Value,
) -> Result<(), Failure> {
    let (Some(origin), Some(token)) = (
        s.config.notification_service_url.as_deref(),
        s.config.notification_internal_token.as_deref(),
    ) else {
        return Err(unavailable(()));
    };
    let status = s
        .http_client
        .request(method, format!("{}{path}", origin.trim_end_matches('/')))
        .header("X-Internal-Token", token)
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(unavailable)?
        .status();
    match status.as_u16() {
        200..=299 => Ok(()),
        400 => Err(invalid()),
        _ => Err(unavailable(())),
    }
}

/// After sign-in, on every app start and when FCM rotates the token. Signing in on an install another
/// Student used moves the install to this Student.
async fn register(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<DeviceScope>,
    j: Result<Json<Device>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(d) = j.map_err(|_| invalid())?;
    let id = recipient(&s, &h, &q).await?;
    let body = json!({"recipientId": id, "installationId": d.installation_id, "token": d.token,
        "platform": d.platform, "appVersion": d.app_version, "locale": d.locale});
    devices(&s, "/api/push/v1/devices", reqwest::Method::PUT, body).await?;
    Ok(Json(json!({"saved": true})))
}

/// On sign-out, before the app drops its session. Removes the install only if it is the caller's.
async fn remove(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<DeviceScope>,
    j: Result<Json<Installation>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(i) = j.map_err(|_| invalid())?;
    let id = recipient(&s, &h, &q).await?;
    let body = json!({"recipientId": id, "installationId": i.installation_id});
    devices(
        &s,
        "/api/push/v1/devices/delete",
        reqwest::Method::POST,
        body,
    )
    .await?;
    Ok(Json(json!({"removed": true})))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/learning/owner/student/push-devices",
            post(register).delete(remove),
        )
        .route("/api/v1/sis-service/learning/student-push", post(notify))
        .layer(axum::extract::DefaultBodyLimit::max(16_384))
        .layer(axum::middleware::map_response(
            |mut r: axum::response::Response| async move {
                r.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                r
            },
        ))
}

#[cfg(test)]
mod database_tests;
#[cfg(test)]
mod tests;
