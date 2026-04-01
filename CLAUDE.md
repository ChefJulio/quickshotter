# QuickShotter - Claude Code Instructions

## Project Overview
Tauri 2 desktop app (Rust + TypeScript) for screen capture on Windows and macOS. Lives in the system tray.

## Architecture
- `src-tauri/` — Rust backend (Tauri app, capture logic, hotkeys, overlay, recording)
- `src/` — TypeScript frontend (settings UI, overlay canvas, annotation editor)
- `overlay/` — Separate Rust binary for GPU-accelerated overlay (Windows only; macOS uses webview overlay)
- Overlay daemon is Windows-only. macOS uses a Tauri webview window (`overlay.html`) instead.

## Release / Push / Build Checklist

**CRITICAL: Follow this EVERY time you push changes that trigger a release build.**

1. **Bump the version** in ALL THREE files (they must match):
   - `src-tauri/tauri.conf.json` → `"version": "X.Y.Z"`
   - `src-tauri/Cargo.toml` → `version = "X.Y.Z"`
   - `package.json` → `"version": "X.Y.Z"`
2. **Run `cargo check`** to update `Cargo.lock` with the new version
3. **Commit** the version bump (can be part of the feature commit or separate)
4. **Tag** with the new version: `git tag vX.Y.Z`
5. **Push** commit and tag: `git push origin main && git push origin vX.Y.Z`

**NEVER retag an existing version.** The auto-updater uses version tags to detect updates. Retagging breaks the update mechanism for users who already have that version. Always increment the version instead.

**Version bumping guide:**
- Patch (1.1.0 → 1.1.1): bug fixes, polish, small changes
- Minor (1.1.0 → 1.2.0): new features, significant UX changes
- Major (1.0.0 → 2.0.0): breaking changes

## macOS-Specific Notes
- Screen recording permission is handled via onboarding welcome window on first launch
- `CGRequestScreenCaptureAccess()` is only called when user clicks "Grant Access" (never on startup)
- The overlay uses a webview (Tauri window with `overlay.html`), NOT the native daemon
- `xcap` returns logical points on macOS — no scale factor conversion needed for Tauri window positioning
- Permission check (`CGPreflightScreenCaptureAccess`) is unreliable for unsigned dev builds

## Build Commands
- Dev: `cargo tauri dev` (also need `cd overlay && cargo build` for the overlay daemon on first run)
- Production: `cargo tauri build`
- Overlay only: `cd overlay && cargo build`
- CI builds are triggered by pushing a `v*` tag

## Code Signing (macOS)
- Signed with Developer ID Application certificate (Hunter Graves, Team ID: 3G9C6KNHVR)
- Notarized via Apple. Secrets stored in GitHub Actions.
- Certificate, Apple ID, app-specific password, and team ID are in repo secrets.
