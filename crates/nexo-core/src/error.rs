use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Config(String),
    #[error("nie zalogowano (profil `{0}`); uruchom `nexo login`")]
    NotSignedIn(String),
    #[error("sesja się skończyła (wylogowana w panelu albo wygasła); uruchom `nexo login`")]
    SessionEnded,
    #[error("{message} ({status} {code})")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("sieć: {0}")]
    Http(#[from] reqwest::Error),
    #[error("przechowywanie tokenów: {0}")]
    Store(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// The API's error code, when the API answered with one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Api { code, .. } => Some(code),
            _ => None,
        }
    }
}
