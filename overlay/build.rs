fn main() {
    // On macOS, the `screencapturekit` crate links against
    // @rpath/libswift_Concurrency.dylib. We add the system Swift library path
    // so dyld can resolve it. On macOS 12+ this is in /usr/lib/swift (via the
    // dyld shared cache). The Xcode toolchain path is a fallback for dev setups.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
