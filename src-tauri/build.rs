fn main() {
    // Only the desktop app is a Tauri app; a headless `ledger-server` build has nothing to generate.
    #[cfg(feature = "desktop")]
    tauri_build::build()
}
