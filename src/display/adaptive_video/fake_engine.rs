//! A stand-in for the emulator's `streamScreenshot`, served over gRPC on the
//! loopback interface, that keeps the habits of the real one (the engine's
//! `EmulatorService.cpp`, `SharedMemoryLibrary.cpp` and `SharedMemory_*.cpp`):
//!
//! * A stream sends nothing until the guest has a frame, then the current
//!   picture at once and one message per frame the guest posts.
//! * Every frame is produced by `getScreenshot`, which borrows the region for
//!   the length of the call. The stream takes its own, long-lived borrow only
//!   after its first frame, so between the first frame and the second the
//!   region is closed and opened again.
//! * Closing a file-backed region deletes the file on Windows, always; on
//!   POSIX only a file the emulator created itself is removed. A handle that
//!   is not a `file://` URL is a named mapping on Windows.
//! * A file outside the allowed directories is refused with the emulator's
//!   words; a region that can not be opened or mapped is `INVALID_ARGUMENT`.
//! * `FAILED_PRECONDITION` from `getScreenshot` is swallowed by the stream.
#![allow(clippy::result_large_err)]
use super::super::{Format, Frame, Rotation, Transport};
use crate::display::shared_frames::MMAP;
#[cfg(windows)]
use crate::display::shared_frames::Mapping;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    future::Future,
    io::{self, Seek, SeekFrom, Write},
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::sync::{mpsc, watch};
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

