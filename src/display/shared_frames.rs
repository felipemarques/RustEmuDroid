//! Shared-memory ("MMAP") frame transport of the emulator's `streamScreenshot`.
//!
//! The emulator writes every frame's pixels into a region we name in the
//! request and then sends a message that carries only the frame's format
//! (`ImageTransport.channel = MMAP`, empty `image`).
//!
//! The region is memory, never a file on a disk:
//!
//! * Windows: a named mapping backed by the paging file. The emulator turns a
//!   handle that is not a `file://` URL into the mapping `SHM_<handle>` and
//!   opens ours because we create it first. It lives exactly as long as a
//!   handle to it, so nothing is left behind and the emulator has nothing to
//!   delete. (A file does not work there: the Windows emulator closes the
//!   region between the first frame and the rest of the stream, deletes a
//!   file whenever it closes it, and continues in a new file of that name.)
//! * Linux: a file in `/dev/shm` (tmpfs, one of the directories the emulator
//!   accepts), created exclusively with mode 0600 and filled with zeros, so
//!   every page exists before the emulator maps it: a full tmpfs fails here,
//!   in the client, never as a fault in the emulator's write. The Linux
//!   emulator does not delete a file it did not create. Files of clients that
//!   no longer run are removed at the next start ([`remove_leftovers`]).
//!
//! Proof that the emulator writes where we read: after every copy we put a
//! mark in the first [`MARK`] bytes of the region. A frame is a rewrite of the
//! whole image, so the mark is gone when the next frame is there. A copy that
//! still starts with our mark was not written since the previous copy; it gets
//! the bytes the mark replaced back (the mark is never shown) and is counted
//! ([`SharedFrames::ignored`]). The emulator can write a frame just before we
//! put the mark and announce it just after; that single frame also counts as
//! not written and its first four pixels are the previous frame's, which is
//! why only several such frames in a row, over some time, mean that the
//! emulator writes somewhere else. The price of that caution: fewer such
//! frames followed by a still screen are noticed only with the next frames.
//!
//! Tearing: the protocol has one region per subscription and no lock or
//! sequence inside it; `seq`/`timestampUs` only number the messages. The
//! emulator writes frame N+1 into the region before it sends message N+1, and
//! it may start that write while we are still copying frame N. The reader
//! therefore jumps to the newest message already received before copying, and
//! after copying it checks again: if a newer message arrived meanwhile the copy
//! is dropped and the newer frame is copied instead ([`copy_newest`]). A write
//! that starts during the copy and whose message has not arrived yet can still
//! mix two frames in one displayed image; the next frame replaces it. The
//! region is never read beyond the size we created.
#[cfg(any(not(windows), test))]
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use std::{
    io,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// `ImageTransport.TransportChannel.MMAP`.
pub(super) const MMAP: i32 = 1;
/// Where this client knows how to share a region with the emulator.
pub(super) const SUPPORTED: bool = cfg!(any(windows, target_os = "linux"));
/// No region is larger than this (an 8K RGBA frame is half of it).
pub(super) const MAX_REGION: u64 = 256 << 20;
/// A copy is redone at most this many times for newer messages, so a fast
/// producer can not keep the reader copying forever.
pub(super) const MAX_RECOPY: u32 = 4;
/// At most this many pending messages are skipped in one scan.
const MAX_SKIP: u32 = 256;
/// Bytes of the mark at the start of the region (four RGBA pixels).
const MARK: usize = 16;
const PREFIX: &str = "rustemudroid-frames-";
#[cfg(any(not(windows), test))]
const SUFFIX: &str = ".rgba";
#[cfg(not(windows))]
const DIRECTORY: &str = "/dev/shm";

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A number that differs between two processes that were given the same id.
fn instance() -> u64 {
    static INSTANCE: OnceLock<u64> = OnceLock::new();
    *INSTANCE.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64)
    })
}

