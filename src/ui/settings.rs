use std::{
    collections::BTreeMap,
    fs, io,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use super::{
    Action, BootStage, Gpu, Launcher, Locale, Profile, Sdk, default_kemu_image, gpu_label,
    path_input, theme,
};
use crate::android::{Camera, Hardware, PerformanceEngine, amd_uma_adapter, amd_uma_sdk_image};
use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct LaunchConfig {
    pub sdk: String,
    pub state: String,
    pub image: String,
    pub gpu: Gpu,
    pub hardware: Hardware,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub amd_uma_vulkan_by_profile: BTreeMap<PathBuf, bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) vulkan_cache_runtime_by_profile: BTreeMap<PathBuf, PathBuf>,
    /// Sibling of the cache runtime. A profile uses at most one of the two.
    /// Read leniently: a bad entry is dropped instead of resetting the file.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "lenient_map"
    )]
    pub(super) performance_engine_by_profile: BTreeMap<PathBuf, PerformanceEngine>,
    /// Windows power mode overlay while this profile's VM runs. Default off.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "lenient_map"
    )]
    pub(super) performance_power_by_profile: BTreeMap<PathBuf, bool>,
    pub show_fps: bool,
    #[serde(default)]
    pub android_fps: bool,
    #[serde(default)]
    pub adaptive_video: bool,
    #[serde(default = "super::mascot::enabled_by_default")]
    pub animate_mascot: bool,
    #[serde(default)]
    pub locale: Locale,
}

/// A per-profile map whose entries that do not parse (malformed, or written by
/// a newer client in another shape) are skipped, so one bad entry never makes
/// the whole `launcher.json` fall back to defaults.
fn lenient_map<'de, D, V>(deserializer: D) -> Result<BTreeMap<PathBuf, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    V: serde::de::DeserializeOwned,
{
    let serde_json::Value::Object(entries) = serde_json::Value::deserialize(deserializer)? else {
        return Ok(BTreeMap::new());
    };
    Ok(entries
        .into_iter()
        .filter_map(|(key, value)| {
            serde_json::from_value(value)
                .ok()
                .map(|value| (PathBuf::from(key), value))
        })
        .collect())
}

impl LaunchConfig {
    fn boot_changed(&self, other: &Self) -> bool {
        self.sdk != other.sdk
            || self.image != other.image
            || self.state != other.state
            || self.gpu != other.gpu
            || self.hardware != other.hardware
            || self.amd_uma_vulkan() != other.amd_uma_vulkan()
            || self.vulkan_cache_runtime() != other.vulkan_cache_runtime()
            || self.performance_engine() != other.performance_engine()
            || self.performance_power() != other.performance_power()
    }

    fn profile_key(&self) -> Option<PathBuf> {
        if self.state.trim().is_empty() {
            return None;
        }
        std::path::absolute(self.state.trim()).ok()
    }

    pub(super) fn amd_uma_vulkan(&self) -> bool {
        self.profile_key()
            .and_then(|key| self.amd_uma_vulkan_by_profile.get(&key).copied())
            .unwrap_or(false)
    }

    pub(super) fn performance_power(&self) -> bool {
        self.profile_key()
            .and_then(|key| self.performance_power_by_profile.get(&key).copied())
            .unwrap_or(false)
    }

    #[cfg(any(windows, test))]
    pub(super) fn set_performance_power(&mut self, enabled: bool) {
        if let Some(key) = self.profile_key() {
            if enabled {
                self.performance_power_by_profile.insert(key, true);
            } else {
                self.performance_power_by_profile.remove(&key);
            }
        }
    }

    fn set_amd_uma_vulkan(&mut self, enabled: bool) {
        if let Some(key) = self.profile_key() {
            if enabled {
                self.amd_uma_vulkan_by_profile.insert(key, true);
            } else {
                self.amd_uma_vulkan_by_profile.remove(&key);
            }
        }
    }

    pub(super) fn vulkan_cache_runtime(&self) -> Option<&Path> {
        self.profile_key()
            .and_then(|key| self.vulkan_cache_runtime_by_profile.get(&key))
            .map(PathBuf::as_path)
    }

    #[cfg(any(windows, test))]
    pub(super) fn set_vulkan_cache_runtime(&mut self, path: Option<PathBuf>) {
        if let Some(key) = self.profile_key() {
            if let Some(path) = path {
                self.performance_engine_by_profile.remove(&key);
                self.vulkan_cache_runtime_by_profile.insert(key, path);
            } else {
                self.vulkan_cache_runtime_by_profile.remove(&key);
            }
        }
    }

    pub(super) fn performance_engine(&self) -> Option<&PerformanceEngine> {
        self.profile_key()
            .and_then(|key| self.performance_engine_by_profile.get(&key))
    }

    /// Choosing another folder keeps the profile's switches.
    #[cfg(any(windows, test))]
    pub(super) fn set_performance_engine(&mut self, path: Option<PathBuf>) {
        let Some(key) = self.profile_key() else {
            return;
        };
        match path {
            Some(path) => {
                self.vulkan_cache_runtime_by_profile.remove(&key);
                self.performance_engine_by_profile
                    .entry(key)
                    .and_modify(|engine| engine.path = path.clone())
                    .or_insert_with(|| PerformanceEngine {
                        path,
                        options: Default::default(),
                    });
            }
            None => {
                self.performance_engine_by_profile.remove(&key);
            }
        }
    }

    #[cfg(any(windows, test))]
    fn performance_options_mut(&mut self) -> Option<&mut crate::android::PerformanceOptions> {
        let key = self.profile_key()?;
        self.performance_engine_by_profile
            .get_mut(&key)
            .map(|engine| &mut engine.options)
    }

    pub fn load(sdk: Option<PathBuf>, state: PathBuf, gpu: Option<Gpu>, file: &Path) -> Self {
        let mut config = fs::File::open(file)
            .ok()
            .and_then(|f| serde_json::from_reader(f).ok())
            .unwrap_or_else(|| Self {
                sdk: String::new(),
                image: String::new(),
                state: state.display().to_string(),
                gpu: if cfg!(windows) { Gpu::Host } else { Gpu::Auto },
                hardware: Hardware::default(),
                amd_uma_vulkan_by_profile: BTreeMap::new(),
                vulkan_cache_runtime_by_profile: BTreeMap::new(),
                performance_engine_by_profile: BTreeMap::new(),
                performance_power_by_profile: BTreeMap::new(),
                show_fps: true,
                android_fps: false,
                adaptive_video: false,
                animate_mascot: true,
                locale: Locale::default(),
            });
        if let Some(sdk) = sdk {
            config.sdk = sdk.display().to_string();
        } else {
            config.migrate_default_sdk(crate::android::default_sdk_path().as_deref());
        }
        if let Some(gpu) = gpu {
            config.gpu = gpu;
        }
        config
    }

