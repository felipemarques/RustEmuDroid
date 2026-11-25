#[cfg(windows)]
mod viewer;

fn main() {
    #[cfg(windows)]
    let result = viewer::run();
    #[cfg(not(windows))]
    let result: Result<(), String> = Err("DWM native viewer requires Windows".into());

    if let Err(error) = result {
        eprintln!("dwm-native-viewer: {error}");
        std::process::exit(1);
    }
}
