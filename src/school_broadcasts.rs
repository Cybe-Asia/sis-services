//! School-wide broadcast: a school administrator posts one announcement to an audience of their own
//! school (all parents, a grade, classes, students' parents, or the staff). Parents get it on the
//! school board and as a push; staff get the push. Recipients are always resolved here from SIS,
//! never taken from the caller, and `dryRun` only counts them.
pub mod repository;
use crate::school_portal::model::identifier;
use crate::AppState;
use axum::{
    extract::{rejection::JsonRejection, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use repository::{Claim, Recipient};
use serde::Deserialize;
use serde_json::{json, Value};

type Failure = (StatusCode, Json<Value>);
fn refuse(status: StatusCode, code: &str) -> Failure {
    (status, Json(json!({"error":{"code":code}})))
}
fn unavailable<E>(_: E) -> Failure {
    refuse(StatusCode::SERVICE_UNAVAILABLE, "DEPENDENCY_UNAVAILABLE")
}

const MAX_IDS: usize = 500;
const MAX_TITLE: usize = 120;
const MAX_BODY: usize = 2000;
/// Pushes in flight at once; notification-services sends each one to FCM and Web Push.
const PUSH_BATCH: usize = 5;

/// Exactly one shape per type: `school` and `staff` take no ids, the others at least one.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Audience {
    School {},
    Grade { ids: Vec<String> },
    Class { ids: Vec<String> },
    Students { ids: Vec<String> },
    Staff {},
}
impl Audience {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::School {} => "school",
            Self::Grade { .. } => "grade",
            Self::Class { .. } => "class",
            Self::Students { .. } => "students",
            Self::Staff {} => "staff",
        }
    }
    pub fn ids(&self) -> &[String] {
        match self {
            Self::Grade { ids } | Self::Class { ids } | Self::Students { ids } => ids,
            Self::School {} | Self::Staff {} => &[],
        }
    }
    fn valid(&self) -> bool {
        match self {
            Self::School {} | Self::Staff {} => true,
            Self::Grade { ids } | Self::Class { ids } | Self::Students { ids } => {
                let mut sorted: Vec<&String> = ids.iter().collect();
                sorted.sort();
                sorted.dedup();
                (1..=MAX_IDS).contains(&ids.len())
                    && sorted.len() == ids.len()
                    && ids.iter().all(|id| match self {
                        // A grade is `grade_code`, else the free-text `year_group` ("Year 8").
                        Self::Grade { .. } => {
                            (1..=128).contains(&id.len()) && !id.chars().any(char::is_control)
                        }
                        _ => identifier(id),
                    })
            }
        }
    }
    fn staff(&self) -> bool {
        matches!(self, Self::Staff {})
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Broadcast {
    pub tenant_id: String,
    pub school_id: String,
    /// Chosen by the client once per announcement; a retry sends the same id.
    pub id: String,
    pub audience: Audience,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub dry_run: bool,
}
impl Broadcast {
    fn valid(&self) -> bool {
        let text = |v: &str, max: usize| !v.trim().is_empty() && v.chars().count() <= max;
        identifier(&self.tenant_id)
            && identifier(&self.school_id)
            && identifier(&self.id)
            && self.audience.valid()
            && text(&self.title, MAX_TITLE)
            && text(&self.body, MAX_BODY)
    }
}

/// Lock screens are not private: a fixed text in the reader's language, never the announcement.
fn push_payload(r: &Recipient, staff: bool) -> Value {
    let (title, body) = if r.locale.starts_with("id") {
        ("Pengumuman sekolah", "Ada pengumuman baru dari sekolah.")
    } else {
        (
            "School announcement",
            "There is a new announcement from the school.",
        )
    };
    let url = if staff { "/teacher" } else { "/parent" };
    json!({"recipientId": r.id, "title": title, "body": body, "url": url})
}

/// What happens after the claim. Only a fresh claim pushes, and only to those who did not opt out.
pub(crate) fn to_push(claim: &Claim, recipients: Vec<Recipient>) -> Vec<Recipient> {
    match claim {
        Claim::Fresh => recipients.into_iter().filter(|r| r.notify).collect(),
        _ => vec![],
    }
}

/// Fans the push out through notification-services `/api/push/v1/send`, `PUSH_BATCH` at a time, with
/// [payload] per recipient. That service drops dead tokens and expired browser subscriptions as it
/// sends. `None` when notification-services is not configured (nothing is sent).
pub(crate) async fn deliver(
    s: &AppState,
    recipients: &[Recipient],
    payload: impl Fn(&Recipient) -> Value,
) -> Option<(i64, i64)> {
    let (Some(origin), Some(token)) = (
        s.config.notification_service_url.clone(),
        s.config.notification_internal_token.clone(),
    ) else {
        return None;
    };
    let url = format!("{}/api/push/v1/send", origin.trim_end_matches('/'));
    let (mut pushed, mut failed) = (0_i64, 0_i64);
    for batch in recipients.chunks(PUSH_BATCH) {
        let mut set = tokio::task::JoinSet::new();
        for r in batch {
            let request = s
                .http_client
                .post(&url)
                .header("X-Internal-Token", &token)
                .json(&payload(r))
                .timeout(std::time::Duration::from_secs(15));
            set.spawn(async move {
                request
                    .send()
                    .await
                    .map(|r| r.status().is_success())
                    .unwrap_or(false)
            });
        }
        while let Some(done) = set.join_next().await {
            if done.unwrap_or(false) {
                pushed += 1
            } else {
                failed += 1
            }
        }
    }
    Some((pushed, failed))
}
// shortcut: the fan-out lives in this process; if it restarts mid-way the rest is not pushed and the
// broadcast stays `sending` (notices are already posted). Move to a durable outbox if that matters.
async fn fan_out(s: AppState, id: String, staff: bool, recipients: Vec<Recipient>) {
    let (pushed, failed) = match deliver(&s, &recipients, |r| push_payload(r, staff)).await {
        Some(outcome) => outcome,
        None => {
            tracing::warn!(
                "broadcast {id}: notification-services is not configured, nothing pushed"
            );
            (0, recipients.len() as i64)
        }
    };
    if repository::finish(&s.graph, &id, pushed, failed)
        .await
        .is_err()
    {
        tracing::warn!("broadcast {id}: outcome not recorded");
    }
}
async fn send(
    State(s): State<AppState>,
    h: HeaderMap,
    j: Result<Json<Broadcast>, JsonRejection>,
) -> Result<Json<Value>, Failure> {
    let Json(v) = j.map_err(|_| refuse(StatusCode::BAD_REQUEST, "INVALID_INPUT"))?;
    if !v.valid() {
        return Err(refuse(StatusCode::BAD_REQUEST, "INVALID_INPUT"));
    }
    let actor = crate::school_articles::manager(&s, &h, &v.tenant_id, &v.school_id).await?;
    let outcome = prepare(&s.graph, crate::school_articles::capable(&actor), &v).await?;
    if !outcome.push.is_empty() {
        tokio::spawn(fan_out(
            s.clone(),
            v.id.clone(),
            v.audience.staff(),
            outcome.push,
        ));
    }
    Ok(Json(json!({"data": outcome.answer})))
}

pub(crate) struct Prepared {
    pub answer: Value,
    /// Recipients the caller pushes to; empty for a dry run or a repeated id.
    pub push: Vec<Recipient>,
}

/// Validates the audience against this school, counts it, and on a real send claims the id.
pub(crate) async fn prepare(
    graph: &neo4rs::Graph,
    actor: crate::school_articles::repository::Actor<'_>,
    v: &Broadcast,
) -> Result<Prepared, Failure> {
    let (school, tenant) = (v.school_id.as_str(), v.tenant_id.as_str());
    if !repository::known(graph, school, tenant, &v.audience)
        .await
        .map_err(unavailable)?
    {
        return Err(refuse(StatusCode::UNPROCESSABLE_ENTITY, "UNKNOWN_AUDIENCE"));
    }
    let recipients = repository::recipients(graph, school, tenant, &v.audience)
        .await
        .map_err(unavailable)?;
    let count = recipients.len();
    if v.dry_run {
        return Ok(Prepared {
            answer: json!({"dryRun": true, "audience": v.audience.kind(), "recipients": count}),
            push: vec![],
        });
    }
    let send = repository::Outgoing {
        id: &v.id,
        school,
        tenant,
        audience: &v.audience,
        title: v.title.trim(),
        body: v.body.trim(),
    };
    let claim = repository::claim(graph, actor, &send, &recipients)
        .await
        .map_err(unavailable)?;
    let answer = match &claim {
        Claim::Fresh => {
            json!({"id": v.id, "recipients": count, "duplicate": false, "status": "sending"})
        }
        Claim::Duplicate { recipients, status } => {
            json!({"id": v.id, "recipients": recipients, "duplicate": true, "status": status})
        }
        Claim::Conflict => return Err(refuse(StatusCode::CONFLICT, "ID_IN_USE")),
        Claim::Forbidden => return Err(refuse(StatusCode::FORBIDDEN, "FORBIDDEN")),
    };
    Ok(Prepared {
        answer,
        push: to_push(&claim, recipients),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Scope {
    tenant_id: String,
    school_id: String,
}

async fn audiences(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<Scope>,
) -> Result<Json<Value>, Failure> {
    if !identifier(&q.tenant_id) || !identifier(&q.school_id) {
        return Err(refuse(StatusCode::BAD_REQUEST, "INVALID_INPUT"));
    }
    crate::school_articles::manager(&s, &h, &q.tenant_id, &q.school_id).await?;
    repository::audiences(&s.graph, &q.school_id, &q.tenant_id)
        .await
        .map(|data| Json(json!({"data": data})))
        .map_err(unavailable)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/school-broadcasts",
            axum::routing::post(send),
        )
        .route(
            "/api/v1/sis-service/school-broadcasts/audiences",
            get(audiences),
        )
        .layer(axum::extract::DefaultBodyLimit::max(32_768))
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
mod tests {
    use super::*;
    fn parse(v: Value) -> Option<Audience> {
        serde_json::from_value::<Audience>(v)
            .ok()
            .filter(Audience::valid)
    }
    #[test]
    fn each_audience_type_has_exactly_one_shape() {
        assert_eq!(parse(json!({"type":"school"})), Some(Audience::School {}));
        assert_eq!(parse(json!({"type":"staff"})), Some(Audience::Staff {}));
        assert_eq!(
            parse(json!({"type":"grade","ids":["Y7"]})),
            Some(Audience::Grade {
                ids: vec!["Y7".into()]
            })
        );
        assert!(parse(json!({"type":"grade","ids":["Year 8"]})).is_some());
        assert!(parse(json!({"type":"class","ids":["7a","7b"]})).is_some());
        assert!(parse(json!({"type":"students","ids":["s-1"]})).is_some());
        for bad in [
            json!({"type":"school","ids":["x"]}),
            json!({"type":"grade"}),
            json!({"type":"grade","ids":[]}),
            json!({"type":"class","ids":["7a","7a"]}),
            json!({"type":"students","ids":["a b"]}),
            json!({"type":"grade","ids":["Y7\n"]}),
            json!({"type":"everyone"}),
            json!({"type":"class","ids":["7a"],"query":"MATCH(n) RETURN n"}),
        ] {
            assert_eq!(parse(bad.clone()), None, "{bad}");
        }
    }
    #[test]
    fn the_lock_screen_carries_no_announcement_text() {
        let r = |locale: &str| Recipient {
            id: "parent:u1".into(),
            user: Some("u1".into()),
            locale: locale.into(),
            notify: true,
        };
        let id = push_payload(&r("id"), false);
        assert_eq!(id["title"], "Pengumuman sekolah");
        assert_eq!(id["url"], "/parent");
        assert_eq!(
            push_payload(&r("en"), false)["title"],
            "School announcement"
        );
        assert_eq!(push_payload(&r("id"), true)["url"], "/teacher");
    }
    #[test]
    fn only_a_fresh_claim_pushes_and_only_to_those_who_want_it() {
        let r = |id: &str, notify: bool| Recipient {
            id: id.into(),
            user: None,
            locale: "id".into(),
            notify,
        };
        let all = vec![r("parent:a", true), r("parent:b", false)];
        assert_eq!(
            to_push(&Claim::Fresh, all.clone()),
            vec![r("parent:a", true)]
        );
        assert!(to_push(
            &Claim::Duplicate {
                recipients: 2,
                status: "sent".into()
            },
            all.clone()
        )
        .is_empty());
        assert!(to_push(&Claim::Forbidden, all).is_empty());
    }
}