    fn migrate_default_sdk(&mut self, default: Option<&Path>) {
        // Older clients saved the detected system SDK in the same field as a manual override.
        let saved = Path::new(&self.sdk);
        if !self.sdk.is_empty()
            && Some(saved) == default
            && Sdk::find(Some(saved.to_owned())).is_err()
        {
            self.sdk.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_runtime_stays_portable_and_saved_overrides_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let settings = state.join("launcher.json");
        let mut config = LaunchConfig::load(None, state.clone(), None, &settings);
        assert!(config.sdk.is_empty());
        assert!(config.image.is_empty());
        write_json(&settings, &config).unwrap();
        assert!(
            LaunchConfig::load(None, state.clone(), None, &settings)
                .sdk
                .is_empty()
        );

        config.sdk = temp.path().join("older-sdk").display().to_string();
        config.image = temp.path().join("custom-image").display().to_string();
        write_json(&settings, &config).unwrap();
        let saved = LaunchConfig::load(None, state.clone(), None, &settings);
        assert_eq!(saved.sdk, config.sdk);
        assert_eq!(saved.image, config.image);
        let explicit = temp.path().join("another-sdk");
        let selected = LaunchConfig::load(Some(explicit.clone()), state, None, &settings);
        assert_eq!(Path::new(&selected.sdk), explicit);
        assert_eq!(selected.image, config.image);

        let mut migrated = saved;
        migrated.migrate_default_sdk(Some(&temp.path().join("system-sdk")));
        assert_eq!(migrated.sdk, config.sdk);
        migrated.migrate_default_sdk(Some(Path::new(&config.sdk)));
        assert!(migrated.sdk.is_empty());
        assert_eq!(migrated.image, config.image);
    }

    #[test]
    fn display_refresh_migrates_and_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let prefs = temp.path().join("launcher.json");
        let mut config = LaunchConfig::load(None, temp.path().into(), None, &prefs);
        let old_json = serde_json::to_value(&config).unwrap();
        assert!(old_json["hardware"].get("refresh_rate").is_none());
        let legacy: LaunchConfig = serde_json::from_value(old_json).unwrap();
        assert_eq!(legacy.hardware.refresh_rate, None);

        config.hardware.refresh_rate = std::num::NonZeroU16::new(90);
        assert!(legacy.boot_changed(&config));
        let restored: LaunchConfig =
            serde_json::from_value(serde_json::to_value(config).unwrap()).unwrap();
        assert_eq!(
            restored.hardware.refresh_rate.map(|rate| rate.get()),
            Some(90)
        );
    }

    #[test]
    fn amd_uma_vulkan_pref_defaults_off_and_is_scoped_to_the_selected_profile() {
        let dir = tempfile::tempdir().unwrap();
        let mut original: LaunchConfig = serde_json::from_value(serde_json::json!({
            "sdk": "",
            "state": dir.path(),
            "image": "",
            "gpu": "Auto",
            "hardware": {
                "adapter": "",
                "camera": "Profile",
                "webcam_index": 0
            },
            "show_fps": true
        }))
        .unwrap();
        let old = original.clone();
        assert!(!original.amd_uma_vulkan());
        original.set_amd_uma_vulkan(true);
        assert!(original.boot_changed(&old));

        let mut saved: LaunchConfig =
            serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
        assert!(saved.amd_uma_vulkan());
        let first = saved.state.clone();
        saved.state = dir.path().join("other").display().to_string();
        assert!(!saved.amd_uma_vulkan());
        saved.state = first;
        assert!(saved.amd_uma_vulkan());
        saved.set_amd_uma_vulkan(false);
        assert!(!saved.amd_uma_vulkan());
        assert!(!saved.boot_changed(&old));
    }

    #[test]
    fn bad_performance_entries_do_not_reset_the_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir.path().join("launcher.json");
        let profile = dir.path().join("android-17");
        let mut config = LaunchConfig::load(None, profile.clone(), None, &prefs);
        config.locale = Locale::EsEs;
        config.set_performance_engine(Some(dir.path().join("engine")));
        let key = std::path::absolute(&profile).unwrap().display().to_string();
        let mut json = serde_json::to_value(&config).unwrap();
        json["performance_engine_by_profile"]["other-profile"] = serde_json::json!(42);
        json["performance_engine_by_profile"]["future-profile"] =
            serde_json::json!({"engine": {"path": 1}});
        json["performance_power_by_profile"] = serde_json::json!({
            (key.clone()): true, "broken-profile": "yes", "future-profile": {"on": true}
        });
        write_json(&prefs, &json).unwrap();
        let loaded = LaunchConfig::load(None, dir.path().join("unused"), None, &prefs);
        assert_eq!(loaded.locale, Locale::EsEs);
        assert_eq!(loaded.state, config.state, "selected profile kept");
        assert_eq!(loaded.performance_engine(), config.performance_engine());
        assert_eq!(loaded.performance_engine_by_profile.len(), 1);
        assert_eq!(
            loaded
                .performance_power_by_profile
                .keys()
                .collect::<Vec<_>>(),
            [&PathBuf::from(&key)]
        );
        // A map replaced by another type is dropped as a whole, nothing else.
        let mut json = serde_json::to_value(&config).unwrap();
        json["performance_engine_by_profile"] = serde_json::json!("x");
        json["performance_power_by_profile"] = serde_json::json!(null);
        write_json(&prefs, &json).unwrap();
        let loaded = LaunchConfig::load(None, dir.path().join("unused"), None, &prefs);
        assert_eq!(loaded.locale, Locale::EsEs);
        assert!(loaded.performance_engine_by_profile.is_empty());
        assert!(loaded.performance_power_by_profile.is_empty());
    }

    #[test]
    fn vulkan_cache_runtime_migrates_and_restarts_only_the_selected_profile() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir.path().join("launcher.json");
        let profile = dir.path().join("android-17");
        let legacy = LaunchConfig::load(None, profile.clone(), None, &prefs);
        let json = serde_json::to_value(&legacy).unwrap();
        assert!(json.get("vulkan_cache_runtime_by_profile").is_none());
        let mut configured: LaunchConfig = serde_json::from_value(json).unwrap();
        assert!(configured.vulkan_cache_runtime().is_none());

        let prepared = dir.path().join("prepared");
        configured.set_vulkan_cache_runtime(Some(prepared.clone()));
        assert!(configured.boot_changed(&legacy));
        assert_eq!(configured.state, legacy.state);
        assert_eq!(configured.sdk, legacy.sdk);
        assert_eq!(configured.image, legacy.image);

        write_json(&prefs, &configured).unwrap();
        let mut restored = LaunchConfig::load(None, profile, None, &prefs);
        assert_eq!(restored.vulkan_cache_runtime(), Some(prepared.as_path()));
        assert!(!restored.boot_changed(&configured));
        let original = dir.path().join("original").display().to_string();
        restored.state = original.clone();
        assert!(restored.vulkan_cache_runtime().is_none());
        let mut other = legacy.clone();
        other.state = original;
        assert!(!restored.boot_changed(&other));

        let second = dir.path().join("another").join("sdk");
        restored.set_vulkan_cache_runtime(Some(second.clone()));
        assert!(restored.boot_changed(&other));
        restored.state = legacy.state.clone();
        restored.set_vulkan_cache_runtime(None);
        assert!(!restored.boot_changed(&legacy));
        restored.state = other.state;
        assert_eq!(restored.vulkan_cache_runtime(), Some(second.as_path()));
    }

    #[test]
    fn performance_power_defaults_off_and_is_scoped_to_the_selected_profile() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir.path().join("launcher.json");
        let legacy = LaunchConfig::load(None, dir.path().join("android-17"), None, &prefs);
        assert!(!legacy.performance_power());
        let json = serde_json::to_value(&legacy).unwrap();
        assert!(json.get("performance_power_by_profile").is_none());

        let mut configured = legacy.clone();
        configured.set_performance_power(true);
        assert!(configured.boot_changed(&legacy));
        assert!(
            configured.performance_engine().is_none(),
            "independent of the engine"
        );
        write_json(&prefs, &configured).unwrap();
        let mut restored = LaunchConfig::load(None, dir.path().join("android-17"), None, &prefs);
        assert!(restored.performance_power());
        let first = restored.state.clone();
        restored.state = dir.path().join("other").display().to_string();
        assert!(!restored.performance_power());
        restored.state = first;
        restored.set_performance_power(false);
        assert!(!restored.boot_changed(&legacy));
    }

    #[test]
    fn performance_engine_loads_old_preferences_and_replaces_the_cache_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir.path().join("launcher.json");
        let profile = dir.path().join("android-17");
        let cache = dir.path().join("cache-engine");
        // A v0.22.0 file: no performance field, one cache runtime.
        fs::write(
            &prefs,
            serde_json::json!({
                "sdk": "",
                "state": profile,
                "image": "",
                "gpu": "Host",
                "hardware": {"adapter": "", "camera": "Profile", "webcam_index": 0},
                "vulkan_cache_runtime_by_profile": {
                    (std::path::absolute(&profile).unwrap().display().to_string()): cache
                },
                "show_fps": true
            })
            .to_string(),
        )
        .unwrap();
        let legacy = LaunchConfig::load(None, profile.clone(), None, &prefs);
        assert_eq!(legacy.vulkan_cache_runtime(), Some(cache.as_path()));
        assert!(legacy.performance_engine().is_none());
        let json = serde_json::to_value(&legacy).unwrap();
        assert!(json.get("performance_engine_by_profile").is_none());

        let engine = dir.path().join("perf-engine");
        let mut configured = legacy.clone();
        configured.set_performance_engine(Some(engine.clone()));
        assert!(configured.vulkan_cache_runtime().is_none());
        assert_eq!(
            configured.performance_engine(),
            Some(&PerformanceEngine {
                path: engine.clone(),
                options: Default::default()
            })
        );
        assert!(configured.boot_changed(&legacy));
        let options = configured.performance_options_mut().unwrap();
        assert!(!options.guest_kernel_parameters);
        options.dedicated_vcpu_cores = false;
        options.guest_kernel_parameters = true;
        let tuned = configured.clone();

        write_json(&prefs, &configured).unwrap();
        let mut restored = LaunchConfig::load(None, profile.clone(), None, &prefs);
        assert!(!restored.boot_changed(&tuned));
        assert_eq!(restored.performance_engine(), tuned.performance_engine());
        // Another folder keeps the switches; another profile sees nothing.
        restored.set_performance_engine(Some(dir.path().join("rebuilt")));
        let options = &restored.performance_engine().unwrap().options;
        assert!(options.guest_kernel_parameters && !options.dedicated_vcpu_cores);
        let first = restored.state.clone();
        restored.state = dir.path().join("other").display().to_string();
        assert!(restored.performance_engine().is_none());
        assert!(restored.performance_options_mut().is_none());
        restored.state = first;

        // Missing or unknown option fields keep the conservative defaults.
        let key = std::path::absolute(&profile).unwrap().display().to_string();
        let mut partial = serde_json::to_value(&legacy).unwrap();
        partial["performance_engine_by_profile"] = serde_json::json!({
            (key.clone()): {"path": engine, "options": {"clock_sync": false, "future": 1}}
        });
        let partial: LaunchConfig = serde_json::from_value(partial).unwrap();
        let options = &partial.performance_engine().unwrap().options;
        assert!(!options.clock_sync && options.vulkan_memory_cache && options.asg_transport);
        assert!(options.vulkan_composition && options.dedicated_vcpu_cores);
        assert!(!options.guest_kernel_parameters);
        let mut bare = serde_json::to_value(&legacy).unwrap();
        bare["performance_engine_by_profile"] = serde_json::json!({ (key): {"path": engine} });
        let bare: LaunchConfig = serde_json::from_value(bare).unwrap();
        assert_eq!(
            bare.performance_engine().unwrap().options,
            crate::android::PerformanceOptions::default()
        );

        // Choosing the cache runtime again removes the performance engine.
        restored.set_vulkan_cache_runtime(Some(cache.clone()));
        assert!(restored.performance_engine().is_none());
        assert!(!restored.boot_changed(&legacy));
        restored.set_vulkan_cache_runtime(None);
        restored.set_performance_engine(Some(engine));
        restored.set_performance_engine(None);
        assert!(
            restored.performance_engine().is_none() && restored.vulkan_cache_runtime().is_none()
        );
    }
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> crate::Result<()> {
    let parent = path.parent().ok_or("Diretório de destino inválido.")?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(temp.as_file_mut(), value)?;
    temp.flush()?;
    temp.persist(path)?;
    Ok(())
}

