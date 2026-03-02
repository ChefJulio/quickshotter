// avwriter.m -- Thin Objective-C wrapper around AVAssetWriter + VideoToolbox.
// Compiled by cc::Build in build.rs (macOS only) with -fobjc-arc.
// Exposes a plain C API so Rust can call it via extern "C" FFI.

#import <AVFoundation/AVFoundation.h>
#import <CoreVideo/CoreVideo.h>
#import <CoreMedia/CoreMedia.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

// ---------------------------------------------------------------------------
// Opaque handle passed to/from Rust.
// Fields holding ObjC objects use __strong so ARC manages retain/release
// when we assign to them (even in a calloc'd struct, modern Clang handles this).
// IMPORTANT: All ObjC fields MUST be set to nil before free().
// ---------------------------------------------------------------------------
typedef struct {
    __strong AVAssetWriter *writer;
    __strong AVAssetWriterInput *input;
    __strong AVAssetWriterInputPixelBufferAdaptor *adaptor;
    uint32_t width;
    uint32_t height;
    uint32_t fps;
    int32_t timescale;
    int pool_available;  // Whether pixelBufferPool was usable
    char error_buf[512];
} AVWriterHandle;

static void set_error(AVWriterHandle *h, const char *fmt, ...) {
    va_list args;
    va_start(args, fmt);
    vsnprintf(h->error_buf, sizeof(h->error_buf), fmt, args);
    va_end(args);
}

// ---------------------------------------------------------------------------
// Create + configure the writer.
// Returns handle on success (error_buf empty) or partial handle on failure
// (error_buf set, caller must still call avwriter_destroy).
// Returns NULL only on malloc failure.
// ---------------------------------------------------------------------------
AVWriterHandle* avwriter_create(
    const char *path,
    uint32_t width,
    uint32_t height,
    uint32_t fps,
    uint32_t bitrate
) {
    AVWriterHandle *h = (AVWriterHandle *)calloc(1, sizeof(AVWriterHandle));
    if (!h) return NULL;

    // H.264 requires even dimensions
    width  = width  & ~1u;
    height = height & ~1u;
    if (width == 0 || height == 0) {
        set_error(h, "Dimensions too small after even-alignment: %ux%u", width, height);
        return h;
    }

    h->width = width;
    h->height = height;
    h->fps = fps;
    h->timescale = 600; // Divisible by 24, 25, 30, 60

    @autoreleasepool {
        NSString *nsPath = [NSString stringWithUTF8String:path];
        if (!nsPath) {
            set_error(h, "Invalid UTF-8 path");
            return h;
        }
        NSURL *url = [NSURL fileURLWithPath:nsPath];

        // AVAssetWriter refuses to overwrite; delete first
        [[NSFileManager defaultManager] removeItemAtURL:url error:nil];

        NSError *error = nil;
        h->writer = [[AVAssetWriter alloc] initWithURL:url
                                              fileType:AVFileTypeMPEG4
                                                 error:&error];
        if (!h->writer) {
            set_error(h, "AVAssetWriter init failed: %s",
                      error ? error.localizedDescription.UTF8String : "unknown");
            return h;
        }

        // H.264 output settings with VideoToolbox hardware acceleration
        NSDictionary *compressionProps = @{
            AVVideoAverageBitRateKey: @(bitrate),
            AVVideoMaxKeyFrameIntervalKey: @(fps * 2),
            AVVideoExpectedSourceFrameRateKey: @(fps),
            AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
        };

        NSDictionary *outputSettings = @{
            AVVideoCodecKey: AVVideoCodecTypeH264,
            AVVideoWidthKey: @(width),
            AVVideoHeightKey: @(height),
            AVVideoCompressionPropertiesKey: compressionProps,
        };

        // Verify settings are compatible before creating the input
        if (![h->writer canApplyOutputSettings:outputSettings forMediaType:AVMediaTypeVideo]) {
            // Fall back to simpler settings without compression properties
            outputSettings = @{
                AVVideoCodecKey: AVVideoCodecTypeH264,
                AVVideoWidthKey: @(width),
                AVVideoHeightKey: @(height),
            };
        }

        h->input = [[AVAssetWriterInput alloc]
            initWithMediaType:AVMediaTypeVideo
               outputSettings:outputSettings];
        if (!h->input) {
            set_error(h, "AVAssetWriterInput creation failed");
            return h;
        }
        h->input.expectsMediaDataInRealTime = YES;

        // Pixel buffer attributes: BGRA 32-bit (we convert RGBA -> BGRA in append)
        NSDictionary *pbAttrs = @{
            (NSString *)kCVPixelBufferPixelFormatTypeKey: @(kCVPixelFormatType_32BGRA),
            (NSString *)kCVPixelBufferWidthKey: @(width),
            (NSString *)kCVPixelBufferHeightKey: @(height),
            // IOSurface-backed for GPU encoding path
            (NSString *)kCVPixelBufferIOSurfacePropertiesKey: @{},
        };

        h->adaptor = [[AVAssetWriterInputPixelBufferAdaptor alloc]
            initWithAssetWriterInput:h->input
         sourcePixelBufferAttributes:pbAttrs];
        if (!h->adaptor) {
            set_error(h, "AVAssetWriterInputPixelBufferAdaptor creation failed");
            return h;
        }

        if (![h->writer canAddInput:h->input]) {
            set_error(h, "AVAssetWriter cannot add video input (incompatible settings)");
            return h;
        }
        [h->writer addInput:h->input];

        if (![h->writer startWriting]) {
            set_error(h, "startWriting failed: %s",
                      h->writer.error
                          ? h->writer.error.localizedDescription.UTF8String
                          : "unknown");
            return h;
        }
        [h->writer startSessionAtSourceTime:kCMTimeZero];

        // Check if pixel buffer pool is available now (it might take one frame)
        h->pool_available = (h->adaptor.pixelBufferPool != NULL) ? 1 : 0;
    }

    return h;
}

