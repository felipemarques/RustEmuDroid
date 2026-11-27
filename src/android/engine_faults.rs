//! Failures the engine explains only in its own log: the process ends with a
//! bare signal after the graphics driver lost the device, or it stays alive
//! with Android frozen after a KVM error.

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
        match classify(&line) {
            Some(Fault::DeviceLost) => self.device_lost = true,
            Some(Fault::VmFrozen) if self.frozen.is_none() => {
                self.frozen = Some(line.trim().chars().take(200).collect());
            }
            _ => {}
        }
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
        let reason = if self.device_lost {
            locale.pick(
                "O driver gráfico perdeu o dispositivo e o motor encerrou",
                "The graphics driver lost the device and the engine exited",
                "El controlador gráfico ha perdido el dispositivo y el motor se ha cerrado",
            )
        } else {
            fallback
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
}
