// sccapture.m -- ScreenCaptureKit wrapper for QuickShotter.
// Compiled by cc::Build in build.rs (macOS only) with -fobjc-arc.
// Exposes a plain C API so Rust can call it via extern "C" FFI.
//
// Replaces CGWindowListCreateImage with SCScreenshotManager for
// single-frame captures and SCShareableContent for display enumeration.

#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <CoreGraphics/CoreGraphics.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

// ---------------------------------------------------------------------------
// Display info struct passed to Rust
// ---------------------------------------------------------------------------
typedef struct {
    uint32_t display_id;
    int32_t x;       // logical origin x
    int32_t y;       // logical origin y
    uint32_t width;  // logical width
    uint32_t height; // logical height
} SCDisplayInfo;

// ---------------------------------------------------------------------------
// Get available displays via SCShareableContent.
// Also serves as the permission check — triggers the system permission
// dialog on first call if not yet granted.
// Returns 0 on success, -1 on failure (permission denied or other error).
// Caller must free *out_displays with free().
// ---------------------------------------------------------------------------
int sccapture_get_displays(SCDisplayInfo **out_displays, int *out_count, char *error_buf, int error_buf_len) {
    if (!out_displays || !out_count) return -1;
    *out_displays = NULL;
    *out_count = 0;

    __block NSArray<SCDisplay *> *displays = nil;
    __block NSError *scError = nil;
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);

    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *content, NSError *error) {
        if (error) {
            scError = error;
        } else {
            displays = content.displays;
        }
        dispatch_semaphore_signal(sem);
    }];

    dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC));

    if (scError || !displays) {
        if (error_buf && error_buf_len > 0) {
            snprintf(error_buf, error_buf_len, "%s",
                scError ? scError.localizedDescription.UTF8String : "Timeout getting displays");
        }
        return -1;
    }

    int count = (int)displays.count;
    SCDisplayInfo *infos = (SCDisplayInfo *)calloc(count, sizeof(SCDisplayInfo));
    if (!infos) return -1;

    for (int i = 0; i < count; i++) {
        SCDisplay *d = displays[i];
        infos[i].display_id = d.displayID;
        infos[i].x = (int32_t)d.frame.origin.x;
        infos[i].y = (int32_t)d.frame.origin.y;
        infos[i].width = (uint32_t)d.frame.size.width;
        infos[i].height = (uint32_t)d.frame.size.height;
    }

    *out_displays = infos;
    *out_count = count;
    return 0;
}

