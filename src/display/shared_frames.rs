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
//!   One fault of the emulator is left alone: when it can open a mapping but
//!   not map it (a frame larger than the mapping), it never closes its
//!   handle, and that memory stays until the emulator exits. A region is as
//!   large as the largest frame of its display, so this is not expected.
//! * Linux: a file in `/dev/shm` (tmpfs, one of the directories the emulator
//!   accepts), created exclusively with mode 0600 and filled with zeros, so
//!   every page exists before the emulator maps it: a full tmpfs fails here,
//!   in the client, never as a fault in the emulator's write. The Linux
//!   emulator does not delete a file it did not create. The client holds a
//!   lock on its file for as long as it uses it, and the files nobody holds
//!   are removed at the next start ([`remove_leftovers`]). This path has never
//!   run against an emulator, so on Linux the transport is used only when it
//!   is asked for ([`DEFAULT`]).
//!
//! A region's name carries 128 bits that can not be predicted from outside
//! this process, so another program can not take the name first (which would
//! only keep the frames inside the messages).
//!
//! Proof that the emulator writes where we read: after every copy we put a
//! mark into the region, and the next frame, a rewrite of the whole image,
//! removes it. The mark is [`MARK`] bytes, each in the alpha byte of one of
//! the first pixels. The client writes nothing else into the region, so in
//! whatever order the emulator's write and ours land, every colour byte of a
//! copy is a byte the emulator wrote. (The emulator writes RGBA8888, all four
//! bytes of every pixel on every frame, and takes the alpha from the guest's
//! picture without forcing it to opaque. The mark therefore assumes no alpha
//! value: an alpha byte of a copy that still is its mark byte is given the
//! byte the mark replaced. No mark byte is 0, 255 or the alpha it replaces,
//! so a transparent pixel, an opaque one and one that keeps its alpha are
//! never taken for the mark; a pixel whose alpha changes to exactly its mark
//! byte, one value in 254, gets the alpha of the frame before.)
//!
//! A copy in which every mark byte is still there was not written since the
//! previous copy and is counted ([`SharedFrames::unwritten`]). The emulator
//! can write a frame just before we put the mark and announce it just after:
//! that copy is the whole frame (the alpha of its first pixels is the previous
//! frame's, the same on an opaque screen) but it counts as not written. This
//! can only happen to frames whose write began before the mark was there, and
//! the emulator writes one frame at a time, so fewer than
//! [`UNWRITTEN_FRAMES`] copies in a row are affected; that many, over some
//! time, mean that the emulator writes somewhere else. The price of that
//! caution: fewer such frames followed by a still screen are noticed only
//! with the next frames. A region the emulator has never written has no such
//! doubt, its mark is there before the request is sent: one announced frame
//! that is not in it is enough.
//!
//! Tearing: the protocol has one region per subscription and no lock or
//! sequence inside it; `seq`/`timestampUs` only number the messages. The
//! emulator writes frame N+1 into the region before it sends message N+1, and
//! it may start that write while we are still copying frame N. Such a copy is
//! a mix of two frames, and nothing in the region says so. The reader does
//! what the messages allow (`receive_shared`): it copies for the newest
//! message it has, looks again right after the copy, and copies anew when a
//! newer frame was announced meanwhile; a copy made while a frame of another
//! size or rotation was announced is never drawn. A write whose message has
//! not arrived when the copy ends goes unnoticed: that picture is drawn torn
//! (at a rotation, one frame can be drawn with the shape of the previous one)
//! and the next frame replaces it. The region is never read beyond the size
//! we created.
#[cfg(any(not(windows), test))]
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use std::{
    hash::{BuildHasher, RandomState},
    io,
    time::{Duration, Instant},
};