/// A whole frame as the emulator writes it: opaque pixels of one value.
pub(crate) fn picture(value: u8, bytes: usize) -> Vec<u8> {
    [value, value, value, 255].repeat(bytes / 4)
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

enum Region {
    File {
        file: Option<File>,
        path: PathBuf,
        delete: bool,
        size: usize,
    },
    #[cfg(windows)]
    Named(Mapping, usize),
    /// Opening or mapping failed; `getScreenshot` then answers
    /// `INVALID_ARGUMENT`.
    Unmapped,
}

impl Region {
    fn size(&self) -> usize {
        match self {
            Self::File { size, .. } => *size,
            #[cfg(windows)]
            Self::Named(_, size) => *size,
            Self::Unmapped => 0,
        }
    }

    fn write(&mut self, pixels: &[u8]) {
        match self {
            Self::File { file, .. } => {
                let mut file = file.as_ref().unwrap();
                file.seek(SeekFrom::Start(0)).unwrap();
                file.write_all(pixels).unwrap();
            }
            #[cfg(windows)]
            Self::Named(mapping, _) => mapping.write(pixels),
            Self::Unmapped => {}
        }
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

#[derive(Default)]
struct State {
    /// The guest's current frame.
    picture: Option<u8>,
    stopped: bool,
    /// Regions by handle with their number of borrows.
    library: HashMap<String, (usize, Region)>,
    /// One entry per stream: whether it asked for shared memory.
    subscriptions: Vec<bool>,
    handles: Vec<String>,
    ignore_transport: bool,
    misread_handles: bool,
    starve_shared: bool,
    refuse_next: Option<(Code, String)>,
}

pub(crate) struct FakeEngine {
    native: [u32; 2],
    files: Files,
    allowed: Vec<PathBuf>,
    state: Mutex<State>,
    /// Frame events: the guest posted a frame, or the VM stopped.
    posted: watch::Sender<u64>,
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
            allowed: allowed.into_iter().collect(),
            state: Mutex::default(),
            posted: watch::Sender::new(0),
        })
    }

    /// Bytes of a frame of the native raster.
    pub fn frame_bytes(&self) -> usize {
        (self.native[0] * self.native[1] * 4) as usize
    }

    /// The guest posts a frame.
    pub fn post(&self, value: u8) {
        self.state.lock().unwrap().picture = Some(value);
        self.posted.send_modify(|events| *events += 1);
    }

    /// The VM stops: every stream ends.
    pub fn stop(&self) {
        self.state.lock().unwrap().stopped = true;
        self.posted.send_modify(|events| *events += 1);
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

    /// `getScreenshot` has no frame for shared-memory requests, as while the
    /// guest has not posted one: their streams send nothing.
    pub fn starve_shared(&self) {
        self.state.lock().unwrap().starve_shared = true;
    }

    /// The next `getScreenshot` of a shared-memory stream answers this.
    pub fn refuse_next(&self, code: Code, message: &str) {
        self.state.lock().unwrap().refuse_next = Some((code, message.into()));
    }

    /// Whether each stream so far asked for shared memory, in order.
    pub fn subscriptions(&self) -> Vec<bool> {
        self.state.lock().unwrap().subscriptions.clone()
    }

    /// The handles of the shared-memory streams so far, in order.
    pub fn handles(&self) -> Vec<String> {
        self.state.lock().unwrap().handles.clone()
    }

    /// Starts serving and returns a connected client.
    pub async fn serve(self: Arc<Self>) -> Grpc<Channel> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(
            Server::builder()
                .add_service(Controller(self))
                .serve_with_incoming(TcpListenerStream::new(listener)),
        );
        let channel = Endpoint::from_shared(format!("http://{address}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        Grpc::new(channel)
    }

    fn open(&self, handle: &str, size: usize, misread: bool) -> Result<Region, Status> {
        let Some(path) = handle.strip_prefix("file://") else {
            #[cfg(windows)]
            return Ok(match Mapping::named(handle, size) {
                Ok((mapping, _)) => Region::Named(mapping, size),
                Err(_) => Region::Unmapped,
            });
            #[cfg(not(windows))]
            return Ok(Region::Unmapped);
        };
        let path = PathBuf::from(if misread {
            format!("{path}~")
        } else {
            path.to_string()
        });
        if !self.allowed.iter().any(|dir| path.starts_with(dir)) {
            return Err(Status::permission_denied(format!(
                "Rejected shared memory handle '{handle}': it resolves to '{}', which is not \
                 inside a directory that the emulator allows for shared memory regions.",
                path.display()
            )));
        }
        let grow = |file: &File| {
            if file.metadata().unwrap().len() < size as u64 {
                file.set_len(size as u64).unwrap();
            }
        };
        match self.files {
            Files::Windows => {
                let mut options = OpenOptions::new();
                options.read(true).write(true).create(true);
                // FILE_SHARE_READ | FILE_SHARE_WRITE, as the emulator asks.
                #[cfg(windows)]
                std::os::windows::fs::OpenOptionsExt::share_mode(&mut options, 3);
                Ok(match options.open(&path) {
                    Ok(file) => {
                        grow(&file);
                        Region::File {
                            file: Some(file),
                            path,
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
                let (file, created) = match options.open(&path) {
                    Ok(file) => (file, false),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        match options.create_new(true).open(&path) {
                            Ok(file) => (file, true),
                            Err(error) => return Err(unable_to_open(handle, &path, &error)),
                        }
                    }
                    Err(error) => return Err(unable_to_open(handle, &path, &error)),
                };
                grow(&file);
                Ok(Region::File {
                    file: Some(file),
                    path,
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
        let Some(value) = state.picture else {
            return Err(no_frame());
        };
        let [width, height] = if request.width == 0 || request.height == 0 {
            self.native
        } else {
            [
                request.width.min(self.native[0]),
                request.height.min(self.native[1]),
            ]
        };
        let bytes = (width * height * 4) as usize;
        let mut format = Format {
            format: 1,
            width,
            height,
            rotation: Some(Rotation { value: 0 }),
            ..Default::default()
        };
        let handle = request
            .transport
            .as_ref()
            .filter(|transport| transport.channel == MMAP && !state.ignore_transport)
            .map(|transport| transport.handle.clone());
        let Some(handle) = handle else {
            return Ok(Frame {
                format: Some(format),
                pixels: picture(value, bytes).into(),
            });
        };
        if state.starve_shared {
            return Err(no_frame());
        }
        if let Some((code, message)) = state.refuse_next.take() {
            return Err(Status::new(code, message));
        }
        // Borrowed here, released when this call returns.
        self.borrow(&mut state, &handle, bytes)?;
        let written = match &mut state.library.get_mut(&handle).unwrap().1 {
            Region::Unmapped => Err(Status::invalid_argument(format!(
                "Unable to open or map shared memory region: {handle}"
            ))),
            region if bytes > region.size() => Err(Status::failed_precondition(format!(
                "The shared memory region needs to have a size of at least: {bytes}"
            ))),
            region => {
                region.write(&picture(value, bytes));
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
        let mut posted = self.posted.subscribe();
        // Taken after the first frame and kept until the stream ends.
        let mut own: Option<Borrow> = None;
        let mut wait = false;
        loop {
            if wait {
                tokio::select! {
                    changed = posted.changed() => if changed.is_err() { return },
                    () = replies.closed() => return,
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
            let shared = reply.format.as_ref().and_then(|format| {
                let transport = format.transport.as_ref()?;
                Some((transport.handle.clone(), format.width * format.height * 4))
            });
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
        }
    }
}

fn unable_to_open(handle: &str, path: &std::path::Path, error: &io::Error) -> Status {
    Status::invalid_argument(format!(
        "Rejected shared memory handle '{handle}': unable to open '{}' ({error}).",
        path.display()
    ))
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
            state.handles.extend(handle);
        }
        tokio::spawn(self.0.clone().stream(request, replies));
        std::future::ready(Ok(tonic::Response::new(ReceiverStream::new(stream))))
    }
}
