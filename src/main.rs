mod android;
#[cfg(feature = "ui")]
mod controls;
#[cfg(feature = "ui")]
mod display;
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

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Parser)]
#[command(version, about = "Launcher Android isolado para Windows e Linux")]
struct Cli {
    /// Diretório raiz do Android SDK. Também lê ANDROID_HOME.
    #[arg(long, global = true)]
    sdk: Option<PathBuf>,

    /// Diretório dos dados do protótipo.
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,

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

    /// Localiza as ferramentas e verifica o hipervisor.
    Doctor,

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

        /// Tempo máximo de espera pelo Android.
        #[arg(long, default_value_t = 180, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,
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

fn run() -> Result<()> {
    let cli = Cli::parse();
    #[cfg(feature = "ui")]
    if matches!(cli.command, Action::GpuInventory) {
        return ui::print_gpu_inventory();
    }
    let state = cli
        .state_dir
        .unwrap_or_else(|| PathBuf::from(".rustemudroid").join(std::env::consts::OS));
    let state = absolute(state)?;
    #[cfg(feature = "ui")]
    if let Action::Ui { play, gpu } = &cli.command {
        return ui::run(cli.sdk, state, *play, *gpu);
    }

    let sdk = Sdk::find(cli.sdk)?;
    let profile = Profile::new(state);

    match cli.command {
        #[cfg(feature = "ui")]
        Action::Ui { .. } | Action::GpuInventory => unreachable!(),
        Action::Doctor => {
            println!("SDK: {}", sdk.root.display());
            println!("ADB: {}", sdk.adb.display());
            println!("Dados: {}", profile.root.display());
            sdk.check_acceleration()?;
        }
        Action::Init { image, system_dir } => {
            let image = match system_dir {
                Some(path) => SystemImage::from_dir(path)?,
                None => match image {
                    Some(package) => SystemImage::from_sdk(&sdk, &package)?,
                    None => SystemImage::preferred_sdk(&sdk)?,
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
            timeout,
        } => android::boot(
            &sdk,
            &profile,
            android::BootOptions {
                gpu,
                hardware,
                engine,
                headless,
                check,
                embedded: false,
                timeout: Duration::from_secs(timeout),
            },
            &std::sync::atomic::AtomicBool::new(false),
            |event| println!("{event}"),
        )?,
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("RustEmuDroid: {error}");
            ExitCode::FAILURE
        }
    }
}
