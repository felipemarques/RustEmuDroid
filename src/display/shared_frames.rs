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
//! Proof that the emulator writes where we read. Every way the emulator has
//! of producing a frame writes the colours of all its pixels. Most also write
//! the alpha, which they take from the guest's picture without forcing it to
//! opaque; the one that rotates or scales on the processor for a renderer
//! that is all Vulkan leaves the alpha bytes of the region as they are. So a
//! write shows in one of two ways:
//!
//! * The mark is gone. The mark is [`MARK`] bytes, each in the alpha byte of
//!   one of the first pixels, put there after a copy that found it gone. It
//!   assumes no alpha value: an alpha byte of a copy that still is its mark
//!   byte is given the byte the mark replaced. No mark byte is 0, 255 or the
//!   alpha it replaces, so a transparent pixel, an opaque one and one that
//!   keeps its alpha are never taken for the mark; a pixel whose alpha changes
//!   to exactly its mark byte, one value in 254, gets the alpha of the frame
//!   before.
//! * The colours changed: with the mark still whole, the copy differs from
//!   the copy before. Before the first frame the copy before is the seal:
//!   colours no frame has, put into the first pixels when the region is
//!   created, which any frame, a black one too, replaces.
//!
//! The seal is written once, before the emulator knows the region. After
//! that the client writes mark bytes and nothing else, so in whatever order
//! the emulator's write and ours land, every colour byte of a copy is a byte
//! the emulator wrote.
//!
//! A copy that shows neither sign counts against the region
//! ([`SharedFrames::unwritten`]) when the emulator is known to rewrite the
//! alpha here: it has removed a mark. Then a mark that stays says that
//! nothing was written, with one exception. The emulator can write a frame
//! just before we put the mark and announce it just after; that copy is the
//! whole frame (the alpha of its first pixels is the previous frame's, the
//! same on an opaque screen), and if the frame also has the colours of the
//! one before, it counts. This can only happen to frames whose write began
//! before the mark was there, and the emulator writes one frame at a time, so
//! fewer than [`UNWRITTEN_FRAMES`] copies in a row are affected; that many,
//! over some time, mean that nothing changes in the region any more: the
//! emulator writes somewhere else. The price of that caution: fewer such
//! frames followed by a still screen are noticed only with the next frames.
//!
//! An emulator known here only by the colours it changes proves nothing by a
//! frame that changes none, so such copies are not counted: a still screen
//! that is announced again and again is not mistaken for a region that is
//! not written. (An emulator that first removed marks and then stops, on a
//! screen that stands still, is doubted all the same; it is believed again
//! when colours change under a mark that has stayed for more copies in a row
//! than a writer of alpha leaves it.) A region the emulator has never written
//! has no such doubts, its seal and its mark are there before the request is
//! sent: one announced frame that left both is enough.
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
use prost::bytes::Bytes;
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
/// Pixels at the start of the region that carry the seal and the mark.
const MARK: usize = 8;
/// Bytes those pixels take: a smaller frame has no room for them.
const MARK_SPAN: usize = MARK * 4;
/// Colour bytes of those pixels.
const SEAL: usize = MARK * 3;
const PREFIX: &str = "rustemudroid-frames-";
#[cfg(any(not(windows), test))]
const SUFFIX: &str = ".rgba";
/// Added to the name of a frame file that is being made.
#[cfg(any(not(windows), test))]
const NEW: &str = ".new";
#[cfg(not(windows))]
const DIRECTORY: &str = "/dev/shm";

/// Where the alpha byte of the `index`th pixel is.
const fn alpha(index: usize) -> usize {
    index * 4 + 3
}

