//! File-backed ("MMAP") frame transport of the emulator's `streamScreenshot`.
//!
//! The emulator maps a file we name in the request and writes every frame's
//! pixels there before it sends a message that carries only the frame's format
//! (`ImageTransport.channel = MMAP`, empty `image`). It accepts only regular
//! files inside its temporary or discovery directories
//! (`SharedMemoryLibrary::defaultAllowedDirectories`).
//!
//! Where the file lives (see [`directory`]): on Linux the emulator's discovery
//! directory `$ANDROID_EMULATOR_DISCOVERY_DIR/avd/running`, which the launcher
//! points at the profile's home and the emulator creates with mode 0700, so the
//! live screen is not in a world-readable `/tmp`; elsewhere the user's own
//! temporary directory (`%LOCALAPPDATA%\Temp` on Windows, `$TMPDIR` on macOS).
//! On every Unix the file itself is created with mode 0600.
//!
//! Reading: the file is read with positioned reads instead of a second mapping.
//! For a local file the operating system keeps reads and the emulator's mapped
//! view coherent (the same cached pages), and the copy we have to make anyway
//! (the panel keeps the frame after the next one is written) is a single read.
//!
//! Tearing: the protocol has one region per subscription and no lock or
//! sequence inside the region; `seq`/`timestampUs` only number the messages.
//! The emulator writes frame N+1 into the region before it sends message N+1,
//! and it may start that write while we are still copying frame N. The reader
//! therefore jumps to the newest message already received before copying, and
//! after copying it checks again: if a newer message arrived meanwhile the copy
//! is dropped and the newer frame is copied instead ([`copy_newest`]). A write
//! that starts during the copy and whose message has not arrived yet can still
//! mix two frames in one displayed image; the next frame replaces it. The
//! region is never read beyond the size we created.
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// `ImageTransport.TransportChannel.MMAP`.
pub(super) const MMAP: i32 = 1;
const PREFIX: &str = "rustemudroid-frames-";
const SUFFIX: &str = ".rgba";
/// A copy is redone at most this many times for newer messages, so a fast
/// producer can not keep the reader copying forever.
pub(super) const MAX_RECOPY: u32 = 4;
/// At most this many pending messages are skipped in one scan.
const MAX_SKIP: u32 = 256;

static NEXT: AtomicU64 = AtomicU64::new(0);
/// Files the emulator still held open when we tried to remove them.
static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub(super) struct SharedFrames {
    path: PathBuf,
    file: File,
    capacity: u64,
}

impl SharedFrames {
    /// Creates a private region big enough for `max_pixels` RGBA pixels in
    /// `directory` (the emulator opens it instead of creating its own).
    pub fn create(directory: &Path, max_pixels: u64) -> io::Result<Self> {
        let capacity = max_pixels
            .checked_mul(4)
            .filter(|bytes| *bytes > 0)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty raster"))?;
        retry_pending_removals();
        fs::create_dir_all(directory)?;
        let name = format!(
            "{PREFIX}{}-{}{SUFFIX}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = directory.join(name);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(&path)?;
        let frames = Self {
            path,
            file,
            capacity,
        };
        frames.file.set_len(capacity)?;
        Ok(frames)
    }

    pub fn handle(&self) -> String {
        file_url(&self.path)
    }

    /// Copies the first `len` bytes of the region.
    pub fn read(&self, len: usize) -> io::Result<Vec<u8>> {
        if len as u64 > self.capacity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "frame of {len} bytes exceeds the {} byte region",
                    self.capacity
                ),
            ));
        }
        let mut pixels = vec![0; len];
        read_exact_at(&self.file, &mut pixels, 0)?;
        Ok(pixels)
    }
}

impl Drop for SharedFrames {
    fn drop(&mut self) {
        // On Windows the emulator opens the file without delete sharing (and
        // deletes it itself when it closes it). If it still has it open, the
        // removal is retried on the next subscription and at the next start.
        if fs::remove_file(&self.path).is_err() && self.path.exists() {
            PENDING.lock().unwrap().push(self.path.clone());
        }
    }
}

