use std::{ffi::OsString, process::ExitCode};

use clap::{
    Command, CommandFactory, FromArgMatches, ValueEnum,
    error::{ContextKind, ErrorKind},
};

use crate::{Cli, locale::Locale};

pub(crate) fn parse() -> Result<Cli, ExitCode> {
    parse_args(std::env::args_os().collect()).map_err(|(error, locale)| {
        let code = if error.use_stderr() { 2 } else { 0 };
        let message = error_text(&error, locale);
        if error.use_stderr() {
            eprint!("{message}");
        } else {
            print!("{message}");
        }
        ExitCode::from(code)
    })
}

fn parse_args(args: Vec<OsString>) -> Result<Cli, (clap::Error, Locale)> {
    let locale = command_locale(&args);
    // A file manager starts the desktop build without command-line arguments.
    let desktop_start = (cfg!(feature = "ui") && args.len() == 1).then(|| OsString::from("ui"));
    let mut matches = cli_command(locale)
        .try_get_matches_from(args.into_iter().chain(desktop_start))
        .map_err(|e| (e, locale))?;
    Cli::from_arg_matches_mut(&mut matches).map_err(|e| (e, locale))
}

#[cfg(all(test, feature = "ui"))]
#[test]
fn desktop_binary_opens_ui_without_arguments_and_keeps_cli_flags() {
    let parse = |args: &[&str]| parse_args(args.iter().map(OsString::from).collect());
    assert!(matches!(
        parse(&["rustemudroid"]),
        Ok(Cli {
            command: crate::Action::Ui { .. },
            ..
        })
    ));
    assert!(matches!(
        parse(&["rustemudroid", "--version"]),
        Err((error, _)) if error.kind() == ErrorKind::DisplayVersion
    ));
    assert!(matches!(
        parse(&["rustemudroid", "doctor"]),
        Ok(Cli {
            command: crate::Action::Doctor,
            ..
        })
    ));
}

fn command_locale(args: &[OsString]) -> Locale {
    let mut locale = Locale::default();
    let mut pending = false;
    for raw in args.iter().skip(1) {
        let arg = raw.to_str().unwrap_or_default();
        if arg == "--" {
            break;
        }
        // Parsing for help must work even when Clap stops at --help before reading
        // global arguments written after the subcommand or help flag.
        let tag = if pending {
            Some(arg)
        } else {
            arg.strip_prefix("--lang=")
        };
        pending = arg == "--lang";
        if let Some(tag) = tag
            && let Ok(value) = Locale::from_str(tag, true)
        {
            locale = value;
        }
    }
    locale
}

fn cli_command(l: Locale) -> Command {
    let command = root_help(Cli::command(), l)
        .about(l.pick(
            "Launcher Android isolado para Windows e Linux",
            "Isolated Android launcher for Windows and Linux",
            "Lanzador de Android aislado para Windows y Linux",
        ))
        .mut_subcommand("doctor", |cmd| {
            cmd.about(l.pick(
                "Localiza as ferramentas e verifica o hipervisor.",
                "Find the tools and check the hypervisor.",
                "Localiza las herramientas y comprueba el hipervisor.",
            ))
        })
        .mut_subcommand("init", |cmd| {
            init_help(cmd, l).about(l.pick(
                "Cria um AVD local a partir de uma imagem já instalada.",
                "Create a local AVD from an installed image.",
                "Crea un AVD local a partir de una imagen instalada.",
            ))
        })
        .mut_subcommand("boot", |cmd| {
            boot_help(cmd, l).about(l.pick(
                "Inicia o Android e confirma o boot pelo ADB.",
                "Start Android and verify boot through ADB.",
                "Inicia Android y confirma el arranque mediante ADB.",
            ))
        });
    #[cfg(feature = "ui")]
    let command = command.mut_subcommand("ui", |cmd| {
        ui_help(cmd, l).about(l.pick(
            "Abre o painel do RustEmuDroid.",
            "Open the RustEmuDroid desktop window.",
            "Abre la ventana de RustEmuDroid.",
        ))
    });
    #[cfg(windows)]
    let command = command.mut_subcommand("bluestacks", |cmd| {
        cmd.about(l.pick(
            "Abre uma instância ou um jogo no BlueStacks 5.",
            "Open an instance or game in BlueStacks 5.",
            "Abre una instancia o un juego en BlueStacks 5.",
        ))
        .mut_subcommand("list", |sub| {
            sub.about(l.pick(
                "Lista as instâncias configuradas.",
                "List configured instances.",
                "Enumera las instancias configuradas.",
            ))
        })
        .mut_subcommand("open", |sub| {
            open_help(sub, l).about(l.pick(
                "Abre uma instância ou um jogo já instalado.",
                "Open an instance or an installed game.",
                "Abre una instancia o un juego instalado.",
            ))
        })
    });
    command
}

