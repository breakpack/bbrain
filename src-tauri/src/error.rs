use serde::Serialize;

/// Stable machine-readable code the UI maps to a calm Korean message.
/// Never carries secrets: see `AppError::redacted_message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Storage,
    Migration,
    CredentialStore,
    ProviderAuth,
    ProviderRateLimit,
    ProviderUnavailable,
    ProviderResponse,
    ModelUnsupported,
    Network,
    NotFound,
    InvalidInput,
    Rejected,
    Cancelled,
    NoteConflict,
    VaultUnavailable,
    EmbeddingModelUnavailable,
    Internal,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("storage error")]
    Storage(#[from] rusqlite::Error),

    #[error("migration error: {0}")]
    Migration(String),

    #[error("credential store error")]
    CredentialStore(#[from] keyring::Error),

    #[error("provider authentication failed")]
    ProviderAuth,

    #[error("provider rate limited")]
    ProviderRateLimit { retry_after_seconds: Option<u64> },

    #[error("provider unavailable")]
    ProviderUnavailable,

    #[error("provider returned an unusable response: {0}")]
    ProviderResponse(String),

    #[error("model is not usable: {0}")]
    ModelUnsupported(String),

    #[error("network error")]
    Network,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("file rejected: {0:?}")]
    Rejected(crate::import::pdf_file::RejectReason),

    #[error("cancelled by the user")]
    Cancelled,

    #[error("obsidian note markers are damaged")]
    NoteConflict(crate::obsidian::note::ParseError),

    #[error("obsidian vault unavailable: {0}")]
    VaultUnavailable(String),

    #[error("local embedding model unavailable")]
    EmbeddingModelUnavailable,

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Storage(_) => ErrorCode::Storage,
            Self::Migration(_) => ErrorCode::Migration,
            Self::CredentialStore(_) => ErrorCode::CredentialStore,
            Self::ProviderAuth => ErrorCode::ProviderAuth,
            Self::ProviderRateLimit { .. } => ErrorCode::ProviderRateLimit,
            Self::ProviderUnavailable => ErrorCode::ProviderUnavailable,
            Self::ProviderResponse(_) => ErrorCode::ProviderResponse,
            Self::ModelUnsupported(_) => ErrorCode::ModelUnsupported,
            Self::Network => ErrorCode::Network,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::InvalidInput(_) => ErrorCode::InvalidInput,
            Self::Rejected(_) => ErrorCode::Rejected,
            Self::Cancelled => ErrorCode::Cancelled,
            Self::NoteConflict(_) => ErrorCode::NoteConflict,
            Self::VaultUnavailable(_) => ErrorCode::VaultUnavailable,
            Self::EmbeddingModelUnavailable => ErrorCode::EmbeddingModelUnavailable,
            Self::Internal(_) => ErrorCode::Internal,
        }
    }

    /// Message safe to log and to show. Provider bodies and credentials are
    /// never interpolated into these strings (DEVELOPMENT.md §16.1).
    pub fn redacted_message(&self) -> String {
        match self {
            Self::Storage(_) => "로컬 데이터베이스에 접근하지 못했습니다.".into(),
            Self::Migration(_) => "데이터베이스를 최신 버전으로 갱신하지 못했습니다.".into(),
            Self::CredentialStore(_) => "시스템 키체인에 접근하지 못했습니다.".into(),
            Self::ProviderAuth => "API 키가 올바르지 않거나 권한이 없습니다.".into(),
            Self::ProviderRateLimit { .. } => "공급자 요청 한도에 도달했습니다.".into(),
            Self::ProviderUnavailable => "공급자 서비스에 일시적인 문제가 있습니다.".into(),
            Self::ProviderResponse(_) => "공급자 응답을 이해하지 못했습니다.".into(),
            Self::ModelUnsupported(_) => "선택한 모델을 사용할 수 없습니다.".into(),
            Self::Network => "네트워크에 연결하지 못했습니다.".into(),
            Self::NotFound(_) => "대상을 찾지 못했습니다.".into(),
            Self::InvalidInput(_) => "입력값을 처리할 수 없습니다.".into(),
            Self::Rejected(reason) => reason.message().into(),
            Self::Cancelled => "요청을 취소했습니다.".into(),
            Self::NoteConflict(reason) => reason.message().into(),
            Self::VaultUnavailable(_) => {
                "Obsidian Vault에 접근하지 못했습니다. 앱은 계속 사용할 수 있고 동기화만 중지됩니다.".into()
            }
            Self::EmbeddingModelUnavailable => {
                "로컬 임베딩 모델을 사용할 수 없습니다. 다운로드 후 다시 시도하세요.".into()
            }
            Self::Internal(_) => "알 수 없는 오류가 발생했습니다.".into(),
        }
    }

    pub fn retry_after_seconds(&self) -> Option<u64> {
        match self {
            Self::ProviderRateLimit {
                retry_after_seconds,
            } => *retry_after_seconds,
            _ => None,
        }
    }
}

/// Wire shape every failing command returns.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: ErrorCode,
    pub message: String,
    pub retry_after_seconds: Option<u64>,
}

impl Serialize for AppErrorWire {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        CommandError {
            code: self.0.code(),
            message: self.0.redacted_message(),
            retry_after_seconds: self.0.retry_after_seconds(),
        }
        .serialize(s)
    }
}

/// Newtype so `AppError` can stay a plain error type while commands return a
/// serializable payload.
pub struct AppErrorWire(pub AppError);

impl<E: Into<AppError>> From<E> for AppErrorWire {
    fn from(e: E) -> Self {
        let err = e.into();
        // The Display impl of AppError is deliberately free of secrets.
        tracing::warn!(code = ?err.code(), "command failed: {err}");
        Self(err)
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
pub type CommandResult<T> = std::result::Result<T, AppErrorWire>;
