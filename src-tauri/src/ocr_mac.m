// macOS Vision framework OCR wrapper.
// Called from Rust via extern "C" FFI -- same pattern as recording/avwriter.m.

#import <Foundation/Foundation.h>
#import <Vision/Vision.h>
#import <CoreGraphics/CoreGraphics.h>

// Recognize text in an RGBA pixel buffer.
// Returns a malloc'd C string (caller must free with ocr_free_string).
// Returns NULL on failure.
char* ocr_recognize(const uint8_t* rgba, uint32_t width, uint32_t height) {
    if (!rgba || width == 0 || height == 0) return NULL;

    @autoreleasepool {
        // Create CGImage from RGBA pixel data
        size_t bytesPerRow = width * 4;
        size_t totalBytes = bytesPerRow * height;
        CGColorSpaceRef colorSpace = CGColorSpaceCreateDeviceRGB();
        CGContextRef context = CGBitmapContextCreate(
            (void*)rgba, width, height, 8, bytesPerRow, colorSpace,
            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big
        );
        CGColorSpaceRelease(colorSpace);
        if (!context) return NULL;

        CGImageRef cgImage = CGBitmapContextCreateImage(context);
        CGContextRelease(context);
        if (!cgImage) return NULL;

        // Create Vision request handler
        VNImageRequestHandler *handler = [[VNImageRequestHandler alloc]
            initWithCGImage:cgImage options:@{}];
        CGImageRelease(cgImage);

        // Create text recognition request
        VNRecognizeTextRequest *request = [[VNRecognizeTextRequest alloc] init];
        request.recognitionLevel = VNRequestTextRecognitionLevelAccurate;
        request.usesLanguageCorrection = YES;

        // Perform synchronously (we're called from a blocking Rust thread)
        NSError *error = nil;
        [handler performRequests:@[request] error:&error];
        if (error) {
            NSLog(@"OCR error: %@", error.localizedDescription);
            return NULL;
        }

        // Collect recognized text from results
        NSMutableString *text = [NSMutableString string];
        for (VNRecognizedTextObservation *observation in request.results) {
            NSArray<VNRecognizedText *> *candidates = [observation topCandidates:1];
            if (candidates.count > 0) {
                if (text.length > 0) [text appendString:@"\n"];
                [text appendString:candidates[0].string];
            }
        }

        if (text.length == 0) return NULL;

        // Return as malloc'd C string (Rust frees via ocr_free_string)
        const char *utf8 = [text UTF8String];
        size_t len = strlen(utf8);
        char *result = (char*)malloc(len + 1);
        if (result) memcpy(result, utf8, len + 1);
        return result;
    }
}

void ocr_free_string(char* s) {
    free(s);
}