fn arguments<const N: usize>(mut cmd: Command, help: [(&'static str, &'static str); N]) -> Command {
    for (name, text) in help {
        cmd = cmd.mut_arg(name, |arg| arg.help(text));
    }
    cmd
}

fn root_help(command: Command, l: Locale) -> Command {
    arguments(command, [
            ("sdk", l.pick(
                "Pasta com emulator e platform-tools. Omita para detecção automática.",
                "Directory containing emulator and platform-tools. Omit for automatic detection.",
                "Carpeta con emulator y platform-tools. Omítela para la detección automática.",
            )),
            ("state_dir", l.pick(
                "Diretório de dados do protótipo.",
                "Prototype data directory.",
                "Directorio de datos del prototipo.",
            )),
            ("lang", l.pick(
                "Idioma da interface e da linha de comando. Padrão do CLI: pt-br.",
                "Desktop and command-line language. CLI default: pt-br.",
                "Idioma de la interfaz y de la línea de comandos. Predeterminado en CLI: pt-br.",
            )),
    ])
}

#[cfg(feature = "ui")]
fn ui_help(command: Command, l: Locale) -> Command {
    arguments(command, [
            ("play", l.pick(
                "Compatibilidade com atalhos antigos. O Android já inicia ao abrir a interface.",
                "Compatibility with old shortcuts. Android already starts when the window opens.",
                "Compatibilidad con accesos directos antiguos. Android ya se inicia al abrir la ventana.",
            )),
            ("gpu", l.pick(
                "Modo de renderização inicial do painel.",
                "Initial desktop rendering mode.",
                "Modo de renderizado inicial de la interfaz.",
            )),
    ])
}

fn init_help(command: Command, l: Locale) -> Command {
    arguments(command, [
            ("image", l.pick(
                "Identificador de pacote usado pelo sdkmanager.",
                "Package identifier used by sdkmanager.",
                "Identificador del paquete que utiliza sdkmanager.",
            )),
            ("system_dir", l.pick(
                "Diretório x86-64 com system.img e build.prop, como system-img do Kemu.",
                "x86-64 directory containing system.img and build.prop, such as Kemu's system-img.",
                "Directorio x86-64 con system.img y build.prop, como system-img de Kemu.",
            )),
    ])
}

fn boot_help(command: Command, l: Locale) -> Command {
    arguments(command, [
            ("adapter", l.pick(
                "Nome da placa Vulkan para os modos host e auto.",
                "Vulkan adapter name for the host and auto modes.",
                "Nombre de la tarjeta Vulkan para los modos host y auto.",
            )),
            ("camera", l.pick(
                "Origem da câmera do Android.",
                "Android camera source.",
                "Origen de la cámara de Android.",
            )),
            ("webcam_index", l.pick(
                "Índice da webcam do computador.",
                "Index of the computer's webcam.",
                "Índice de la cámara web del equipo.",
            )),
            ("arm_lite", l.pick(
                "Tradução ARM leve para apps na imagem Android 16 do Kemu com Berberis.",
                "Light ARM translation for apps on Kemu's Android 16 image with Berberis.",
                "Traducción ARM ligera para aplicaciones en la imagen Android 16 de Kemu con Berberis.",
            )),
            ("headless", l.pick(
                "Executa sem abrir a janela do Android Emulator.",
                "Runs without opening an Android Emulator window.",
                "Se ejecuta sin abrir la ventana de Android Emulator.",
            )),
            ("check", l.pick(
                "Encerra a VM após confirmar o boot.",
                "Stops the VM after confirming that Android has booted.",
                "Detiene la máquina virtual tras confirmar que Android ha arrancado.",
            )),
            ("gpu", l.pick(
                "Modo de renderização do Android Emulator.",
                "Android Emulator rendering mode.",
                "Modo de renderizado de Android Emulator.",
            )),
            ("engine", l.pick(
                "Motor da VM. O modo auto usa o motor correspondente à imagem do Kemu no Windows.",
                "VM engine. Auto uses the matching engine for the Kemu image on Windows.",
                "Motor de la máquina virtual. Auto utiliza el motor correspondiente a la imagen de Kemu en Windows.",
            )),
            ("timeout", l.pick(
                "Tempo máximo de espera pelo Android.",
                "Maximum time to wait for Android.",
                "Tiempo máximo de espera de Android.",
            )),
    ])
}

#[cfg(windows)]
fn open_help(command: Command, l: Locale) -> Command {
    arguments(
        command,
        [
            (
                "instance",
                l.pick(
                    "Identificador mostrado por bluestacks list.",
                    "Instance identifier shown by bluestacks list.",
                    "Identificador de instancia que muestra bluestacks list.",
                ),
            ),
            (
                "package",
                l.pick(
                    "Pacote Android, como org.supertuxkart.stk.",
                    "Android package name, such as org.supertuxkart.stk.",
                    "Nombre del paquete de Android, como org.supertuxkart.stk.",
                ),
            ),
        ],
    )
}

