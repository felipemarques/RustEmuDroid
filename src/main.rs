mod android;
mod audio_output;
#[cfg(windows)]
mod bluestacks;
#[cfg(feature = "ui")]
mod capture;
mod cli_i18n;
#[cfg(feature = "ui")]
mod controls;
#[cfg(feature = "ui")]
mod display;
mod locale;
mod runtime_install;
#[cfg(feature = "ui")]
mod surface_fps;
#[cfg(feature = "ui")]
mod toolbar;
#[cfg(feature = "ui")]
mod ui;

use std::{
    error::Error,
    path::{PathBuf, absolute},
    process::ExitCode,
    time::Duration,
};

use android::{Profile, Sdk, SystemImage};
use clap::{Parser, Subcommand, ValueEnum};
use locale::Locale;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Diretório raiz do Android SDK. Também lê ANDROID_HOME.
    #[arg(long, global = true)]
    sdk: Option<PathBuf>,

    /// Diretório dos dados do protótipo.
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,

    #[arg(long, global = true, value_enum, ignore_case = true)]
    lang: Option<Locale>,

    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Abre o painel do RustEmuDroid.
    #[cfg(feature = "ui")]
    Ui {
        /// Compatibilidade com atalhos antigos. A interface já inicia o Android ao abrir.
        #[arg(long)]
        play: bool,
        /// Modo de renderização inicial do painel.
        #[arg(long, value_enum)]
        gpu: Option<Gpu>,
    },
    #[cfg(feature = "ui")]
    #[command(hide = true)]
    GpuInventory,

    /// Abre uma instância ou jogo no BlueStacks 5 para Windows.
    #[cfg(windows)]
    #[command(name = "bluestacks", visible_alias = "blue-stacks")]
    BlueStacks {
        #[command(subcommand)]
        action: BlueStacksAction,
    },

    /// Localiza as ferramentas e verifica o hipervisor.
    Doctor,
    /// Lista as saídas de áudio do computador para boot --audio-output.
    AudioOutputs,
    /// Prepara o runtime Android gerenciado pelo RustEmuDroid.
    RuntimeSetup(runtime_install::SetupArgs),

    /// Cria um AVD local a partir de uma imagem já instalada.
    Init {
        /// Identificador de pacote usado pelo sdkmanager.
        #[arg(long, conflicts_with = "system_dir")]
        image: Option<String>,

        /// Diretório x86-64 com system.img e build.prop, como system-img do Kemu.
        #[arg(long)]
        system_dir: Option<PathBuf>,
    },

    /// Inicia o Android e confirma o boot pelo ADB.
    Boot {
        #[command(flatten)]
        hardware: android::Hardware,
        /// Executa sem abrir a janela do Android Emulator.
        #[arg(long)]
        headless: bool,

        /// Encerra a VM após confirmar o boot.
        #[arg(long)]
        check: bool,

        /// Modo de renderização do Android Emulator.
        #[arg(long, value_enum, default_value_t = Gpu::Auto)]
        gpu: Gpu,

        /// Motor da VM. Auto usa o motor correspondente à imagem do Kemu no Windows.
        #[arg(long, value_enum, default_value_t = Engine::Auto)]
        engine: Engine,

        /// Motor de cache Vulkan preparado para Windows.
        #[arg(long, value_name = "DIR")]
        vulkan_cache_runtime: Option<PathBuf>,

        /// Motor de desempenho para Windows com rustemudroid-engine.json (predefinição padrão).
        #[arg(long, value_name = "DIR", conflicts_with = "vulkan_cache_runtime")]
        performance_engine: Option<PathBuf>,

        /// Tempo máximo de espera pelo Android.
        #[arg(long, default_value_t = 180, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,
    },
}

#[cfg(windows)]
#[derive(Subcommand)]
enum BlueStacksAction {
    /// Exibe a instância na janela do RustEmuDroid.
    #[cfg(feature = "ui")]
    Mirror {
        #[arg(long)]
        instance: String,
        #[arg(long)]
        package: Option<String>,
    },
    /// Lista as instâncias configuradas.
    List,
    /// Abre uma instância ou solicita a execução de um jogo já instalado.
    Open {
        /// Identificador mostrado por bluestacks list.
        #[arg(long)]
        instance: Option<String>,
        /// Nome do pacote Android, como org.supertuxkart.stk.
        #[arg(long)]
        package: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum Engine {
    Auto,
    Sdk,
}

#[derive(Clone, Copy, PartialEq, ValueEnum)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
enum Gpu {
    Auto,
    Host,
    Software,
    #[value(name = "swiftshader")]
    SwiftShader,
}

impl Gpu {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Host => "host",
            Self::Software => "software",
            Self::SwiftShader => "swiftshader",
        }
    }
}

fn default_state_dir() -> PathBuf {
    PathBuf::from(".rustemudroid").join(std::env::consts::OS)
}