// Called only by the probe process. Loading/unloading a Vulkan ICD in the live
// OpenGL frontend can invalidate state shared by both parts of a graphics driver.
pub(crate) fn print_gpu_inventory() -> crate::Result<()> {
    serde_json::to_writer(io::stdout().lock(), &vulkan_adapters())?;
    Ok(())
}

fn vulkan_adapters() -> Vec<String> {
    // The names come from the same Vulkan API the emulator uses to select a physical device.
    unsafe {
        let Ok(entry) = ash::Entry::load() else {
            return vec![];
        };
        let Ok(instance) = entry.create_instance(&ash::vk::InstanceCreateInfo::default(), None)
        else {
            return vec![];
        };
        let mut names: Vec<_> = instance
            .enumerate_physical_devices()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|device| {
                let properties = instance.get_physical_device_properties(device);
                properties
                    .device_name_as_c_str()
                    .ok()
                    .map(|s| s.to_string_lossy().into_owned())
            })
            .collect();
        instance.destroy_instance(None);
        names.sort();
        names.dedup();
        names
    }
}

#[derive(Default)]
pub(super) struct AdapterList {
    pub(super) names: Vec<String>,
    pending: Option<(Child, Instant)>,
    error: bool,
}

impl AdapterList {
    fn request(&mut self) {
        if self.pending.is_some() || !self.names.is_empty() {
            return;
        }
        let exe = std::env::current_exe();
        #[cfg(test)]
        let exe = std::env::var_os("RUSTEMUDROID_TEST_EXE").map_or(exe, |path| Ok(path.into()));
        let child = exe.and_then(|path| {
            let mut cmd = Command::new(path);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
            }
            cmd.arg("gpu-inventory")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
        });
        self.error = child.is_err();
        self.pending = child.ok().map(|child| (child, Instant::now()));
    }

    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        let Some((child, start)) = &mut self.pending else {
            return;
        };
        match child.try_wait() {
            Ok(None) if start.elapsed() < Duration::from_secs(8) => {
                ctx.request_repaint_after(Duration::from_millis(50));
                return;
            }
            Ok(Some(status)) if status.success() => {
                let result = child
                    .stdout
                    .take()
                    .and_then(|out| serde_json::from_reader(out).ok());
                self.error = result.is_none();
                self.names = result.unwrap_or_default();
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                self.error = true;
            }
        }
        self.pending = None;
    }
}

impl Drop for AdapterList {
    fn drop(&mut self) {
        if let Some((mut child, _)) = self.pending.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn camera_label(camera: Camera, locale: Locale) -> &'static str {
    match camera {
        Camera::Profile => locale.pick(
            "Padrão da imagem Android",
            "Android image default",
            "Predeterminada de la imagen Android",
        ),
        Camera::EmulatedFront => locale.pick(
            "Câmera frontal simulada",
            "Simulated front camera",
            "Cámara frontal simulada",
        ),
        Camera::WebcamFront => locale.pick(
            "Webcam do computador · frontal",
            "Host webcam · front",
            "Cámara web del equipo · frontal",
        ),
        Camera::WebcamBack => locale.pick(
            "Webcam do computador · traseira",
            "Host webcam · rear",
            "Cámara web del equipo · trasera",
        ),
    }
}

fn current_monitor_rate(frame: Option<&eframe::Frame>) -> Option<u16> {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::Graphics::Gdi::{
            DEVMODEW, DM_DISPLAYFREQUENCY, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW,
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFOEXW, MonitorFromWindow,
        };
        let RawWindowHandle::Win32(window) = frame?.window_handle().ok()?.as_raw() else {
            return None;
        };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        let mut mode = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        unsafe {
            let monitor = MonitorFromWindow(window.hwnd.get() as _, MONITOR_DEFAULTTONEAREST);
            if monitor.is_null()
                || GetMonitorInfoW(monitor, &mut info.monitorInfo) == 0
                || EnumDisplaySettingsW(info.szDevice.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode)
                    == 0
            {
                return None;
            }
        }
        u16::try_from(mode.dmDisplayFrequency)
            .ok()
            .filter(|&hz| hz > 1 && mode.dmFields & DM_DISPLAYFREQUENCY != 0)
    }
    #[cfg(not(windows))]
    {
        let _ = frame;
        None
    }
}

#[derive(Default)]
pub(super) struct ProfileImages {
    sdk: Option<String>,
    original: Option<String>,
    portable_original: Option<PathBuf>,
    missing_original: bool,
    original_state: PathBuf,
    google_state: PathBuf,
    google: PathBuf,
    play_ready: bool,
    has_berberis_image: bool,
    amd_uma_google_image: bool,
    amd_uma_google_profile: bool,
    amd_uma_original_profile: bool,
}

