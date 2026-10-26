use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct AppError {
    pub code: String,
    pub stage: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
pub type AppResult<T> = Result<T, AppError>;
impl AppError {
    pub fn new(code: &str, stage: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            stage: stage.into(),
            message: message.into(),
            details: None,
        }
    }
    pub fn io(stage: &str, error: impl std::fmt::Display) -> Self {
        Self::new("E_IO", stage, error.to_string())
    }
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}
impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for AppError {}
