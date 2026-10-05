// sis-service
//
// Owns the Student Information System side of the Digital School
// domain — post-enrolment concerns: Sections, Enrolments, Attendance,
// Grades. Sibling service to admission-services, which owns the
// admissions-funnel side (Lead, Application, ApplicantStudent, Test,
// Offer).
//
// Neo4j is shared between the two services; label ownership is
// documented in CLAUDE.md:
//
//   admission-services owns: Lead, Application, :Student
//                            (ApplicantStudent), TestSchedule,
//                            TestSession, TestResult, DocumentRequest,
//                            DocumentArtifact, DocumentReview, Offer,
//                            OfferAcceptance, AdmissionDecision.
//
//   sis-services owns:       Section, AttendanceRecord, GradeEntry,
//                            permanent EnrolledStudent creation through
//                            explicit paid-offer handover, its receipt/audit,
//                            and the :ENROLLED_IN placement. The existing
//                            handed_to_sis applicant stage mirrors that
//                            committed handover; legacy enrollment records
//                            remain unchanged.
//
// auth-services owns the credentials. Parent sessions use JWT_SECRET;
// Admin enrollment/billing use the distinct typed staff downstream key.

use axum::{routing::get, Router};
use serde_json::json;
use std::net::SocketAddr;
use tower_http::trace::TraceLayer;
use tracing::{info, Level};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

mod config;
mod billing_owner;
mod enrollment_handover;
mod database;
mod handlers;
mod learning_access;
mod school_portal;
mod education_calendar;
mod extra_curricular;
mod models;
mod repositories;
mod routes;
mod utils;

use config::config::AppConfig;
use database::neo4j::init_neo4j;
use routes::sis_routes::sis_router;

#[derive(OpenApi)]
#[openapi(
    paths(health_check),
    components(schemas(utils::response::ApiResponse<String>))
)]
pub struct ApiDoc;

#[utoipa::path(
    get,
    path = "/api/v1/sis-service/health",
    responses((status = 200, description = "Service is healthy"))
)]
async fn health_check() -> impl axum::response::IntoResponse {
    axum::response::Json(json!({ "status": "ok", "service": "sis-service" }))
}

#[derive(Clone)]
pub struct AppState {
    pub graph: neo4rs::Graph,
    pub config: AppConfig,
    pub http_client: reqwest::Client,
}

fn load_env() {
    // Identical .env loading pattern to admission-services so ops
    // procedures port over. APP_ENV=production reads .env.production;
    // unset falls back to .env.local then plain .env.
    match std::env::var("APP_ENV") {
        Ok(env) if !env.trim().is_empty() => {
            let file = format!(".env.{}", env);
            dotenv::from_filename(file).ok();
        }
        _ => {
            dotenv::from_filename(".env.local").ok();
        }
    }
    dotenv::dotenv().ok();
}

#[tokio::main]
async fn main() {
    load_env();
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env().add_directive(Level::INFO.into()))
        .init();

    let cfg = AppConfig::from_env().expect("failed to load config");
    let graph: neo4rs::Graph = init_neo4j(&cfg)
        .await
        .expect("failed to connect to neo4j");
    if std::env::args().nth(1).as_deref()==Some("migrate-learning-access") {
        learning_access::migrate(&graph).await.expect("failed to initialize governed learning access schema");
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--migrate-enrollment-handover") {
        enrollment_handover::migrate(&graph)
            .await
            .expect("failed to initialize enrollment handover schema");
        return;
    }

    // Index setup for labels owned by this service.
    // Lead/Student indexes are owned by admission-services; don't
    // double-declare them here or we'll get constraint conflicts.
    repositories::sis_repository::init_section_indexes(&graph)
        .await
        .expect("failed to initialize section indexes");
    school_portal::repository::init(&graph)
        .await
        .expect("failed to initialize school portal indexes");

    education_calendar::repository::init(&graph).await.expect("failed to initialize education calendar indexes");
    extra_curricular::repository::init(&graph).await.expect("extracurricular schema initialization failed");

    let state = AppState {
        graph,
        config: cfg.clone(),
        http_client: reqwest::Client::new(),
    };
    let app = Router::new()
        .route("/api/v1/sis-service/health", get(health_check))
        .merge(sis_router())
        .merge(billing_owner::router())
        .merge(enrollment_handover::router())
        .merge(learning_access::router())
        .merge(school_portal::router())
        .merge(education_calendar::router())
        .merge(extra_curricular::router())
        .merge(
            SwaggerUi::new("/api/v1/sis-service/swagger-ui")
                .url("/api/v1/sis-service/api-docs/openapi.json", ApiDoc::openapi()),
        )
        .with_state(state.clone())
        .layer(TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<axum::body::Body>| tracing::info_span!("sis_request",method=%request.method())));

    let addr: SocketAddr = format!("0.0.0.0:{}", cfg.server_port)
        .parse()
        .expect("invalid server port");
    info!("starting sis-service on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
