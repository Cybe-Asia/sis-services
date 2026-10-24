//! Admin teacher list (slice 1 of the teacher assignments spec, `docs/TEACHER_LIST.md`): who
//! teaches what, homerooms, coaches and scheduled load, read from data SIS already stores.
//! School administrators and Owners of the school only; nothing is written.
use crate::AppState;
use axum::{
    extract::{rejection::QueryRejection, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub mod model;
pub mod repository;

use crate::school_portal::model::identifier;

type Failure = (StatusCode, Json<Value>);
fn failure(status: StatusCode) -> Failure {
    let code = match status {
        StatusCode::BAD_REQUEST => "INVALID_INPUT",
        StatusCode::UNAUTHORIZED => "UNAUTHORIZED",
        StatusCode::FORBIDDEN => "FORBIDDEN",
        StatusCode::NOT_FOUND => "NOT_FOUND",
        _ => "DEPENDENCY_UNAVAILABLE",
    };
    (status, Json(json!({"error":{"code":code}})))
}
fn unavailable<E>(_: E) -> Failure {
    failure(StatusCode::SERVICE_UNAVAILABLE)
}
async fn deadline<T>(
    work: impl std::future::Future<Output = Result<T, Failure>>,
) -> Result<T, Failure> {
    tokio::time::timeout(Duration::from_secs(8), work)
        .await
        .map_err(unavailable)?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Lookup {
    school_id: String,
    tenant_id: String,
    academic_year: String,
    /// Any date of the school week to read; today (WIB) when absent.
    week: Option<String>,
}
impl Lookup {
    fn week(&self) -> Option<(NaiveDate, NaiveDate)> {
        let anchor = match &self.week {
            Some(day) if day.len() == 10 => NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?,
            Some(_) => return None,
            None => model::today_wib(chrono::Utc::now().timestamp_millis()),
        };
        Some(model::school_week(anchor))
    }
    fn valid(&self) -> bool {
        identifier(&self.school_id)
            && identifier(&self.tenant_id)
            && !self.academic_year.trim().is_empty()
            && self.academic_year.len() <= 32
            && !self.academic_year.chars().any(char::is_control)
    }
}

/// Session, graph authority, the read, then the session again: a role revoked while the read
/// ran does not get the answer.
async fn read(
    state: &AppState,
    headers: &HeaderMap,
    q: &Lookup,
) -> Result<(model::Report, Value), Failure> {
    let (monday, friday) = q
        .week()
        .filter(|_| q.valid())
        .ok_or_else(|| failure(StatusCode::BAD_REQUEST))?;
    let session =
        crate::learning_access::administrator_actor(headers, &q.tenant_id, &q.school_id).await?;
    let report = deadline(async {
        if !repository::authorize(&state.graph, &session.0, &q.school_id, &q.tenant_id)
            .await
            .map_err(unavailable)?
        {
            return Err(failure(StatusCode::FORBIDDEN));
        }
        let (from, to) = model::window_ms(monday);
        let snapshot = repository::snapshot(
            &state.graph,
            &q.school_id,
            &q.tenant_id,
            &q.academic_year,
            from,
            to,
        )
        .await
        .map_err(unavailable)?;
        model::report(&snapshot).ok_or_else(|| failure(StatusCode::SERVICE_UNAVAILABLE))
    })
    .await?;
    if crate::learning_access::administrator_actor(headers, &q.tenant_id, &q.school_id).await?
        != session
    {
        return Err(failure(StatusCode::FORBIDDEN));
    }
    let frame = json!({
        "schoolId": q.school_id,
        "tenantId": q.tenant_id,
        "academicYear": q.academic_year,
        "week": {"from": monday.to_string(), "to": friday.to_string(), "lessons": report.lessons},
        "limits": {
            "lowerPeriods": model::LOWER_PERIODS,
            "upperPeriods": model::UPPER_PERIODS,
            "juniorPeriodMinutes": model::JUNIOR_PERIOD_MINUTES,
            "seniorPeriodMinutes": model::SENIOR_PERIOD_MINUTES,
        },
    });
    Ok((report, frame))
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    q: Result<Query<Lookup>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    let (report, mut data) = read(&state, &headers, &q).await?;
    data["teachers"] = json!(report.teachers);
    data["classWarnings"] = json!(report.class_warnings);
    data["summary"] = json!(report.summary);
    Ok(Json(json!({"data": data})))
}

async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(staff_member_id): Path<String>,
    q: Result<Query<Lookup>, QueryRejection>,
) -> Result<Json<Value>, Failure> {
    let Query(q) = q.map_err(|_| failure(StatusCode::BAD_REQUEST))?;
    if !identifier(&staff_member_id) {
        return Err(failure(StatusCode::BAD_REQUEST));
    }
    let (report, mut data) = read(&state, &headers, &q).await?;
    // Only people the list would show; anyone else is the same 404 as an unknown id.
    let teacher = report
        .teachers
        .into_iter()
        .find(|t| t.staff_member_id == staff_member_id)
        .ok_or_else(|| failure(StatusCode::NOT_FOUND))?;
    data["schedule"] = json!(teacher.schedule);
    data["teacher"] = json!(teacher);
    Ok(Json(json!({"data": data})))
}

/// The schools the caller administers and their academic years, newest first.
async fn context(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, Failure> {
    let (actor, role, schools, tenants) = crate::learning_access::staff_session(&headers)
        .await?
        .scope();
    let all = role == "owner" && schools.is_empty() && tenants.is_empty();
    let rows = deadline(async {
        repository::schools(&state.graph, &actor, schools, tenants, all)
            .await
            .map_err(unavailable)
    })
    .await?;
    let mut by_school: BTreeMap<(String, String), Vec<Value>> = BTreeMap::new();
    for r in rows {
        by_school
            .entry((r.school_id, r.tenant_id))
            .or_default()
            .push(json!({"academicYear": r.academic_year, "classes": r.classes}));
    }
    let schools: Vec<Value> = by_school
        .into_iter()
        .map(|((school, tenant), years)| json!({"schoolId": school, "tenantId": tenant, "academicYears": years}))
        .collect();
    Ok(Json(json!({"data": {"role": role, "schools": schools}})))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/sis-service/academic/teachers/context",
            get(context),
        )
        .route("/api/v1/sis-service/academic/teachers", get(list))
        .route(
            "/api/v1/sis-service/academic/teachers/:staff_member_id",
            get(detail),
        )
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
