use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub kind: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            kind: "invalid_request_error",
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            kind: "not_found_error",
            message: message.into(),
        }
    }

    pub fn upstream(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            kind: "upstream_error",
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "error": { "message": self.message, "type": self.kind, "code": null } });
        (self.status, Json(body)).into_response()
    }
}