// ---------------------------------------------------------------------------
// Capture a region of the screen as RGBA pixels.
// Coordinates are in logical points (same as CoreGraphics).
// The region is captured from whichever display(s) it overlaps.
// Returns 0 on success, -1 on failure.
// Caller must free *out_rgba with sccapture_free_pixels().
// ---------------------------------------------------------------------------
int sccapture_capture_region(
    double x, double y, double w, double h,
    uint8_t **out_rgba,
    uint32_t *out_width,
    uint32_t *out_height,
    char *error_buf, int error_buf_len
) {
    if (!out_rgba || !out_width || !out_height) return -1;
    *out_rgba = NULL;
    *out_width = 0;
    *out_height = 0;

    __block NSArray<SCDisplay *> *displays = nil;
    __block NSError *scError = nil;
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);

    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *content, NSError *error) {
        if (error) {
            scError = error;
        } else {
            displays = content.displays;
        }
        dispatch_semaphore_signal(sem);
    }];

    dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC));

    if (scError || !displays || displays.count == 0) {
        if (error_buf && error_buf_len > 0) {
            snprintf(error_buf, error_buf_len, "%s",
                scError ? scError.localizedDescription.UTF8String : "No displays found");
        }
        return -1;
    }

    // Find the display that contains the region center
    CGPoint center = CGPointMake(x + w / 2.0, y + h / 2.0);
    SCDisplay *targetDisplay = nil;
    for (SCDisplay *d in displays) {
        if (CGRectContainsPoint(d.frame, center)) {
            targetDisplay = d;
            break;
        }
    }
    if (!targetDisplay) {
        targetDisplay = displays[0]; // fallback to primary
    }

    // Create content filter for the target display (capture everything on it)
    SCContentFilter *filter = [[SCContentFilter alloc]
        initWithDisplay:targetDisplay excludingWindows:@[]];

    // Configure: capture the specific source rect, output at Retina resolution
    SCStreamConfiguration *config = [[SCStreamConfiguration alloc] init];

    // sourceRect is in display-local logical coordinates
    CGRect displayFrame = targetDisplay.frame;
    CGRect sourceRect = CGRectMake(
        x - displayFrame.origin.x,
        y - displayFrame.origin.y,
        w, h
    );
    config.sourceRect = sourceRect;

    // Get the display's pixel scale (Retina factor)
    CGFloat scale = (CGFloat)CGDisplayPixelsWide(targetDisplay.displayID) / displayFrame.size.width;
    uint32_t pixelW = (uint32_t)(w * scale);
    uint32_t pixelH = (uint32_t)(h * scale);
    config.width = pixelW;
    config.height = pixelH;

    // Capture pixel format: BGRA (standard for macOS, we convert to RGBA below)
    config.pixelFormat = kCVPixelFormatType_32BGRA;
    config.showsCursor = NO;

    // Perform the screenshot
    __block CGImageRef capturedImage = NULL;
    __block NSError *captureError = nil;
    dispatch_semaphore_t capSem = dispatch_semaphore_create(0);

    if (@available(macOS 14.0, *)) {
        [SCScreenshotManager captureImageWithFilter:filter
                                      configuration:config
                                  completionHandler:^(CGImageRef image, NSError *error) {
            if (image) {
                capturedImage = CGImageRetain(image);
            }
            captureError = error;
            dispatch_semaphore_signal(capSem);
        }];
    } else {
        if (error_buf && error_buf_len > 0) {
            snprintf(error_buf, error_buf_len, "SCScreenshotManager requires macOS 14.0+");
        }
        return -1;
    }

    dispatch_semaphore_wait(capSem, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC));

    if (captureError || !capturedImage) {
        if (error_buf && error_buf_len > 0) {
            snprintf(error_buf, error_buf_len, "%s",
                captureError ? captureError.localizedDescription.UTF8String : "Screenshot capture timeout");
        }
        if (capturedImage) CGImageRelease(capturedImage);
        return -1;
    }

    // Convert CGImage to RGBA pixel buffer
    uint32_t imgW = (uint32_t)CGImageGetWidth(capturedImage);
    uint32_t imgH = (uint32_t)CGImageGetHeight(capturedImage);
    size_t bytesPerRow = imgW * 4;
    size_t totalBytes = bytesPerRow * imgH;

    uint8_t *rgba = (uint8_t *)malloc(totalBytes);
    if (!rgba) {
        CGImageRelease(capturedImage);
        return -1;
    }

    // Draw into an RGBA bitmap context
    CGColorSpaceRef colorSpace = CGColorSpaceCreateDeviceRGB();
    CGContextRef ctx = CGBitmapContextCreate(
        rgba, imgW, imgH, 8, bytesPerRow,
        colorSpace,
        kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big // RGBA
    );
    CGColorSpaceRelease(colorSpace);

    if (!ctx) {
        free(rgba);
        CGImageRelease(capturedImage);
        if (error_buf && error_buf_len > 0) {
            snprintf(error_buf, error_buf_len, "Failed to create bitmap context");
        }
        return -1;
    }

    CGContextDrawImage(ctx, CGRectMake(0, 0, imgW, imgH), capturedImage);
    CGContextRelease(ctx);
    CGImageRelease(capturedImage);

    *out_rgba = rgba;
    *out_width = imgW;
    *out_height = imgH;
    return 0;
}

// ---------------------------------------------------------------------------
// Capture a full display by display ID.
// Returns 0 on success, -1 on failure.
// ---------------------------------------------------------------------------
int sccapture_capture_display(
    uint32_t display_id,
    uint8_t **out_rgba,
    uint32_t *out_width,
    uint32_t *out_height,
    char *error_buf, int error_buf_len
) {
    if (!out_rgba || !out_width || !out_height) return -1;
    *out_rgba = NULL;
    *out_width = 0;
    *out_height = 0;

    __block SCDisplay *targetDisplay = nil;
    __block NSError *scError = nil;
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);

    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *content, NSError *error) {
        if (!error) {
            for (SCDisplay *d in content.displays) {
                if (d.displayID == display_id) {
                    targetDisplay = d;
                    break;
                }
            }
        }
        scError = error;
        dispatch_semaphore_signal(sem);
    }];

    dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_SEC));

    if (scError || !targetDisplay) {
        if (error_buf && error_buf_len > 0) {
            snprintf(error_buf, error_buf_len, "%s",
                scError ? scError.localizedDescription.UTF8String : "Display not found");
        }
        return -1;
    }

    // Capture full display
    CGRect frame = targetDisplay.frame;
    return sccapture_capture_region(
        frame.origin.x, frame.origin.y,
        frame.size.width, frame.size.height,
        out_rgba, out_width, out_height,
        error_buf, error_buf_len
    );
}

// ---------------------------------------------------------------------------
// Check permission by attempting to enumerate displays.
// Returns 0 if granted, -1 if denied or error.
// ---------------------------------------------------------------------------
int sccapture_check_permission(char *error_buf, int error_buf_len) {
    SCDisplayInfo *displays = NULL;
    int count = 0;
    int result = sccapture_get_displays(&displays, &count, error_buf, error_buf_len);
    if (displays) free(displays);
    return result;
}

// ---------------------------------------------------------------------------
// Free pixel buffer allocated by capture functions.
// ---------------------------------------------------------------------------
void sccapture_free_pixels(uint8_t *pixels) {
    if (pixels) free(pixels);
}
