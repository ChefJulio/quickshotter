use image::RgbaImage;
use crate::error::AppError;

const CATBOX_URL: &str = "https://catbox.moe/user/api.php";

/// Upload an RgbaImage to catbox.moe and return the file URL.
/// No API key or authentication required.
pub async fn upload(img: &RgbaImage) -> Result<String, AppError> {
  // Encode image to PNG in memory
  let mut buf = std::io::Cursor::new(Vec::new());
  img.write_to(&mut buf, image::ImageFormat::Png)
    .map_err(|e| AppError::Upload(format!("Failed to encode image: {e}")))?;
  let png_bytes = buf.into_inner();

  let part = reqwest::multipart::Part::bytes(png_bytes)
    .file_name("screenshot.png")
    .mime_str("image/png")
    .map_err(|e| AppError::Upload(format!("Failed to create multipart: {e}")))?;

  let form = reqwest::multipart::Form::new()
    .text("reqtype", "fileupload")
    .part("fileToUpload", part);

  let client = reqwest::Client::builder()
    .user_agent("QuickShotter/1.0")
    .build()
    .map_err(|e| AppError::Upload(format!("Failed to create HTTP client: {e}")))?;
  let resp = client
    .post(CATBOX_URL)
    .multipart(form)
    .send()
    .await
    .map_err(|e| AppError::Upload(format!("Upload request failed: {e:?}")))?;

  if !resp.status().is_success() {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    return Err(AppError::Upload(format!("Catbox returned {status}: {body}")));
  }

  let url = resp.text().await
    .map_err(|e| AppError::Upload(format!("Failed to read response: {e}")))?;

  let url = url.trim().to_string();
  if url.starts_with("https://") {
    Ok(url)
  } else {
    Err(AppError::Upload(format!("Unexpected response: {url}")))
  }
}
