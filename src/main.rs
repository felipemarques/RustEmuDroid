mod android;
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
    Ui,

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
        /// Executa sem abrir a janela do Android Emulator.
        #[arg(long)]
        headless: bool,

        /// Encerra a VM após confirmar o boot.
        #[arg(long)]
        check: bool,

        /// Modo de renderização do Android Emulator.
        #[arg(long, value_enum, default_value_t = Gpu::Auto)]
        gpu: Gpu,

        /// Tempo máximo de espera pelo Android.
        #[arg(long, default_value_t = 180, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,
    },
}

#[derive(Clone, Copy, PartialEq, ValueEnum)]
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
    let state = cli
        .state_dir
        .unwrap_or_else(|| PathBuf::from(".rustemudroid").join(std::env::consts::OS));
    let state = absolute(state)?;
    #[cfg(feature = "ui")]
    if matches!(&cli.command, Action::Ui) {
        return ui::run(cli.sdk, state);
    }

    let sdk = Sdk::find(cli.sdk)?;
    let profile = Profile::new(state);

    match cli.command {
        #[cfg(feature = "ui")]
        Action::Ui => unreachable!(),
        Action::Doctor => {
            println!("SDK: {}", sdk.root.display());
            println!("ADB: {}", sdk.adb.display());
            println!("Dados: {}", profile.root.display());
            sdk.check_acceleration()?;
        }
        Action::Init { image, system_dir } => {
            let image = match system_dir {
                Some(path) => SystemImage::from_dir(path)?,
                None => SystemImage::from_sdk(
                    &sdk,
                    image
                        .as_deref()
                        .unwrap_or("system-images;android-35;google_apis;x86_64"),
                )?,
            };
            profile.init(image)?;
        }
        Action::Boot {
            headless,
            check,
            gpu,
            timeout,
        } => android::boot(
            &sdk,
            &profile,
            gpu.as_str(),
            headless,
            check,
            Duration::from_secs(timeout),
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