impl ProfileImages {
    fn supports_amd_uma(&self, draft: &LaunchConfig, base: &Path, google: &Path) -> bool {
        if !cfg!(windows) || !amd_uma_adapter(draft.gpu, &draft.hardware.adapter) {
            return false;
        }
        let state = Path::new(draft.state.trim());
        if state.as_os_str().is_empty() {
            return false;
        }
        let image = draft.image.trim();
        if !image.is_empty() {
            return self.amd_uma_google_image
                && std::path::absolute(image).is_ok_and(|path| path == self.google);
        }
        (state == google && self.amd_uma_google_profile)
            || (state == base && self.amd_uma_original_profile)
    }

    fn refresh(&mut self, sdk_path: &str, base: &Path) {
        let explicit_sdk = sdk_path.trim();
        let sdk = Sdk::find((!explicit_sdk.is_empty()).then(|| explicit_sdk.into())).ok();
        let managed = sdk
            .as_ref()
            .filter(|sdk| super::runtime_setup::managed(sdk));
        let selected_state = |choice| {
            let [primary, fallback] = super::runtime_setup::profile_dirs(base, choice);
            if let Some(sdk) = managed {
                let requested = super::runtime_setup::image_path(&sdk.root, choice.package());
                if Profile::new(fallback.clone())
                    .system_dir(sdk)
                    .ok()
                    .and_then(|p| p.canonicalize().ok())
                    .is_some_and(|p| {
                        requested
                            .canonicalize()
                            .is_ok_and(|requested| p == requested)
                    })
                {
                    return fallback;
                }
            }
            primary
        };
        self.original_state = selected_state(crate::runtime_install::ImageChoice::Original);
        self.google_state = selected_state(crate::runtime_install::ImageChoice::GooglePlay);
        let base = &self.original_state;
        let original_dir = sdk
            .as_ref()
            .and_then(|sdk| Profile::new(base.into()).system_dir(sdk).ok())
            .or_else(|| sdk.as_ref().and_then(default_kemu_image));
        self.portable_original = managed
            .map(|sdk| {
                super::runtime_setup::image_path(&sdk.root, super::runtime_setup::ORIGINAL_ID)
            })
            .filter(|path| path.join("system.img").is_file());
        self.missing_original = managed.is_some()
            && self.portable_original.is_none()
            && !base.join("avd/RustEmuDroid.ini").is_file();
        self.has_berberis_image = original_dir
            .as_deref()
            .is_some_and(crate::android::is_kemu_image);
        self.original = fs::read_to_string(base.join("avd/RustEmuDroid.ini"))
            .ok()
            .and_then(|ini| {
                ini.lines()
                    .find_map(|l| l.trim().strip_prefix("target=android-").map(str::to_owned))
            })
            .map(|api| match api.as_str() {
                "36" => "Android 16 · API 36".into(),
                _ => format!("API {api}"),
            });
        self.google = super::runtime_setup::image_path(
            sdk.as_ref()
                .map_or(Path::new(sdk_path), |sdk| sdk.root.as_path()),
            super::runtime_setup::GOOGLE_ID,
        );
        self.play_ready = self.google.join("system.img").is_file();
        self.amd_uma_google_image = false;
        self.amd_uma_google_profile = false;
        self.amd_uma_original_profile = false;
        if cfg!(windows)
            && let Some(sdk) = &sdk
        {
            self.amd_uma_google_image = amd_uma_sdk_image(sdk, &self.google);
            let profile_supports_amd_uma = |state: &Path| {
                if state.join("avd/RustEmuDroid.ini").is_file() {
                    Profile::new(state.into())
                        .system_dir(sdk)
                        .ok()
                        .is_some_and(|dir| amd_uma_sdk_image(sdk, &dir))
                } else {
                    self.amd_uma_google_image
                }
            };
            self.amd_uma_google_profile = profile_supports_amd_uma(&self.google_state);
            self.amd_uma_original_profile = profile_supports_amd_uma(base);
        }
        self.sdk = Some(sdk_path.to_owned());
    }
}

#[cfg(windows)]
fn vulkan_cache_runtime_picker(
    ui: &mut egui::Ui,
    frame: Option<&eframe::Frame>,
    draft: &mut LaunchConfig,
    locale: Locale,
) {
    ui.add_space(7.);
    ui.label(
        RichText::new(locale.pick(
            "Cache de memória Vulkan (experimental)",
            "Vulkan memory cache (experimental)",
            "Caché de memoria Vulkan (experimental)",
        ))
        .strong(),
    );
    theme::label(
        ui,
        locale.pick(
            "Selecione a pasta sdk do motor preparado ou a pasta que a contém. A escolha vale só para este perfil e mantém a imagem Android e os dados.",
            "Select the prepared engine’s sdk folder or its parent. The choice applies to this profile and keeps its Android image and data.",
            "Selecciona la carpeta sdk del motor preparado o la carpeta que la contiene. El cambio se aplica a este perfil y conserva la imagen y los datos de Android.",
        ),
    );
    if let Some(path) = draft.vulkan_cache_runtime() {
        let path = path.to_string_lossy();
        ui.add(egui::Label::new(path.as_ref()).truncate())
            .on_hover_text(path.as_ref());
    } else {
        ui.weak(locale.pick("Motor padrão", "Default engine", "Motor predeterminado"));
    }
    ui.add_enabled_ui(draft.profile_key().is_some(), |ui| {
        ui.horizontal_wrapped(|ui| {
            let choose = locale.pick(
                "Escolher pasta do motor…",
                "Choose runtime folder…",
                "Elegir carpeta del motor…",
            );
            if ui.add(theme::button(choose)).clicked() {
                let mut dialog = rfd::FileDialog::new().set_title(choose);
                if let Some(frame) = frame {
                    dialog = dialog.set_parent(frame);
                }
                if let Some(path) = draft.vulkan_cache_runtime() {
                    dialog = dialog.set_directory(path);
                }
                if let Some(path) = dialog.pick_folder() {
                    draft.set_vulkan_cache_runtime(Some(path));
                }
            }
            if ui
                .add_enabled(
                    draft.vulkan_cache_runtime().is_some(),
                    theme::button(locale.pick(
                        "Voltar ao motor padrão",
                        "Use default engine",
                        "Volver al motor predeterminado",
                    )),
                )
                .clicked()
            {
                draft.set_vulkan_cache_runtime(None);
            }
        });
    });
}

