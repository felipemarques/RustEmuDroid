use std::{fs, io, path::Path};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{Gpu, KEMU_EMULATOR, Locale, Result, RuntimeKind, Sdk};

const ENGINE_LAYOUT: &str = include_str!("../../experiments/whpx-clock/overlay-manifest.json");

pub(super) fn select(path: &Path, sdk_image: bool, gpu: Gpu, locale: Locale) -> Result<Sdk> {
    if !cfg!(all(windows, target_arch = "x86_64"))
        || !sdk_image
        || !matches!(gpu, Gpu::Host | Gpu::Auto)
    {
        return Err(locale.pick(
            "O motor com cache Vulkan exige Windows x64, uma imagem do Android SDK e GPU do computador ou automática.",
            "The Vulkan cache engine requires Windows x64, an Android SDK image and host or automatic GPU mode.",
            "El motor con caché Vulkan requiere Windows x64, una imagen del SDK de Android y GPU del equipo o automática.",
        ).into());
    }
    let stage = stage_directory(path).ok_or_else(|| {
        locale.pick(
            "Escolha a pasta preparada que contém staging.json ou a subpasta sdk.",
            "Choose the prepared folder containing staging.json or its sdk subfolder.",
            "Elige la carpeta preparada que contiene staging.json o su subcarpeta sdk.",
        )
    })?;
    let root = stage.join("sdk");
    let receipt: Value = serde_json::from_reader(fs::File::open(stage.join("staging.json"))?)?;
    verify(&root, &receipt, locale)?;
    Sdk::from_root(root, RuntimeKind::Sdk, locale)
}

fn stage_directory(path: &Path) -> Option<&Path> {
    if path.join("staging.json").is_file() {
        Some(path)
    } else {
        path.parent().filter(|parent| {
            path.file_name().is_some_and(|name| name == "sdk")
                && parent.join("staging.json").is_file()
        })
    }
}

fn verify(root: &Path, receipt: &Value, locale: Locale) -> Result<()> {
    let invalid = locale.pick(
        "O pacote precisa ser preparado com stage.py usando --variant cache.",
        "Prepare this engine with stage.py using --variant cache.",
        "Prepara este motor con stage.py usando --variant cache.",
    );
    if receipt["variant"].as_str() != Some("cache") {
        return Err(invalid.into());
    }
    let layout: Value = serde_json::from_str(ENGINE_LAYOUT)?;
    let stock = &layout["stock_checks"];
    let core = receipt["qemu_sha256"].as_str().ok_or(invalid)?;
    if core
        == stock[format!("emulator/{KEMU_EMULATOR}")]
            .as_str()
            .ok_or(invalid)?
        || receipt["replaced"][KEMU_EMULATOR].as_str() != Some(core)
        || receipt["emulator_sha256"] != stock["emulator/emulator.exe"]
    {
        return Err(invalid.into());
    }

    let check = |path: &Path, expected: &Value| -> Result<()> {
        let expected = expected.as_str().ok_or(invalid)?;
        let mut hash = Sha256::new();
        io::copy(
            &mut fs::File::open(path).map_err(|e| format!("{} ({e})", path.display()))?,
            &mut hash,
        )?;
        if format!("{:x}", hash.finalize()) != expected {
            return Err(format!("SHA-256: {}", path.display()).into());
        }
        Ok(())
    };
    for name in [
        "emulator/emulator.exe",
        "emulator/lib64/libgfxstream_backend.dll",
    ] {
        check(&root.join(name), &stock[name])?;
    }
    // Only visit paths declared in the project's build layout. Rebuilds can use
    // the hashes in their own staging receipt in place of reference-build hashes.
    let tools = root.join("emulator");
    for row in layout["overlay"].as_array().ok_or(invalid)? {
        if row["origin"] == "microsoft_dia" {
            continue;
        }
        for destination in row["destinations"].as_array().ok_or(invalid)? {
            let name = destination.as_str().ok_or(invalid)?;
            check(&tools.join(name), &receipt["replaced"][name])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_movable_stage_from_either_folder() {
        let stage = tempfile::tempdir().unwrap();
        let sdk = stage.path().join("sdk");
        fs::create_dir(&sdk).unwrap();
        assert_eq!(stage_directory(&sdk), None);
        fs::write(stage.path().join("staging.json"), "{}").unwrap();
        assert_eq!(stage_directory(stage.path()), Some(stage.path()));
        assert_eq!(stage_directory(&sdk), Some(stage.path()));
        assert_eq!(stage_directory(&stage.path().join("other")), None);
    }

    #[test]
    fn rejects_clock_or_unmodified_core_before_reading_tools() {
        let dir = tempfile::tempdir().unwrap();
        for variant in ["clock", "cache"] {
            let layout: Value = serde_json::from_str(ENGINE_LAYOUT).unwrap();
            let stock = &layout["stock_checks"];
            let core = &stock[format!("emulator/{KEMU_EMULATOR}")];
            let receipt = json!({"variant": variant, "qemu_sha256": core,
                "emulator_sha256": stock["emulator/emulator.exe"], "replaced": { (KEMU_EMULATOR): core }});
            assert!(
                verify(dir.path(), &receipt, Locale::EnUs)
                    .unwrap_err()
                    .to_string()
                    .contains("--variant cache")
            );
        }
    }
}
