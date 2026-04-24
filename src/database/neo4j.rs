use crate::config::config::AppConfig;
use neo4rs::{ConfigBuilder, Graph};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Neo4jError {
    #[error("failed to connect to neo4j")]
    ConnectError,
}

fn normalize_uri(uri: &str) -> String {
    uri.strip_prefix("bolt://").unwrap_or(uri).to_string()
}

pub async fn init_neo4j(cfg: &AppConfig) -> Result<Graph, Neo4jError> {
    let uri = normalize_uri(&cfg.neo4j_uri);
    let config = ConfigBuilder::default()
        .uri(uri)
        .user(cfg.neo4j_user.clone())
        .password(cfg.neo4j_password.clone())
        .db("neo4j")
        .max_connections(8)
        .build()
        .map_err(|_| Neo4jError::ConnectError)?;
    Graph::connect(config).await.map_err(|_| Neo4jError::ConnectError)
}
