fn main() {
    // 只在 desktop feature 启用时才调用 tauri_build
    #[cfg(feature = "desktop")]
    tauri_build::build()
}
