//! A stand-in for the emulator's `streamScreenshot`, served over gRPC on the
//! loopback interface, that keeps the habits of the real one (the engine's
//! `EmulatorService.cpp`, `SharedMemoryLibrary.cpp` and `SharedMemory_*.cpp`):
//!
//! * It has a thread of its own: its gRPC server, its guest and every write
//!   into a region run there, at their own pace, while the client's thread
//!   copies. Nothing tells the client when a write begins or ends.
//! * A stream sends nothing while the guest has no frame (Android still
//!   boots, or the screen is off: `getScreenshot` answers
//!   `FAILED_PRECONDITION`, which the stream swallows). Until its first frame
//!   it asks again by itself; then it sends the current picture at once and
//!   one message per frame the guest posts.
//! * Every frame is produced by `getScreenshot`, which borrows the region for
//!   the length of the call. The stream takes its own, long-lived borrow only
//!   after its first frame, so between the first frame and the second the
//!   region is closed and opened again.
//! * A handle that is not a `file://` URL is named shared memory: on Windows
//!   the section `SHM_<handle>`, opened here with the calls of
//!   `SharedMemory_win32.cpp` and none of the client's code.
//! * A file handle must be an absolute path that resolves into one of the
//!   allowed directories and is a regular file or nothing, never a link.
//!   Otherwise it is refused with the emulator's words.
//! * Closing a file-backed region deletes the file on Windows, always. On
//!   POSIX the file is created only when it is missing, refused when it has
//!   another hard link, grown to the frame's size, and removed only if the
//!   emulator created it.
//! * A region that can not be opened or mapped is `INVALID_ARGUMENT`.
//! * On request it writes as the readback does that rotates or scales on the
//!   processor: the red, green and blue of every pixel and no alpha.
//!
//! What it is not: the "POSIX" rules are the emulator's choices of what to
//! open, create and delete, run on the file system of the test machine, so a
//! Windows run shows nothing about tmpfs, unlinked files or pages that are
//! not allocated. Its writes are one plain copy from the first byte to the
//! last; the engine's come out of a GPU readback.
#![allow(clippy::result_large_err)]
use crate::display::engine_rpc::{Format, Frame, Rotation, Transport};
use crate::display::shared_frames::MMAP;
use hyper_util::rt::TokioIo;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    future::Future,
    io::{self, Read, Seek, SeekFrom, Write},
    net::SocketAddr,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time,
};
use tonic::{
    Code, Status,
    body::BoxBody,
    client::Grpc,
    codec::ProstCodec,
    codegen::{
        Service, http,
        tokio_stream::wrappers::{ReceiverStream, TcpListenerStream},
    },
    server::{NamedService, ServerStreamingService},
    transport::{Channel, Endpoint, Server},
};

/// How often a stream that has not sent a frame yet asks for one again (the
/// engine: every 125 ms).
const RETRY: Duration = Duration::from_millis(5);

/// A whole frame as the emulator writes it: opaque pixels of one colour, the
/// number of the frame.
pub(crate) fn picture(value: u32, bytes: usize) -> Vec<u8> {
    let [red, green, blue, _] = value.to_le_bytes();
    [red, green, blue, 255].repeat(bytes / 4)
}

/// What is left of a frame when its alpha was never written: the zeros of
/// the buffer.
fn without_alpha(pixels: &mut [u8]) {
    pixels.iter_mut().skip(3).step_by(4).for_each(|a| *a = 0);
}

/// How the emulator of each system treats a file-backed region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Files {
    /// Opens or creates the file, sharing reads and writes but not deletion,
    /// and deletes it whenever it closes it.
    Windows,
    /// Opens the file, creates it only when it is missing, and removes only
    /// a file it created.
    Posix,
}

/// A region as the emulator holds it open (its `SharedMemory`).
pub(crate) enum Region {
    File {
        file: Option<File>,
        path: PathBuf,
        /// There was no such file: the emulator made this one for itself.
        created: bool,
        delete: bool,
        size: usize,
    },
    #[cfg(windows)]
    Named(Section),
    /// Opening or mapping failed; `getScreenshot` then answers
    /// `INVALID_ARGUMENT`.
    Unmapped,
}

