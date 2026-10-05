//! Isolated local preview. The primary SIS registration remains owned by LMS.
mod config;
mod education_calendar;
mod extra_curricular;
mod learning_access;
mod models;
mod school_portal;
mod utils;
#[derive(Clone)]
pub struct AppState {
    pub graph: neo4rs::Graph,
    pub config: config::config::AppConfig,
    pub http_client: reqwest::Client,
}
#[tokio::main]
async fn main() {
    let c = config::config::AppConfig::from_env().expect("preview configuration");
    assert_eq!(c.server_port, 3235, "isolated preview port only");
    assert_eq!(
        c.neo4j_uri, "127.0.0.1:3223",
        "dedicated fixture graph only"
    );
    let graph = neo4rs::Graph::new(&c.neo4j_uri, &c.neo4j_user, &c.neo4j_password)
        .await
        .expect("fixture graph unavailable");
    extra_curricular::repository::init(&graph)
        .await
        .expect("additive schema");
    let state = AppState {
        graph,
        config: c,
        http_client: reqwest::Client::new(),
    };
    let app = extra_curricular::router().with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3235")
        .await
        .expect("preview port unavailable");
    axum::serve(listener, app).await.unwrap();
}
