use image::RgbaImage;

use crate::error::AppError;

/// Recognize text in an RGBA image using the platform's native OCR engine.
pub fn recognize_text(img: &RgbaImage) -> Result<String, AppError> {
  // Upscale small images for better recognition accuracy.
  // Windows.Media.Ocr and Apple Vision both perform better on larger text.
  let img = upscale_if_needed(img);
  platform::recognize(&img)
}

/// Upscale image 2x if either dimension is under 256px.
/// Uses nearest-neighbor to keep text sharp.
fn upscale_if_needed(img: &RgbaImage) -> RgbaImage {
  if img.width() < 256 || img.height() < 256 {
    image::imageops::resize(
      img,
      img.width() * 2,
      img.height() * 2,
      image::imageops::FilterType::Nearest,
    )
  } else {
    img.clone()
  }
}

// -- Windows: Windows.Media.Ocr (WinRT) --

#[cfg(target_os = "windows")]
mod platform {
  use image::RgbaImage;
  use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
  use windows::Media::Ocr::OcrEngine;
  use windows::Storage::Streams::{IBuffer, DataWriter};

  use crate::error::AppError;

  pub fn recognize(img: &RgbaImage) -> Result<String, AppError> {
    // Convert RGBA to BGRA (Windows expects BGRA8)
    let (w, h) = (img.width(), img.height());
    let mut bgra = img.as_raw().to_vec();
    for pixel in bgra.chunks_exact_mut(4) {
      pixel.swap(0, 2); // R <-> B
    }

    // Create an IBuffer from the BGRA pixel data
    let buffer = create_buffer_from_bytes(&bgra)?;

    // Create SoftwareBitmap from buffer
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
      &buffer,
      BitmapPixelFormat::Bgra8,
      w as i32,
      h as i32,
    ).map_err(|e| AppError::Ocr(format!("SoftwareBitmap::CreateCopyFromBuffer failed: {e}")))?;

    // Create OCR engine (auto-detects language from user profile)
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
      .map_err(|e| AppError::Ocr(format!("OcrEngine creation failed: {e}")))?;

    // Run recognition -- block on the WinRT async operation.
    // IAsyncOperation implements IntoFuture, so we convert and poll manually.
    let async_op = engine.RecognizeAsync(&bitmap)
      .map_err(|e| AppError::Ocr(format!("RecognizeAsync failed: {e}")))?;

    let result: windows::Media::Ocr::OcrResult = block_on_winrt(async_op)
      .map_err(|e| AppError::Ocr(format!("OCR recognition failed: {e}")))?;

    let text = result.Text()
      .map_err(|e| AppError::Ocr(format!("OcrResult.Text() failed: {e}")))?
      .to_string();

    Ok(text.trim().to_string())
  }

  /// Block the current thread on a WinRT async operation.
  /// Uses IntoFuture + a minimal poll loop with a thread-parking waker.
  fn block_on_winrt<F>(op: F) -> <F::IntoFuture as std::future::Future>::Output
  where
    F: std::future::IntoFuture,
    F::IntoFuture: Unpin,
  {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    // Create a waker that unparks the current thread
    fn make_waker() -> Waker {
      let thread = std::thread::current();
      let data = Box::into_raw(Box::new(thread)) as *const ();

      unsafe fn clone(ptr: *const ()) -> RawWaker {
        let thread = &*(ptr as *const std::thread::Thread);
        let cloned = Box::into_raw(Box::new(thread.clone())) as *const ();
        RawWaker::new(cloned, &VTABLE)
      }
      unsafe fn wake(ptr: *const ()) {
        let thread = Box::from_raw(ptr as *mut std::thread::Thread);
        thread.unpark();
      }
      unsafe fn wake_by_ref(ptr: *const ()) {
        let thread = &*(ptr as *const std::thread::Thread);
        thread.unpark();
      }
      unsafe fn drop_waker(ptr: *const ()) {
        drop(Box::from_raw(ptr as *mut std::thread::Thread));
      }

      static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_waker);
      unsafe { Waker::from_raw(RawWaker::new(data, &VTABLE)) }
    }

    let waker = make_waker();
    let mut cx = Context::from_waker(&waker);
    let mut future = std::future::IntoFuture::into_future(op);
    let mut pinned = Pin::new(&mut future); // AsyncFuture is Unpin

    loop {
      match pinned.as_mut().poll(&mut cx) {
        Poll::Ready(result) => return result,
        Poll::Pending => std::thread::park_timeout(std::time::Duration::from_millis(50)),
      }
    }
  }

  fn create_buffer_from_bytes(data: &[u8]) -> Result<IBuffer, AppError> {
    let writer = DataWriter::new()
      .map_err(|e| AppError::Ocr(format!("DataWriter::new failed: {e}")))?;
    writer.WriteBytes(data)
      .map_err(|e| AppError::Ocr(format!("WriteBytes failed: {e}")))?;
    let buffer = writer.DetachBuffer()
      .map_err(|e| AppError::Ocr(format!("DetachBuffer failed: {e}")))?;
    Ok(buffer)
  }
}

// -- macOS: Apple Vision framework via ObjC FFI --

#[cfg(target_os = "macos")]
mod platform {
  use image::RgbaImage;
  use crate::error::AppError;

  extern "C" {
    fn ocr_recognize(rgba: *const u8, width: u32, height: u32) -> *mut std::os::raw::c_char;
    fn ocr_free_string(s: *mut std::os::raw::c_char);
  }

  pub fn recognize(img: &RgbaImage) -> Result<String, AppError> {
    let (w, h) = (img.width(), img.height());
    let ptr = unsafe { ocr_recognize(img.as_raw().as_ptr(), w, h) };
    if ptr.is_null() {
      return Err(AppError::Ocr("Vision framework returned null".to_string()));
    }
    let c_str = unsafe { std::ffi::CStr::from_ptr(ptr) };
    let text = c_str.to_string_lossy().to_string();
    unsafe { ocr_free_string(ptr) };
    Ok(text.trim().to_string())
  }
}
