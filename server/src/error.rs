use axum::{
    Json,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub fields: Vec<Value>,
    pub request_id: String,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            fields: vec![],
            request_id: Uuid::new_v4().to_string(),
        }
    }
    pub fn field(field: impl Into<String>, message: impl Into<String>) -> Self {
        let message = message.into();
        let mut error = Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "Payload validation failed",
        );
        error
            .fields
            .push(json!({"field":field.into(), "message":message}));
        error
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }
    pub fn unavailable() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "backpressure",
            "Server is busy; retry with a fresh request ID",
        )
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(error = %error, "Database operation failed");
        if matches!(error, sqlx::Error::PoolTimedOut) {
            return Self::unavailable();
        }
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "The operation could not be completed",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(json!({"error":{
            "code":self.code,"message":self.message,"fields":self.fields,"request_id":self.request_id
        }}))).into_response();
        if let Ok(value) = HeaderValue::from_str(&self.request_id) {
            response.headers_mut().insert("x-request-id", value);
        }
        response
            .headers_mut()
            .insert("cache-control", HeaderValue::from_static("no-store"));
        if self.status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert("retry-after", HeaderValue::from_static("1"));
        }
        response
    }
}
