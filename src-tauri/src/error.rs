use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
  #[error("Capture failed: {0}")]
  Capture(String),
  #[error("Clipboard error: {0}")]
  Clipboard(String),
  #[error("IO error: {0}")]
  Io(#[from] std::io::Error),
  #[error("Config error: {0}")]
  Config(String),
  #[error("Tauri error: {0}")]
  Tauri(#[from] tauri::Error),
}

impl Serialize for AppError {
  fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
  where
    S: serde::Serializer,
  {
    serializer.serialize_str(&self.to_string())
  }
}