/// `rustemudroid-frames-<process id>-<instance>-<counter>`.
fn unique_name() -> String {
    format!(
        "{PREFIX}{}-{:x}-{}",
        std::process::id(),
        instance(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Debug)]
pub(super) enum CopyError {
    /// The announced frame does not fit in the region.
    TooLarge { frame: u64, region: u64 },
    /// The operating system refused the read or the write.
    System(io::Error),
}

impl From<io::Error> for CopyError {
    fn from(error: io::Error) -> Self {
        Self::System(error)
    }
}

pub(super) struct SharedFrames {
    region: Region,
    handle: String,
    capacity: usize,
    /// What we last put at the start of the region, and the frame bytes it
    /// replaced.
    mark: [u8; MARK],
    under: [u8; MARK],
    marks: u64,
    /// The emulator wrote over a mark at least once.
    written: bool,
    /// Copies in a row that still started with our mark, and since when.
    unwritten: u32,
    unwritten_since: Option<Instant>,
}

impl SharedFrames {
    /// Creates this platform's region of `bytes` bytes, all of them allocated
    /// and zero, for the emulator to open instead of creating its own.
    pub fn create(bytes: usize) -> io::Result<Self> {
        #[cfg(windows)]
        {
            let name = unique_name();
            let (mapping, existed) = Mapping::named(&name, bytes)?;
            if existed {
                // Somebody else's memory under our name is not ours to share.
                return Err(io::Error::from_raw_os_error(
                    windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS as i32,
                ));
            }
            Self::over(Region::Mapping(mapping), name, bytes)
        }
        #[cfg(not(windows))]
        {
            Self::in_directory(Path::new(DIRECTORY), bytes)
        }
    }

    /// A region that is a file in `directory`; the handle is the `file://`
    /// URL the Unix emulator opens verbatim.
    #[cfg(any(not(windows), test))]
    pub fn in_directory(directory: &Path, bytes: usize) -> io::Result<Self> {
        let file = FrameFile::create(directory, bytes)?;
        let handle = format!("file://{}", file.path.display());
        Self::over(Region::File(file), handle, bytes)
    }

    fn over(region: Region, handle: String, capacity: usize) -> io::Result<Self> {
        let mut frames = Self {
            region,
            handle,
            capacity,
            mark: [0; MARK],
            under: [0; MARK],
            marks: 0,
            written: false,
            unwritten: 0,
            unwritten_since: None,
        };
        if capacity >= MARK {
            frames.put_mark()?;
        }
        Ok(frames)
    }

    /// The `ImageTransport.handle` that makes the emulator open this region.
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// Whether the emulator wrote a frame into this region at least once.
    pub fn written(&self) -> bool {
        self.written
    }

    /// Announced frames in a row that the emulator did not write here.
    pub fn unwritten(&self) -> u32 {
        self.unwritten
    }

    /// True when at least `frames` announced frames in a row, the first of
    /// them at least `span` ago, were not written into this region: the
    /// emulator writes somewhere else.
    pub fn ignored(&self, frames: u32, span: Duration) -> bool {
        self.unwritten >= frames
            && self
                .unwritten_since
                .is_some_and(|since| since.elapsed() >= span)
    }

    /// Copies the first `len` bytes of the region, the frame just announced,
    /// and notes whether the emulator wrote them since the previous copy.
    pub fn copy(&mut self, len: usize) -> Result<Vec<u8>, CopyError> {
        if len > self.capacity {
            return Err(CopyError::TooLarge {
                frame: len as u64,
                region: self.capacity as u64,
            });
        }
        let mut pixels = vec![0; len];
        self.region.read(&mut pixels)?;
        if len < MARK {
            // Smaller than the mark (under four pixels): nothing to judge by.
            return Ok(pixels);
        }
        if pixels[..MARK] == self.mark {
            pixels[..MARK].copy_from_slice(&self.under);
            self.unwritten += 1;
            self.unwritten_since.get_or_insert_with(Instant::now);
        } else {
            self.under.copy_from_slice(&pixels[..MARK]);
            self.written = true;
            self.unwritten = 0;
            self.unwritten_since = None;
            self.put_mark()?;
        }
        Ok(pixels)
    }

    /// Puts a mark no frame starts with: its alpha bytes are not opaque and
    /// it changes with every copy.
    fn put_mark(&mut self) -> io::Result<()> {
        self.marks += 1;
        self.mark[..8].copy_from_slice(b"RED\0mrk\x01");
        self.mark[8..].copy_from_slice(&self.marks.to_le_bytes());
        self.region.write(&self.mark)
    }
}

enum Region {
    #[cfg(windows)]
    Mapping(Mapping),
    #[cfg(any(not(windows), test))]
    File(FrameFile),
}

impl Region {
    fn read(&self, buffer: &mut [u8]) -> io::Result<()> {
        match self {
            #[cfg(windows)]
            Self::Mapping(mapping) => {
                mapping.read(buffer);
                Ok(())
            }
            #[cfg(any(not(windows), test))]
            Self::File(file) => file.read(buffer),
        }
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        match self {
            #[cfg(windows)]
            Self::Mapping(mapping) => {
                mapping.write(bytes);
                Ok(())
            }
            #[cfg(any(not(windows), test))]
            Self::File(file) => file.write(bytes),
        }
    }
}

/// A view of the named mapping the Windows emulator calls `SHM_<name>`.
#[cfg(windows)]
pub(super) struct Mapping {
    handle: windows_sys::Win32::Foundation::HANDLE,
    view: *mut u8,
    bytes: usize,
}

// SAFETY: a mapping handle and its view belong to the process, not to a thread.
#[cfg(windows)]
unsafe impl Send for Mapping {}

#[cfg(windows)]
impl Mapping {
    /// Opens the mapping `SHM_<name>` or creates it, backed by the paging
    /// file with every page committed and zero, and maps its first `bytes`
    /// bytes. The flag says whether it existed before the call.
    pub fn named(name: &str, bytes: usize) -> io::Result<(Self, bool)> {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, INVALID_HANDLE_VALUE},
            System::Memory::{
                CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE,
            },
        };
        let name: Vec<u16> = format!("SHM_{name}").encode_utf16().chain([0]).collect();
        let size = bytes as u64;
        // SAFETY: plain Win32 calls with a NUL-terminated name; the handle and
        // the view are owned by the returned value, which releases both.
        unsafe {
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                (size >> 32) as u32,
                size as u32,
                name.as_ptr(),
            );
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            let view = MapViewOfFile(handle, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, bytes).Value;
            if view.is_null() {
                let error = io::Error::last_os_error();
                CloseHandle(handle);
                return Err(error);
            }
            Ok((
                Self {
                    handle,
                    view: view.cast(),
                    bytes,
                },
                existed,
            ))
        }
    }

    fn read(&self, buffer: &mut [u8]) {
        let len = buffer.len().min(self.bytes);
        // SAFETY: the view is `bytes` long and mapped while `self` lives. The
        // emulator may write the same memory meanwhile, so the bytes are
        // copied through raw pointers and never borrowed as a slice.
        unsafe { std::ptr::copy_nonoverlapping(self.view, buffer.as_mut_ptr(), len) }
    }

    pub fn write(&self, bytes: &[u8]) {
        let len = bytes.len().min(self.bytes);
        // SAFETY: as in `read`.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.view, len) }
    }
}