/// Main label and hover text. Technical names stay in the hover text.
#[cfg(windows)]
fn tuning_text(tuning: crate::android::Tuning, locale: Locale) -> (&'static str, &'static str) {
    use crate::android::Tuning;
    match tuning {
        // One switch for the three CPU virtualization gates.
        Tuning::ClockSync | Tuning::LapicFastpath | Tuning::KvmPvEoi => (
            locale.pick(
                "Ajustes de virtualização do processador",
                "CPU virtualization tuning",
                "Ajustes de virtualización del procesador",
            ),
            locale.pick(
                "Reduz as pausas da máquina virtual: o relógio do Android usa o contador do processador (RUSTEMUDROID_QA_WHPX_SYNC_TSC=1), os acessos ao APIC usam um caminho rápido (RUSTEMUDROID_QA_LAPIC_FASTPATH=1) e o fim de interrupção é paravirtualizado (RUSTEMUDROID_QA_KVM_PV_EOI=1). Com este último, o Android identifica o hipervisor como KVM. Se o motor oferecer, o controlador de interrupções (APIC) passa a ser emulado pelo próprio Windows (RUSTEMUDROID_QA_WHPX_APIC=hv); nesse modo o caminho rápido do APIC e o fim de interrupção paravirtualizado não se aplicam e o Android não vê KVM. Se este computador não permitir, o motor volta ao APIC do modo usuário com os dois ajustes anteriores. Cada item pode ser desligado no arquivo de preferências.",
                "Reduces virtual machine exits: Android’s clock uses the processor’s counter (RUSTEMUDROID_QA_WHPX_SYNC_TSC=1), APIC accesses take a fast path (RUSTEMUDROID_QA_LAPIC_FASTPATH=1) and end of interrupt is paravirtualized (RUSTEMUDROID_QA_KVM_PV_EOI=1). With the last one, Android identifies its hypervisor as KVM. When the engine offers it, the interrupt controller (APIC) is emulated by Windows itself (RUSTEMUDROID_QA_WHPX_APIC=hv); in that mode the fast APIC path and paravirtualized end of interrupt do not apply and Android does not see KVM. If this computer does not allow it, the engine falls back to the user-mode APIC with the two previous settings. Each item can be turned off in the preferences file.",
                "Reduce las salidas de la máquina virtual: el reloj de Android usa el contador del procesador (RUSTEMUDROID_QA_WHPX_SYNC_TSC=1), los accesos al APIC usan una ruta rápida (RUSTEMUDROID_QA_LAPIC_FASTPATH=1) y el fin de interrupción está paravirtualizado (RUSTEMUDROID_QA_KVM_PV_EOI=1). Con este último, Android identifica el hipervisor como KVM. Si el motor lo ofrece, el controlador de interrupciones (APIC) lo emula el propio Windows (RUSTEMUDROID_QA_WHPX_APIC=hv); en ese modo la ruta rápida del APIC y el fin de interrupción paravirtualizado no se aplican y Android no ve KVM. Si este equipo no lo permite, el motor vuelve al APIC del modo usuario con los dos ajustes anteriores. Cada elemento se puede desactivar en el archivo de preferencias.",
            ),
        ),
        Tuning::VulkanMemoryCache => (
            locale.pick(
                "Memória Vulkan com cache",
                "Cached Vulkan memory",
                "Memoria Vulkan con caché",
            ),
            locale.pick(
                "Os jogos escrevem na memória gráfica por um caminho com cache, inclusive na memória da GPU quando o motor permite. Variáveis do motor: RUSTEMUDROID_QA_VULKAN_CACHED_HOST=1 e RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL=2.",
                "Games write graphics memory through a cached path, including GPU memory when the engine supports it. Engine variables: RUSTEMUDROID_QA_VULKAN_CACHED_HOST=1 and RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL=2.",
                "Los juegos escriben en la memoria gráfica por una ruta con caché, incluida la memoria de la GPU cuando el motor lo permite. Variables del motor: RUSTEMUDROID_QA_VULKAN_CACHED_HOST=1 y RUSTEMUDROID_QA_VULKAN_CACHED_DEVICE_LOCAL=2.",
            ),
        ),
        Tuning::AsgTransport => (
            locale.pick(
                "Comandos gráficos por memória compartilhada",
                "Graphics commands through shared memory",
                "Comandos gráficos por memoria compartida",
            ),
            locale.pick(
                "Os comandos gráficos passam por um anel de memória compartilhada e o motor aguarda novos comandos por 3 ms (3000 µs) antes de pausar, o que ocupa cerca de um núcleo extra do processador enquanto um jogo roda. Grava hw.gltransport=asg no config.ini do perfil; ao desligar, o valor anterior é restaurado. Variável do motor: RUSTEMUDROID_QA_ASG_SPIN_US=3000.",
                "Graphics commands go through a shared-memory ring, and the engine waits 3 ms (3000 µs) for new commands before pausing, which keeps about one extra processor core busy while a game runs. Writes hw.gltransport=asg to the profile’s config.ini; turning it off restores the previous value. Engine variable: RUSTEMUDROID_QA_ASG_SPIN_US=3000.",
                "Los comandos gráficos pasan por un anillo de memoria compartida y el motor espera 3 ms (3000 µs) nuevos comandos antes de pausar, lo que ocupa aproximadamente un núcleo adicional del procesador mientras se ejecuta un juego. Escribe hw.gltransport=asg en el config.ini del perfil; al desactivarlo se restaura el valor anterior. Variable del motor: RUSTEMUDROID_QA_ASG_SPIN_US=3000.",
            ),
        ),
        Tuning::VulkanComposition => (
            locale.pick(
                "Compor a tela com Vulkan",
                "Compose the screen with Vulkan",
                "Componer la pantalla con Vulkan",
            ),
            locale.pick(
                "O computador compõe a tela do Android com Vulkan e o Android usa ANGLE para OpenGL ES. Recurso do emulador: VulkanNativeSwapchain.",
                "The computer composes Android’s screen with Vulkan, and Android uses ANGLE for OpenGL ES. Emulator feature: VulkanNativeSwapchain.",
                "El equipo compone la pantalla de Android con Vulkan y Android usa ANGLE para OpenGL ES. Función del emulador: VulkanNativeSwapchain.",
            ),
        ),
        Tuning::DedicatedVcpuCores => (
            locale.pick(
                "Núcleos dedicados para o Android",
                "Dedicated cores for Android",
                "Núcleos dedicados para Android",
            ),
            locale.pick(
                "Cada processador virtual do Android recebe um dos últimos núcleos físicos, nunca o núcleo 0, onde o Windows concentra interrupções. As demais tarefas do motor usam os outros núcleos. Variável do motor: RUSTEMUDROID_QA_VCPU_PLACEMENT=sets-high.",
                "Each Android virtual processor gets one of the last physical cores, never core 0, where Windows concentrates interrupts. The engine’s other tasks use the remaining cores. Engine variable: RUSTEMUDROID_QA_VCPU_PLACEMENT=sets-high.",
                "Cada procesador virtual de Android recibe uno de los últimos núcleos físicos, nunca el núcleo 0, donde Windows concentra las interrupciones. Las demás tareas del motor usan los otros núcleos. Variable del motor: RUSTEMUDROID_QA_VCPU_PLACEMENT=sets-high.",
            ),
        ),
        Tuning::GuestKernelParameters => (
            locale.pick(
                "Reduzir proteções do kernel do Android",
                "Reduce Android kernel protections",
                "Reducir las protecciones del kernel de Android",
            ),
            locale.pick(
                "Grava kernel.parameters=mitigations=off init_on_alloc=0 transparent_hugepage=always no config.ini do perfil. Se o perfil já tiver kernel.parameters próprio, os valores dele são mantidos e estes são acrescentados. Ao desligar, a linha original é restaurada.",
                "Writes kernel.parameters=mitigations=off init_on_alloc=0 transparent_hugepage=always to the profile’s config.ini. If the profile already has its own kernel.parameters, its values are kept and these are appended. Turning it off restores the original line.",
                "Escribe kernel.parameters=mitigations=off init_on_alloc=0 transparent_hugepage=always en el config.ini del perfil. Si el perfil ya tiene su propio kernel.parameters, se conservan sus valores y se añaden estos. Al desactivarlo se restaura la línea original.",
            ),
        ),
    }
}

/// Host setting, independent of the engine manifest.
#[cfg(windows)]
fn performance_power_toggle(ui: &mut egui::Ui, draft: &mut LaunchConfig, locale: Locale) {
    let mut enabled = draft.performance_power();
    ui.add_space(4.);
    let changed = ui
        .add_enabled(
            draft.profile_key().is_some(),
            egui::Checkbox::new(
                &mut enabled,
                locale.pick(
                    "Modo de energia de melhor desempenho do Windows enquanto o Android estiver aberto",
                    "Windows best performance power mode while Android runs",
                    "Modo de energía de máximo rendimiento de Windows mientras Android esté abierto",
                ),
            ),
        )
        .on_hover_text(locale.pick(
            "Ativa o modo de energia \"Melhor desempenho\" do Windows ao iniciar o Android e restaura o anterior ao fechar. O plano de energia não muda. Não é usado na bateria.",
            "Turns on the Windows \"Best performance\" power mode when Android starts and restores the previous mode when it closes. The power plan is not changed. Not used on battery power.",
            "Activa el modo de energía \"Máximo rendimiento\" de Windows al iniciar Android y restaura el anterior al cerrarlo. El plan de energía no cambia. No se usa con batería.",
        ))
        .changed();
    if changed {
        draft.set_performance_power(enabled);
    }
    theme::label(
        ui,
        locale.pick(
            "Aumenta o consumo de energia e o ruído dos ventiladores enquanto o Android estiver aberto.",
            "Raises power use and fan noise while Android runs.",
            "Aumenta el consumo de energía y el ruido de los ventiladores mientras Android esté abierto.",
        ),
    );
}

