//! Failures the engine explains only in its own log: the process ends with a
//! bare signal after the graphics driver lost the device, or it stays alive
//! with Android frozen after a KVM error. The preload library for AMD graphics
//! (`amd_linux`) writes there too, when the driver rejected commands and the
//! library sent them again: a long wait would otherwise look like a freeze
//! nobody explains.

use std::{
    fmt::Display,
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use super::{Locale, amd_linux::Host};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    /// The graphics driver rejected the engine's commands or reported the
    /// device lost. The engine aborts right after.
    DeviceLost,
    /// KVM could not run the guest any more. The engine process stays alive
    /// with Android frozen.
    VmFrozen,
}

/// What one line of the engine log means, if anything.
pub(crate) fn classify(line: &str) -> Option<Fault> {
    if line.contains("kvm run failed") {
        Some(Fault::VmFrozen)
    } else if line.contains("Encountered device lost") || line.contains("The CS has been rejected")
    {
        Some(Fault::DeviceLost)
    } else {
        None
    }
}

/// The code a process ends with when an integer division by zero is not
/// handled.
const DIVIDE_BY_ZERO: u32 = 0xC000_0094;

/// Why the engine ended, from its exit code alone, for the codes whose cause
/// is known. Its log says nothing in these cases: the process stops at once.
///
/// Integer division by zero: some games that use a compressed texture format
/// the graphics card does not support end the engine this way. The engine
/// asks the card's driver about such a texture before it switches to its own
/// handling of the format, and the driver divides by zero inside that call.
/// Copied frames and the engine's own surface in the panel end the same way.
pub(crate) fn exit_code_reason(code: Option<i32>, locale: Locale) -> Option<&'static str> {
    // The code is negative as an i32.
    (code.map(|code| code as u32) == Some(DIVIDE_BY_ZERO)).then(|| {
        locale.pick(
            "O motor parou numa divisão por zero dentro do driver da placa de vídeo (0xC0000094). Isso acontece com alguns jogos que usam um formato de textura comprimida que a placa de vídeo não suporta. Mudar as configurações do emulador não evita; deixe esse jogo fechado",
            "The engine stopped on a division by zero inside the graphics card's driver (0xC0000094). This is known to happen with some games that use a compressed texture format the graphics card does not support. Changing the emulator's settings does not avoid it; keep that game closed",
            "El motor se ha detenido en una división por cero dentro del controlador de la tarjeta gráfica (0xC0000094). Ocurre con algunos juegos que usan un formato de textura comprimida que la tarjeta gráfica no admite. Cambiar los ajustes del emulador no lo evita; deja ese juego cerrado",
        )
    })
}

/// One line of the preload library about a submission the driver rejected
/// with busy pages and the library sent again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Resend {
    /// The driver took the submission in the end. Otherwise the library gave
    /// up, and the driver reports the device as lost.
    accepted: bool,
    /// How long the submission was held back.
    waited_ms: u64,
}

const RESEND_LINE: &str =
    "RustEmuDroid: userptr guard: the driver rejected a submission (busy pages); ";

/// `... (busy pages); accepted after N more attempts in M ms (K so far)`, or
/// `still rejected` in place of `accepted`. The library prints the first eight
/// and then every power of two; `still rejected` always.
pub(crate) fn parse_resend(line: &str) -> Option<Resend> {
    let (_, rest) = line.trim_end().split_once(RESEND_LINE)?;
    let (accepted, rest) = match rest.strip_prefix("accepted after ") {
        Some(rest) => (true, rest),
        None => (false, rest.strip_prefix("still rejected after ")?),
    };
    let (attempts, rest) = rest.split_once(" more attempts in ")?;
    let (waited, rest) = rest.split_once(" ms (")?;
    let count = rest.strip_suffix(" so far)")?;
    attempts.parse::<u64>().ok()?;
    count.parse::<u64>().ok()?;
    Some(Resend {
        accepted,
        waited_ms: waited.parse().ok()?,
    })
}

/// A wait this long is told to the user while Android runs. Shorter ones stay
/// silent: they were 5 ms to about 1 s in every measurement.
const STALL_NOTICE_MS: u64 = 2000;

/// Whether a wait deserves a message: the first one of [`STALL_NOTICE_MS`] or
/// more, and later only one at least twice as long as the one last told.
fn stall_is_news(told_ms: Option<u64>, longest_ms: u64) -> bool {
    match told_ms {
        None => longest_ms >= STALL_NOTICE_MS,
        Some(told) => longest_ms >= told.saturating_mul(2),
    }
}

/// `3 seconds`, from milliseconds, never less than one second.
fn seconds(milliseconds: u64, locale: Locale) -> String {
    let seconds = (milliseconds.saturating_add(500) / 1000).max(1);
    let unit = if seconds == 1 {
        locale.pick("segundo", "second", "segundo")
    } else {
        locale.pick("segundos", "seconds", "segundos")
    };
    format!("{seconds} {unit}")
}

/// How much of the log is read at a time.
#[derive(Clone, Copy)]
struct Limits {
    /// Most bytes read in one poll; the rest waits for the next one.
    poll_bytes: u64,
    /// Polls made after the engine exited, to reach the end of the log.
    final_passes: usize,
    /// Read from the end of the log when those polls did not reach it.
    tail_bytes: u64,
}

/// 1 MiB per poll and 64 MiB forward after the engine exited. An engine that
/// wrote more than that since the last poll still has its last 1 MiB read,
/// where it explains why it ended.
const LIMITS: Limits = Limits {
    poll_bytes: 1024 * 1024,
    final_passes: 64,
    tail_bytes: 1024 * 1024,
};
/// A line longer than this is examined in pieces.
const LINE_BYTES: usize = 16 * 1024;
/// Kept from a cut line so a marker across the cut is still found.
const MARKER_BYTES: usize = 64;