/// `ImageTransport.TransportChannel.MMAP`.
pub(super) const MMAP: i32 = 1;
/// Where this client knows how to share a region with the emulator.
pub(super) const SUPPORTED: bool = cfg!(any(windows, target_os = "linux"));
/// Where the transport is used without being asked for: only where it has
/// run against the emulator.
pub(super) const DEFAULT: bool = cfg!(windows);
/// No region is larger than this (an 8K RGBA frame is half of it).
pub(super) const MAX_REGION: u64 = 256 << 20;
/// This many copies in a row that still had the whole mark are more than the
/// emulator writing a frame just before the mark can explain (see the module).
pub(super) const UNWRITTEN_FRAMES: u32 = 4;
/// Pixels at the start of the region whose alpha byte carries the mark.
const MARK: usize = 8;
/// Bytes those pixels take: a smaller frame has no room for the mark.
const MARK_SPAN: usize = MARK * 4;
const PREFIX: &str = "rustemudroid-frames-";
#[cfg(any(not(windows), test))]
const SUFFIX: &str = ".rgba";
#[cfg(not(windows))]
const DIRECTORY: &str = "/dev/shm";

/// Where the alpha byte of the `index`th pixel is.
const fn alpha(index: usize) -> usize {
    index * 4 + 3
}

/// `rustemudroid-frames-<process id>-<32 hexadecimal digits>`. The digits
/// come from two hashers of the standard library, whose keys start at what
/// the operating system's random generator gave this thread.
fn unique_name() -> String {
    let random = || RandomState::new().hash_one(0u8);
    format!(
        "{PREFIX}{}-{:016x}{:016x}",
        std::process::id(),
        random(),
        random()
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
    /// What we last put into the alpha bytes of the first pixels, and the
    /// alpha bytes it replaced.
    mark: [u8; MARK],
    under: [u8; MARK],
    /// Makes every mark differ from the one before, in a way the guest can
    /// not foresee.
    marks: u64,
    keys: RandomState,
    /// The emulator wrote over a mark at least once.
    written: bool,
    /// Copies in a row that still had the whole mark, and since when.
    unwritten: u32,
    unwritten_since: Option<Instant>,
    /// Runs inside every copy, before and after the region is read.
    #[cfg(test)]
    watcher: Option<Box<dyn FnMut(Moment) + Send>>,
}

/// Where a copy is when it calls the test that watches it.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Moment {
    BeforeRead,
    AfterRead,
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
            // The region is all zeros.
            under: [0; MARK],
            marks: 0,
            keys: RandomState::new(),
            written: false,
            unwritten: 0,
            unwritten_since: None,
            #[cfg(test)]
            watcher: None,
        };
        if capacity >= MARK_SPAN {
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
    /// them at least `span` ago, were not written into this region.
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
        #[cfg(test)]
        self.tell(Moment::BeforeRead);
        let mut pixels = vec![0; len];
        self.region.read(&mut pixels)?;
        #[cfg(test)]
        self.tell(Moment::AfterRead);
        if len < MARK_SPAN {
            // Too small for the mark: nothing to judge by.
            return Ok(pixels);
        }
        let mut intact = 0;
        for (index, (mark, under)) in self.mark.iter().zip(&mut self.under).enumerate() {
            let byte = &mut pixels[alpha(index)];
            if *byte == *mark {
                // Not written since the mark: the frame has here what the
                // mark replaced.
                *byte = *under;
                intact += 1;
            } else {
                *under = *byte;
            }
        }
        if intact == MARK {
            self.unwritten += 1;
            self.unwritten_since.get_or_insert_with(Instant::now);
        } else {
            self.written = true;
            self.unwritten = 0;
            self.unwritten_since = None;
            self.put_mark()?;
        }
        Ok(pixels)
    }

    /// Puts a new mark, byte by byte, so that no byte between them (the
    /// colours, which the emulator may be writing) is ever written from here.
    fn put_mark(&mut self) -> io::Result<()> {
        self.marks += 1;
        let bytes: [u8; MARK] = self.keys.hash_one(self.marks).to_le_bytes();
        for (index, byte) in bytes.into_iter().enumerate() {
            // 1..=254 and not the alpha it replaces: a transparent pixel, an
            // opaque one and one that keeps its alpha are never taken for it.
            let mut mark = 1 + byte % 254;
            if mark == self.under[index] {
                mark = 1 + mark % 254;
            }
            self.mark[index] = mark;
            self.region.put(alpha(index), mark)?;
        }
        Ok(())
    }

    /// Makes every copy call `watcher` before and after it reads the region.
    #[cfg(test)]
    pub fn watch_copies(&mut self, watcher: impl FnMut(Moment) + Send + 'static) {
        self.watcher = Some(Box::new(watcher));
    }

    #[cfg(test)]
    fn tell(&mut self, moment: Moment) {
        if let Some(watcher) = &mut self.watcher {
            watcher(moment);
        }
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

    /// Writes the one byte at `at`.
    fn put(&self, at: usize, byte: u8) -> io::Result<()> {
        match self {
            #[cfg(windows)]
            Self::Mapping(mapping) => {
                mapping.put(at, byte);
                Ok(())
            }
            #[cfg(any(not(windows), test))]
            Self::File(file) => file.put(at, byte),
        }
    }
}

