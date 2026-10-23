use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Forbidden(String),
    NotFound(String),
    Unprocessable(String),
    Conflict(String),
    Upstream(String),
    Internal(String),
}

impl ApiError {
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::Upstream(_) => StatusCode::BAD_GATEWAY,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ApiError::BadRequest(message)
            | ApiError::Forbidden(message)
            | ApiError::NotFound(message)
            | ApiError::Unprocessable(message)
            | ApiError::Conflict(message)
            | ApiError::Upstream(message)
            | ApiError::Internal(message) => message,
        }
    }

    pub fn from_core(error: sing_portal_core::Error) -> Self {
        match error {
            sing_portal_core::Error::NoOutbounds
            | sing_portal_core::Error::EmptyContent
            | sing_portal_core::Error::HtmlContent
            | sing_portal_core::Error::UnrecognizedFormat
            | sing_portal_core::Error::TooManyOutbounds(_, _)
            | sing_portal_core::Error::Json(_)
            | sing_portal_core::Error::Yaml(_)
            | sing_portal_core::Error::Uri(_)
            | sing_portal_core::Error::UnsupportedScheme(_) => {
                ApiError::Unprocessable(error.to_string())
            }
            other @ sing_portal_core::Error::ClashApi(_) => ApiError::Upstream(other.to_string()),
            other => ApiError::Internal(other.to_string()),
        }
    }
}

impl std::error::Error for ApiError {}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        } else {
            tracing::debug!(error = %self, "request rejected");
        }
        (status, Json(json!({ "error": self.message() }))).into_response()
    }
}

impl From<sing_portal_store::StoreError> for ApiError {
    fn from(error: sing_portal_store::StoreError) -> Self {
        match error {
            sing_portal_store::StoreError::NotFound => ApiError::NotFound("not found".into()),
            other => ApiError::Internal(other.to_string()),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