impl Region {
    fn size(&self) -> usize {
        match self {
            Self::File { size, .. } => *size,
            #[cfg(windows)]
            Self::Named(section) => section.bytes,
            Self::Unmapped => 0,
        }
    }

    /// Whether this is the region the client created and not one the
    /// emulator made under that name.
    pub fn is_the_clients(&self) -> bool {
        match self {
            Self::File { created, .. } => !created,
            #[cfg(windows)]
            Self::Named(section) => section.existed,
            Self::Unmapped => false,
        }
    }

    pub fn write_at(&self, offset: usize, bytes: &[u8]) {
        match self {
            Self::File { file, .. } => {
                let mut file = file.as_ref().unwrap();
                file.seek(SeekFrom::Start(offset as u64)).unwrap();
                file.write_all(bytes).unwrap();
            }
            #[cfg(windows)]
            Self::Named(section) => section.write_at(offset, bytes),
            Self::Unmapped => {}
        }
    }

    /// As the readback that leaves the alpha alone: the red, green and blue
    /// of every pixel of `pixels` and nothing else.
    pub fn write_colours(&self, pixels: &[u8]) {
        for (pixel, bytes) in pixels.chunks_exact(4).enumerate() {
            self.write_at(pixel * 4, &bytes[..3]);
        }
    }

    /// The first `len` bytes of the region as the emulator sees them.
    pub fn read(&self, len: usize) -> Vec<u8> {
        let mut bytes = vec![0; len];
        match self {
            Self::File { file, .. } => {
                let mut file = file.as_ref().unwrap();
                file.seek(SeekFrom::Start(0)).unwrap();
                file.read_exact(&mut bytes).unwrap();
            }
            #[cfg(windows)]
            Self::Named(section) => section.read(&mut bytes),
            Self::Unmapped => {}
        }
        bytes
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        if let Self::File {
            file, path, delete, ..
        } = self
        {
            // Close, then delete; a file somebody else holds open without
            // sharing deletion stays, and nobody is told.
            drop(file.take());
            if *delete {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// The named kind of `SharedMemory_win32.cpp`: the section `SHM_<handle>`,
/// opened if there is one and otherwise created in the paging file, and a
/// writable view of the size asked for.
#[cfg(windows)]
pub(crate) struct Section {
    section: windows_sys::Win32::Foundation::HANDLE,
    view: *mut u8,
    bytes: usize,
    /// The section was there already: it is the client's.
    pub existed: bool,
}

// SAFETY: a section handle and its view belong to the process, not to a thread.
#[cfg(windows)]
unsafe impl Send for Section {}

#[cfg(windows)]
impl Section {
    pub fn open(handle: &str, bytes: usize) -> Option<Self> {
        use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
        use windows_sys::Win32::{
            Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
            System::Memory::{CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE},
        };
        let name: Vec<u16> = OsStr::new("SHM_")
            .encode_wide()
            .chain(OsStr::new(handle).encode_wide())
            .chain([0])
            .collect();
        let size = bytes as u64;
        // SAFETY: plain Win32 calls with a NUL-terminated name; the handle and
        // the view are owned by the returned value, which releases both.
        unsafe {
            // No file: the engine's file handle is still null here.
            let section = CreateFileMappingW(
                std::ptr::null_mut(),
                std::ptr::null(),
                PAGE_READWRITE,
                (size >> 32) as u32,
                size as u32,
                name.as_ptr(),
            );
            if section.is_null() {
                return None;
            }
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            let view = MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, bytes).Value;
            if view.is_null() {
                // As the engine: it has not stored the handle yet, so its
                // `close` leaves the section open for as long as it runs.
                return None;
            }
            Some(Self {
                section,
                view: view.cast(),
                bytes,
                existed,
            })
        }
    }

    fn write_at(&self, offset: usize, bytes: &[u8]) {
        assert!(offset + bytes.len() <= self.bytes);
        // SAFETY: inside the view, which is mapped while `self` lives; the
        // client may read or write the same memory meanwhile, so the bytes
        // are copied through raw pointers.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.view.add(offset), bytes.len()) }
    }

    fn read(&self, buffer: &mut [u8]) {
        assert!(buffer.len() <= self.bytes);
        // SAFETY: as in `write_at`.
        unsafe { std::ptr::copy_nonoverlapping(self.view, buffer.as_mut_ptr(), buffer.len()) }
    }
}

#[cfg(windows)]
impl Drop for Section {
    fn drop(&mut self) {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Memory::{MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile},
        };
        // SAFETY: the view and the handle came from `open` and are released once.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.cast(),
            });
            CloseHandle(self.section);
        }
    }
}

