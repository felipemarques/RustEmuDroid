use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=assets/rustemudroid.ico");
    #[cfg(windows)]
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/rustemudroid.ico")
            .set("ProductName", "RustEmuDroid")
            .set("FileDescription", "RustEmuDroid")
            .compile()
            .expect("embed the RustEmuDroid icon");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        build_userptr_guard();
    }
}

/// Linux: the preload library the client embeds and gives to the engine on
/// AMD graphics (`src/android/amd_linux.rs`). A Linux Rust toolchain already
/// needs `cc` to link, so no extra tool is required.
fn build_userptr_guard() {
    const SOURCE: &str = "native/linux/userptr_guard.c";
    println!("cargo:rerun-if-changed={SOURCE}");
    println!("cargo:rerun-if-env-changed=CC");
    let library = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"))
        .join("libuserptr_guard.so");
    // CC may carry arguments ("ccache gcc"), as it may for other build tools.
    let configured = env::var("CC").unwrap_or_default();
    let mut words = configured.split_whitespace();
    let compiler = words.next().unwrap_or("cc");
    let mut command = Command::new(compiler);
    command
        .args(words)
        .args(["-O2", "-fPIC", "-shared", "-Wall", "-Wextra", "-o"])
        .arg(&library)
        .arg(SOURCE)
        .args(["-ldl", "-lpthread"]);
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => panic!(
            "could not compile {SOURCE} with `{compiler}` ({status}). The Linux build embeds this \
             library: see the compiler output above, or set CC to a working C compiler."
        ),
        Err(error) => panic!(
            "could not run the C compiler `{compiler}` for {SOURCE}: {error}. The Linux build embeds \
             this library: install a C compiler (for example gcc) or set CC."
        ),
    }
}