fn retry_pending_removals() {
    PENDING
        .lock()
        .unwrap()
        .retain(|path| fs::remove_file(path).is_err() && path.exists());
}

/// The directory for the frame files: on Linux the emulator's per-user
/// discovery directory under the profile (`avd` is `<profile>/avd/<name>.avd`,
/// the launcher sets `ANDROID_EMULATOR_DISCOVERY_DIR=<profile>/home`), else the
/// user's temporary directory. `RUSTEMUDROID_VIDEO_SHARED_DIR` replaces it for
/// tests of the fallback.
pub(super) fn directory(avd: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os("RUSTEMUDROID_VIDEO_SHARED_DIR") {
        return PathBuf::from(dir);
    }
    if cfg!(target_os = "linux")
        && let Some(profile) = avd.parent().and_then(Path::parent)
    {
        return profile.join("home").join("avd").join("running");
    }
    std::env::temp_dir()
}

/// Removes frame files that earlier clients of this user left behind (a crash,
/// or an emulator that kept the file open past the client). Files of running
/// processes are kept; files still open without delete sharing (Windows) fail
/// to delete and are kept too. Never fails.
pub(super) fn remove_stale(directory: &Path) -> usize {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let own = std::process::id();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix(PREFIX))
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if pid != own && !process_alive(pid) && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, STILL_ACTIVE},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    // SAFETY: plain Win32 calls on a handle we own and close.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // Gone, or not ours to inspect: only an exited process is safe to clean up after.
            return windows_sys::Win32::Foundation::GetLastError()
                != windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER;
        }
        let mut code = 0;
        let alive = GetExitCodeProcess(handle, &mut code) == 0 || code == STILL_ACTIVE as u32;
        CloseHandle(handle);
        alive
    }
}

#[cfg(target_os = "linux")]
fn process_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_alive(_pid: u32) -> bool {
    true
}

/// The handle the emulator parses: on Windows a `file:///` URL that
/// `PathCreateFromUrlW` decodes, so characters with a meaning in URLs are
/// percent-encoded; on Unix the emulator takes everything after `file://` as
/// the path verbatim (no decoding), so the path is passed as it is.
pub(super) fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        let mut url = String::from("file:///");
        for c in text.replace('\\', "/").chars() {
            match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' | '/' | ':' => {
                    url.push(c)
                }
                c if !c.is_ascii() => url.push(c),
                c => url.push_str(&format!("%{:02X}", c as u32)),
            }
        }
        url
    } else {
        format!("file://{text}")
    }
}