#[cfg(windows)]
fn performance_engine_picker(
    ui: &mut egui::Ui,
    frame: Option<&eframe::Frame>,
    draft: &mut LaunchConfig,
    locale: Locale,
) {
    use crate::android::{
        Capability, Tuning, avd_vcpus, manifest_capabilities, manifest_modified, physical_cores,
        placement_fits,
    };
    ui.add_space(7.);
    ui.label(
        RichText::new(locale.pick(
            "Motor de desempenho (experimental)",
            "Performance engine (experimental)",
            "Motor de rendimiento (experimental)",
        ))
        .strong(),
    );
    theme::label(
        ui,
        locale.pick(
            "Selecione a pasta do motor com rustemudroid-engine.json ou a subpasta sdk. Os arquivos do motor são conferidos a cada início e, se algo não conferir, o Android inicia com o motor padrão. A escolha vale só para este perfil e mantém a imagem Android e os dados.",
            "Select the engine folder containing rustemudroid-engine.json or its sdk subfolder. The engine files are checked at every start, and Android starts with the default engine if anything does not match. The choice applies to this profile and keeps its Android image and data.",
            "Selecciona la carpeta del motor con rustemudroid-engine.json o su subcarpeta sdk. Los archivos del motor se comprueban en cada inicio y, si algo no coincide, Android se inicia con el motor predeterminado. El cambio se aplica a este perfil y conserva la imagen y los datos de Android.",
        ),
    );
    let selected = draft.performance_engine().map(|engine| engine.path.clone());
    if let Some(path) = &selected {
        let path = path.to_string_lossy();
        ui.add(egui::Label::new(path.as_ref()).truncate())
            .on_hover_text(path.as_ref());
    } else {
        ui.weak(locale.pick(
            "Nenhum motor de desempenho",
            "No performance engine",
            "Ningún motor de rendimiento",
        ));
    }
    ui.add_enabled_ui(draft.profile_key().is_some(), |ui| {
        ui.horizontal_wrapped(|ui| {
            let choose = locale.pick(
                "Escolher motor de desempenho…",
                "Choose performance engine…",
                "Elegir motor de rendimiento…",
            );
            if ui.add(theme::button(choose)).clicked() {
                let mut dialog = rfd::FileDialog::new().set_title(choose);
                if let Some(frame) = frame {
                    dialog = dialog.set_parent(frame);
                }
                if let Some(path) = &selected {
                    dialog = dialog.set_directory(path);
                }
                if let Some(path) = dialog.pick_folder() {
                    draft.set_performance_engine(Some(path));
                }
            }
            if ui
                .add_enabled(
                    selected.is_some(),
                    theme::button(locale.pick(
                        "Remover motor de desempenho",
                        "Remove performance engine",
                        "Quitar el motor de rendimiento",
                    )),
                )
                .clicked()
            {
                draft.set_performance_engine(None);
            }
        });
    });
    let Some(path) = draft.performance_engine().map(|engine| engine.path.clone()) else {
        return;
    };
    // Read the manifest and the host's cores once per folder, profile and change.
    #[derive(Clone)]
    struct Cached {
        path: PathBuf,
        state: String,
        modified: Option<std::time::SystemTime>,
        capabilities: Option<Vec<Capability>>,
        cores: Option<usize>,
        vcpus: usize,
    }
    let id = egui::Id::new("performance-engine-capabilities");
    let modified = manifest_modified(&path);
    let cached = match ui.data_mut(|data| data.get_temp::<Cached>(id)) {
        Some(cached)
            if cached.path == path
                && cached.state == draft.state
                && cached.modified == modified =>
        {
            cached
        }
        _ => {
            let cached = Cached {
                capabilities: manifest_capabilities(&path),
                path,
                state: draft.state.clone(),
                modified,
                cores: physical_cores(),
                vcpus: avd_vcpus(&Path::new(draft.state.trim()).join("avd/RustEmuDroid.avd")),
            };
            ui.data_mut(|data| data.insert_temp(id, cached.clone()));
            cached
        }
    };
    let fits = placement_fits(cached.cores, cached.vcpus);
    let capabilities = cached.capabilities;
    let warning = ui.visuals().warn_fg_color;
    let Some(capabilities) = capabilities else {
        ui.colored_label(
            warning,
            locale.pick(
                "O manifesto do motor não foi encontrado ou é inválido. O Android vai iniciar com o motor padrão.",
                "The engine manifest is missing or invalid. Android will start with the default engine.",
                "No se ha encontrado el manifiesto del motor o no es válido. Android se iniciará con el motor predeterminado.",
            ),
        );
        return;
    };
    let Some(options) = draft.performance_options_mut() else {
        return;
    };
    let mut offered = false;
    let listed = |tuning: Tuning| capabilities.contains(&tuning.capability());
    for tuning in Tuning::ALL {
        if matches!(tuning, Tuning::LapicFastpath | Tuning::KvmPvEoi) {
            continue; // shown with ClockSync
        }
        if tuning == Tuning::ClockSync {
            let group: Vec<_> = Tuning::CPU_VIRTUALIZATION
                .into_iter()
                .filter(|tuning| listed(*tuning))
                .collect();
            if group.is_empty() {
                continue;
            }
            offered = true;
            let (label, help) = tuning_text(tuning, locale);
            let mut enabled = group.iter().all(|tuning| tuning.enabled(options));
            if ui
                .checkbox(&mut enabled, label)
                .on_hover_text(help)
                .changed()
            {
                for tuning in Tuning::CPU_VIRTUALIZATION {
                    *tuning.enabled_mut(options) = enabled;
                }
            }
            continue;
        }
        if !listed(tuning) {
            continue;
        }
        offered = true;
        let (label, help) = tuning_text(tuning, locale);
        if tuning == Tuning::DedicatedVcpuCores && !fits {
            ui.add_enabled(false, egui::Checkbox::new(&mut false, label))
                .on_disabled_hover_text(help);
            ui.weak(format!(
                "{} {} · {}",
                locale.pick(
                    "Exige mais núcleos físicos que os processadores virtuais do perfil mais dois:",
                    "Needs more physical cores than the profile’s virtual processors plus two:",
                    "Requiere más núcleos físicos que los procesadores virtuales del perfil más dos:",
                ),
                cached
                    .cores
                    .map_or("?".to_owned(), |cores| cores.to_string()),
                cached.vcpus,
            ));
            continue;
        }
        ui.checkbox(tuning.enabled_mut(options), label)
            .on_hover_text(help);
        if tuning == Tuning::GuestKernelParameters {
            ui.colored_label(
                warning,
                locale.pick(
                    "Atenção: desliga as proteções contra falhas do processador e a limpeza de memória dentro do Android. Um app malicioso no Android fica mais perigoso. Use só em um perfil dedicado a jogos.",
                    "Warning: turns off processor vulnerability mitigations and memory clearing inside Android. A malicious Android app becomes more dangerous. Use it only on a profile dedicated to games.",
                    "Atención: desactiva las mitigaciones de vulnerabilidades del procesador y la limpieza de memoria dentro de Android. Una aplicación maliciosa de Android resulta más peligrosa. Úsalo solo en un perfil dedicado a juegos.",
                ),
            );
        }
    }
    if offered {
        theme::label(
            ui,
            locale.pick(
                "Depois do boot, a saída do launcher informa quais ajustes o motor confirmou.",
                "After startup, the launcher output lists the settings the engine confirmed.",
                "Tras el arranque, la salida del iniciador indica qué ajustes ha confirmado el motor.",
            ),
        );
    } else {
        ui.weak(locale.pick(
            "Este motor não oferece ajustes de desempenho.",
            "This engine offers no performance settings.",
            "Este motor no ofrece ajustes de rendimiento.",
        ));
    }
}