/// A view of the named mapping the Windows emulator calls `SHM_<name>`.
#[cfg(windows)]
struct Mapping {
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
    fn named(name: &str, bytes: usize) -> io::Result<(Self, bool)> {
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

    fn put(&self, at: usize, byte: u8) {
        if at < self.bytes {
            // SAFETY: inside the view, as in `read`; a store of one byte.
            unsafe { self.view.add(at).write_volatile(byte) }
        }
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
        // Where a file lock only advises (on Windows it would keep the
        // emulator from writing), it says that the file is in use.
        if cfg!(unix) {
            frames.hold()?;
        }
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

    /// Takes the lock that keeps every client's [`remove_leftovers`] away
    /// from this file. It ends with the file or with this process, and it is
    /// seen wherever the directory is, also by a client for which this
    /// process's id means nothing (another container).
    fn hold(&self) -> io::Result<()> {
        self.file.lock()?;
        // A sweep that came between the creation and the lock has removed the
        // name, and the emulator would create a file of its own under it.
        fs::symlink_metadata(&self.path).map(drop)
    }

    fn read(&self, buffer: &mut [u8]) -> io::Result<()> {
        let mut file = &self.file;
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(buffer)
    }

    fn put(&self, at: usize, byte: u8) -> io::Result<()> {
        let mut file = &self.file;
        file.seek(SeekFrom::Start(at as u64))?;
        file.write_all(&[byte])
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
    remove_leftovers_in(Path::new(DIRECTORY));
}

/// Removes the frame files in `directory` that nobody holds (see
/// [`FrameFile::hold`]). A file that can not be opened or locked stays.
#[cfg(any(not(windows), test))]
fn remove_leftovers_in(directory: &Path) -> usize {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let ours = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(PREFIX) && name.ends_with(SUFFIX));
        // Only files: a link under one of our names is nothing we made.
        if !ours || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let Ok(file) = File::open(&path) else {
            continue;
        };
        // The lock is kept until the name is gone: a client that has just
        // created this file waits for it and then finds its file removed.
        if file.try_lock().is_ok() && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::{
        super::adaptive_video::fake_engine::{self, FakeEngine, Files, picture},
        *,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    };

    const BYTES: usize = 8 * 4 * 4;

    /// A frame of one colour with the given alpha in every pixel.
    fn translucent(value: u32, alpha: u8, bytes: usize) -> Vec<u8> {
        let mut pixels = picture(value, bytes);
        pixels
            .iter_mut()
            .skip(3)
            .step_by(4)
            .for_each(|a| *a = alpha);
        pixels
    }

    /// Opens a region the way the emulator does, with the emulator's own
    /// calls and none of the client's.
    type Emulator = Box<dyn Fn() -> fake_engine::Region + Send>;

    /// The regions to test, `bytes` large: a file everywhere (opened by the
    /// stand-in with the rules of the POSIX emulator), the named mapping on
    /// Windows.
    fn regions(dir: &Path, bytes: usize) -> Vec<(SharedFrames, Emulator)> {
        let opener = |frames: &SharedFrames| -> Emulator {
            let engine = FakeEngine::new([1, 1], Files::Posix, [dir.into()]);
            let handle = frames.handle().to_string();
            Box::new(move || {
                let region = engine.open(&handle, bytes, false).unwrap();
                assert!(
                    region.is_the_clients(),
                    "the emulator opens the client's region"
                );
                region
            })
        };
        let file = SharedFrames::in_directory(dir, bytes).unwrap();
        let emulator = opener(&file);
        #[allow(unused_mut)]
        let mut regions = vec![(file, emulator)];
        #[cfg(windows)]
        {
            let mapping = SharedFrames::create(bytes).unwrap();
            let emulator = opener(&mapping);
            regions.push((mapping, emulator));
        }
        regions
    }

    #[test]
    fn a_copy_made_after_a_complete_write_is_that_frame_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path(), BYTES) {
            assert!(!frames.written());
            // Opaque and transparent: the alpha is the emulator's.
            for (value, alpha) in [(7, 255), (8, 255), (9, 0), (10, 255)] {
                let frame = translucent(value, alpha, BYTES);
                emulator().write_at(0, &frame);
                assert_eq!(frames.copy(BYTES).unwrap(), frame);
                assert!(frames.written());
                assert_eq!(frames.unwritten(), 0);
            }
            // So is any other alpha that does not happen to be the mark's
            // byte at its pixel, and no later mark has the byte of an alpha
            // that stays.
            let between = (1..=254).find(|alpha| !frames.mark.contains(alpha));
            let between = between.unwrap();
            for value in 11..60 {
                let frame = translucent(value, between, BYTES);
                emulator().write_at(0, &frame);
                assert_eq!(frames.copy(BYTES).unwrap(), frame);
                assert!(!frames.mark.contains(&between));
            }
            // A smaller frame (another raster) is the start of the region.
            emulator().write_at(0, &picture(3, 64));
            assert_eq!(frames.copy(64).unwrap(), picture(3, 64));
        }
    }

