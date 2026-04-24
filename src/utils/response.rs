// Generic JSON response wrapper, identical shape to admission-services
// so the frontend can consume both services with the same code path.
// Kept as a verbatim copy rather than a shared crate because dep-wise
// it's four lines and the two services rarely change this shape.

use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[allow(non_snake_case)]
pub struct ApiResponse<T>
where
    T: Serialize + ToSchema,
{
    pub responseCode: i32,
    pub responseMessage: String,
    pub responseError: Option<String>,
    pub data: Option<T>,
}

impl<T> ApiResponse<T>
where
    T: Serialize + ToSchema,
{
    pub fn success(data: T) -> Self {
        Self {
            responseCode: 200,
            responseMessage: "success".to_string(),
            responseError: None,
            data: Some(data),
        }
    }

    pub fn error(message: &str, code: i32) -> Self {
        Self {
            responseCode: code,
            responseMessage: "failed".to_string(),
            responseError: Some(message.to_string()),
            data: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_response_builds() {
        let res = ApiResponse::success("ok".to_string());
        assert_eq!(res.responseCode, 200);
        assert_eq!(res.responseMessage, "success");
        assert!(res.responseError.is_none());
        assert_eq!(res.data.as_deref(), Some("ok"));
    }

    #[test]
    fn error_response_carries_code() {
        let res: ApiResponse<String> = ApiResponse::error("boom", 500);
        assert_eq!(res.responseCode, 500);
        assert_eq!(res.responseError.as_deref(), Some("boom"));
        assert!(res.data.is_none());
    }
}
