fn main() {
    tauri_build::build();

    // Compile the Objective-C AVAssetWriter wrapper on macOS.
    #[cfg(target_os = "macos")]
    {
        cc::Build::new()
            .file("src/recording/avwriter.m")
            .flag("-fobjc-arc")              // ARC memory management
            .flag("-Wno-deprecated-declarations") // suppress deprecation warnings
            .flag("-Wno-nullability-completeness")
            .compile("avwriter");

        // Link required frameworks (explicit, don't rely on -fmodules)
        println!("cargo:rustc-link-lib=framework=AVFoundation");
        println!("cargo:rustc-link-lib=framework=CoreVideo");
        println!("cargo:rustc-link-lib=framework=CoreMedia");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=Foundation");

        // Compile OCR wrapper (Vision framework)
        cc::Build::new()
            .file("src/ocr_mac.m")
            .flag("-fobjc-arc")
            .compile("ocr_mac");

        println!("cargo:rustc-link-lib=framework=Vision");

        // Compile notification delegate (forces banners when app is frontmost)
        cc::Build::new()
            .file("src/notification_mac.m")
            .flag("-fobjc-arc")
            .compile("notification_mac");

        println!("cargo:rustc-link-lib=framework=UserNotifications");
    }
}