#[cfg(windows)]
impl Drop for Mapping {
    fn drop(&mut self) {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Memory::{MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile},
        };
        // SAFETY: the view and the handle came from `named` and are released once.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.cast(),
            });
            CloseHandle(self.handle);
        }
    }
}

/// A frame file, removed when dropped.
#[cfg(any(not(windows), test))]
struct FrameFile {
    path: PathBuf,
    file: File,
}

#[cfg(any(not(windows), test))]
impl FrameFile {
    fn create(directory: &Path, bytes: usize) -> io::Result<Self> {
        let path = directory.join(format!("{}{SUFFIX}", unique_name()));
        let mut options = OpenOptions::new();
        // `create_new` refuses every existing name, a symbolic link included.
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options.open(&path)?;
        // From here on a failure removes the file again.
        let frames = Self { path, file };
        // Writing the zeros allocates every page now; a hole would only be
        // allocated by the emulator's write through its mapping, and a full
        // filesystem at that moment kills the emulator (SIGBUS).
        let zeros = vec![0u8; 64 * 1024];
        let mut left = bytes;
        while left > 0 {
            let part = left.min(zeros.len());
            (&frames.file).write_all(&zeros[..part])?;
            left -= part;
        }
        Ok(frames)
    }

    fn read(&self, buffer: &mut [u8]) -> io::Result<()> {
        let mut file = &self.file;
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(buffer)
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut file = &self.file;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(bytes)
    }
}