    #[test]
    fn a_region_the_emulator_never_wrote_is_empty_and_counts_every_announced_frame() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, _) in regions(dir.path(), BYTES) {
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
        for (mut frames, emulator) in regions(dir.path(), BYTES) {
            emulator().write_at(0, &picture(5, BYTES));
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
            emulator().write_at(0, &picture(6, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(6, BYTES));
            assert_eq!(frames.unwritten(), 0);
            assert!(!frames.ignored(1, Duration::ZERO));
        }
    }

    #[test]
    fn the_mark_is_written_into_alpha_bytes_of_the_first_pixels_and_nowhere_else() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path(), BYTES) {
            let before = picture(0x00a1_b2c3, BYTES);
            emulator().write_at(0, &before);
            assert_eq!(frames.copy(BYTES).unwrap(), before);
            // What the region holds now, read by the emulator's side.
            let after = emulator().read(BYTES);
            for (at, (was, is)) in before.iter().zip(&after).enumerate() {
                match (0..MARK).position(|index| alpha(index) == at) {
                    Some(index) => {
                        assert_eq!(*is, frames.mark[index]);
                        assert!((1..=254).contains(is), "{is}");
                    }
                    None => assert_eq!(was, is, "byte {at} was changed"),
                }
            }
            let first = frames.mark;
            emulator().write_at(0, &before);
            frames.copy(BYTES).unwrap();
            assert_ne!(frames.mark, first, "every mark is a new one");
        }
    }

    #[test]
    fn an_alpha_that_changes_to_the_byte_of_its_mark_comes_back_as_the_alpha_before() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path(), BYTES) {
            emulator().write_at(0, &picture(1, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(1, BYTES));
            // The one alpha in 254 that the third pixel must not change to.
            let mark = frames.mark;
            let frame = translucent(2, mark[2], BYTES);
            emulator().write_at(0, &frame);
            let mut expected = frame.clone();
            for index in (0..MARK).filter(|index| mark[*index] == mark[2]) {
                expected[alpha(index)] = 255;
            }
            // Every colour is the frame's, and every other alpha.
            assert_eq!(frames.copy(BYTES).unwrap(), expected);
        }
    }

    /// The state after the emulator wrote `frame` between our copy and our
    /// mark: the whole frame, with the mark on top.
    fn written_just_before_the_mark(
        frames: &SharedFrames,
        emulator: &fake_engine::Region,
        frame: &[u8],
    ) {
        emulator.write_at(0, frame);
        for index in 0..MARK {
            emulator.write_at(alpha(index), &[frames.mark[index]]);
        }
    }

    #[test]
    fn a_frame_written_just_before_the_mark_keeps_every_colour() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path(), BYTES) {
            emulator().write_at(0, &picture(1, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(1, BYTES));
            written_just_before_the_mark(&frames, &emulator(), &picture(2, BYTES));
            // The whole of frame 2, although it counts as not written.
            assert_eq!(frames.copy(BYTES).unwrap(), picture(2, BYTES));
            assert_eq!(frames.unwritten(), 1);
            // It stays whole on a still screen, announced again or not.
            assert_eq!(frames.copy(BYTES).unwrap(), picture(2, BYTES));
            emulator().write_at(0, &picture(3, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(3, BYTES));
            assert_eq!(frames.unwritten(), 0);

            // With an alpha that changes from frame to frame the colours are
            // still the new frame's; the marked pixels have the alpha of the
            // frame before.
            written_just_before_the_mark(&frames, &emulator(), &translucent(4, 90, BYTES));
            let shown = frames.copy(BYTES).unwrap();
            let mut expected = translucent(4, 90, BYTES);
            (0..MARK).for_each(|index| expected[alpha(index)] = 255);
            assert_eq!(shown, expected);
        }
    }

    #[test]
    fn a_write_that_has_only_reached_some_of_the_marked_pixels_shows_no_mark_byte() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path(), BYTES) {
            emulator().write_at(0, &picture(1, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(1, BYTES));
            // The emulator is three pixels into frame 2 when we read.
            emulator().write_at(0, &picture(2, 12));
            let torn = [picture(2, 12), picture(1, BYTES - 12)].concat();
            assert_eq!(frames.copy(BYTES).unwrap(), torn);
            assert_eq!(frames.unwritten(), 0);
            // It goes on while our next mark lands, and finishes.
            emulator().write_at(12, &picture(2, BYTES - 12));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(2, BYTES));
        }
    }

    #[test]
    fn copies_racing_a_writer_that_never_pauses_keep_every_colour_and_are_not_taken_for_unwritten()
    {
        // Frames large enough for a write and a copy to overlap: 256x128.
        const LARGE: usize = 256 * 128 * 4;
        const COPIES: u32 = 2000;
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path(), LARGE) {
            let stop = Arc::new(AtomicBool::new(false));
            // The frame whose write has begun, and the last one complete.
            let begun = Arc::new(AtomicU32::new(0));
            let complete = Arc::new(AtomicU32::new(0));
            let (announce, announced) = mpsc::channel();
            let writer = {
                let (stop, begun, complete) = (stop.clone(), begun.clone(), complete.clone());
                std::thread::spawn(move || {
                    // The stream's own long-lived borrow of the region.
                    let region = emulator();
                    let mut frame = 0;
                    while !stop.load(Ordering::Relaxed) {
                        frame += 1;
                        let pixels = picture(frame, LARGE);
                        begun.store(frame, Ordering::SeqCst);
                        region.write_at(0, &pixels);
                        complete.store(frame, Ordering::SeqCst);
                        // As the emulator: the message follows the write.
                        if announce.send(frame).is_err() {
                            break;
                        }
                    }
                })
            };
            let started = Instant::now();
            let (mut copies, mut torn, mut raced) = (0, 0, 0);
            while copies < COPIES && started.elapsed() < Duration::from_secs(30) {
                // As the reader: wait for a message, jump to the newest.
                let mut newest: u32 = announced.recv().unwrap();
                while let Ok(newer) = announced.try_recv() {
                    newest = newer;
                }
                // Everything the region holds from here on is at least the
                // last complete frame and at most the last one begun.
                let floor = complete.load(Ordering::SeqCst);
                assert!(floor >= newest);
                let pixels = frames.copy(LARGE).unwrap();
                let ceiling = begun.load(Ordering::SeqCst);
                let mut values = pixels.chunks_exact(4).map(|pixel| {
                    assert_eq!(pixel[3], 255, "an alpha the emulator never wrote");
                    u32::from_le_bytes([pixel[0], pixel[1], pixel[2], 0])
                });
                let first = values.next().unwrap();
                let (low, high) = values.fold((first, first), |(low, high), value| {
                    (low.min(value), high.max(value))
                });
                assert!(
                    floor <= low && high <= ceiling,
                    "colours of frames {low}..={high} in a copy made between frames {floor} and {ceiling}"
                );
                assert!(frames.written());
                assert!(
                    frames.unwritten() < UNWRITTEN_FRAMES,
                    "{} copies in a row taken for unwritten",
                    frames.unwritten()
                );
                copies += 1;
                torn += u32::from(low != high);
                raced += u32::from(frames.unwritten() > 0);
            }
            stop.store(true, Ordering::Relaxed);
            drop(announced);
            writer.join().unwrap();
            assert_eq!(copies, COPIES, "too slow to say anything");
            // How often the race really happened depends on the machine.
            println!("{copies} copies: {torn} torn, {raced} written just before the mark");
        }
    }

    #[test]
    fn a_frame_larger_than_the_region_is_refused_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, _) in regions(dir.path(), BYTES) {
            assert!(matches!(
                frames.copy(BYTES + 4),
                Err(CopyError::TooLarge { frame, region })
                    if frame == BYTES as u64 + 4 && region == BYTES as u64
            ));
            assert_eq!(frames.unwritten(), 0);
        }
    }

    #[test]
    fn every_region_has_its_own_name_with_this_process_and_digits_of_its_own_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let a = SharedFrames::in_directory(dir.path(), BYTES).unwrap();
        let b = SharedFrames::in_directory(dir.path(), BYTES).unwrap();
        let start = format!("{PREFIX}{}-", std::process::id());
        let digits = |frames: &SharedFrames| {
            let path = PathBuf::from(frames.handle().strip_prefix("file://").unwrap());
            assert_eq!(path.parent(), Some(dir.path()));
            let name = path.file_name().unwrap().to_str().unwrap();
            let digits = name
                .strip_prefix(&start)
                .and_then(|rest| rest.strip_suffix(SUFFIX))
                .unwrap_or_else(|| panic!("{name}"));
            assert!(
                digits.len() == 32 && digits.bytes().all(|digit| digit.is_ascii_hexdigit()),
                "{name}"
            );
            (
                u64::from_str_radix(&digits[..16], 16).unwrap(),
                u64::from_str_radix(&digits[16..], 16).unwrap(),
            )
        };
        let (a, b) = (digits(&a), digits(&b));
        // No half of one name is a half of the other, or of itself.
        let halves = [a.0, a.1, b.0, b.1];
        for (index, half) in halves.iter().enumerate() {
            assert!(!halves[index + 1..].contains(half), "{halves:x?}");
        }
    }

    #[test]
    fn the_frame_file_has_the_frame_size_is_zero_under_the_mark_and_goes_with_the_region() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = 3 * 64 * 1024 + 4;
        let frames = SharedFrames::in_directory(dir.path(), bytes).unwrap();
        let path = PathBuf::from(frames.handle().strip_prefix("file://").unwrap());
        assert_eq!(fs::metadata(&path).unwrap().len(), bytes as u64);
        // Zeros but for the mark, so no earlier picture can show through.
        let mut content = fs::read(&path).unwrap();
        for index in 0..MARK {
            assert_eq!(content[alpha(index)], frames.mark[index]);
            content[alpha(index)] = 0;
        }
        assert!(content.iter().all(|byte| *byte == 0));
        drop(frames);
        assert!(!path.exists());
        // A directory that does not exist is an error for the caller, who
        // continues without shared memory.
        assert!(SharedFrames::in_directory(&dir.path().join("missing"), bytes).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn on_unix_the_frame_file_is_for_its_owner_only_and_survives_a_sweep_while_it_is_in_use() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let frames = SharedFrames::in_directory(dir.path(), BYTES).unwrap();
        let path = PathBuf::from(frames.handle().strip_prefix("file://").unwrap());
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // What a client that crashed left behind: the same name, nobody's.
        let left = dir.path().join(format!("{}{SUFFIX}", unique_name()));
        fs::write(&left, b"x").unwrap();
        assert_eq!(remove_leftovers_in(dir.path()), 1);
        assert!(path.exists() && !left.exists());
        drop(frames);
        assert!(!path.exists());
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
        use fake_engine::Section;
        let frames = SharedFrames::create(BYTES).unwrap();
        let name = frames.handle().to_string();
        // Not a file URL: the emulator's SharedMemory takes it as a name.
        assert!(name.starts_with(PREFIX) && !name.contains("://"), "{name}");
        // The emulator opens, writes, closes and opens again, as it does
        // between the first frame and the rest of a stream.
        for _ in 0..2 {
            let emulator = Section::open(&name, BYTES).unwrap();
            assert!(emulator.existed, "the emulator opens the client's mapping");
        }
        drop(frames);
        // Closed by its only owner, the memory is gone: nothing to clean up.
        let fresh = Section::open(&name, BYTES).unwrap();
        assert!(!fresh.existed);
        drop(fresh);

        // A larger view than the region is refused, not silently created.
        let frames = SharedFrames::create(BYTES).unwrap();
        assert!(Section::open(frames.handle(), BYTES * 1024).is_none());
    }

    #[test]
    fn the_sweep_removes_the_frame_files_nobody_has_locked_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let frame_file = || dir.path().join(format!("{}{SUFFIX}", unique_name()));
        // A running client's file: its lock is what a sweep goes by. (The
        // lock is taken here because a region takes it only on Unix.)
        let in_use = frame_file();
        let holder = File::create(&in_use).unwrap();
        holder.lock().unwrap();
        // Left behind by clients that are gone, the second by a build that
        // put a counter after the process id.
        let left = frame_file();
        let older = dir.path().join(format!("{PREFIX}4242-17f3a-0{SUFFIX}"));
        let unrelated = dir.path().join("rustemudroid-other.rgba");
        let other_kind = dir.path().join(format!("{PREFIX}1-0.txt"));
        let folder = frame_file();
        for path in [&left, &older, &unrelated, &other_kind] {
            fs::write(path, b"x").unwrap();
        }
        fs::create_dir(&folder).unwrap();
        assert_eq!(remove_leftovers_in(dir.path()), 2);
        assert!(!left.exists() && !older.exists());
        assert!(in_use.exists() && unrelated.exists() && other_kind.exists() && folder.exists());
        // The client is gone: so is its lock, and then its file.
        drop(holder);
        assert_eq!(remove_leftovers_in(dir.path()), 1);
        assert!(!in_use.exists());
        assert_eq!(remove_leftovers_in(&dir.path().join("missing")), 0);
    }
}