fn run(cli: Cli) -> Result<()> {
    let language = cli.lang.unwrap_or_default();
    #[cfg(windows)]
    if let Action::BlueStacks { action } = &cli.command {
        match action {
            #[cfg(feature = "ui")]
            BlueStacksAction::Mirror { instance, package } => {
                let state = absolute(cli.state_dir.unwrap_or_else(default_state_dir))?;
                return ui::run(
                    cli.sdk,
                    state,
                    ui::Startup::BlueStacks(instance.clone(), package.clone()),
                    None,
                    cli.lang,
                );
            }
            BlueStacksAction::List => {
                for instance in
                    bluestacks::instances().map_err(|error| error.localized(language))?
                {
                    println!("{}\t{}", instance.id, instance.name);
                }
            }
            BlueStacksAction::Open { instance, package } => {
                bluestacks::launch(instance.as_deref(), package.as_deref())
                    .map_err(|error| error.localized(language))?;
            }
        }
        return Ok(());
    }
    #[cfg(feature = "ui")]
    if matches!(cli.command, Action::GpuInventory) {
        return ui::print_gpu_inventory();
    }
    if let Action::RuntimeSetup(args) = &cli.command {
        return runtime_install::command(args, language);
    }
    if matches!(cli.command, Action::AudioOutputs) {
        let inventory = audio_output::inventory(language)?;
        let marker = language.pick(
            "padrão do sistema",
            "system default",
            "predeterminada del sistema",
        );
        for device in &inventory.devices {
            let default = inventory.default.as_deref() == Some(device.id.as_str());
            println!(
                "{}\t{}{}",
                device.id,
                device.name,
                if default {
                    format!("\t({marker})")
                } else {
                    String::new()
                }
            );
        }
        return Ok(());
    }
    if let Action::Boot {
        hardware, headless, ..
    } = &cli.command
    {
        // Asked for on the command line: refuse it rather than boot without it.
        audio_output::check_explicit(
            &hardware.audio_output,
            || audio_output::inventory(language),
            audio_output::Mode {
                windows: cfg!(windows),
                embedded: false,
                headless: *headless,
                engine_sound: !*headless,
            },
            language,
        )
        .map_err(UsageError)?;
    }
    let state = cli.state_dir.unwrap_or_else(default_state_dir);
    let state = absolute(state)?;
    #[cfg(feature = "ui")]
    if let Action::Ui { gpu, .. } = &cli.command {
        return ui::run(cli.sdk, state, ui::Startup::Android, *gpu, cli.lang);
    }

    let sdk = Sdk::find_localized(cli.sdk, language)?;
    let profile = Profile::localized(state, language);

    match cli.command {
        Action::RuntimeSetup(_) | Action::AudioOutputs => unreachable!(),
        #[cfg(windows)]
        Action::BlueStacks { .. } => unreachable!(),
        #[cfg(feature = "ui")]
        Action::Ui { .. } | Action::GpuInventory => unreachable!(),
        Action::Doctor => {
            let runtime = if sdk.is_standalone_kemu() {
                "Kemu"
            } else if sdk.is_portable() {
                language.pick("Portátil", "Portable", "Portátil")
            } else {
                "SDK"
            };
            println!("{runtime}: {}", sdk.root.display());
            let engine = language.pick("Motor", "Engine", "Motor");
            println!("{engine}: {}", sdk.engine_path().display());
            println!("ADB: {}", sdk.adb.display());
            let label = language.pick("Dados", "Data", "Datos");
            println!("{label}: {}", profile.root.display());
            if let Some(advice) = android::amd_linux_advice(language) {
                println!("{advice}");
            }
            sdk.check_acceleration()?;
        }
        Action::Init { image, system_dir } => {
            let image = match system_dir {
                Some(path) => {
                    sdk.require_matching_image(&path)?;
                    SystemImage::from_dir_localized(path, language)?
                }
                None => match image {
                    Some(package) => SystemImage::from_sdk(&sdk, &package)?,
                    None => SystemImage::preferred(&sdk)?,
                },
            };
            profile.init(image)?;
        }
        Action::Boot {
            hardware,
            headless,
            check,
            gpu,
            engine,
            vulkan_cache_runtime,
            performance_engine,
            timeout,
        } => android::boot(
            &sdk,
            &profile,
            android::BootOptions {
                gpu,
                hardware,
                amd_uma_vulkan: false,
                vulkan_cache_runtime,
                performance_engine: performance_engine.map(|path| android::PerformanceEngine {
                    path,
                    options: Default::default(),
                }),
                performance_power: false,
                engine,
                headless,
                check,
                embedded: false,
                engine_panel: false,
                timeout: Duration::from_secs(timeout),
            },
            &std::sync::atomic::AtomicBool::new(false),
            |event| println!("{event}"),
        )?,
    }

    Ok(())
}

/// An option that parsed but cannot be honoured on this computer or in this
/// mode. It ends the process like a command-line error.
#[derive(Debug)]
struct UsageError(String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for UsageError {}

/// 2 for a usage error, as for an option Clap rejects; 1 for any other failure.
fn failure_code(error: &(dyn Error + 'static)) -> u8 {
    if error.is::<UsageError>() { 2 } else { 1 }
}

fn main() -> ExitCode {
    let cli = match cli_i18n::parse() {
        Ok(cli) => cli,
        Err(status) => return status,
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("RustEmuDroid: {error}");
            ExitCode::from(failure_code(error.as_ref()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot(arguments: &[&str]) -> Cli {
        Cli::try_parse_from(
            ["rustemudroid", "--state-dir", "unused", "boot"]
                .iter()
                .chain(arguments),
        )
        .unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn an_audio_output_that_cannot_be_honoured_is_a_usage_error() {
        // Refused before the runtime is looked for, with the exit code of a
        // command-line error. No device needs to be listed for these two.
        let headless = run(boot(&["--headless", "--audio-output", "Headphones"])).unwrap_err();
        assert_eq!(failure_code(headless.as_ref()), 2);
        assert!(headless.to_string().contains("--headless"));
        if cfg!(windows) {
            let windows = run(boot(&["--audio-output", "Headphones"])).unwrap_err();
            assert_eq!(failure_code(windows.as_ref()), 2);
            assert!(windows.to_string().contains("Windows"));
        }
        let other: Box<dyn Error> = "Emulator exited before boot".into();
        assert_eq!(failure_code(other.as_ref()), 1);
    }
}