/// Where the `index`th colour byte is, the alpha bytes not counted.
const fn colour(index: usize) -> usize {
    index / 3 * 4 + index % 3
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
    /// The colours put into the first pixels at the creation, for as long as
    /// the emulator has not written.
    seal: Option<[u8; SEAL]>,
    /// The emulator wrote into this region at least once.
    written: bool,
    /// The emulator has removed a mark, so it rewrites the alpha here.
    rewrites_alpha: bool,
    /// Copies in a row that still had the whole mark.
    marked: u32,
    /// Those of them, in a row, that count against the region, and since
    /// when.
    unwritten: u32,
    unwritten_since: Option<Instant>,
    /// The copy before, to see a change of colours by.
    previous: Option<Bytes>,
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
    /// and zero but for the seal and the mark, for the emulator to open
    /// instead of creating its own.
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
        let file = FrameFile::create(directory, bytes, |_| ())?;
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
            seal: None,
            written: false,
            rewrites_alpha: false,
            marked: 0,
            unwritten: 0,
            unwritten_since: None,
            previous: None,
            #[cfg(test)]
            watcher: None,
        };
        if capacity >= MARK_SPAN {
            frames.put_seal()?;
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

    /// Announced frames in a row that showed no sign of a write and count
    /// against the region.
    pub fn unwritten(&self) -> u32 {
        self.unwritten
    }

    /// True when at least `frames` announced frames in a row, the first of
    /// them at least `span` ago, count against this region.
    pub fn ignored(&self, frames: u32, span: Duration) -> bool {
        self.unwritten >= frames
            && self
                .unwritten_since
                .is_some_and(|since| since.elapsed() >= span)
    }

    /// Copies the first `len` bytes of the region, the frame just announced,
    /// and notes whether the emulator wrote them since the previous copy.
    pub fn copy(&mut self, len: usize) -> Result<Bytes, CopyError> {
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
            // Too small for the seal and the mark: nothing to judge by.
            return Ok(pixels.into());
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
        if intact < MARK {
            // The mark is gone, or going.
            self.written = true;
            self.rewrites_alpha = true;
            self.seal = None;
            self.marked = 0;
            self.unwritten = 0;
            self.unwritten_since = None;
            self.put_mark()?;
        } else if self.changed(&pixels) {
            // Colours were written and no alpha.
            self.written = true;
            self.seal = None;
            self.marked += 1;
            if self.marked >= UNWRITTEN_FRAMES {
                // An emulator that rewrote the alpha here would have removed
                // the mark by now.
                self.rewrites_alpha = false;
            }
            self.unwritten = 0;
            self.unwritten_since = None;
        } else {
            self.marked += 1;
            if !self.written || self.rewrites_alpha {
                self.unwritten += 1;
                self.unwritten_since.get_or_insert_with(Instant::now);
            }
        }
        let pixels = Bytes::from(pixels);
        self.previous = Some(pixels.clone());
        Ok(pixels)
    }

    /// Whether a copy has other colours than the region had at the copy
    /// before: than the seal as long as nothing was written, than the previous
    /// copy afterwards.
    fn changed(&self, pixels: &[u8]) -> bool {
        match (&self.seal, &self.previous) {
            (Some(seal), _) => seal
                .iter()
                .enumerate()
                .any(|(index, byte)| pixels[colour(index)] != *byte),
            (None, Some(previous)) => {
                let common = previous.len().min(pixels.len());
                previous[..common] != pixels[..common]
            }
            (None, None) => false,
        }
    }

    /// Puts the seal, the only colours the client ever writes into the
    /// region, before the emulator knows of it.
    fn put_seal(&mut self) -> io::Result<()> {
        let mut seal = [0; SEAL];
        for (part, bytes) in seal.chunks_mut(8).enumerate() {
            bytes.copy_from_slice(&self.keys.hash_one(("seal", part)).to_le_bytes());
        }
        for (index, byte) in seal.iter().enumerate() {
            self.region.put(colour(index), *byte)?;
        }
        self.seal = Some(seal);
        Ok(())
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
    /// Makes the file under a name of its own for that, locks it and only
    /// then gives it the name the emulator and every client's
    /// [`remove_leftovers`] know: under that name a file is never without the
    /// lock of the client that uses it. A sweep can take the file while it is
    /// being made; then it is made again. `made` is told each name a file is
    /// made under (a test's sweep comes in there).
    fn create(directory: &Path, bytes: usize, mut made: impl FnMut(&Path)) -> io::Result<Self> {
        let mut attempt = 0;
        let frames = loop {
            let name = unique_name();
            let path = directory.join(format!("{name}{SUFFIX}"));
            let making = directory.join(format!("{name}{SUFFIX}{NEW}"));
            let mut options = OpenOptions::new();
            // `create_new` refuses every existing name, a symbolic link included.
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let file = options.open(&making)?;
            made(&making);
            match Self::hold(&file).and_then(|()| fs::rename(&making, &path)) {
                // From here on a failure removes the file again.
                Ok(()) => break Self { path, file },
                Err(error) => {
                    drop(file);
                    let _ = fs::remove_file(&making);
                    attempt += 1;
                    if attempt == 3 {
                        return Err(error);
                    }
                }
            }
        };
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
    /// from a file. It ends with the file or with this process, and it is
    /// seen wherever the directory is, also by a client for which this
    /// process's id means nothing (another container). Only where a file lock
    /// advises: on Windows it would keep the emulator from writing.
    fn hold(file: &File) -> io::Result<()> {
        if cfg!(unix) {
            file.lock()?;
        }
        Ok(())
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

/// Removes the frame files in `directory`, finished or still being made,
/// that nobody holds (see [`FrameFile::create`]). A file that can not be
/// opened or locked stays.
#[cfg(any(not(windows), test))]
fn remove_leftovers_in(directory: &Path) -> usize {
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let ours = entry.file_name().to_str().is_some_and(|name| {
            name.starts_with(PREFIX)
                && (name.ends_with(SUFFIX)
                    || name
                        .strip_suffix(NEW)
                        .is_some_and(|name| name.ends_with(SUFFIX)))
        });
        // Only files: a link under one of our names is nothing we made.
        if !ours || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let Ok(file) = File::open(&path) else {
            continue;
        };
        // The lock is kept until the name is gone.
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
    fn a_region_the_emulator_never_wrote_holds_nothing_of_a_frame_and_counts_every_one_announced() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, _) in regions(dir.path(), BYTES) {
            for announced in 1..=3 {
                // The seal in the colours of the first pixels, zeros after.
                let empty = frames.copy(BYTES).unwrap();
                let seal = frames.seal.unwrap();
                for (at, byte) in empty.iter().enumerate() {
                    let sealed = (0..SEAL).find(|index| colour(*index) == at);
                    assert_eq!(*byte, sealed.map_or(0, |index| seal[index]), "byte {at}");
                }
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
    fn the_client_writes_the_seal_once_and_then_only_alpha_bytes_of_the_first_pixels() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path(), BYTES) {
            // Before the first frame: the seal and the mark, zeros after.
            let (seal, mark) = (frames.seal.unwrap(), frames.mark);
            for (at, byte) in emulator().read(BYTES).into_iter().enumerate() {
                let sealed = (0..SEAL).find(|index| colour(*index) == at);
                let marked = (0..MARK).find(|index| alpha(*index) == at);
                let expected = sealed.map(|index| seal[index]);
                let expected = expected.or(marked.map(|index| mark[index]));
                assert_eq!(byte, expected.unwrap_or(0), "byte {at}");
            }
            assert!(seal.iter().any(|byte| *byte != 0));
            let before = picture(0x00a1_b2c3, BYTES);
            emulator().write_at(0, &before);
            assert_eq!(frames.copy(BYTES).unwrap(), before);
            assert!(frames.seal.is_none());
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
            // The whole of frame 2, and its colours say that it was written.
            assert_eq!(frames.copy(BYTES).unwrap(), picture(2, BYTES));
            assert_eq!(frames.unwritten(), 0);
            // It stays whole on a still screen. Announced again it counts:
            // this emulator rewrites the alpha, and the mark is still there.
            assert_eq!(frames.copy(BYTES).unwrap(), picture(2, BYTES));
            assert_eq!(frames.unwritten(), 1);
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
    fn an_emulator_that_writes_colours_and_no_alpha_is_known_by_its_colours() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path(), BYTES) {
            // Its first frame is black: no colour byte differs from the
            // zeros of a new region, but all differ from the seal.
            emulator().write_colours(&picture(0, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), vec![0; BYTES]);
            assert!(frames.written());
            // A still screen announced again and again: nothing says that it
            // was written, and nothing that it was not.
            for _ in 0..3 * UNWRITTEN_FRAMES {
                emulator().write_colours(&picture(0, BYTES));
                assert_eq!(frames.copy(BYTES).unwrap(), vec![0; BYTES]);
                assert_eq!(frames.unwritten(), 0);
            }
            // The alpha stays what the region had, the mark is never shown
            // and never removed.
            let mark = frames.mark;
            for value in 1..20 {
                emulator().write_colours(&picture(value, BYTES));
                assert_eq!(frames.copy(BYTES).unwrap(), translucent(value, 0, BYTES));
                assert_eq!(frames.unwritten(), 0);
            }
            assert_eq!(frames.mark, mark);
            assert!(!frames.ignored(1, Duration::ZERO));
        }
    }

    #[test]
    fn an_emulator_that_stops_rewriting_the_alpha_is_believed_once_its_colours_have_shown_it() {
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path(), BYTES) {
            emulator().write_at(0, &picture(1, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(1, BYTES));
            // From here on it writes colours only (it turned the picture, or
            // its readback on the graphics card failed once). On a screen
            // that stands still right then it looks like one that has stopped
            // writing here.
            emulator().write_colours(&picture(2, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(2, BYTES));
            for announced in 1..=UNWRITTEN_FRAMES {
                emulator().write_colours(&picture(2, BYTES));
                assert_eq!(frames.copy(BYTES).unwrap(), picture(2, BYTES));
                assert_eq!(frames.unwritten(), announced);
            }
            assert!(frames.ignored(UNWRITTEN_FRAMES, Duration::ZERO));
            // Frames that change clear that, and after more of them in a row
            // under one mark than an emulator that rewrites the alpha could
            // leave, a still screen is no longer held against it.
            for value in 3..3 + UNWRITTEN_FRAMES {
                emulator().write_colours(&picture(value, BYTES));
                assert_eq!(frames.copy(BYTES).unwrap(), picture(value, BYTES));
                assert_eq!(frames.unwritten(), 0);
            }
            let last = picture(2 + UNWRITTEN_FRAMES, BYTES);
            for _ in 0..3 * UNWRITTEN_FRAMES {
                emulator().write_colours(&last);
                assert_eq!(frames.copy(BYTES).unwrap(), last);
                assert_eq!(frames.unwritten(), 0);
            }
            // When it rewrites the alpha again, a mark that stays counts again.
            emulator().write_at(0, &picture(90, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(90, BYTES));
            assert_eq!(frames.copy(BYTES).unwrap(), picture(90, BYTES));
            assert_eq!(frames.unwritten(), 1);
        }
    }

    #[test]
    fn copies_overlapping_the_writes_of_a_writer_that_never_pauses_keep_every_colour_and_are_not_taken_for_unwritten()
     {
        // Frames large enough for a copy to fall into a write: 256x128.
        const LARGE: usize = 256 * 128 * 4;
        const PIECES: usize = 8;
        const OVERLAPS: u32 = 300;
        let dir = tempfile::tempdir().unwrap();
        for (mut frames, emulator) in regions(dir.path(), LARGE) {
            let stop = Arc::new(AtomicBool::new(false));
            // Twice the number of the last frame written, and one less while
            // that frame is being written.
            let progress = Arc::new(AtomicU32::new(0));
            let (announce, announced) = mpsc::channel();
            let writer = {
                let (stop, progress) = (stop.clone(), progress.clone());
                std::thread::spawn(move || {
                    // The stream's own long-lived borrow of the region.
                    let region = emulator();
                    let mut frame = 0;
                    while !stop.load(Ordering::Relaxed) {
                        frame += 1;
                        let pixels = picture(frame, LARGE);
                        progress.store(2 * frame - 1, Ordering::SeqCst);
                        // In pieces, with a chance for the reader between
                        // them, so that copies fall into the middle of a
                        // write on a machine with one processor too.
                        for (piece, bytes) in pixels.chunks(LARGE / PIECES).enumerate() {
                            region.write_at(piece * (LARGE / PIECES), bytes);
                            std::thread::yield_now();
                        }
                        progress.store(2 * frame, Ordering::SeqCst);
                        // As the emulator: the message follows the write.
                        if announce.send(frame).is_err() {
                            break;
                        }
                    }
                })
            };
            let started = Instant::now();
            let (mut copies, mut overlapping, mut torn) = (0, 0, 0);
            while overlapping < OVERLAPS && started.elapsed() < Duration::from_secs(60) {
                // As the reader: wait for a message, jump to the newest.
                let mut newest: u32 = announced.recv().unwrap();
                while let Ok(newer) = announced.try_recv() {
                    newest = newer;
                }
                let before = progress.load(Ordering::SeqCst);
                let pixels = frames.copy(LARGE).unwrap();
                let after = progress.load(Ordering::SeqCst);
                // Everything the region held meanwhile is at least the last
                // frame complete before and at most the last one begun after.
                let (floor, ceiling) = (before / 2, after.div_ceil(2));
                assert!(floor >= newest);
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
                // A write was going on when the copy began, or one began or
                // ended during it.
                overlapping += u32::from(before % 2 == 1 || after != before);
                torn += u32::from(low != high);
            }
            stop.store(true, Ordering::Relaxed);
            drop(announced);
            writer.join().unwrap();
            assert!(
                overlapping >= OVERLAPS,
                "only {overlapping} of {copies} copies overlapped a write"
            );
            println!("{copies} copies: {overlapping} during a write, {torn} of two frames");
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
    fn the_frame_file_has_the_frame_size_and_one_name_and_goes_with_the_region() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = 3 * 64 * 1024 + 4;
        let frames = SharedFrames::in_directory(dir.path(), bytes).unwrap();
        let path = PathBuf::from(frames.handle().strip_prefix("file://").unwrap());
        assert_eq!(fs::metadata(&path).unwrap().len(), bytes as u64);
        // Nothing is left of the name it was made under.
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].path(), path);
        // Zeros behind the first pixels, so no earlier picture can show
        // through.
        let content = fs::read(&path).unwrap();
        assert!(content[MARK_SPAN..].iter().all(|byte| *byte == 0));
        drop(frames);
        assert!(!path.exists());
        // A directory that does not exist is an error for the caller, who
        // continues without shared memory.
        assert!(SharedFrames::in_directory(&dir.path().join("missing"), bytes).is_err());
    }

    #[test]
    fn a_sweep_that_takes_a_frame_file_while_it_is_made_costs_one_more_attempt() {
        let dir = tempfile::tempdir().unwrap();
        // Another client's sweep finds the file between its creation and its
        // lock, the first two times.
        let mut swept = vec![];
        let file = FrameFile::create(dir.path(), BYTES, |making| {
            if swept.len() < 2 {
                swept.push((making.to_path_buf(), remove_leftovers_in(dir.path())));
            }
        })
        .unwrap();
        assert_eq!(swept.len(), 2);
        for (making, removed) in &swept {
            assert_eq!(*removed, 1);
            assert!(!making.exists());
        }
        // The third file is whole, under the name the emulator is given.
        assert_eq!(fs::metadata(&file.path).unwrap().len(), BYTES as u64);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        drop(file);
        // And a sweep that never lets go is an error for the caller.
        let failed = FrameFile::create(dir.path(), BYTES, |_| {
            remove_leftovers_in(dir.path());
        });
        assert!(failed.is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
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
        // Left behind by clients that are gone: a finished file, one that was
        // still being made, and one of a build that put a counter after the
        // process id.
        let left = frame_file();
        let unfinished = dir.path().join(format!("{}{SUFFIX}{NEW}", unique_name()));
        let older = dir.path().join(format!("{PREFIX}4242-17f3a-0{SUFFIX}"));
        let unrelated = dir.path().join("rustemudroid-other.rgba");
        let other_kind = dir.path().join(format!("{PREFIX}1-0.txt"));
        let other_new = dir.path().join(format!("{PREFIX}1-0.txt{NEW}"));
        let folder = frame_file();
        for path in [
            &left,
            &unfinished,
            &older,
            &unrelated,
            &other_kind,
            &other_new,
        ] {
            fs::write(path, b"x").unwrap();
        }
        fs::create_dir(&folder).unwrap();
        assert_eq!(remove_leftovers_in(dir.path()), 3);
        assert!(!left.exists() && !unfinished.exists() && !older.exists());
        assert!(in_use.exists() && unrelated.exists() && folder.exists());
        assert!(other_kind.exists() && other_new.exists());
        // The client is gone: so is its lock, and then its file.
        drop(holder);
        assert_eq!(remove_leftovers_in(dir.path()), 1);
        assert!(!in_use.exists());
        assert_eq!(remove_leftovers_in(&dir.path().join("missing")), 0);
    }
}
