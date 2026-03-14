// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(feature = "desktop")]
    kamism_lib::run();

    #[cfg(not(feature = "desktop"))]
    panic!("请使用 desktop feature 编译桌面客户端: cargo build --features desktop");
}
