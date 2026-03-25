use serde::Serialize;

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum AppError {
  #[error("Capture failed: {0}")]
  Capture(String),
  #[error("Clipboard error: {0}")]
  Clipboard(String),
  #[error("IO error: {0}")]
  Io(#[from] std::io::Error),
  #[error("Config error: {0}")]
  Config(String),
  #[error("Window detection error: {0}")]
  Window(String),
  #[error("Annotation error: {0}")]
  Annotation(String),
  #[error("Recording error: {0}")]
  Recording(String),
  #[error("OCR error: {0}")]
  Ocr(String),
  #[error("Upload error: {0}")]
  Upload(String),
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