/// Jumps to the newest pending item that `newer` accepts, copies it, and
/// starts over when a newer item arrived during the copy (at most
/// [`MAX_RECOPY`] times). An item `newer` rejects stops the scan and is
/// returned so it is handled next, in order. Returns the item whose pixels were
/// copied, the copy, that leftover item and how many copies were dropped.
pub(super) fn copy_newest<T, E>(
    mut current: T,
    mut pending: impl FnMut() -> Option<T>,
    newer: impl Fn(&T) -> bool,
    mut copy: impl FnMut(&T) -> Result<Vec<u8>, E>,
) -> (T, Result<Vec<u8>, E>, Option<T>, u32) {
    let mut rest = None;
    let mut skip = |current: &mut T, rest: &mut Option<T>| {
        let mut jumped = false;
        // Only messages already received are pending; the bound only guards
        // against a source that never runs dry.
        let mut budget = MAX_SKIP;
        while rest.is_none()
            && budget > 0
            && let Some(item) = pending()
        {
            budget -= 1;
            if newer(&item) {
                *current = item;
                jumped = true;
            } else {
                *rest = Some(item);
            }
        }
        jumped
    };
    skip(&mut current, &mut rest);
    let mut dropped = 0;
    loop {
        let pixels = copy(&current);
        if pixels.is_err() || dropped >= MAX_RECOPY || !skip(&mut current, &mut rest) {
            return (current, pixels, rest, dropped);
        }
        dropped += 1;
    }
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut buffer: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buffer.is_empty() {
        match file.seek_read(buffer, offset)? {
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            read => {
                buffer = &mut buffer[read..];
                offset += read as u64;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn read_exact_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buffer, offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Seek, SeekFrom, Write},
        sync::{Arc, atomic::AtomicBool, mpsc},
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn region_is_sized_named_uniquely_read_back_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let a = SharedFrames::create(dir.path(), 4).unwrap();
        let b = SharedFrames::create(dir.path(), 4).unwrap();
        assert_ne!(a.handle(), b.handle());
        assert_eq!(fs::metadata(&a.path).unwrap().len(), 16);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&a.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // Another writer (the emulator) updates the file in place.
        let mut writer = OpenOptions::new().write(true).open(&a.path).unwrap();
        writer.write_all(&[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        writer.flush().unwrap();
        assert_eq!(a.read(8).unwrap(), [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(a.read(16).unwrap()[8..], [0; 8]);
        assert!(a.read(17).is_err());
        let path = a.path.clone();
        drop(writer);
        drop(a);
        assert!(!path.exists());
    }

    #[test]
    fn empty_or_overflowing_rasters_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(SharedFrames::create(dir.path(), 0).is_err());
        assert!(SharedFrames::create(dir.path(), u64::MAX).is_err());
    }

    #[test]
    fn handles_are_absolute_file_urls_the_emulator_decodes() {
        if cfg!(windows) {
            assert_eq!(
                file_url(Path::new(r"C:\Users\A B\Temp #1\50%\çã\f.rgba")),
                "file:///C:/Users/A%20B/Temp%20%231/50%25/çã/f.rgba"
            );
        } else {
            // The Unix emulator strips "file://" and opens the rest verbatim.
            assert_eq!(
                file_url(Path::new("/home/a b/#1/50%/f.rgba")),
                "file:///home/a b/#1/50%/f.rgba"
            );
        }
    }

    #[test]
    fn regions_in_folders_with_url_characters_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let odd = dir.path().join("a b#c%d");
        let frames = SharedFrames::create(&odd, 1).unwrap();
        assert!(frames.path.starts_with(&odd));
        let handle = frames.handle();
        assert!(!handle.contains('#') || !cfg!(windows), "{handle}");
    }

    #[test]
    fn stale_files_of_exited_processes_are_removed_and_others_kept() {
        let dir = tempfile::tempdir().unwrap();
        let own = dir
            .path()
            .join(format!("{PREFIX}{}-7{SUFFIX}", std::process::id()));
        // A pid that cannot be running (above the usual limits).
        let dead = dir.path().join(format!("{PREFIX}4294967290-1{SUFFIX}"));
        let unrelated = dir.path().join("rustemudroid-other.rgba");
        for path in [&own, &dead, &unrelated] {
            fs::write(path, b"x").unwrap();
        }
        assert_eq!(remove_stale(dir.path()), 1);
        assert!(own.exists() && !dead.exists() && unrelated.exists());
        assert_eq!(remove_stale(&dir.path().join("missing")), 0);
    }

    #[test]
    fn newest_pending_item_is_copied_and_older_copies_are_dropped() {
        // Items: (sequence, pixel value). 9 is not a shared frame.
        let mut queue: std::collections::VecDeque<(u32, u8)> =
            [(2, 2), (3, 3), (9, 0), (4, 4)].into();
        let mut copies = vec![];
        let (item, pixels, rest, dropped) = copy_newest(
            (1, 1),
            || queue.pop_front(),
            |item| item.0 != 9,
            |item| {
                copies.push(item.0);
                Ok::<_, ()>(vec![item.1])
            },
        );
        // It jumped from 1 to 3, stopped at the non-frame 9, which is handed back.
        assert_eq!(
            (item, pixels, rest, dropped),
            ((3, 3), Ok(vec![3]), Some((9, 0)), 0)
        );
        assert_eq!(copies, [3]);
        assert_eq!(queue.front(), Some(&(4, 4)));

        // A message arriving during the copy makes the reader drop and redo it.
        let mut arrivals = [None, Some((6, 6)), None];
        let mut calls = 0;
        let (item, pixels, _, dropped) = copy_newest(
            (5, 5),
            || {
                calls += 1;
                arrivals.get_mut(calls - 1).and_then(Option::take)
            },
            |_| true,
            |item| Ok::<_, ()>(vec![item.1]),
        );
        assert_eq!((item, pixels, dropped), ((6, 6), Ok(vec![6]), 1));

        // A producer that never pauses is bounded.
        let mut seq = 10;
        let (_, _, _, dropped) = copy_newest(
            (seq, 0),
            || {
                seq += 1;
                Some((seq, 0))
            },
            |_| true,
            |_| Ok::<_, ()>(vec![]),
        );
        assert_eq!(dropped, MAX_RECOPY);
    }

    /// The reviewer's tear test: a writer overwrites the region with
    /// alternating patterns at ~120 Hz (in chunks, as a GPU readback lands)
    /// and announces each frame; a slow consumer (20 ms per frame) reads it
    /// with the old strategy (every message in order) and with `copy_newest`.
    /// Counts copies that mix patterns (torn) and copies whose pixels do not
    /// belong to the message they are shown with (stale). Run with
    /// `cargo test --features ui tear_rate -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing measurement, prints numbers"]
    fn tear_rate_of_in_order_and_newest_consumers() {
        const BYTES: usize = 800 * 450 * 4;
        fn run(newest: bool, seconds: u64) -> (u32, u32, u32, u32) {
            let dir = tempfile::tempdir().unwrap();
            let region = SharedFrames::create(dir.path(), (BYTES / 4) as u64).unwrap();
            let path = region.path.clone();
            let (send, receive) = mpsc::channel::<(u32, u8)>();
            let stop = Arc::new(AtomicBool::new(false));
            let writer_stop = stop.clone();
            let writer = thread::spawn(move || {
                let mut file = OpenOptions::new().write(true).open(&path).unwrap();
                let chunk = vec![0u8; 64 * 1024];
                let mut seq = 0u32;
                while !writer_stop.load(Ordering::Relaxed) {
                    let started = Instant::now();
                    seq += 1;
                    let value = if seq.is_multiple_of(2) { 0xAA } else { 0x55 };
                    let block: Vec<u8> = chunk.iter().map(|_| value).collect();
                    file.seek(SeekFrom::Start(0)).unwrap();
                    let mut left = BYTES;
                    while left > 0 {
                        let n = left.min(block.len());
                        file.write_all(&block[..n]).unwrap();
                        left -= n;
                    }
                    if send.send((seq, value)).is_err() {
                        break;
                    }
                    let period = Duration::from_micros(8_333);
                    if let Some(rest) = period.checked_sub(started.elapsed()) {
                        thread::sleep(rest);
                    }
                }
            });
            let check = |item: &(u32, u8), pixels: &[u8], torn: &mut u32, stale: &mut u32| {
                let mixed = pixels.iter().step_by(4096).any(|b| *b != pixels[0]);
                if mixed {
                    *torn += 1;
                } else if pixels[0] != item.1 {
                    *stale += 1;
                }
            };
            let (mut shown, mut torn, mut stale, mut dropped_total) = (0, 0, 0, 0);
            let deadline = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < deadline {
                let Ok(first) = receive.recv() else { break };
                if newest {
                    let (item, pixels, _, dropped) = copy_newest(
                        first,
                        || receive.try_recv().ok(),
                        |_| true,
                        |_| region.read(BYTES),
                    );
                    dropped_total += dropped;
                    check(&item, &pixels.unwrap(), &mut torn, &mut stale);
                } else {
                    let pixels = region.read(BYTES).unwrap();
                    check(&first, &pixels, &mut torn, &mut stale);
                }
                shown += 1;
                thread::sleep(Duration::from_millis(20));
            }
            stop.store(true, Ordering::Relaxed);
            drop(receive);
            writer.join().unwrap();
            (shown, torn, stale, dropped_total)
        }
        for (name, newest) in [("in order", false), ("newest", true)] {
            let (shown, torn, stale, dropped) = run(newest, 6);
            println!(
                "{name}: shown={shown} torn={torn} ({:.1}%) stale={stale} ({:.1}%) dropped_copies={dropped}",
                100.0 * f64::from(torn) / f64::from(shown.max(1)),
                100.0 * f64::from(stale) / f64::from(shown.max(1))
            );
        }
    }
}