#[derive(Default)]
struct State {
    /// The guest's current frame, none before its first one.
    picture: Option<u32>,
    /// The screen is off: `getScreenshot` has no frame for anybody.
    dark: bool,
    /// How the device is turned (`Rotation.SkinRotation`).
    rotation: i32,
    /// The guest draws by itself, a new frame after every pause this long.
    pace: Option<Duration>,
    /// A stream has read the guest's current frame.
    taken: bool,
    stopped: bool,
    /// Regions by handle with their number of borrows.
    library: HashMap<String, (usize, Region)>,
    /// One entry per stream: whether it asked for shared memory.
    subscriptions: Vec<bool>,
    handles: Vec<String>,
    /// Messages sent for frames written into a region.
    announced: u64,
    ignore_transport: bool,
    misread_handles: bool,
    refuse_next: Option<(Code, String)>,
    /// Frames are written without their alpha, inside the messages too.
    colours_only: bool,
    /// `getScreenshot` never has a frame for a shared-memory request.
    silent_shared: bool,
    /// The screen goes off when the next shared-memory stream begins.
    dark_at_shared: bool,
}

pub(crate) struct FakeEngine {
    native: [u32; 2],
    files: Files,
    allowed: Vec<PathBuf>,
    state: Mutex<State>,
    /// Events for the streams and the guest: a frame was posted, the screen
    /// or the device changed, or the VM stopped.
    events: watch::Sender<u64>,
    /// The engine's thread and what ends it.
    thread: Mutex<Option<(oneshot::Sender<()>, std::thread::JoinHandle<()>)>>,
    /// A second handle of the client's socket.
    client: Arc<Mutex<Option<std::net::TcpStream>>>,
}

/// Connects the client by TCP, as tonic does by itself, and keeps a second
/// handle of that socket for [`FakeEngine::announcement_waits_at_the_client`].
struct Tapped {
    address: SocketAddr,
    kept: Arc<Mutex<Option<std::net::TcpStream>>>,
}

impl Service<http::Uri> for Tapped {
    type Response = TokioIo<tokio::net::TcpStream>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<Self::Response>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: http::Uri) -> Self::Future {
        let (address, kept) = (self.address, self.kept.clone());
        Box::pin(async move {
            let stream = tokio::net::TcpStream::connect(address).await?;
            stream.set_nodelay(true)?;
            let stream = stream.into_std()?;
            *kept.lock().unwrap() = Some(stream.try_clone()?);
            Ok(TokioIo::new(tokio::net::TcpStream::from_std(stream)?))
        })
    }
}

/// The stream's own borrow of its region.
struct Borrow {
    engine: Arc<FakeEngine>,
    handle: String,
}

impl Drop for Borrow {
    fn drop(&mut self) {
        FakeEngine::release(&mut self.engine.state.lock().unwrap(), &self.handle);
    }
}

