//! Puts Lumen's icon into the Windows .exe, so File Explorer, the desktop and
//! the taskbar show it for the file itself (the window icon is set at runtime
//! in main.rs).

fn main() {
    println!("cargo:rerun-if-changed=assets/lumen.ico");
    // Only the MSVC toolchain (the Windows CI and release builds) has the
    // resource compiler; `cargo check --target x86_64-pc-windows-gnu` on Linux
    // has none and skips it.
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows && msvc {
        winresource::WindowsResource::new()
            .set_icon("assets/lumen.ico")
            .compile()
            .expect("the icon resource compiles");
    }
}