// ---------------------------------------------------------------------------
// Create a CVPixelBuffer, trying pool first then direct allocation.
// ---------------------------------------------------------------------------
static CVPixelBufferRef create_pixel_buffer(AVWriterHandle *h, uint32_t width, uint32_t height) {
    CVPixelBufferRef pb = NULL;
    CVReturn status;

    // Try pool first (reuses buffers, much faster)
    CVPixelBufferPoolRef pool = h->adaptor.pixelBufferPool;
    if (pool) {
        status = CVPixelBufferPoolCreatePixelBuffer(NULL, pool, &pb);
        if (status == kCVReturnSuccess && pb) {
            if (!h->pool_available) {
                h->pool_available = 1;
            }
            return pb;
        }
        // Pool failed, fall through to direct allocation
    }

    // Direct allocation with IOSurface backing for GPU compatibility
    NSDictionary *attrs = @{
        (NSString *)kCVPixelBufferIOSurfacePropertiesKey: @{},
    };
    status = CVPixelBufferCreate(
        NULL, width, height, kCVPixelFormatType_32BGRA,
        (__bridge CFDictionaryRef)attrs, &pb);

    if (status != kCVReturnSuccess || !pb) {
        set_error(h, "CVPixelBuffer creation failed (pool=%s, status=%d)",
                  pool ? "tried" : "unavailable", (int)status);
        return NULL;
    }

    return pb;
}

