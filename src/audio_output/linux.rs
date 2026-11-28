//! The Linux sound server's outputs, listed through its command-line client.

use super::{Device, Inventory};
use crate::locale::Locale;
use std::{
    io::Read,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// The listing takes milliseconds; a server that hangs must not hold the boot.
const LIMIT: Duration = Duration::from_secs(5);

pub(super) fn inventory(locale: Locale) -> Result<Inventory, String> {
    let sinks = server(&["list", "sinks"], locale)?;
    let mut inventory = Inventory {
        devices: parse_sinks(&sinks),
        default: None,
    };
    // Older servers lack get-default-sink; `info` has carried the line for long.
    inventory.default = server(&["info"], locale)
        .ok()
        .and_then(|info| default_sink(&info));
    Ok(inventory)
}

/// Asks the sound server through its command-line client.
fn server(args: &[&str], locale: Locale) -> Result<String, String> {
    let mut command = Command::new("pactl");
    // Field labels are translated; the parser reads the untranslated ones.
    command.args(args).env("LC_ALL", "C");
    capture(command, LIMIT).map_err(|failure| failure.text(locale))
}

#[derive(Debug)]
enum Failure {
    /// The program could not be started; the system's own message.
    Start(std::io::Error),
    /// It ended with this exit code, or by a signal.
    Exit(Option<i32>),
    Timeout,
    /// Its output could not be read.
    Output,
}

impl Failure {
    fn text(&self, locale: Locale) -> String {
        match self {
            Self::Start(error) => format!(
                "{}: {error}",
                locale.pick(
                    "o comando do servidor de som não pôde ser iniciado",
                    "the sound server's command could not be started",
                    "no se pudo iniciar el comando del servidor de sonido",
                )
            ),
            Self::Exit(Some(code)) => format!(
                "{} {code}",
                locale.pick(
                    "o comando do servidor de som terminou com o código",
                    "the sound server's command exited with code",
                    "el comando del servidor de sonido terminó con el código",
                )
            ),
            Self::Exit(None) => locale
                .pick(
                    "o comando do servidor de som foi interrompido",
                    "the sound server's command was interrupted",
                    "el comando del servidor de sonido fue interrumpido",
                )
                .into(),
            Self::Timeout => locale
                .pick(
                    "o servidor de som não respondeu a tempo",
                    "the sound server did not answer in time",
                    "el servidor de sonido no respondió a tiempo",
                )
                .into(),
            Self::Output => locale
                .pick(
                    "a resposta do servidor de som não pôde ser lida",
                    "the sound server's answer could not be read",
                    "no se pudo leer la respuesta del servidor de sonido",
                )
                .into(),
        }
    }
}

fn end(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs `command` and returns its standard output, within `limit`. The output
/// is read while the program runs: one that wrote more than the pipe holds
/// would otherwise wait for a reader until the time limit.
fn capture(mut command: Command, limit: Duration) -> Result<String, Failure> {
    let deadline = Instant::now() + limit;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(Failure::Start)?;
    let Some(mut stdout) = child.stdout.take() else {
        end(&mut child);
        return Err(Failure::Output);
    };
    let (sender, output) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout.read_to_end(&mut bytes);
        let _ = sender.send(read.map(|_| bytes));
    });
    // The output ends when the program exits or closes it.
    let bytes = match output.recv_timeout(limit) {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            end(&mut child);
            return Err(Failure::Output);
        }
        Err(_) => {
            end(&mut child);
            return Err(Failure::Timeout);
        }
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(status)) => return Err(Failure::Exit(status.code())),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) | Err(_) => {
                end(&mut child);
                return Err(Failure::Timeout);
            }
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(super) fn parse_sinks(text: &str) -> Vec<Device> {
    let mut devices = Vec::new();
    let mut current: Option<(Option<String>, Option<String>)> = None;
    let mut finish = |entry: Option<(Option<String>, Option<String>)>| {
        if let Some((Some(id), name)) = entry {
            let name = name
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| id.clone());
            devices.push(Device { id, name });
        }
    };
    for line in text.lines() {
        if line.starts_with("Sink #") {
            finish(current.take());
            current = Some((None, None));
            continue;
        }
        let Some((id, name)) = current.as_mut() else {
            continue;
        };
        // Only the sink's own fields (one tab), not nested property lists.
        let Some(field) = line.strip_prefix('\t').filter(|f| !f.starts_with('\t')) else {
            continue;
        };
        if let Some(value) = field.strip_prefix("Name: ") {
            *id = Some(value.trim().to_owned());
        } else if let Some(value) = field.strip_prefix("Description: ") {
            *name = Some(value.trim().to_owned());
        }
    }
    finish(current);
    devices
}

pub(super) fn default_sink(info: &str) -> Option<String> {
    info.lines()
        .find_map(|line| line.strip_prefix("Default Sink: "))
        .map(|sink| sink.trim().to_owned())
        .filter(|sink| !sink.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_outputs_and_the_default_of_the_sound_server() {
        let sinks = "Sink #47\n\tState: SUSPENDED\n\tName: output.pci-0000_00_1f.3.analog-stereo\n\tDescription: Built-in Audio Analog Stereo\n\tDriver: sound-server\n\tProperties:\n\t\tdevice.description = \"Other\"\n\t\tName: nested\n\nSink #52\n\tName: wireless_output.00_11_22.1\n\tDescription: \n";
        assert_eq!(
            parse_sinks(sinks),
            [
                Device {
                    id: "output.pci-0000_00_1f.3.analog-stereo".into(),
                    name: "Built-in Audio Analog Stereo".into(),
                },
                Device {
                    id: "wireless_output.00_11_22.1".into(),
                    name: "wireless_output.00_11_22.1".into(),
                },
            ]
        );
        assert!(parse_sinks("").is_empty());
        let info = "Server Name: sound server 1.0.5\nDefault Sink: wireless_output.00_11_22.1\nDefault Source: x\n";
        assert_eq!(
            default_sink(info).as_deref(),
            Some("wireless_output.00_11_22.1")
        );
        assert_eq!(default_sink("Default Sink: \n"), None);
    }

    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn output_larger_than_the_pipe_is_read_while_the_program_runs() {
        // 300 000 bytes, several times a pipe's 64 KiB: without a reader the
        // program would block and only the time limit would end it.
        let started = Instant::now();
        let text = capture(
            shell("head -c 300000 /dev/zero | tr '\\0' 'a'"),
            Duration::from_secs(20),
        )
        .unwrap();
        assert_eq!(text.len(), 300_000);
        assert!(text.bytes().all(|byte| byte == b'a'));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_program_that_hangs_or_fails_is_reported() {
        let started = Instant::now();
        let hung = capture(shell("exec sleep 30"), Duration::from_millis(300)).unwrap_err();
        assert!(matches!(hung, Failure::Timeout), "{hung:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "killed at the limit"
        );
        let failed = capture(shell("echo partial; exit 3"), LIMIT).unwrap_err();
        assert!(matches!(failed, Failure::Exit(Some(3))), "{failed:?}");
        let missing = capture(Command::new("rustemudroid-no-such-program"), LIMIT).unwrap_err();
        assert!(matches!(missing, Failure::Start(_)), "{missing:?}");
        for failure in [
            Failure::Exit(Some(1)),
            Failure::Exit(None),
            Failure::Timeout,
            Failure::Output,
        ] {
            let texts = [Locale::PtBr, Locale::EnUs, Locale::EsEs].map(|l| failure.text(l));
            assert!(texts[0] != texts[1] && texts[1] != texts[2] && texts[0] != texts[2]);
        }
    }
}
