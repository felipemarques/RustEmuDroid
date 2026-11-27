use std::{env, fs, path::PathBuf, process::Command};

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
/// AMD graphics (`src/android/amd_linux.rs`).
///
/// A native build uses `CC` or `cc`; a Linux Rust toolchain already needs `cc`
/// to link, so no extra tool is required. A build for another machine only
/// uses a compiler named for the target (`CC_<target>` or `TARGET_CC`): the
/// host's `cc` would produce a library the target can not load. Without one,
/// an empty file is embedded and the client reports that it was built without
/// the library, so `cargo check --target` works from any host.
fn build_userptr_guard() {
    const SOURCE: &str = "native/linux/userptr_guard.c";
    let target = env::var("TARGET").unwrap_or_default();
    let host = env::var("HOST").unwrap_or_default();
    let target_variable = format!("CC_{}", target.replace(['-', '.'], "_"));
    println!("cargo:rerun-if-changed={SOURCE}");
    for variable in [target_variable.as_str(), "TARGET_CC", "CC"] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    let library = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"))
        .join("libuserptr_guard.so");
    let set = |variable: &str| {
        env::var(variable)
            .ok()
            .filter(|value| !value.trim().is_empty())
    };
    let configured = set(&target_variable)
        .or_else(|| set("TARGET_CC"))
        .or_else(|| (host == target).then(|| set("CC").unwrap_or_else(|| "cc".into())));
    let Some(configured) = configured else {
        fs::write(&library, b"").expect("write the empty protection library");
        println!(
            "cargo:warning=RustEmuDroid for {target} is being built on {host} without a C compiler \
             for the target: this Linux build is made WITHOUT the AMD graphics protection library \
             ({SOURCE}). To include it, set {target_variable} or TARGET_CC to a C compiler that \
             produces code for {target}."
        );
        return;
    };
    // The variable may carry arguments ("ccache gcc"), as it may for other build tools.
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
             library: see the compiler output above, or set CC to a working C compiler \
             ({target_variable} or TARGET_CC when building for another machine)."
        ),
        Err(error) => panic!(
            "could not run the C compiler `{compiler}` for {SOURCE}: {error}. The Linux build embeds \
             this library: install a C compiler (for example gcc) or set CC ({target_variable} or \
             TARGET_CC when building for another machine)."
        ),
    }
}