fn error_text(error: &clap::Error, l: Locale) -> String {
    if l == Locale::EnUs {
        return error.render().to_string();
    }
    if matches!(
        error.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            | ErrorKind::DisplayVersion
    ) {
        // Keep generated command names and syntax, and translate Clap's built-in help text.
        let headings = [
            ("Usage:", l.pick("Uso:", "Usage:", "Uso:")),
            ("Commands:", l.pick("Comandos:", "Commands:", "Comandos:")),
            ("Options:", l.pick("Opções:", "Options:", "Opciones:")),
            (
                "Arguments:",
                l.pick("Argumentos:", "Arguments:", "Argumentos:"),
            ),
            (
                "Print help",
                l.pick("Mostra a ajuda", "Print help", "Muestra la ayuda"),
            ),
            (
                "Print version",
                l.pick("Mostra a versão", "Print version", "Muestra la versión"),
            ),
            (
                "Print this message or the help of the given subcommand(s)",
                l.pick(
                    "Mostra esta mensagem ou a ajuda dos subcomandos",
                    "Print this message or the help of the given subcommand(s)",
                    "Muestra este mensaje o la ayuda de los subcomandos",
                ),
            ),
            (
                "[possible values:",
                l.pick(
                    "[valores possíveis:",
                    "[possible values:",
                    "[valores posibles:",
                ),
            ),
            (
                "[default:",
                l.pick("[padrão:", "[default:", "[valor predeterminado:"),
            ),
        ];
        return headings.into_iter().fold(
            error.render().to_string(),
            |message, (original, translated)| message.replace(original, translated),
        );
    }

    let summary = match error.kind() {
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => {
            l.pick("Valor inválido.", "Invalid value.", "Valor no válido.")
        }
        ErrorKind::UnknownArgument => l.pick(
            "Opção desconhecida.",
            "Unknown option.",
            "Opción desconocida.",
        ),
        ErrorKind::InvalidSubcommand => l.pick(
            "Comando desconhecido.",
            "Unknown command.",
            "Comando desconocido.",
        ),
        ErrorKind::ArgumentConflict => l.pick(
            "As opções não podem ser usadas juntas.",
            "The options cannot be used together.",
            "Las opciones no se pueden utilizar juntas.",
        ),
        ErrorKind::MissingSubcommand => l.pick(
            "Informe um comando.",
            "A command is required.",
            "Indica un comando.",
        ),
        ErrorKind::MissingRequiredArgument => l.pick(
            "Falta uma opção obrigatória.",
            "A required option is missing.",
            "Falta una opción obligatoria.",
        ),
        ErrorKind::TooFewValues
        | ErrorKind::WrongNumberOfValues
        | ErrorKind::TooManyValues
        | ErrorKind::NoEquals => l.pick(
            "Confira o formato e a quantidade de valores da opção.",
            "Check the format and number of values for the option.",
            "Comprueba el formato y el número de valores de la opción.",
        ),
        _ => return error.render().to_string(),
    };
    let mut message = format!("RustEmuDroid: {summary}\n");
    for (context, label) in [
        (ContextKind::InvalidArg, l.pick("Opção", "Option", "Opción")),
        (
            ContextKind::PriorArg,
            l.pick("Outra opção", "Other option", "Otra opción"),
        ),
        (
            ContextKind::InvalidSubcommand,
            l.pick("Comando", "Command", "Comando"),
        ),
        (ContextKind::InvalidValue, l.pick("Valor", "Value", "Valor")),
        (
            ContextKind::ValidValue,
            l.pick("Valores aceitos", "Accepted values", "Valores aceptados"),
        ),
        (
            ContextKind::SuggestedValue,
            l.pick("Valor sugerido", "Suggested value", "Valor sugerido"),
        ),
        (
            ContextKind::SuggestedArg,
            l.pick("Opção sugerida", "Suggested option", "Opción sugerida"),
        ),
        (
            ContextKind::SuggestedSubcommand,
            l.pick("Comando sugerido", "Suggested command", "Comando sugerido"),
        ),
    ] {
        if let Some(value) = error.get(context) {
            message.push_str(&format!("{label}: {value}\n"));
        }
    }
    if let Some(usage) = error.get(ContextKind::Usage) {
        let usage = usage.to_string();
        let usage = usage.strip_prefix("Usage:").unwrap_or(&usage);
        message.push_str(&format!("\nUso:{usage}\n"));
    }
    message.push_str(l.pick(
        "\nUse --help para ver as opções disponíveis.\n",
        "\nUse --help to see available options.\n",
        "\nUsa --help para ver las opciones disponibles.\n",
    ));
    message
}