#[cfg(any(not(windows), test))]
impl Drop for FrameFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Removes the frame files that clients which no longer run left in the
/// shared directory (after a crash; they are memory until the next restart of
/// the computer). Nothing to do where a region is not a file. Never fails.
pub(super) fn remove_leftovers() {
    #[cfg(target_os = "linux")]
    remove_leftovers_in(Path::new(DIRECTORY), |pid| {
        Path::new("/proc").join(pid.to_string()).exists()
    });
}

/// Removes the frame files in `directory` of processes that `alive` denies,
/// and those that carry this process's id but were not created by it (an
/// earlier process that had the same id).
#[cfg(any(not(windows), test))]
fn remove_leftovers_in(directory: &Path, alive: impl Fn(u32) -> bool) -> usize {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let own = (std::process::id(), format!("{:x}", instance()));
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(of) = name
            .to_str()
            .and_then(|name| name.strip_prefix(PREFIX))
            .and_then(|rest| {
                let mut parts = rest.split('-');
                let pid = parts.next()?.parse::<u32>().ok()?;
                Some((pid, parts.next()?.to_string()))
            })
        else {
            continue;
        };
        let left_behind = if of.0 == own.0 {
            of.1 != own.1
        } else {
            !alive(of.0)
        };
        if left_behind && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
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

#[cfg(test)]
mod tests {
    use super::*;

    const BYTES: usize = 8 * 4 * 4;

    /// A whole frame as the emulator writes it: opaque pixels of one value.
    fn picture(value: u8, bytes: usize) -> Vec<u8> {
        [value, value, value, 255].repeat(bytes / 4)
    }

    /// The regions to test: a file everywhere, the named mapping on Windows.
    /// The second item writes into the region the way the emulator does,
    /// through its own handle.
    #[allow(clippy::type_complexity)]
    #[cfg_attr(not(windows), allow(unused_mut))]
    fn regions(dir: &Path) -> Vec<(SharedFrames, Box<dyn Fn(usize, &[u8])>)> {
        let file = SharedFrames::in_directory(dir, BYTES).unwrap();
        let path = PathBuf::from(file.handle().strip_prefix("file://").unwrap());
        let mut regions: Vec<(SharedFrames, Box<dyn Fn(usize, &[u8])>)> = vec![(
            file,
            Box::new(move |offset, bytes| {
                let mut emulator = OpenOptions::new().write(true).open(&path).unwrap();
                emulator.seek(SeekFrom::Start(offset as u64)).unwrap();
                emulator.write_all(bytes).unwrap();
            }),
        )];
        #[cfg(windows)]
        {
            let mapping = SharedFrames::create(BYTES).unwrap();
            let name = mapping.handle().to_string();
            regions.push((
                mapping,
                Box::new(move |offset, bytes| {
                    let (emulator, existed) = Mapping::named(&name, offset + bytes.len()).unwrap();
                    assert!(existed, "the emulator opens the client's mapping");
                    let mut whole = vec![0; offset + bytes.len()];
                    emulator.read(&mut whole);
                    whole[offset..].copy_from_slice(bytes);
                    emulator.write(&whole);
                }),
            ));
        }
        regions
    }

    #[test]
    fn a_copy_is_the_frame_the_emulator_wrote_and_never_shows_the_mark() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path()) {
            assert!(!frames.written());
            for value in [7, 8, 9] {
                emulator(0, &picture(value, BYTES));
                assert_eq!(frames.copy(BYTES).unwrap(), picture(value, BYTES));
                assert!(frames.written());
                assert_eq!(frames.unwritten(), 0);
            }
            // A smaller frame (another raster) is the start of the region.
            emulator(0, &picture(3, 64));
            assert_eq!(frames.copy(64).unwrap(), picture(3, 64));
        }
    }

    #[test]
    fn a_region_the_emulator_never_wrote_is_empty_and_counts_every_announced_frame() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, _) in regions(dir.path()) {
            for announced in 1..=3 {
                assert_eq!(frames.copy(BYTES).unwrap(), vec![0; BYTES]);
                assert!(!frames.written());
                assert_eq!(frames.unwritten(), announced);
            }
            assert!(frames.ignored(3, Duration::ZERO));
            assert!(!frames.ignored(4, Duration::ZERO));
            assert!(!frames.ignored(3, Duration::from_secs(3600)));
        }
    }

    #[test]
    fn frames_announced_without_a_write_repeat_the_last_picture_and_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path()) {
            emulator(0, &picture(5, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(5, BYTES));
            // The emulator now writes elsewhere and keeps announcing frames.
            for announced in 1..=4 {
                assert_eq!(frames.copy(BYTES).unwrap(), picture(5, BYTES));
                assert_eq!(frames.unwritten(), announced);
            }
            assert!(frames.written());
            assert!(frames.ignored(4, Duration::ZERO));
            std::thread::sleep(Duration::from_millis(30));
            assert!(frames.ignored(4, Duration::from_millis(20)));
            // One real write clears the suspicion.
            emulator(0, &picture(6, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(6, BYTES));
            assert_eq!(frames.unwritten(), 0);
            assert!(!frames.ignored(1, Duration::ZERO));
        }
    }

    #[test]
    fn a_frame_written_just_before_the_mark_keeps_all_but_its_first_four_pixels() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path()) {
            emulator(0, &picture(1, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(1, BYTES));
            // The emulator wrote frame 2 before our mark landed on its first
            // bytes: the region now holds the mark followed by frame 2.
            emulator(MARK, &picture(2, BYTES - MARK));
            let shown = frames.copy(BYTES).unwrap();
            assert_eq!(shown[..MARK], picture(1, MARK));
            assert_eq!(shown[MARK..], picture(2, BYTES - MARK));
            assert_eq!(frames.unwritten(), 1);
            emulator(0, &picture(3, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(3, BYTES));
            assert_eq!(frames.unwritten(), 0);
        }
    }

    #[test]
    fn a_frame_larger_than_the_region_is_refused_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, _) in regions(dir.path()) {
            assert!(matches!(
                frames.copy(BYTES + 4),
                Err(CopyError::TooLarge { frame, region })
                    if frame == BYTES as u64 + 4 && region == BYTES as u64
            ));
            assert_eq!(frames.unwritten(), 0);
        }
    }

    #[test]
    fn every_region_has_its_own_name_with_this_process_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let a = SharedFrames::in_directory(dir.path(), BYTES).unwrap();
        let b = SharedFrames::in_directory(dir.path(), BYTES).unwrap();
        assert_ne!(a.handle(), b.handle());
        let start = format!("{PREFIX}{}-{:x}-", std::process::id(), instance());
        for frames in [&a, &b] {
            let path = PathBuf::from(frames.handle().strip_prefix("file://").unwrap());
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with(&start) && name.ends_with(SUFFIX), "{name}");
            assert_eq!(path.parent(), Some(dir.path()));
        }
    }

    #[test]
    fn the_frame_file_is_private_fully_allocated_and_removed_with_the_region() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = 3 * 64 * 1024 + 4;
        let frames = SharedFrames::in_directory(dir.path(), bytes).unwrap();
        let path = PathBuf::from(frames.handle().strip_prefix("file://").unwrap());
        let info = fs::metadata(&path).unwrap();
        assert_eq!(info.len(), bytes as u64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(info.permissions().mode() & 0o777, 0o600);
        }
        // Zeros behind the mark, so no earlier picture can show through.
        let content = fs::read(&path).unwrap();
        assert_eq!(content[..MARK], frames.mark);
        assert!(content[MARK..].iter().all(|byte| *byte == 0));
        drop(frames);
        assert!(!path.exists());
        // A directory that does not exist is an error for the caller, who
        // continues without shared memory.
        assert!(SharedFrames::in_directory(&dir.path().join("missing"), bytes).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_region_is_an_owner_only_file_in_dev_shm_with_every_page_allocated() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if !Path::new(DIRECTORY).is_dir() {
            return;
        }
        let bytes = 1600 * 900 * 4;
        let frames = SharedFrames::create(bytes).unwrap();
        let path = PathBuf::from(frames.handle().strip_prefix("file://").unwrap());
        assert_eq!(path.parent(), Some(Path::new(DIRECTORY)));
        let info = fs::metadata(&path).unwrap();
        assert_eq!(info.len(), bytes as u64);
        assert_eq!(info.permissions().mode() & 0o777, 0o600);
        // tmpfs counts a page as soon as it exists: nothing is left for the
        // emulator's mapped write to allocate.
        assert!(info.blocks() * 512 >= bytes as u64, "{}", info.blocks());
        drop(frames);
        assert!(!path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn the_windows_region_is_a_mapping_the_emulator_opens_by_name_and_leaves_no_file() {
        let frames = SharedFrames::create(BYTES).unwrap();
        let name = frames.handle().to_string();
        // Not a file URL: the emulator's SharedMemory takes it as a name.
        assert!(name.starts_with(PREFIX) && !name.contains("://"), "{name}");
        // The emulator opens, writes, closes and opens again, as it does
        // between the first frame and the rest of a stream.
        for _ in 0..2 {
            let (emulator, existed) = Mapping::named(&name, BYTES).unwrap();
            assert!(existed);
            drop(emulator);
        }
        // A larger view than the region is refused, not silently created.
        assert!(Mapping::named(&name, BYTES * 1024).is_err());
        drop(frames);
        // Closed by its only owner, the memory is gone: nothing to clean up.
        let (fresh, existed) = Mapping::named(&name, BYTES).unwrap();
        assert!(!existed);
        drop(fresh);
    }

    #[test]
    fn files_of_dead_processes_and_of_an_earlier_process_with_our_id_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let own = std::process::id();
        let name = |pid: u32, instance: &str| {
            dir.path()
                .join(format!("{PREFIX}{pid}-{instance}-0{SUFFIX}"))
        };
        // The same process id as ours, from before this process existed. Its
        // name can not be the name of a region of this process.
        let reused = name(own, "1");
        fs::write(&reused, b"x").unwrap();
        let live = SharedFrames::in_directory(dir.path(), BYTES).unwrap();
        let live_path = PathBuf::from(live.handle().strip_prefix("file://").unwrap());
        let dead = name(4_000_000_001, "abc");
        let running = name(4_000_000_002, "abc");
        let unrelated = dir.path().join("rustemudroid-other.rgba");
        let unparsed = dir.path().join(format!("{PREFIX}x-1-0{SUFFIX}"));
        for path in [&dead, &running, &unrelated, &unparsed] {
            fs::write(path, b"x").unwrap();
        }
        let removed = remove_leftovers_in(dir.path(), |pid| pid == 4_000_000_002);
        assert_eq!(removed, 2);
        assert!(!reused.exists() && !dead.exists());
        assert!(live_path.exists() && running.exists());
        assert!(unrelated.exists() && unparsed.exists());
        assert_eq!(
            remove_leftovers_in(&dir.path().join("missing"), |_| false),
            0
        );
    }

    #[test]
    fn the_newest_announced_frame_is_copied_and_a_superseded_copy_is_redone() {
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

        // A failed copy is returned at once with the item it was for.
        let (item, pixels, _, _) =
            copy_newest((7, 7), || None, |_| true, |_| Err::<Vec<u8>, _>("gone"));
        assert_eq!((item, pixels), ((7, 7), Err("gone")));
    }
}
