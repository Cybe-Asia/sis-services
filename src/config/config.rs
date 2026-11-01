use std::{env, io::Read};
use thiserror::Error;

// Trimmed from admission-services' AppConfig — SIS doesn't send email
// (no notification_service_url), doesn't serve password-reset links
// (no frontend_url), and doesn't call Moodle (no moodle_* block).
// Only the core triple (port, neo4j, jwt) remains. If SIS ever grows
// a cross-service call (e.g. to admission-services for Student
// lookups that can't go direct-to-graph), add it here.
#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub server_port: u16,
    pub neo4j_uri: String,
    pub neo4j_user: String,
    pub neo4j_password: String,
    pub jwt_secret: String,
    // Everything below is optional: a route that needs a missing value answers 503, as it did
    // when it read the environment itself.
    /// `AUTH_PORTAL_INTROSPECTION_URL`: Admin portal sessions (Learning access, staff routes).
    pub auth_portal_introspection_url: Option<String>,
    /// `AUTH_SCHOOL_MEMBERSHIP_URL`: Teacher memberships.
    pub auth_school_membership_url: Option<String>,
    /// `AUTH_STUDENT_SESSION_URL`: live Student sessions.
    pub auth_student_session_url: Option<String>,
    /// `AUTH_PORTAL_CA_FILE`, read at startup: the private trust anchor for calls to Auth.
    pub auth_portal_ca: Option<Vec<u8>>,
    /// `LEARNING_SERVICE_ORIGIN`: Learning grade exports.
    pub learning_service_origin: Option<String>,
    /// `LEARNING_SERVICE_CA_FILE`, read at startup: the private trust anchor for Learning.
    pub learning_service_ca: Option<Vec<u8>>,
    /// `STAFF_DOWNSTREAM_JWT_SECRET` and `_ISSUER`: Admin's staff-api+jwt credential family.
    pub staff_downstream_jwt_secret: Option<String>,
    pub staff_downstream_jwt_issuer: Option<String>,
    /// `SIS_PARENT_LEARNING_BINDING_ENABLED=true`.
    pub parent_learning_binding_enabled: bool,
    /// `APP_ENV=test`: loopback HTTP Auth and Learning endpoints are accepted.
    pub local_test: bool,
    /// `MINIO_*`: the object store for article covers and recitation clips.
    pub object_store: Option<ObjectStoreSettings>,
}

/// The object store this deployment names; `None` when any required `MINIO_*` value is missing or blank.
#[derive(Debug, Clone)]
pub struct ObjectStoreSettings {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
}

fn object_store() -> Option<ObjectStoreSettings> {
    let var = |name: &str| env::var(name).ok().filter(|v| !v.trim().is_empty());
    Some(ObjectStoreSettings {
        endpoint: var("MINIO_ENDPOINT")?,
        bucket: var("MINIO_BUCKET")?,
        region: var("MINIO_REGION").unwrap_or_else(|| "us-east-1".into()),
        access_key: var("MINIO_ACCESS_KEY")?,
        secret_key: var("MINIO_SECRET_KEY")?,
    })
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing environment variable: {0}")]
    MissingVar(String),
    #[error("invalid port")]
    InvalidPort,
    #[error("{0} names a file that cannot be read")]
    UnreadableFile(String),
}

/// A trust anchor named by `key`, read once here. A configured file that cannot be read stops
/// start-up: a route must never fall back to default trust. Its content is checked where it is used.
fn ca_file(key: &str) -> Result<Option<Vec<u8>>, ConfigError> {
    let Some(path) = env::var_os(key) else {
        return Ok(None);
    };
    let mut pem = Vec::new();
    std::fs::File::open(&path)
        .and_then(|file| file.take(262_145).read_to_end(&mut pem))
        .map_err(|_| ConfigError::UnreadableFile(key.into()))?;
    Ok(Some(pem))
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
        // or cross-service bearer tokens won't decode. No default: a
        // missing secret stops the service at startup.
        let jwt_secret = env::var("JWT_SECRET")
            .ok()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ConfigError::MissingVar("JWT_SECRET".into()))?;
        if jwt_secret.len() < 32 {
            // shortcut: warn instead of refusing so a short live secret cannot take SIS down; refuse once every environment is >= 32 bytes.
            tracing::warn!("JWT_SECRET is shorter than 32 bytes");
        }

        Ok(Self {
            server_port,
            neo4j_uri,
            neo4j_user,
            neo4j_password,
            jwt_secret,
            auth_portal_introspection_url: env::var("AUTH_PORTAL_INTROSPECTION_URL").ok(),
            auth_school_membership_url: env::var("AUTH_SCHOOL_MEMBERSHIP_URL").ok(),
            auth_student_session_url: env::var("AUTH_STUDENT_SESSION_URL").ok(),
            auth_portal_ca: ca_file("AUTH_PORTAL_CA_FILE")?,
            learning_service_origin: env::var("LEARNING_SERVICE_ORIGIN").ok(),
            learning_service_ca: ca_file("LEARNING_SERVICE_CA_FILE")?,
            staff_downstream_jwt_secret: env::var("STAFF_DOWNSTREAM_JWT_SECRET").ok(),
            staff_downstream_jwt_issuer: env::var("STAFF_DOWNSTREAM_JWT_ISSUER").ok(),
            parent_learning_binding_enabled: env::var("SIS_PARENT_LEARNING_BINDING_ENABLED")
                .as_deref()
                == Ok("true"),
            local_test: env::var("APP_ENV").as_deref() == Ok("test"),
            object_store: object_store(),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_configured_trust_anchor_that_cannot_be_read_stops_start_up() {
        // Unique names: no other test reads them.
        std::env::set_var(
            "SIS_TEST_MISSING_CA_FILE",
            "/nonexistent/sis-private-ca.pem",
        );
        std::env::set_var("SIS_TEST_EMPTY_CA_FILE", "");
        assert!(super::ca_file("SIS_TEST_MISSING_CA_FILE").is_err());
        assert!(super::ca_file("SIS_TEST_EMPTY_CA_FILE").is_err());
        assert!(super::ca_file("SIS_TEST_UNSET_CA_FILE").unwrap().is_none());
    }
}
