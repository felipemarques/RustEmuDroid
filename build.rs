fn main() {
    println!("cargo:rerun-if-changed=assets/rustemudroid.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/rustemudroid.ico")
            .set("ProductName", "RustEmuDroid")
            .set("FileDescription", "RustEmuDroid")
            .compile()
            .expect("embed the RustEmuDroid icon");
    }
}
