use std::env;
use thiserror::Error;

// Trimmed from admission-services' AppConfig — SIS doesn't send email
// (no notification_service_url), doesn't serve password-reset links
// (no frontend_url), and doesn't call Moodle (no moodle_* block).
// Only the core triple (port, neo4j, jwt) remains. If SIS ever grows
// a cross-service call (e.g. to admission-services for Student
// lookups that can't go direct-to-graph), add it here.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub server_port: u16,
    pub neo4j_uri: String,
    pub neo4j_user: String,
    pub neo4j_password: String,
    pub jwt_secret: String,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing environment variable: {0}")]
    MissingVar(String),
    #[error("invalid port")]
    InvalidPort,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let server_port = env::var("SERVER_PORT")
            .map_err(|_| ConfigError::MissingVar("SERVER_PORT".into()))?
            .parse::<u16>()
            .map_err(|_| ConfigError::InvalidPort)?;

        let neo4j_uri =
            env::var("NEO4J_URI").map_err(|_| ConfigError::MissingVar("NEO4J_URI".into()))?;
        let neo4j_user =
            env::var("NEO4J_USER").map_err(|_| ConfigError::MissingVar("NEO4J_USER".into()))?;
        let neo4j_password = env::var("NEO4J_PASSWORD")
            .map_err(|_| ConfigError::MissingVar("NEO4J_PASSWORD".into()))?;

        // JWT_SECRET must match admission-services and auth-services
        // or cross-service bearer tokens won't decode. Default is
        // intentionally dev-only so a misconfigured prod pod fails
        // loudly at first auth rather than silently minting tokens
        // nobody else trusts.
        let jwt_secret = env::var("JWT_SECRET").unwrap_or_else(|_| "super-secret-key".to_string());

        Ok(Self {
            server_port,
            neo4j_uri,
            neo4j_user,
            neo4j_password,
            jwt_secret,
        })
    }
}
