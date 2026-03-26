use image::RgbaImage;
use std::io::Write;
use crate::error::AppError;

const CATBOX_URL: &str = "https://catbox.moe/user/api.php";

/// Upload an RgbaImage to catbox.moe and return the file URL.
/// Shells out to curl, which handles multipart encoding reliably.
pub async fn upload(img: &RgbaImage) -> Result<String, AppError> {
  // Encode image to PNG in a temp file
  let temp_dir = std::env::temp_dir();
  let temp_path = temp_dir.join("quickshotter_upload.png");

  let mut file = std::fs::File::create(&temp_path)
    .map_err(|e| AppError::Upload(format!("Failed to create temp file: {e}")))?;

  let mut buf = std::io::Cursor::new(Vec::new());
  img.write_to(&mut buf, image::ImageFormat::Png)
    .map_err(|e| AppError::Upload(format!("Failed to encode PNG: {e}")))?;
  let png_bytes = buf.into_inner();

  if png_bytes.is_empty() {
    return Err(AppError::Upload("PNG encoding produced empty output".to_string()));
  }

  file.write_all(&png_bytes)
    .map_err(|e| AppError::Upload(format!("Failed to write temp file: {e}")))?;
  drop(file);

  // Shell out to curl for the multipart upload
  let temp_str = temp_path.to_string_lossy().to_string();
  let output = tauri::async_runtime::spawn_blocking(move || {
    std::process::Command::new("curl")
      .args([
        "-s",
        "-F", "reqtype=fileupload",
        "-F", &format!("fileToUpload=@{temp_str}"),
        CATBOX_URL,
      ])
      .output()
  })
  .await
  .map_err(|e| AppError::Upload(format!("Upload task failed: {e}")))?
  .map_err(|e| AppError::Upload(format!("Failed to run curl: {e}")))?;

  // Clean up temp file
  let _ = std::fs::remove_file(&temp_path);

  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    return Err(AppError::Upload(format!("curl failed: {stderr}")));
  }

  let url = String::from_utf8_lossy(&output.stdout).trim().to_string();

  if url.starts_with("https://") {
    Ok(url)
  } else {
    Err(AppError::Upload(format!("Unexpected catbox response: {url}")))
  }
}