impl FakeEngine {
    pub fn new(
        native: [u32; 2],
        files: Files,
        allowed: impl IntoIterator<Item = PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            native,
            files,
            // Resolved once, as the engine does.
            allowed: allowed
                .into_iter()
                .filter_map(|directory| directory.canonicalize().ok())
                .collect(),
            state: Mutex::default(),
            events: watch::Sender::new(0),
            thread: Mutex::default(),
            client: Arc::default(),
        })
    }

    fn notify(&self) {
        self.events.send_modify(|events| *events += 1);
    }

    /// Bytes of a frame of the native raster.
    pub fn frame_bytes(&self) -> usize {
        (self.native[0] * self.native[1] * 4) as usize
    }

    /// A frame of the native raster as it reaches the client from this
    /// engine: with the alpha the guest gave it, or with none.
    pub fn frame(&self, value: u32) -> Vec<u8> {
        let mut pixels = picture(value, self.frame_bytes());
        if self.state.lock().unwrap().colours_only {
            without_alpha(&mut pixels);
        }
        pixels
    }

    /// The guest posts a frame.
    pub fn post(&self, value: u32) {
        self.state.lock().unwrap().picture = Some(value);
        self.notify();
    }

    /// The guest draws by itself from now on, in the engine's thread: frame
    /// after frame with `pause` between them (none: a new frame as soon as a
    /// stream has read the one before).
    pub fn draw(&self, pause: Duration) {
        self.state.lock().unwrap().pace = Some(pause);
        self.notify();
    }

    /// The guest stops drawing by itself; its last frame stays.
    pub fn rest(&self) {
        self.state.lock().unwrap().pace = None;
    }

    /// The number of the guest's newest frame.
    pub fn drawn(&self) -> u32 {
        self.state.lock().unwrap().picture.unwrap_or(0)
    }

    /// The screen goes off (`getScreenshot` has no frame for anybody, every
    /// stream is silent) or comes back with the picture it had.
    pub fn screen(&self, on: bool) {
        self.state.lock().unwrap().dark = !on;
        self.notify();
    }

    /// The device is turned (an odd value swaps the sides of the picture) and
    /// the guest posts its first frame for the new position.
    pub fn turn_and_post(&self, rotation: i32, value: u32) {
        {
            let mut state = self.state.lock().unwrap();
            state.rotation = rotation;
            state.picture = Some(value);
        }
        self.notify();
    }

    /// The VM stops: every stream ends.
    pub fn stop(&self) {
        self.state.lock().unwrap().stopped = true;
        self.notify();
    }

    /// The pixels stay inside the messages whatever the request says.
    pub fn ignore_transport(&self) {
        self.state.lock().unwrap().ignore_transport = true;
    }

    /// The emulator reads another path out of the handle than the client
    /// meant (as with a path that is not valid UTF-8).
    pub fn misread_handles(&self) {
        self.state.lock().unwrap().misread_handles = true;
    }

    /// The next `getScreenshot` of a shared-memory stream answers this.
    pub fn refuse_next(&self, code: Code, message: &str) {
        self.state.lock().unwrap().refuse_next = Some((code, message.into()));
    }

    /// Every frame is written as by the readback that leaves the alpha alone:
    /// colours only, into a region and into a message (whose buffer starts as
    /// zeros) alike.
    pub fn write_colours_only(&self) {
        self.state.lock().unwrap().colours_only = true;
    }

    /// `getScreenshot` never has a frame for a shared-memory request: such a
    /// stream sends nothing, whatever the guest draws.
    pub fn silence_shared(&self) {
        self.state.lock().unwrap().silent_shared = true;
    }

    /// The screen goes off at the moment the next shared-memory stream begins.
    pub fn darken_on_shared(&self) {
        self.state.lock().unwrap().dark_at_shared = true;
    }

    /// Whether a message that announces a frame in the newest region lies in
    /// the client's socket: received by the system and not yet read by the
    /// client. A test that holds the client inside a copy knows by this that
    /// the client's transport has the message at its next turn.
    pub fn announcement_waits_at_the_client(&self) -> bool {
        let Some(handle) = self.handles().pop() else {
            return false;
        };
        let kept = self.client.lock().unwrap();
        let Some(socket) = kept.as_ref() else {
            return false;
        };
        // The handle is the last field of the message.
        let mut waiting = vec![0; 1 << 16];
        socket.peek(&mut waiting).is_ok_and(|read| {
            waiting[..read]
                .windows(handle.len())
                .any(|bytes| bytes == handle.as_bytes())
        })
    }

    /// Whether each stream so far asked for shared memory, in order.
    pub fn subscriptions(&self) -> Vec<bool> {
        self.state.lock().unwrap().subscriptions.clone()
    }

    /// The handles of the shared-memory streams so far, in order.
    pub fn handles(&self) -> Vec<String> {
        self.state.lock().unwrap().handles.clone()
    }

    /// How many messages were sent for frames written into a region.
    pub fn announced(&self) -> u64 {
        self.state.lock().unwrap().announced
    }

    /// Starts the engine in its own thread and returns a client connected to
    /// it, for the runtime this is called in.
    pub async fn serve(self: &Arc<Self>) -> Grpc<Channel> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let engine = self.clone();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                tokio::spawn(engine.clone().guest());
                let server = Server::builder()
                    .add_service(Controller(engine))
                    .serve_with_incoming(TcpListenerStream::new(listener));
                tokio::select! {
                    served = server => served.unwrap(),
                    _ = stopped => {}
                }
            });
            // The runtime goes, and with it every stream and its regions.
        });
        *self.thread.lock().unwrap() = Some((stop, thread));
        let channel = Endpoint::from_shared(format!("http://{address}"))
            .unwrap()
            .connect_with_connector(Tapped {
                address,
                kept: self.client.clone(),
            })
            .await
            .unwrap();
        Grpc::new(channel)
    }

    /// Ends the engine's thread. Every region the engine had open is closed
    /// when this returns.
    pub fn shut_down(&self) {
        let thread = self.thread.lock().unwrap().take();
        if let Some((stop, thread)) = thread {
            let _ = stop.send(());
            thread.join().unwrap();
        }
    }

    /// The guest, in the engine's thread.
    async fn guest(self: Arc<Self>) {
        let mut events = self.events.subscribe();
        loop {
            // Decided and done under one lock: no frame after `rest`.
            let (pace, posted) = {
                let mut state = self.state.lock().unwrap();
                // Without a pause: the next frame as soon as a stream has
                // read this one.
                let taken = state.taken;
                let posts = state.pace.is_some_and(|pause| !pause.is_zero() || taken);
                if posts {
                    state.picture = Some(state.picture.map_or(1, |frame| frame + 1));
                    state.taken = false;
                }
                (state.pace, posts)
            };
            if posted {
                self.notify();
            }
            let Some(pause) = pace else {
                if events.changed().await.is_err() {
                    return;
                }
                continue;
            };
            if pause.is_zero() {
                tokio::task::yield_now().await;
            } else {
                time::sleep(pause).await;
            }
        }
    }

    /// `SharedMemoryLibrary` opening a region: what the handle names, checked
    /// and opened as the emulator of `self.files` does.
    pub fn open(&self, handle: &str, size: usize, misread: bool) -> Result<Region, Status> {
        let Some(path) = handle.strip_prefix("file://") else {
            // A name. `open`, and `create` when that gave no mapped region.
            #[cfg(windows)]
            return Ok(Section::open(handle, size)
                .or_else(|| Section::open(handle, size))
                .map_or(Region::Unmapped, Region::Named));
            // POSIX shared memory, which the client never asks for.
            #[cfg(not(windows))]
            return Ok(Region::Unmapped);
        };
        let path = PathBuf::from(if misread {
            format!("{path}~")
        } else {
            path.to_string()
        });
        let rejected = |why: String| format!("Rejected shared memory handle '{handle}': {why}");
        // A local absolute path, with every link of its directory resolved.
        let resolved = path
            .parent()
            .filter(|_| path.is_absolute())
            .and_then(|directory| directory.canonicalize().ok())
            .zip(path.file_name())
            .map(|(directory, name)| directory.join(name));
        let Some(resolved) = resolved else {
            return Err(Status::invalid_argument(rejected(format!(
                "unable to resolve path '{}'.",
                path.display()
            ))));
        };
        if !self.allowed.iter().any(|dir| resolved.starts_with(dir)) {
            return Err(Status::permission_denied(rejected(format!(
                "it resolves to '{}', which is not inside a directory that the emulator allows \
                 for shared memory regions.",
                resolved.display()
            ))));
        }
        match fs::symlink_metadata(&resolved) {
            Ok(found) if found.file_type().is_symlink() => {
                return Err(Status::permission_denied(rejected(format!(
                    "'{}' is a symbolic link, which is not allowed.",
                    resolved.display()
                ))));
            }
            Ok(found) if !found.is_file() => {
                return Err(Status::invalid_argument(rejected(format!(
                    "it resolves to '{}', which exists but is not a regular file.",
                    resolved.display()
                ))));
            }
            _ => {}
        }
        let unable_to_open = |error: io::Error| {
            Status::invalid_argument(rejected(format!(
                "unable to open '{}' ({error}).",
                resolved.display()
            )))
        };
        let grow = |file: &File| {
            if file.metadata().unwrap().len() < size as u64 {
                file.set_len(size as u64).unwrap();
            }
        };
        match self.files {
            Files::Windows => {
                let created = !resolved.exists();
                let mut options = OpenOptions::new();
                options.read(true).write(true).create(true);
                // FILE_SHARE_READ | FILE_SHARE_WRITE, as the emulator asks.
                #[cfg(windows)]
                std::os::windows::fs::OpenOptionsExt::share_mode(&mut options, 3);
                Ok(match options.open(&resolved) {
                    Ok(file) => {
                        grow(&file);
                        Region::File {
                            file: Some(file),
                            path: resolved,
                            created,
                            delete: true,
                            size,
                        }
                    }
                    Err(_) => Region::Unmapped,
                })
            }
            Files::Posix => {
                let mut options = OpenOptions::new();
                options.read(true).write(true);
                let (file, created) = match options.open(&resolved) {
                    Ok(file) => (file, false),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        match options.create_new(true).open(&resolved) {
                            Ok(file) => (file, true),
                            Err(error) => return Err(unable_to_open(error)),
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                        return Err(Status::permission_denied(rejected(format!(
                            "the emulator is not allowed to open '{}' for reading and writing \
                             ({error}).",
                            resolved.display()
                        ))));
                    }
                    Err(error) => return Err(unable_to_open(error)),
                };
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if !created && file.metadata().unwrap().nlink() != 1 {
                        return Err(Status::permission_denied(rejected(format!(
                            "'{}' has multiple hard links, which is not allowed.",
                            resolved.display()
                        ))));
                    }
                }
                grow(&file);
                Ok(Region::File {
                    file: Some(file),
                    path: resolved,
                    created,
                    delete: created,
                    size,
                })
            }
        }
    }

    /// `SharedMemoryLibrary::borrow`: opens the region when it is not open,
    /// not mapped or too small, and counts the borrow.
    fn borrow(&self, state: &mut State, handle: &str, size: usize) -> Result<(), Status> {
        let reopen = state
            .library
            .get(handle)
            .is_none_or(|(_, region)| matches!(region, Region::Unmapped) || region.size() < size);
        if reopen {
            let region = self.open(handle, size, state.misread_handles)?;
            let borrows = state.library.get(handle).map_or(0, |(borrows, _)| *borrows);
            state.library.insert(handle.to_string(), (borrows, region));
        }
        state.library.get_mut(handle).unwrap().0 += 1;
        Ok(())
    }

    /// `SharedMemoryLibrary::release`: the last borrow closes the region.
    fn release(state: &mut State, handle: &str) {
        if let Some(entry) = state.library.get_mut(handle) {
            entry.0 -= 1;
            if entry.0 == 0 {
                state.library.remove(handle);
            }
        }
    }

    fn screenshot(&self, request: &Format) -> Result<Frame, Status> {
        let no_frame = || Status::failed_precondition("The guest has not posted new frame yet");
        let mut state = self.state.lock().unwrap();
        let Some(value) = state.picture.filter(|_| !state.dark) else {
            return Err(no_frame());
        };
        state.taken = true;
        // The picture turns with the device.
        let mut native = self.native;
        if state.rotation % 2 != 0 {
            native.swap(0, 1);
        }
        let [width, height] = if request.width == 0 || request.height == 0 {
            native
        } else {
            [request.width.min(native[0]), request.height.min(native[1])]
        };
        let bytes = (width * height * 4) as usize;
        let mut format = Format {
            format: 1,
            width,
            height,
            rotation: Some(Rotation {
                value: state.rotation,
            }),
            ..Default::default()
        };
        let handle = request
            .transport
            .as_ref()
            .filter(|transport| transport.channel == MMAP && !state.ignore_transport)
            .map(|transport| transport.handle.clone());
        let mut pixels = picture(value, bytes);
        let Some(handle) = handle else {
            if state.colours_only {
                without_alpha(&mut pixels);
            }
            return Ok(Frame {
                format: Some(format),
                pixels: pixels.into(),
            });
        };
        if state.silent_shared {
            return Err(no_frame());
        }
        if let Some((code, message)) = state.refuse_next.take() {
            return Err(Status::new(code, message));
        }
        // Borrowed here, released when this call returns.
        self.borrow(&mut state, &handle, bytes)?;
        let written = match &state.library[&handle].1 {
            Region::Unmapped => Err(Status::invalid_argument(format!(
                "Unable to open or map shared memory region: {handle}"
            ))),
            region if bytes > region.size() => Err(Status::failed_precondition(format!(
                "The shared memory region needs to have a size of at least: {bytes}"
            ))),
            region if state.colours_only => {
                region.write_colours(&pixels);
                Ok(())
            }
            region => {
                region.write_at(0, &pixels);
                Ok(())
            }
        };
        Self::release(&mut state, &handle);
        written?;
        format.transport = Some(Transport {
            channel: MMAP,
            handle,
        });
        Ok(Frame {
            format: Some(format),
            pixels: Default::default(),
        })
    }

    async fn stream(
        self: Arc<Self>,
        request: Format,
        replies: mpsc::Sender<Result<Frame, Status>>,
    ) {
        let mut events = self.events.subscribe();
        // Taken after the first frame and kept until the stream ends.
        let mut own: Option<Borrow> = None;
        let mut first = true;
        let mut wait = false;
        loop {
            if wait {
                tokio::select! {
                    changed = events.changed() => if changed.is_err() { return },
                    () = replies.closed() => return,
                    () = time::sleep(RETRY), if first => {}
                }
            }
            wait = true;
            if self.state.lock().unwrap().stopped {
                return;
            }
            let reply = match self.screenshot(&request) {
                Ok(reply) => reply,
                Err(status) if status.code() == Code::FailedPrecondition => {
                    own = None;
                    continue;
                }
                Err(status) => {
                    let _ = replies.send(Err(status)).await;
                    return;
                }
            };
            first = false;
            let shared = reply.format.as_ref().and_then(|format| {
                let transport = format.transport.as_ref()?;
                Some((transport.handle.clone(), format.width * format.height * 4))
            });
            let announces = shared.is_some();
            if let (None, Some((handle, bytes))) = (&own, shared) {
                let borrowed = {
                    let mut state = self.state.lock().unwrap();
                    self.borrow(&mut state, &handle, bytes as usize)
                };
                if let Err(status) = borrowed {
                    let _ = replies.send(Err(status)).await;
                    return;
                }
                own = Some(Borrow {
                    engine: self.clone(),
                    handle,
                });
            }
            if replies.send(Ok(reply)).await.is_err() {
                return;
            }
            if announces {
                self.state.lock().unwrap().announced += 1;
            }
        }
    }
}