impl Launcher {
    pub(super) fn open_settings(&mut self) {
        self.adapters.request();
        self.images.sdk = None;
        self.settings = Some(self.config.clone());
    }

    pub(super) fn show_settings(&mut self, ctx: &egui::Context, frame: Option<&eframe::Frame>) {
        let Some(draft) = &mut self.settings else {
            return;
        };
        let locale = self.config.locale;
        let running = self.playing;
        let mut apply = false;
        let mut android_settings = false;
        let mut check_boot = false;
        let mut setup = false;
        #[cfg(windows)]
        let mut open_bluestacks = false;
        let base_state = self.prefs_file.parent().unwrap_or(Path::new("."));
        if self.images.sdk.as_deref() != Some(&draft.sdk) {
            self.images.refresh(&draft.sdk, base_state);
        }
        let modal_frame = egui::Frame::window(&ctx.style()).inner_margin(20);
        let dialog = egui::Modal::new("launcher-settings".into()).frame(modal_frame).show(ctx, |ui| {
            let width = (ctx.content_rect().width() - 72.).clamp(240., 580.);
            ui.set_width(width);
            theme::form(ui);
            ui.heading(RichText::new(locale.pick("Configurações do emulador", "Emulator settings", "Ajustes del emulador")).strong().size(22.));
            ui.add_space(6.);
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 158.).max(120.))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add_space(4.);
                    ui.label(RichText::new(locale.pick("Perfil do Android", "Android profile", "Perfil de Android")).strong());
                    let base_state = self.images.original_state.as_path();
                    let google = self.images.google_state.as_path();
                    ui.columns(2, |col| {
                        let original_desc = if self.images.missing_original { locale.pick("Imagem não instalada", "Image not installed", "Imagen sin instalar") } else { self.images.original.as_deref().unwrap_or_else(|| locale.pick("Imagem padrão", "Default image", "Imagen predeterminada")) };
                        if col[0].add_enabled_ui(!self.images.missing_original, |ui| theme::profile_card(ui, "Original", original_desc, Path::new(&draft.state) == base_state)).inner.clicked() {
                            draft.state = base_state.display().to_string();
                            draft.image = if base_state.join("avd/RustEmuDroid.ini").is_file() {
                                String::new()
                            } else {
                                self.images.portable_original.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
                            };
                        }
                        let desc = if self.images.play_ready {"Google Play Store"} else {locale.pick("Imagem não instalada", "Image not installed", "Imagen sin instalar")};
                        let pick = col[1].add_enabled_ui(self.images.play_ready, |ui|
                            theme::profile_card(ui, "Android 17", desc, Path::new(&draft.state) == google)).inner;
                        if pick.clicked() {
                            draft.state = google.display().to_string();
                            draft.image = if google.join("avd/RustEmuDroid.ini").is_file() {String::new()} else {self.images.google.display().to_string()};
                        }
                    });
                    theme::label(ui, locale.pick("Cada perfil tem seus próprios aplicativos e dados.", "Each profile has its own apps and data.", "Cada perfil tiene sus propias aplicaciones y datos."));
                    setup = ui.add(theme::button(super::runtime_setup::install_label(locale))).clicked();
                    ui.add_space(7.);
                    let language = theme::label(ui, locale.pick("Idioma", "Language", "Idioma"));
                    theme::select(ui, "launcher-language", draft.locale.name(), |ui| {
                        for language in Locale::ALL {
                            ui.selectable_value(&mut draft.locale, language, language.name());
                        }
                    }).labelled_by(language.id);
                    ui.add_space(7.);
                    ui.label(RichText::new(locale.pick("Gráficos", "Graphics", "Gráficos")).strong());
                    ui.columns(2, |col| {
                        let label = theme::label(&mut col[0], locale.pick("Renderização", "Rendering", "Renderizado"));
                        theme::select(&mut col[0], "render-mode", gpu_label(draft.gpu, locale), |ui| {
                            for gpu in [Gpu::Auto, Gpu::Host, Gpu::Software, Gpu::SwiftShader] {
                                ui.selectable_value(&mut draft.gpu, gpu, gpu_label(gpu, locale));
                            }
                        }).labelled_by(label.id);
                        let label = theme::label(&mut col[1], locale.pick("Placa de vídeo Vulkan", "Vulkan graphics card", "Tarjeta gráfica Vulkan"));
                        col[1].add_enabled_ui(matches!(draft.gpu, Gpu::Host | Gpu::Auto), |ui| {
                            let name = if draft.hardware.adapter.is_empty() {locale.pick("Seleção automática", "Automatic selection", "Selección automática").into()} else {draft.hardware.adapter.clone()};
                            theme::select(ui, "adapter", &name, |ui| {
                                ui.selectable_value(&mut draft.hardware.adapter, String::new(), locale.pick("Seleção automática", "Automatic selection", "Selección automática"));
                                for name in &self.adapters.names {
                                    ui.selectable_value(&mut draft.hardware.adapter, name.clone(), name);
                                }
                            }).labelled_by(label.id);
                        });
                    });
                    if self.adapters.pending.is_some() { ui.weak(locale.pick("Consultando placas de vídeo…", "Searching for graphics cards…", "Buscando tarjetas gráficas…")); }
                    else if self.adapters.error { ui.weak(locale.pick("Consulta às placas indisponível. A seleção automática continua disponível.", "Graphics card detection failed. Automatic selection is still available.", "No se han detectado las tarjetas gráficas. La selección automática sigue disponible.")); }
                    if self.images.supports_amd_uma(draft, base_state, google) {
                        let mut enabled = draft.amd_uma_vulkan();
                        if ui.checkbox(&mut enabled, locale.pick(
                            "Memória Vulkan compartilhada (Radeon 780M)",
                            "Shared Vulkan memory (Radeon 780M)",
                            "Memoria Vulkan compartida (Radeon 780M)",
                        )).on_hover_text(locale.pick(
                            "Opção experimental que pode corrigir falhas na alocação de memória Vulkan em jogos. Salve e reinicie o Android para aplicar.",
                            "Experimental option that may fix Vulkan memory allocation crashes in games. Save and restart Android to apply.",
                            "Opción experimental que puede corregir fallos de asignación de memoria Vulkan en juegos. Guarda y reinicia Android para aplicar el cambio.",
                        )).changed() {
                            draft.set_amd_uma_vulkan(enabled);
                        }
                    }
                    ui.checkbox(&mut draft.adaptive_video, locale.pick(
                        "Ajustar o vídeo ao tamanho da janela",
                        "Fit video frames to the window",
                        "Ajustar el vídeo al tamaño de la ventana",
                    )).on_hover_text(locale.pick(
                        "Reduz a transferência de pixels em janelas menores. O jogo, as capturas e o replay usam a resolução do Android.",
                        "Transfers fewer pixels in smaller windows. Games, captures, and replay use the Android resolution.",
                        "Transfiere menos píxeles en ventanas pequeñas. Los juegos, las capturas y la repetición usan la resolución de Android.",
                    ));
                    let label = theme::label(ui, locale.pick("Taxa de atualização do Android", "Android refresh rate", "Frecuencia de actualización de Android"));
                    let default_rate = locale.pick("Usar a taxa do perfil", "Use profile refresh rate", "Usar la frecuencia del perfil");
                    let selected_rate = draft.hardware.refresh_rate.map(|hz| format!("{hz} Hz")).unwrap_or_else(|| default_rate.to_owned());
                    theme::select(ui, "android-refresh-rate", &selected_rate, |ui| {
                        let monitor = current_monitor_rate(frame);
                        if let Some(hz) = monitor {
                            ui.weak(format!("{} · {hz} Hz", locale.pick("Monitor atual", "Current monitor", "Monitor actual")));
                        }
                        ui.selectable_value(&mut draft.hardware.refresh_rate, None, default_rate);
                        let mut rates = vec![30, 60, 75, 90, 100, 120, 144, 165, 240];
                        rates.extend(monitor);
                        rates.sort_unstable();
                        rates.dedup();
                        for hz in rates.into_iter().take_while(|&hz| monitor.is_none_or(|limit| hz <= limit)) {
                            ui.selectable_value(&mut draft.hardware.refresh_rate, std::num::NonZeroU16::new(hz), format!("{hz} Hz"));
                        }
                    }).labelled_by(label.id);
                    if draft.hardware.refresh_rate.is_some_and(|hz| hz.get() > 60) {
                        theme::label(ui, locale.pick(
                            "Use até a taxa atual do monitor. Durante a sessão, o emulador solicita essa taxa ao Android e tenta liberar jogos acima de 60 Hz. Cada jogo define seu próprio limite.",
                            "Choose a rate your monitor currently supports. For this session the emulator asks Android to use it and allow games above 60 Hz. Each game sets its own limit.",
                            "Elige una frecuencia que el monitor admita actualmente. Durante la sesión, el emulador solicita esa frecuencia a Android e intenta habilitar más de 60 Hz para los juegos. Cada juego fija su propio límite.",
                        ));
                    }
                    let sources = [
                        locale.pick("Desligado", "Off", "Desactivado"),
                        locale.pick("Painel Rust", "Rust panel", "Panel Rust"),
                        locale.pick("Superfície do jogo no Android", "Android game surface", "Superficie del juego en Android"),
                    ];
                    let mut source = if !draft.show_fps { 0 } else if draft.android_fps { 2 } else { 1 };
                    ui.horizontal(|ui| {
                        let label = theme::label(ui, locale.pick("Contador de FPS", "FPS counter", "Contador de FPS"));
                        theme::select(ui, "fps-source", sources[source], |ui| {
                            for (index, label) in sources.into_iter().enumerate() {
                                ui.selectable_value(&mut source, index, label);
                            }
                        }).labelled_by(label.id);
                    });
                    draft.show_fps = source != 0;
                    draft.android_fps = source == 2;
                    ui.checkbox(&mut draft.animate_mascot, locale.pick(
                        "Animar o mascote",
                        "Animate the mascot",
                        "Animar la mascota",
                    )).on_hover_text(locale.pick(
                        "Respeita a preferência de movimento reduzido do sistema.",
                        "Respects your system’s reduced-motion setting.",
                        "Respeta la preferencia de movimiento reducido del sistema.",
                    ));
                    ui.add_space(6.);
                    ui.label(RichText::new(locale.pick("Câmera", "Camera", "Cámara")).strong());
                    ui.columns(2, |col| {
                        let label = theme::label(&mut col[0], locale.pick("Origem da imagem", "Camera source", "Fuente de la cámara"));
                        theme::select(&mut col[0], "camera", camera_label(draft.hardware.camera, locale), |ui| {
                            for camera in [Camera::Profile, Camera::EmulatedFront, Camera::WebcamFront, Camera::WebcamBack] {
                                ui.selectable_value(&mut draft.hardware.camera, camera, camera_label(camera, locale));
                            }
                        }).labelled_by(label.id);
                        if matches!(draft.hardware.camera, Camera::WebcamFront | Camera::WebcamBack) {
                            let label = theme::label(&mut col[1], locale.pick("Índice da webcam (0 = primeira)", "Webcam index (0 = first)", "Índice de la cámara web (0 = primera)"));
                            col[1].add(egui::DragValue::new(&mut draft.hardware.webcam_index).range(0..=255)).labelled_by(label.id);
                        }
                    });
                    ui.add_space(6.);
                    ui.separator();
                    if self.images.has_berberis_image && Path::new(&draft.state) == base_state {
                        egui::CollapsingHeader::new(locale.pick("Compatibilidade de jogos", "Game compatibility", "Compatibilidad de juegos")).id_salt("game-compatibility").show(ui, |ui| {
                            ui.checkbox(&mut draft.hardware.arm_lite, locale.pick("Tradução ARM leve para PUBG", "Lightweight ARM translation for PUBG", "Traducción ARM ligera para PUBG"));
                            theme::label(ui, locale.pick("No Android 16 do Kemu, vale para todos os apps ARM deste perfil e pode alterar o FPS.", "On Kemu's Android 16, this applies to all ARM apps in this profile and can affect FPS.", "En Android 16 de Kemu se aplica a todas las aplicaciones ARM de este perfil y puede afectar a los FPS."));
                        });
                    }
                    egui::CollapsingHeader::new(locale.pick("Runtime, imagem e perfil", "Runtime, image, and profile", "Entorno, imagen y perfil")).id_salt("advanced-paths").show(ui, |ui| {
                        path_input(ui, locale.pick("Pasta do runtime (vazio = automático)", "Runtime folder (empty = automatic)", "Carpeta del entorno (vacía = automático)"), &mut draft.sdk);
                        path_input(ui, locale.pick("Imagem de sistema", "System image", "Imagen del sistema"), &mut draft.image);
                        path_input(ui, locale.pick("Diretório do perfil", "Profile folder", "Carpeta del perfil"), &mut draft.state);
                        #[cfg(windows)]
                        vulkan_cache_runtime_picker(ui, frame, draft, locale);
                        #[cfg(windows)]
                        performance_engine_picker(ui, frame, draft, locale);
                        #[cfg(windows)]
                        performance_power_toggle(ui, draft, locale);
                    });
                    if !self.output.is_empty() {
                        egui::CollapsingHeader::new(locale.pick("Saída do launcher", "Launcher output", "Salida del iniciador")).id_salt("launcher-output").show(ui, |ui| {
                            for line in &self.output {
                                ui.monospace(line);
                            }
                        });
                    }
                    #[cfg(windows)]
                    { open_bluestacks = self.blue.entry_requested(ui, locale); }
                    if running {
                        android_settings = ui
                            .add_enabled(
                                self.stage == BootStage::Booted
                                    && self.controls.as_ref().is_some_and(|c| !c.busy()),
                                theme::button(locale.pick("Abrir configurações do Android", "Open Android settings", "Abrir ajustes de Android")),
                            )
                            .clicked();
                    } else if !draft.boot_changed(&self.config) {
                        check_boot = ui
                            .add_enabled(self.task.is_none(), theme::button(locale.pick("Verificar boot", "Verify startup", "Comprobar el inicio")))
                            .clicked();
                    }
                });
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if running && draft.boot_changed(&self.config) {locale.pick("Salvar e reiniciar Android", "Save and restart Android", "Guardar y reiniciar Android")} else {locale.pick("Salvar", "Save", "Guardar")};
                apply = theme::primary(ui, !draft.state.trim().is_empty() && (self.task.is_none() || running), label).clicked();
                if ui.add(theme::button(locale.pick("Cancelar", "Cancel", "Cancelar"))).clicked() { ui.close(); }
            });
        });
        if apply {
            let changed = draft.boot_changed(&self.config);
            if let Err(error) = write_json(&self.prefs_file, draft) {
                self.status = error.to_string();
                self.failed = true;
                self.settings = None;
                return;
            }
            let language_changed = self.config.locale != draft.locale;
            self.config = draft.clone();
            if language_changed && let Some(controls) = &mut self.controls {
                controls.set_locale(self.config.locale);
                if !changed && self.stage == BootStage::Booted {
                    controls.send(Action::SetHomeLocale(self.config.locale), ctx);
                }
            }
            if changed && running {
                self.restart = true;
                self.request_stop();
            }
        }
        if apply || dialog.should_close() || android_settings || check_boot {
            self.settings = None;
        }
        #[cfg(windows)]
        if open_bluestacks {
            self.blue.open();
        }
        if android_settings && let Some(c) = &mut self.controls {
            c.send(Action::AndroidSettings, ctx);
        }
        if check_boot {
            self.start(true);
        }
        if setup {
            self.open_runtime_setup();
        }
    }
}
