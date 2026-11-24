//! Opt in with RUSTEMUDROID_TEST_SDK_SOURCE, RUSTEMUDROID_TEST_SDK_PACKAGE, and
//! RUSTEMUDROID_TEST_RUNTIME_DEST (a new directory below this checkout's .rustemudroid/qa).
//! RUSTEMUDROID_TEST_SDK_ARCHIVES optionally supplies prepacked copies named emulator.zip,
//! platform-tools.zip, and x86_64.zip. Otherwise the test packs the installed files itself.
use super::{Component, InstallPlan, install_from};
use crate::{
    Result,
    android::{Profile, Sdk, SystemImage},
    locale::Locale,
};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::{
    env, fs,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn hash_file<D: Digest>(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = D::new();
    let mut buffer = vec![0; 512 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn pack_directory(zip: &mut ZipWriter<File>, path: &Path, name: &str) -> Result<()> {
    let info = fs::symlink_metadata(path)?;
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    #[cfg(unix)]
    let options = {
        use std::os::unix::fs::PermissionsExt;
        options.unix_permissions(info.permissions().mode())
    };
    if info.is_dir() {
        zip.add_directory(format!("{name}/"), options)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let filename = entry.file_name();
            let filename = filename.to_str().ok_or("non-UTF-8 archive entry")?;
            pack_directory(zip, &entry.path(), &format!("{name}/{filename}"))?;
        }
    } else if info.is_file() {
        zip.start_file(name, options.large_file(info.len() >= u32::MAX.into()))?;
        io::copy(&mut File::open(path)?, zip)?;
    } else {
        return Err(format!("Unsupported installed file type: {}", path.display()).into());
    }
    Ok(())
}

fn check_version(
    binary: &Path,
    arg: &str,
    expected: &str,
    work: &Path,
    runtime: &Path,
) -> Result<()> {
    let output = Command::new(binary)
        .arg(arg)
        .current_dir(work)
        .env("ANDROID_HOME", runtime)
        .env("ANDROID_SDK_ROOT", runtime)
        .env("ANDROID_USER_HOME", work)
        .env("ANDROID_EMULATOR_HOME", work)
        .env("ANDROID_AVD_HOME", work)
        .env("TMP", work)
        .env("TEMP", work)
        .env("TMPDIR", work)
        .output()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success() && text.contains(expected),
        "{} {arg} returned {}: {text}",
        binary.display(),
        output.status
    );
    println!(
        "{}",
        text.lines().find(|line| line.contains(expected)).unwrap()
    );
    Ok(())
}

#[test]
#[ignore = "uses an installed x86_64 Android runtime and several GiB of private temporary space"]
fn offline_installed_runtime_round_trip() -> Result<()> {
    let locale = Locale::EnUs;
    let source = fs::canonicalize(env::var("RUSTEMUDROID_TEST_SDK_SOURCE")?)?;
    let image_id = env::var("RUSTEMUDROID_TEST_SDK_PACKAGE")?;
    let sdk = Sdk::find_localized(Some(source.clone()), locale)?;
    SystemImage::from_sdk(&sdk, &image_id)?;
    let work = PathBuf::from(env::var("RUSTEMUDROID_TEST_RUNTIME_DEST")?);
    let qa = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join(".rustemudroid/qa"))?;
    let parent = fs::canonicalize(work.parent().ok_or("destination needs a parent")?)?;
    let work = parent.join(
        work.file_name()
            .ok_or("destination needs a new directory name")?,
    );
    if !parent.starts_with(&qa) || parent.starts_with(&source) {
        return Err("Use a new .rustemudroid/qa directory outside the source SDK.".into());
    }
    fs::create_dir(&work)?;
    let external = env::var_os("RUSTEMUDROID_TEST_SDK_ARCHIVES").map(PathBuf::from);
    let archives = external.clone().unwrap_or_else(|| work.join("archives"));
    if external.is_none() {
        fs::create_dir(&archives)?;
    }
    let mut components = Vec::new();
    for package in ["emulator", "platform-tools", &image_id] {
        let relative: PathBuf = package.split(';').collect();
        let prefix = relative
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid package ID")?;
        let file = archives.join(format!("{prefix}.zip"));
        if external.is_none() {
            let mut zip = ZipWriter::new(File::create_new(&file)?);
            pack_directory(&mut zip, &source.join(&relative), prefix)?;
            zip.finish()?;
        }
        let bytes = fs::metadata(&file)?.len();
        let checksum = hash_file::<Sha1>(&file)?;
        println!("{} archive: {bytes} bytes, SHA-1 {checksum}", package);
        components.push(Component {
            package: package.into(),
            url: String::new(),
            relative,
            bytes,
            checksum,
        });
    }
    let plan = InstallPlan {
        image: image_id.clone(),
        total_bytes: components.iter().map(|c| c.bytes).sum(),
        licenses: Vec::new(),
        components,
    };
    let runtime = work.join("runtime");
    let mut last_component = String::new();
    install_from(
        &plan,
        &runtime,
        |p| {
            if p.component != last_component {
                println!("{}: {}/{} bytes", p.component, p.bytes, p.total_bytes);
                last_component = p.component;
            }
        },
        &AtomicBool::new(false),
        locale,
        |c| {
            Ok(File::open(
                archives.join(format!("{}.zip", c.archive_root())),
            )?)
        },
    )?;
    let image_relative: PathBuf = image_id.split(';').collect();
    for relative in [
        PathBuf::from("emulator/source.properties"),
        PathBuf::from("platform-tools/source.properties"),
        image_relative.join("source.properties"),
        image_relative.join("build.prop"),
        image_relative.join("system.img"),
    ] {
        let original = hash_file::<Sha256>(&source.join(&relative))?;
        assert_eq!(
            hash_file::<Sha256>(&runtime.join(&relative))?,
            original,
            "{}",
            relative.display()
        );
        println!("{} matches, SHA-256 {original}", relative.display());
    }
    let copied = Sdk::find_localized(Some(runtime.clone()), locale)?;
    let tools_work = work.join("version-checks");
    fs::create_dir(&tools_work)?;
    check_version(
        &copied.adb,
        "version",
        "Android Debug Bridge version",
        &tools_work,
        &runtime,
    )?;
    let emulator = runtime
        .join("emulator")
        .join(format!("emulator{}", env::consts::EXE_SUFFIX));
    check_version(
        &emulator,
        "-version",
        "Android emulator version",
        &tools_work,
        &runtime,
    )?;
    let profile = Profile::localized(work.join("profile"), locale);
    profile.init(SystemImage::from_sdk(&copied, &image_id)?)?;
    let system_dir = profile.system_dir(&copied)?;
    assert_eq!(
        system_dir.canonicalize()?,
        runtime.join(&image_relative).canonicalize()?
    );
    println!(
        "RustEmuDroid init resolved {image_id} to {}",
        system_dir.display()
    );
    Ok(())
}