// ---------------------------------------------------------------------------
// Append one RGBA frame. Converts RGBA -> BGRA during copy.
// Returns 0 on success, -1 on failure.
// ---------------------------------------------------------------------------
int avwriter_append_frame(
    AVWriterHandle *h,
    const uint8_t *rgba,
    uint32_t width,
    uint32_t height,
    double pts_ms
) {
    if (!h || !h->writer || !h->adaptor || !h->input) return -1;

    // Check writer hasn't failed asynchronously
    if (h->writer.status == AVAssetWriterStatusFailed) {
        set_error(h, "Writer in failed state: %s",
                  h->writer.error
                      ? h->writer.error.localizedDescription.UTF8String
                      : "unknown");
        return -1;
    }

    @autoreleasepool {
        // Wait for the input to be ready
        int wait_count = 0;
        while (!h->input.isReadyForMoreMediaData && wait_count < 200) {
            usleep(1000); // 1ms
            wait_count++;
        }
        if (!h->input.isReadyForMoreMediaData) {
            set_error(h, "AVAssetWriterInput not ready after 200ms");
            return -1;
        }

        CVPixelBufferRef pixelBuffer = create_pixel_buffer(h, h->width, h->height);
        if (!pixelBuffer) return -1;

        // Lock, copy RGBA -> BGRA, unlock
        CVPixelBufferLockBaseAddress(pixelBuffer, 0);
        uint8_t *dst = (uint8_t *)CVPixelBufferGetBaseAddress(pixelBuffer);
        size_t bytesPerRow = CVPixelBufferGetBytesPerRow(pixelBuffer);

        // Use the handle's stored dimensions (even-aligned) not the input dimensions
        uint32_t w = h->width;
        uint32_t h_dim = h->height;
        size_t srcStride = width * 4;

        for (uint32_t y = 0; y < h_dim && y < height; y++) {
            const uint8_t *srcRow = rgba + y * srcStride;
            uint8_t *dstRow = dst + y * bytesPerRow;
            for (uint32_t x = 0; x < w && x < width; x++) {
                uint32_t si = x * 4;
                dstRow[si + 0] = srcRow[si + 2]; // B
                dstRow[si + 1] = srcRow[si + 1]; // G
                dstRow[si + 2] = srcRow[si + 0]; // R
                dstRow[si + 3] = srcRow[si + 3]; // A
            }
        }
        CVPixelBufferUnlockBaseAddress(pixelBuffer, 0);

        CMTime pts = CMTimeMakeWithSeconds(pts_ms / 1000.0, h->timescale);

        BOOL ok = [h->adaptor appendPixelBuffer:pixelBuffer
                           withPresentationTime:pts];
        CVPixelBufferRelease(pixelBuffer);

        if (!ok) {
            set_error(h, "appendPixelBuffer failed (status=%d): %s",
                      (int)h->writer.status,
                      h->writer.error
                          ? h->writer.error.localizedDescription.UTF8String
                          : "unknown");
            return -1;
        }
    }

    return 0;
}

// ---------------------------------------------------------------------------
// Finalize: mark input finished, wait for writer to complete.
// Returns 0 on success, -1 on failure.
// ---------------------------------------------------------------------------
int avwriter_finish(AVWriterHandle *h) {
    if (!h || !h->writer) return -1;

    // If writer already failed, nothing to finalize
    if (h->writer.status == AVAssetWriterStatusFailed) {
        set_error(h, "Writer already in failed state: %s",
                  h->writer.error
                      ? h->writer.error.localizedDescription.UTF8String
                      : "unknown");
        return -1;
    }

    @autoreleasepool {
        [h->input markAsFinished];

        // finishWritingWithCompletionHandler is async; use a semaphore to wait.
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);

        [h->writer finishWritingWithCompletionHandler:^{
            dispatch_semaphore_signal(sem);
        }];

        // Wait up to 30 seconds for encoding to flush
        long result = dispatch_semaphore_wait(
            sem,
            dispatch_time(DISPATCH_TIME_NOW, 30LL * NSEC_PER_SEC)
        );

        if (result != 0) {
            set_error(h, "finishWriting timed out after 30s");
            return -1;
        }

        if (h->writer.status == AVAssetWriterStatusFailed) {
            set_error(h, "finishWriting failed: %s",
                      h->writer.error
                          ? h->writer.error.localizedDescription.UTF8String
                          : "unknown");
            return -1;
        }

        if (h->writer.status != AVAssetWriterStatusCompleted) {
            set_error(h, "finishWriting ended with unexpected status: %d",
                      (int)h->writer.status);
            return -1;
        }
    }

    return 0;
}

// ---------------------------------------------------------------------------
// Free the handle. MUST nil out ObjC fields before free() so ARC releases them.
// ---------------------------------------------------------------------------
void avwriter_destroy(AVWriterHandle *h) {
    if (!h) return;
    @autoreleasepool {
        // Cancel writing if still in progress (prevents crash on release)
        if (h->writer && h->writer.status == AVAssetWriterStatusWriting) {
            [h->writer cancelWriting];
        }
        // Nil out ObjC pointers so ARC releases them before we free the raw memory
        h->adaptor = nil;
        h->input = nil;
        h->writer = nil;
    }
    free(h);
}

// ---------------------------------------------------------------------------
// Get the last error message (empty string if no error).
// ---------------------------------------------------------------------------
const char* avwriter_error(AVWriterHandle *h) {
    if (!h) return "null handle";
    return h->error_buf;
}