/// Follows the engine log between polls. Each poll reads only the bytes
/// appended since the previous one.
pub(crate) struct LogWatch {
    enabled: bool,
    limits: Limits,
    offset: u64,
    /// The line being written: bytes after the last line break.
    line: Vec<u8>,
    device_lost: bool,
    frozen: Option<String>,
    /// `accepted` lines of the library seen so far.
    resends: u64,
    /// The longest wait among them.
    longest_wait_ms: u64,
    /// The wait of the last message given by [`LogWatch::stall_notice`].
    told_wait_ms: Option<u64>,
    /// The wait in a `still rejected` line: the library gave up.
    gave_up_ms: Option<u64>,
}

impl LogWatch {
    /// A watch that is not enabled never reads the log and reports nothing.
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            limits: LIMITS,
            offset: 0,
            line: Vec::new(),
            device_lost: false,
            frozen: None,
            resends: 0,
            longest_wait_ms: 0,
            told_wait_ms: None,
            gave_up_ms: None,
        }
    }

    /// Examines what the engine appended to its log since the last call.
    pub(crate) fn poll(&mut self, log: &Path) {
        if self.enabled {
            self.read(log);
        }
    }

    /// Returns whether anything new was read.
    fn read(&mut self, log: &Path) -> bool {
        let Ok(length) = fs::metadata(log).map(|metadata| metadata.len()) else {
            return false;
        };
        if length < self.offset {
            // Truncated or replaced: start again from its beginning.
            self.offset = 0;
            self.line.clear();
        }
        if length == self.offset {
            return false;
        }
        let mut bytes = Vec::new();
        // Whatever was read before an error still counts.
        let _ = fs::File::open(log).and_then(|mut file| {
            file.seek(SeekFrom::Start(self.offset))?;
            file.take(self.limits.poll_bytes).read_to_end(&mut bytes)
        });
        if bytes.is_empty() {
            return false;
        }
        self.offset += bytes.len() as u64;
        self.feed(&bytes);
        true
    }

    /// After the engine exited: reads the rest of the log, at most
    /// `final_passes` polls forward. When that does not reach the end, the
    /// last `tail_bytes` are examined too, from a line boundary: the engine
    /// says why it ended in its last lines.
    fn finish(&mut self, log: &Path) {
        for _ in 0..self.limits.final_passes {
            if !self.read(log) {
                break;
            }
        }
        let length = fs::metadata(log).map_or(0, |metadata| metadata.len());
        if length > self.offset {
            let start = self
                .offset
                .max(length.saturating_sub(self.limits.tail_bytes));
            let mut bytes = Vec::new();
            let _ = fs::File::open(log).and_then(|mut file| {
                file.seek(SeekFrom::Start(start))?;
                file.take(self.limits.tail_bytes).read_to_end(&mut bytes)
            });
            let mut tail = bytes.as_slice();
            if start > self.offset {
                // Bytes were skipped: the line being written is not continued,
                // and the first line here is cut. Without any line break the
                // whole piece is examined rather than nothing.
                self.line.clear();
                if let Some(end) = tail.iter().position(|byte| *byte == b'\n') {
                    tail = &tail[end + 1..];
                }
            }
            self.offset = start + bytes.len() as u64;
            self.feed(tail);
        }
        // The engine may have ended in the middle of a line.
        self.examine();
    }

    fn feed(&mut self, mut bytes: &[u8]) {
        while let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
            self.line.extend_from_slice(&bytes[..end]);
            self.examine();
            self.line.clear();
            bytes = &bytes[end + 1..];
        }
        // A line still being written waits for its line break, so its whole
        // text is seen at once. `exit_error` examines the last one.
        self.line.extend_from_slice(bytes);
        if self.line.len() > LINE_BYTES {
            self.examine();
            self.line.drain(..self.line.len() - MARKER_BYTES);
        }
    }

    fn examine(&mut self) {
        if self.line.is_empty() {
            return;
        }
        let line = String::from_utf8_lossy(&self.line);
        if let Some(resend) = parse_resend(&line) {
            if resend.accepted {
                self.resends += 1;
                self.longest_wait_ms = self.longest_wait_ms.max(resend.waited_ms);
            } else {
                self.gave_up_ms = Some(resend.waited_ms);
            }
            return;
        }
        match classify(&line) {
            Some(Fault::DeviceLost) => self.device_lost = true,
            Some(Fault::VmFrozen) if self.frozen.is_none() => {
                self.frozen = Some(line.trim().chars().take(200).collect());
            }
            _ => {}
        }
    }

    /// While Android runs: one message for the boot details when the driver
    /// held commands back for [`STALL_NOTICE_MS`] or more and the library got
    /// them accepted. Given once, and again only for a wait at least twice as
    /// long as the one already told.
    pub(crate) fn stall_notice(&mut self, locale: Locale) -> Option<String> {
        if self.resends == 0 || !stall_is_news(self.told_wait_ms, self.longest_wait_ms) {
            return None;
        }
        self.told_wait_ms = Some(self.longest_wait_ms);
        let waited = seconds(self.longest_wait_ms, locale);
        Some(match locale {
            Locale::PtBr => format!(
                "Placa de vídeo AMD no Linux: o driver gráfico segurou comandos por cerca de {waited} e a proteção da memória de vídeo os reenviou. O Android pode congelar por esse tempo quando isso acontece."
            ),
            Locale::EnUs => format!(
                "AMD graphics on Linux: the graphics driver held commands back for about {waited} and the video memory protection sent them again. Android can freeze for that long when it happens."
            ),
            Locale::EsEs => format!(
                "Gráfica AMD en Linux: el controlador gráfico ha retenido comandos durante aproximadamente {waited} y la protección de la memoria de vídeo los ha reenviado. Android puede quedarse congelado ese tiempo cuando ocurre."
            ),
        })
    }

    /// The failure to report when the engine printed a KVM error: its process
    /// is alive but Android no longer runs. `amd`: the AMD card this boot
    /// handled, for the known cause.
    pub(crate) fn frozen_error(
        &self,
        log: &Path,
        amd: Option<&Host>,
        locale: Locale,
    ) -> Option<String> {
        let line = self.frozen.as_deref()?;
        let advice = amd
            .filter(|_| line.contains("Bad address"))
            .and_then(|host| host.kvm_mapping_advice(locale));
        // What to do comes first; the engine's own words and the log close the message.
        let evidence = format!(
            "{} {}. Log: {}",
            locale.pick("Motor:", "Engine:", "Motor:"),
            line.trim_end_matches('.'),
            log.display()
        );
        Some(match advice {
            Some(advice) => format!(
                "{}. {advice} {evidence}",
                locale.pick(
                    "O Android parou porque a virtualização do processador (KVM) não conseguiu mapear a memória de vídeo",
                    "Android stopped because the processor virtualization (KVM) could not map graphics memory",
                    "Android se ha detenido porque la virtualización del procesador (KVM) no ha podido mapear la memoria gráfica",
                )
            ),
            None => format!(
                "{}. {evidence}",
                locale.pick(
                    "O Android parou: a virtualização do processador (KVM) falhou e a máquina virtual ficou congelada",
                    "Android stopped: the processor virtualization (KVM) failed and the virtual machine froze",
                    "Android se ha detenido: la virtualización del procesador (KVM) ha fallado y la máquina virtual se ha quedado congelada",
                )
            ),
        })
    }

    /// The failure to report after the engine process ended with `status`.
    /// Reads the rest of the log first. `fallback` is the reason used when the
    /// log explains nothing.
    pub(crate) fn exit_error(
        &mut self,
        log: &Path,
        status: impl Display,
        fallback: &str,
        amd: Option<&Host>,
        locale: Locale,
    ) -> String {
        if self.enabled {
            self.finish(log);
        }
        if let Some(error) = self.frozen_error(log, amd, locale) {
            return error;
        }
        let reason = if let Some(waited) = self.gave_up_ms {
            // The library's own account of the lost device.
            let waited = seconds(waited, locale);
            match locale {
                Locale::PtBr => format!(
                    "O driver gráfico continuou rejeitando os comandos por cerca de {waited}, embora a proteção da memória de vídeo os tenha reenviado, e o motor encerrou"
                ),
                Locale::EnUs => format!(
                    "The graphics driver kept rejecting the commands for about {waited} although the video memory protection sent them again, and the engine exited"
                ),
                Locale::EsEs => format!(
                    "El controlador gráfico ha seguido rechazando los comandos durante aproximadamente {waited}, aunque la protección de la memoria de vídeo los ha reenviado, y el motor se ha cerrado"
                ),
            }
        } else if self.device_lost {
            locale
                .pick(
                    "O driver gráfico perdeu o dispositivo e o motor encerrou",
                    "The graphics driver lost the device and the engine exited",
                    "El controlador gráfico ha perdido el dispositivo y el motor se ha cerrado",
                )
                .to_owned()
        } else {
            fallback.to_owned()
        };
        format!("{reason} ({status}): {}", log.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::OpenOptions, io::Write, path::PathBuf};

    const LOCALES: [Locale; 3] = [Locale::PtBr, Locale::EnUs, Locale::EsEs];

    // Lines from the engine log of the measured PC (Radeon RX 580, RADV, Linux 6.8).
    const CS_REJECTED: &str =
        "radv/amdgpu: The CS has been rejected, see dmesg for more information (-16).";
    const SUBMIT_FAILED: &str = "dispatchVkQueueSubmit failed: VK_ERROR_DEVICE_LOST";
    const DEVICE_LOST: &str = "FATAL        | Encountered device lost.";
    const KVM_FAILED: &str = "error: kvm run failed Bad address";
    const ORDINARY: &str = "INFO         | Selecting Vulkan device: AMD Radeon RX 580 Series (RADV POLARIS10), Version: 1.4.318";
    const SIGABRT: &str = "signal: 6 (SIGABRT) (core dumped)";

    fn append(log: &Path, text: &str) {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    fn log() -> (tempfile::TempDir, PathBuf) {
        let folder = tempfile::tempdir().unwrap();
        let log = folder.path().join("boot-1-2.log");
        (folder, log)
    }

    /// The AMD card of the measured PC, read as the client reads it.
    fn rx580(folder: &Path) -> Host {
        let device = folder.join("drm/card1/device");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("vendor"), "0x1002\n").unwrap();
        fs::write(device.join("mem_info_vis_vram_total"), "268435456\n").unwrap();
        fs::write(device.join("mem_info_vram_total"), "8589934592\n").unwrap();
        fs::write(folder.join("osrelease"), "6.8.0-151-generic\n").unwrap();
        Host::read(&folder.join("drm"), &folder.join("osrelease")).unwrap()
    }

    #[test]
    fn engine_lines_are_classified() {
        assert_eq!(classify(CS_REJECTED), Some(Fault::DeviceLost));
        assert_eq!(classify(DEVICE_LOST), Some(Fault::DeviceLost));
        assert_eq!(classify(KVM_FAILED), Some(Fault::VmFrozen));
        assert_eq!(
            classify("qemu-system-x86_64: error: kvm run failed Input/output error"),
            Some(Fault::VmFrozen)
        );
        // The submit failure alone is not the engine's final word.
        assert_eq!(classify(SUBMIT_FAILED), None);
        assert_eq!(classify(ORDINARY), None);
        assert_eq!(
            classify("Vulkan - Out of memory! (allocating memory for buffer)"),
            None
        );
        assert_eq!(classify(""), None);
    }

    #[test]
    fn only_appended_bytes_are_read_on_each_poll() {
        let (_folder, log) = log();
        let mut watch = LogWatch::new(true);
        // No log yet.
        watch.poll(&log);
        assert_eq!(watch.offset, 0);

        append(&log, &format!("{ORDINARY}\n"));
        watch.poll(&log);
        let first = watch.offset;
        assert_eq!(first, ORDINARY.len() as u64 + 1);
        assert!(!watch.device_lost && watch.frozen.is_none());

        // Nothing new: nothing is read.
        assert!(!watch.read(&log));
        assert_eq!(watch.offset, first);

        // A marker written in two pieces, across two polls.
        let (start, end) = CS_REJECTED.split_at(30);
        append(&log, start);
        watch.poll(&log);
        assert!(!watch.device_lost);
        assert_eq!(watch.offset, first + 30);
        append(&log, &format!("{end}\n{SUBMIT_FAILED}\n"));
        watch.poll(&log);
        assert!(watch.device_lost);
        assert!(watch.line.is_empty());
        assert_eq!(watch.offset, fs::metadata(&log).unwrap().len());

        // A line is examined when its line break arrives, with its whole text.
        append(&log, KVM_FAILED);
        watch.poll(&log);
        assert_eq!(watch.frozen, None);
        append(&log, "\n");
        watch.poll(&log);
        assert_eq!(watch.frozen.as_deref(), Some(KVM_FAILED));
    }

    #[test]
    fn a_truncated_log_is_read_again_from_its_start() {
        let (_folder, log) = log();
        let mut watch = LogWatch::new(true);
        append(&log, &format!("{ORDINARY}\n{ORDINARY}\npartial"));
        watch.poll(&log);
        assert_eq!(watch.line, b"partial");
        let before = watch.offset;

        fs::write(&log, format!("{DEVICE_LOST}\n")).unwrap();
        assert!(fs::metadata(&log).unwrap().len() < before);
        watch.poll(&log);
        assert_eq!(watch.offset, DEVICE_LOST.len() as u64 + 1);
        assert!(watch.device_lost);
        assert!(watch.line.is_empty());
    }

    #[test]
    fn large_and_unusual_output_stays_bounded() {
        let (_folder, log) = log();
        let mut watch = LogWatch::new(true);
        // More than two polls' worth, without a line break, then the marker.
        let filler = "x".repeat(2 * LIMITS.poll_bytes as usize + 4096);
        append(&log, &filler);
        watch.poll(&log);
        assert_eq!(watch.offset, LIMITS.poll_bytes);
        assert!(watch.line.len() <= LINE_BYTES);
        append(&log, &format!("{DEVICE_LOST}\n"));
        watch.poll(&log);
        assert_eq!(watch.offset, 2 * LIMITS.poll_bytes);
        assert!(!watch.device_lost, "the rest waits for the next poll");
        // After the engine exits the rest is read in one go.
        let error = watch.exit_error(&log, SIGABRT, "Engine exited", None, Locale::EnUs);
        assert!(error.starts_with("The graphics driver lost the device"));
        assert_eq!(watch.offset, fs::metadata(&log).unwrap().len());

        // Bytes that are not UTF-8 do not hide a marker on the same line.
        let (_folder, log) = self::log();
        let mut watch = LogWatch::new(true);
        let mut bytes = vec![0xff, 0xfe, b' '];
        bytes.extend_from_slice(KVM_FAILED.as_bytes());
        bytes.extend_from_slice(b"\r\n");
        fs::write(&log, bytes).unwrap();
        watch.poll(&log);
        assert!(watch.frozen.as_deref().unwrap().ends_with(KVM_FAILED));
    }

    /// The real limits shrunk, so a test need not write more than 64 MiB:
    /// 4 KiB per poll, 16 KiB forward after the exit, and the last 1 KiB.
    const SMALL: Limits = Limits {
        poll_bytes: 4096,
        final_passes: 4,
        tail_bytes: 1024,
    };
    const SMALL_FORWARD: usize = SMALL.poll_bytes as usize * SMALL.final_passes;

    /// A watch with the small limits that has polled a log of one line.
    fn small_watch(log: &Path) -> LogWatch {
        let mut watch = LogWatch {
            limits: SMALL,
            ..LogWatch::new(true)
        };
        append(log, &format!("{ORDINARY}\n"));
        watch.poll(log);
        assert_eq!(watch.offset, ORDINARY.len() as u64 + 1);
        watch
    }

    #[test]
    fn a_log_longer_than_the_final_scan_still_shows_how_the_engine_ended() {
        // The engine wrote far more than the forward limit since the last
        // poll, and then why it ended.
        let (_folder, log) = log();
        let mut watch = small_watch(&log);
        let polled = watch.offset;
        let chatter = format!("{ORDINARY}\n").repeat(1000);
        assert!(chatter.len() > 4 * SMALL_FORWARD);
        append(&log, &chatter);
        append(
            &log,
            &format!("{CS_REJECTED}\n{SUBMIT_FAILED}\n{DEVICE_LOST}\n"),
        );
        let length = fs::metadata(&log).unwrap().len();
        assert!(length - polled > SMALL_FORWARD as u64 + SMALL.tail_bytes);
        assert_eq!(
            watch.exit_error(&log, SIGABRT, "Engine exited", None, Locale::EnUs),
            format!(
                "The graphics driver lost the device and the engine exited ({SIGABRT}): {}",
                log.display()
            )
        );
        assert_eq!(watch.offset, length);
        assert!(watch.line.is_empty());

        // Without those last lines the same log explains nothing.
        let (_folder, log) = self::log();
        let mut watch = small_watch(&log);
        append(&log, &chatter);
        assert_eq!(
            watch.exit_error(&log, SIGABRT, "Engine exited", None, Locale::EnUs),
            format!("Engine exited ({SIGABRT}): {}", log.display())
        );

        // The end of the log is examined from a line boundary: the line cut
        // by the start of that piece is left out, the whole ones are read.
        let (_folder, log) = self::log();
        let mut watch = small_watch(&log);
        append(&log, &chatter);
        let cut = format!(
            "{}{KVM_FAILED}\n",
            "c".repeat(2 * SMALL.tail_bytes as usize)
        );
        append(&log, &cut);
        append(&log, &format!("{ORDINARY}\n{DEVICE_LOST}"));
        let error = watch.exit_error(&log, SIGABRT, "Engine exited", None, Locale::EnUs);
        assert_eq!(watch.frozen, None);
        assert!(
            error.starts_with("The graphics driver lost the device"),
            "{error}"
        );

        // A last piece without any line break is examined as it is.
        let (_folder, log) = self::log();
        let mut watch = small_watch(&log);
        append(&log, &chatter);
        append(&log, &"n".repeat(2 * SMALL.tail_bytes as usize));
        append(&log, DEVICE_LOST);
        assert!(
            watch
                .exit_error(&log, SIGABRT, "Engine exited", None, Locale::EnUs)
                .starts_with("The graphics driver lost the device")
        );
    }

    #[test]
    fn the_final_scan_continues_a_line_when_no_bytes_were_skipped() {
        // The forward polls stop inside the last line and what is left is
        // shorter than the end piece: the line is continued, not cut.
        let (_folder, log) = log();
        let mut watch = small_watch(&log);
        let mut filler = String::new();
        while filler.len() + 100 <= SMALL_FORWARD - 10 {
            filler.push_str(&"z".repeat(99));
            filler.push('\n');
        }
        let rest = SMALL_FORWARD - 10 - filler.len();
        if rest > 0 {
            filler.push_str(&"z".repeat(rest - 1));
            filler.push('\n');
        }
        assert_eq!(filler.len(), SMALL_FORWARD - 10);
        append(&log, &filler);
        append(&log, &format!("{DEVICE_LOST}\n"));
        let length = fs::metadata(&log).unwrap().len();
        assert!(length - watch.offset < SMALL_FORWARD as u64 + SMALL.tail_bytes);
        assert!(
            watch
                .exit_error(&log, SIGABRT, "Engine exited", None, Locale::EnUs)
                .starts_with("The graphics driver lost the device")
        );
        assert_eq!(watch.offset, length);
    }

    #[test]
    fn the_exit_reason_comes_from_the_log_when_it_explains_the_signal() {
        let (_folder, log) = log();
        let shown = log.display().to_string();
        append(
            &log,
            &format!("{ORDINARY}\n{CS_REJECTED}\n{SUBMIT_FAILED}\n{DEVICE_LOST}\n"),
        );
        for (locale, fallback, expected) in [
            (
                Locale::PtBr,
                "Motor encerrou",
                "O driver gráfico perdeu o dispositivo e o motor encerrou",
            ),
            (
                Locale::EnUs,
                "Engine exited",
                "The graphics driver lost the device and the engine exited",
            ),
            (
                Locale::EsEs,
                "El motor se cerró",
                "El controlador gráfico ha perdido el dispositivo y el motor se ha cerrado",
            ),
        ] {
            // Nothing was polled before the exit: the whole log is read now.
            let mut watch = LogWatch::new(true);
            assert_eq!(
                watch.exit_error(&log, SIGABRT, fallback, None, locale),
                format!("{expected} ({SIGABRT}): {shown}")
            );
            // Not enabled: the message is the one from before.
            let mut off = LogWatch::new(false);
            off.poll(&log);
            assert_eq!(off.offset, 0);
            assert_eq!(
                off.exit_error(&log, SIGABRT, fallback, None, locale),
                format!("{fallback} ({SIGABRT}): {shown}")
            );
        }

        // An exit the log does not explain keeps the plain reason.
        let (_folder, log) = self::log();
        append(&log, &format!("{ORDINARY}\n"));
        let mut watch = LogWatch::new(true);
        assert_eq!(
            watch.exit_error(&log, "exit status: 1", "Engine exited", None, Locale::EnUs),
            format!("Engine exited (exit status: 1): {}", log.display())
        );
        assert_eq!(watch.frozen_error(&log, None, Locale::EnUs), None);

        // The engine ended in the middle of its last line: that line counts too.
        append(&log, DEVICE_LOST);
        watch.poll(&log);
        assert!(!watch.device_lost);
        assert!(
            watch
                .exit_error(&log, SIGABRT, "Engine exited", None, Locale::EnUs)
                .starts_with(
                    "The graphics driver lost the device and the engine exited (signal: 6"
                )
        );
    }

    #[test]
    fn a_kvm_failure_is_reported_with_its_known_cause_on_amd_graphics() {
        let (folder, log) = log();
        let shown = log.display().to_string();
        let host = rx580(folder.path());
        let mut watch = LogWatch::new(true);
        append(&log, &format!("{ORDINARY}\n"));
        watch.poll(&log);
        assert_eq!(watch.frozen_error(&log, Some(&host), Locale::EnUs), None);
        append(&log, &format!("{KVM_FAILED}\n"));
        watch.poll(&log);

        assert_eq!(
            watch.frozen_error(&log, Some(&host), Locale::EnUs).unwrap(),
            format!(
                "Android stopped because the processor virtualization (KVM) could not map graphics memory. This happens with AMD graphics on Linux kernels older than 6.13 when the card exposes a small memory window (here: 256 MB of 8192 MB, kernel 6.8). Enabling “Above 4G Decoding” and “Resizable BAR” in the firmware (BIOS/UEFI) or using Linux 6.13 or newer should avoid it. Engine: error: kvm run failed Bad address. Log: {shown}"
            )
        );
        assert_eq!(
            watch.frozen_error(&log, Some(&host), Locale::PtBr).unwrap(),
            format!(
                "O Android parou porque a virtualização do processador (KVM) não conseguiu mapear a memória de vídeo. Isso acontece com placas de vídeo AMD em kernels Linux anteriores ao 6.13 quando a placa expõe uma janela de memória pequena (aqui: 256 MB de 8192 MB, kernel 6.8). Ativar «Above 4G Decoding» e «Resizable BAR» no firmware (BIOS/UEFI) ou usar o Linux 6.13 ou mais novo deve evitar o problema. Motor: error: kvm run failed Bad address. Log: {shown}"
            )
        );
        assert_eq!(
            watch.frozen_error(&log, Some(&host), Locale::EsEs).unwrap(),
            format!(
                "Android se ha detenido porque la virtualización del procesador (KVM) no ha podido mapear la memoria gráfica. Ocurre con gráficas AMD en kernels de Linux anteriores al 6.13 cuando la tarjeta expone una ventana de memoria pequeña (aquí: 256 MB de 8192 MB, kernel 6.8). Activar «Above 4G Decoding» y «Resizable BAR» en el firmware (BIOS/UEFI) o usar Linux 6.13 o posterior debería evitarlo. Motor: error: kvm run failed Bad address. Log: {shown}"
            )
        );

        // Without an AMD card in use, only what the engine said.
        assert_eq!(
            watch.frozen_error(&log, None, Locale::EnUs).unwrap(),
            format!(
                "Android stopped: the processor virtualization (KVM) failed and the virtual machine froze. Engine: error: kvm run failed Bad address. Log: {shown}"
            )
        );
        for locale in LOCALES {
            let plain = watch.frozen_error(&log, None, locale).unwrap();
            assert!(plain.contains(KVM_FAILED) && plain.ends_with(&shown));
            assert!(!plain.contains("6.13"), "{plain}");
        }
        // If the engine then exits by itself, the reason stays the same.
        assert_eq!(
            watch.exit_error(&log, SIGABRT, "Engine exited", Some(&host), Locale::EnUs),
            watch.frozen_error(&log, Some(&host), Locale::EnUs).unwrap()
        );

        // Another KVM error on the same computer is not blamed on the memory window.
        let (_other, log) = self::log();
        append(&log, "error: kvm run failed Input/output error\n");
        let mut watch = LogWatch::new(true);
        watch.poll(&log);
        let other = watch.frozen_error(&log, Some(&host), Locale::EnUs).unwrap();
        assert!(other.starts_with("Android stopped: the processor virtualization (KVM) failed"));
        assert!(!other.contains("6.13"), "{other}");
    }

    // The two lines of the preload library, as it prints them.
    const RESEND_ACCEPTED: &str = "RustEmuDroid: userptr guard: the driver rejected a submission (busy pages); accepted after 2 more attempts in 1050 ms (8 so far)";
    const RESEND_GAVE_UP: &str = "RustEmuDroid: userptr guard: the driver rejected a submission (busy pages); still rejected after 187 more attempts in 10012 ms (1 so far)";

    fn accepted_line(waited_ms: u64) -> String {
        format!(
            "RustEmuDroid: userptr guard: the driver rejected a submission (busy pages); accepted after 3 more attempts in {waited_ms} ms (4 so far)\n"
        )
    }

    #[test]
    fn the_librarys_resend_lines_are_parsed() {
        assert_eq!(
            parse_resend(RESEND_ACCEPTED),
            Some(Resend {
                accepted: true,
                waited_ms: 1050,
            })
        );
        assert_eq!(
            parse_resend(RESEND_GAVE_UP),
            Some(Resend {
                accepted: false,
                waited_ms: 10012,
            })
        );
        // As the engine log may hold them: after another writer's text, with
        // a Windows line end, with the smallest numbers.
        assert_eq!(
            parse_resend(&format!("INFO | {RESEND_ACCEPTED}\r")),
            Some(Resend {
                accepted: true,
                waited_ms: 1050,
            })
        );
        assert_eq!(
            parse_resend(
                "RustEmuDroid: userptr guard: the driver rejected a submission (busy pages); accepted after 1 more attempts in 0 ms (1 so far)"
            ),
            Some(Resend {
                accepted: true,
                waited_ms: 0,
            })
        );

        let start = "RustEmuDroid: userptr guard: the driver rejected a submission (busy pages); ";
        for malformed in [
            String::new(),
            ORDINARY.to_owned(),
            CS_REJECTED.to_owned(),
            // The library's other lines.
            "RustEmuDroid: userptr guard active: 1 buffers registered, 0 closed, 16 MiB now, 16 MiB peak, dontfork_failed=0 populate_failed=0".to_owned(),
            "RustEmuDroid: userptr guard reports all 8192 MiB of video memory as CPU visible (the window is 256 MiB)".to_owned(),
            // Cut short, as a line still being written.
            start.trim_end().to_owned(),
            format!("{start}accepted after 2 more attempts in 1050 ms"),
            format!("{start}accepted after 2 more attempts in 1050 ms (8 so far"),
            // Another outcome, or numbers that are not numbers.
            format!("{start}ignored after 2 more attempts in 1050 ms (8 so far)"),
            format!("{start}accepted after two more attempts in 1050 ms (8 so far)"),
            format!("{start}accepted after 2 more attempts in -5 ms (8 so far)"),
            format!("{start}accepted after 2 more attempts in 1.5 ms (8 so far)"),
            format!("{start}accepted after 2 more attempts in  ms (8 so far)"),
            format!("{start}accepted after 2 more attempts in 1050 ms (many so far)"),
            format!("{start}accepted after 2 more attempts in 99999999999999999999999 ms (8 so far)"),
            format!("{start}accepted after 2 more attempts in 1050 ms (8 so far) and more"),
            format!("{start}still rejected after 2 attempts in 1050 ms (8 so far)"),
        ] {
            assert_eq!(parse_resend(&malformed), None, "{malformed}");
        }
        // Neither line counts as a lost device or a frozen VM by itself.
        assert_eq!(classify(RESEND_ACCEPTED), None);
        assert_eq!(classify(RESEND_GAVE_UP), None);
    }

    #[test]
    fn a_wait_is_told_from_two_seconds_and_again_only_when_it_doubles() {
        for (told, longest, news) in [
            (None, 0, false),
            (None, 1050, false),
            (None, 1999, false),
            (None, 2000, true),
            (None, 9000, true),
            (Some(2000), 2000, false),
            (Some(2000), 3999, false),
            (Some(2000), 4000, true),
            (Some(3500), 6999, false),
            (Some(3500), 7000, true),
            (Some(u64::MAX), u64::MAX, true),
        ] {
            assert_eq!(stall_is_news(told, longest), news, "{told:?} {longest}");
        }
        for (milliseconds, expected) in [
            (0, "1 second"),
            (1050, "1 second"),
            (1499, "1 second"),
            (1500, "2 seconds"),
            (2000, "2 seconds"),
            (2499, "2 seconds"),
            (3600, "4 seconds"),
            (10012, "10 seconds"),
        ] {
            assert_eq!(seconds(milliseconds, Locale::EnUs), expected);
        }
        assert_eq!(seconds(1050, Locale::PtBr), "1 segundo");
        assert_eq!(seconds(10012, Locale::PtBr), "10 segundos");
        assert_eq!(seconds(1050, Locale::EsEs), "1 segundo");
        assert_eq!(seconds(10012, Locale::EsEs), "10 segundos");
    }

    #[test]
    fn long_waits_are_told_once_while_android_runs() {
        let (_folder, log) = log();
        let mut watch = LogWatch::new(true);
        // Nothing in the log, and the waits of every measurement so far: silent.
        watch.poll(&log);
        assert_eq!(watch.stall_notice(Locale::EnUs), None);
        for waited in [5, 9, 33, 1050, 1999] {
            append(&log, &accepted_line(waited));
        }
        append(&log, &format!("{ORDINARY}\n"));
        watch.poll(&log);
        assert_eq!((watch.resends, watch.longest_wait_ms), (5, 1999));
        assert_eq!(watch.stall_notice(Locale::EnUs), None);
        assert_eq!(watch.told_wait_ms, None);

        // The first wait of two seconds or more is told, once.
        append(&log, &accepted_line(2400));
        watch.poll(&log);
        assert_eq!((watch.resends, watch.longest_wait_ms), (6, 2400));
        assert_eq!(
            watch.stall_notice(Locale::EnUs).unwrap(),
            "AMD graphics on Linux: the graphics driver held commands back for about 2 seconds and the video memory protection sent them again. Android can freeze for that long when it happens."
        );
        assert_eq!(watch.stall_notice(Locale::EnUs), None);
        watch.poll(&log);
        assert_eq!(watch.stall_notice(Locale::EnUs), None);

        // Later waits that are shorter, as long, or less than twice as long: silent.
        for waited in [40, 2400, 3000, 4799] {
            append(&log, &accepted_line(waited));
            watch.poll(&log);
            assert_eq!(watch.stall_notice(Locale::EnUs), None, "{waited}");
        }
        assert_eq!((watch.resends, watch.longest_wait_ms), (10, 4799));
        assert_eq!(watch.told_wait_ms, Some(2400));

        // At least twice the wait already told: told again, with the new wait.
        append(&log, &accepted_line(4800));
        watch.poll(&log);
        assert_eq!(
            watch.stall_notice(Locale::PtBr).unwrap(),
            "Placa de vídeo AMD no Linux: o driver gráfico segurou comandos por cerca de 5 segundos e a proteção da memória de vídeo os reenviou. O Android pode congelar por esse tempo quando isso acontece."
        );
        assert_eq!(watch.stall_notice(Locale::PtBr), None);
        append(&log, &accepted_line(9599));
        watch.poll(&log);
        assert_eq!(watch.stall_notice(Locale::EsEs), None);
        append(&log, &accepted_line(9700));
        watch.poll(&log);
        assert_eq!(
            watch.stall_notice(Locale::EsEs).unwrap(),
            "Gráfica AMD en Linux: el controlador gráfico ha retenido comandos durante aproximadamente 10 segundos y la protección de la memoria de vídeo los ha reenviado. Android puede quedarse congelado ese tiempo cuando ocurre."
        );
        assert_eq!(watch.told_wait_ms, Some(9700));

        // Several long waits between two polls give one message, for the longest.
        let (_folder, log) = self::log();
        let mut watch = LogWatch::new(true);
        append(
            &log,
            &format!(
                "{}{}{}",
                accepted_line(2100),
                accepted_line(6400),
                accepted_line(3000)
            ),
        );
        watch.poll(&log);
        let notice = watch.stall_notice(Locale::EnUs).unwrap();
        assert!(notice.contains("for about 6 seconds"), "{notice}");
        assert_eq!(watch.stall_notice(Locale::EnUs), None);

        // A line still being written is not counted before it is complete, and
        // a watch that is not enabled says nothing.
        let (_folder, log) = self::log();
        let mut watch = LogWatch::new(true);
        let line = accepted_line(5000);
        let (first, second) = line.split_at(line.len() - 12);
        append(&log, first);
        watch.poll(&log);
        assert_eq!(watch.stall_notice(Locale::EnUs), None);
        append(&log, second);
        watch.poll(&log);
        assert!(watch.stall_notice(Locale::EnUs).is_some());
        let mut off = LogWatch::new(false);
        off.poll(&log);
        assert_eq!(off.stall_notice(Locale::EnUs), None);
        assert_eq!(off.resends, 0);

        // The library giving up is not a wait that ended well: no notice for it.
        let (_folder, log) = self::log();
        let mut watch = LogWatch::new(true);
        append(&log, &format!("{RESEND_GAVE_UP}\n"));
        watch.poll(&log);
        assert_eq!(watch.gave_up_ms, Some(10012));
        assert_eq!(watch.stall_notice(Locale::EnUs), None);
    }

    #[test]
    fn the_exit_reason_says_when_the_resend_was_rejected_to_the_end() {
        let (_folder, log) = log();
        let shown = log.display().to_string();
        // What the engine log holds when the library gives up: its line, then
        // the driver's and the engine's.
        append(
            &log,
            &format!(
                "{ORDINARY}\n{}{RESEND_GAVE_UP}\n{CS_REJECTED}\n{SUBMIT_FAILED}\n{DEVICE_LOST}\n",
                accepted_line(33)
            ),
        );
        for (locale, fallback, expected) in [
            (
                Locale::PtBr,
                "Motor encerrou",
                "O driver gráfico continuou rejeitando os comandos por cerca de 10 segundos, embora a proteção da memória de vídeo os tenha reenviado, e o motor encerrou",
            ),
            (
                Locale::EnUs,
                "Engine exited",
                "The graphics driver kept rejecting the commands for about 10 seconds although the video memory protection sent them again, and the engine exited",
            ),
            (
                Locale::EsEs,
                "El motor se cerró",
                "El controlador gráfico ha seguido rechazando los comandos durante aproximadamente 10 segundos, aunque la protección de la memoria de vídeo los ha reenviado, y el motor se ha cerrado",
            ),
        ] {
            let mut watch = LogWatch::new(true);
            assert_eq!(
                watch.exit_error(&log, SIGABRT, fallback, None, locale),
                format!("{expected} ({SIGABRT}): {shown}")
            );
            assert!(watch.device_lost);
            assert_eq!((watch.resends, watch.gave_up_ms), (1, Some(10012)));
            // Not enabled (as on Windows): the message is the one from before.
            let mut off = LogWatch::new(false);
            assert_eq!(
                off.exit_error(&log, SIGABRT, fallback, None, locale),
                format!("{fallback} ({SIGABRT}): {shown}")
            );
        }

        // The same when the watch had followed the log while Android ran.
        let mut watch = LogWatch::new(true);
        watch.poll(&log);
        assert!(
            watch
                .exit_error(&log, SIGABRT, "Engine exited", None, Locale::EnUs)
                .starts_with(
                    "The graphics driver kept rejecting the commands for about 10 seconds"
                )
        );

        // Resends that were all accepted do not change the plain reasons.
        let (_folder, log) = self::log();
        append(&log, &accepted_line(2400));
        let mut watch = LogWatch::new(true);
        assert_eq!(
            watch.exit_error(&log, "exit status: 1", "Engine exited", None, Locale::EnUs),
            format!("Engine exited (exit status: 1): {}", log.display())
        );
        append(&log, &format!("{CS_REJECTED}\n{DEVICE_LOST}\n"));
        assert_eq!(
            watch.exit_error(&log, SIGABRT, "Engine exited", None, Locale::EnUs),
            format!(
                "The graphics driver lost the device and the engine exited ({SIGABRT}): {}",
                log.display()
            )
        );

        // A KVM failure still comes first.
        append(&log, &format!("{KVM_FAILED}\n{RESEND_GAVE_UP}\n"));
        assert!(
            watch
                .exit_error(&log, SIGABRT, "Engine exited", None, Locale::EnUs)
                .starts_with("Android stopped: the processor virtualization (KVM) failed")
        );
    }

    #[test]
    fn a_division_by_zero_in_the_engine_is_explained_by_its_code() {
        // 0xC0000094 as the operating system reports it for the engine.
        let divide = Some(0xC000_0094_u32 as i32);
        for locale in LOCALES {
            let reason = exit_code_reason(divide, locale).unwrap();
            assert!(reason.contains("0xC0000094"), "{reason}");
        }
        assert!(
            exit_code_reason(divide, Locale::PtBr)
                .unwrap()
                .starts_with("O motor parou")
        );
        assert!(
            exit_code_reason(divide, Locale::EsEs)
                .unwrap()
                .starts_with("El motor se ha")
        );
        // Other codes and signals keep the plain words.
        for other in [
            None,
            Some(0),
            Some(1),
            Some(0xC000_0005_u32 as i32),
            Some(134),
        ] {
            assert_eq!(exit_code_reason(other, Locale::EnUs), None, "{other:?}");
        }

        // The log explains nothing (the process stopped at once): the code's
        // reason opens the message, followed by the status and the log.
        let (_folder, log) = log();
        append(&log, &format!("{ORDINARY}\n"));
        let reason = exit_code_reason(divide, Locale::EnUs).unwrap();
        let mut watch = LogWatch::new(true);
        assert_eq!(
            watch.exit_error(&log, "exit code: 0xc0000094", reason, None, Locale::EnUs),
            format!("{reason} (exit code: 0xc0000094): {}", log.display())
        );
        // A lost device the log reports still comes first.
        append(&log, &format!("{DEVICE_LOST}\n"));
        assert!(
            LogWatch::new(true)
                .exit_error(&log, "exit code: 0xc0000094", reason, None, Locale::EnUs)
                .starts_with("The graphics driver lost the device")
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_engines_exit_status_carries_the_division_by_zero_code() {
        use std::os::windows::process::ExitStatusExt;
        let status = std::process::ExitStatus::from_raw(0xC000_0094);
        assert_eq!(status.to_string(), "exit code: 0xc0000094");
        assert!(exit_code_reason(status.code(), Locale::EnUs).is_some());
        let ordinary = std::process::ExitStatus::from_raw(1);
        assert_eq!(exit_code_reason(ordinary.code(), Locale::EnUs), None);
    }
}