#[derive(Clone)]
struct Controller(Arc<FakeEngine>);

impl NamedService for Controller {
    const NAME: &'static str = "android.emulation.control.EmulatorController";
}

impl Service<http::Request<BoxBody>> for Controller {
    type Response = http::Response<BoxBody>;
    type Error = std::convert::Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<BoxBody>) -> Self::Future {
        let engine = self.0.clone();
        Box::pin(async move {
            // Every call is taken as streamScreenshot, the only one the video
            // worker makes.
            let mut grpc = tonic::server::Grpc::new(ProstCodec::<Frame, Format>::default());
            Ok(grpc.server_streaming(Screens(engine), request).await)
        })
    }
}

struct Screens(Arc<FakeEngine>);

impl ServerStreamingService<Format> for Screens {
    type Response = Frame;
    type ResponseStream = ReceiverStream<Result<Frame, Status>>;
    type Future = std::future::Ready<Result<tonic::Response<Self::ResponseStream>, Status>>;

    fn call(&mut self, request: tonic::Request<Format>) -> Self::Future {
        let request = request.into_inner();
        let (replies, stream) = mpsc::channel(64);
        {
            let mut state = self.0.state.lock().unwrap();
            let handle = request
                .transport
                .as_ref()
                .filter(|transport| transport.channel == MMAP)
                .map(|transport| transport.handle.clone());
            state.subscriptions.push(handle.is_some());
            if handle.is_some() && std::mem::take(&mut state.dark_at_shared) {
                state.dark = true;
            }
            state.handles.extend(handle);
        }
        tokio::spawn(self.0.clone().stream(request, replies));
        std::future::ready(Ok(tonic::Response::new(ReceiverStream::new(stream))))
    }
}
