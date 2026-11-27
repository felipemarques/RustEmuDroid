//! Shared-memory ("MMAP") subscriptions of `streamScreenshot` and the session
//! plan that decides when to use them. The in-band subscription (`receive` in
//! the parent) is untouched; this path is used only while the plan wants it.
//!
//! Plan, per display session (a "picture" is a frame with pixels):
//!
//! ```text
//!   UntilFirstPicture ── in-band, as without shared memory; the emulator
//!     │                  sends nothing while Android boots
//!     └── the first picture is on the panel ──────────────────▶ Shared
//!   Shared ── every subscription (the first, a resize, a rotation) uses a
//!     │       new region; one that shows nothing is waited for as in-band
//!     └── evidence against shared memory ─────────────────────▶ Inline(reason)
//!   Inline ── in-band for the rest of the session.
//! ```
//!
//! The evidence, all of which repeats the same request in-band at once and is
//! reported with its reason: the emulator answers with a status of its own at
//! any moment of the subscription, it sends the pixels inside the messages,
//! the region can not be created or read, an announced frame does not fit in
//! it, or frames are announced but not written into it (see `shared_frames`
//! for how that is known). A picture counts, and shared memory is reported as
//! the transport, only once the emulator has provably written into the region.
//!
//! A subscription without a picture is no evidence. The emulator sends
//! nothing while the guest has no frame for it (the screen is off, Android is
//! not up), through a region or inside the messages alike, so the wait is the
//! one of the in-band path: none for an unreduced request, and for a reduced
//! one the reduction's own limit, whose miss disables the reduction and leaves
//! shared memory alone. A stream that ends or a connection error is not
//! evidence either: it ends the subscription exactly as in-band (normally the
//! VM stopping) and nothing is reported.
use super::{
    super::{Fallback, Format, Frame, Shared, StreamEnded, Transport, VideoTransport, authorized},
    DisplayResult, FIRST_REDUCED_FRAME, NativeDisplay, Request, Source,
};
use crate::display::shared_frames::{CopyError, MMAP, SharedFrames, UNWRITTEN_FRAMES};
use eframe::egui;
use std::{
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::time;
use tonic::{
    Code, Status, Streaming,
    client::Grpc,
    codec::ProstCodec,
    codegen::{http::uri::PathAndQuery, tokio_stream::Stream},
    metadata::{Ascii, MetadataValue},
    transport::Channel,
};

/// Once the emulator has written into a region, `UNWRITTEN_FRAMES` announced
/// frames in a row that were not written into it, the first of them at least
/// this long ago, mean that it writes somewhere else now. Fewer are normal
/// (see `shared_frames`).
const UNWRITTEN_SPAN: Duration = Duration::from_millis(500);
/// How often the plan looks for the first in-band picture.
const PICTURE_POLL: Duration = Duration::from_millis(200);
/// How long a region is kept after its subscription (see [`Lingering`]): many
/// times the 125 ms after which the emulator looks whether a stream was
/// cancelled.
const LINGER: Duration = Duration::from_secs(2);
/// A copy is made again at most this many times for frames announced during
/// it, so a producer that never pauses can not keep the panel without a
/// picture.
const MAX_RECOPY: u32 = 4;
/// At most this many pending messages are skipped in one scan.
const MAX_SKIP: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Plan {
    UntilFirstPicture,
    Shared,
    Inline,
}

/// The time limits of the plan; the tests shorten them.
#[derive(Clone, Copy, Debug)]
pub(super) struct Timing {
    /// The first picture of a reduced request, as in the in-band path.
    pub reduced_frame: Duration,
    pub unwritten_span: Duration,
    pub picture_poll: Duration,
    pub linger: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            reduced_frame: FIRST_REDUCED_FRAME,
            unwritten_span: UNWRITTEN_SPAN,
            picture_poll: PICTURE_POLL,
            linger: LINGER,
        }
    }
}

/// A region that outlives its subscription.
///
/// When a subscription is replaced, the cancellation of its stream has not
/// even left this process, and the emulator only looks for it between two
/// frames. A stream that holds no borrow of its own at that moment (it has
/// not sent a frame yet, or the screen is off) opens the region by name for
/// its next frame. On Linux a name that is gone makes it create a file of its
/// own there: sparse, written through a mapping, and fatal to the emulator
/// when `/dev/shm` is full. So the region stays for `linger` after the
/// subscription, in a task of the worker's runtime. When the runtime ends
/// (the display session is over, the VM stopping) it is released at once.
pub(super) struct Lingering {
    region: Option<SharedFrames>,
    linger: Duration,
}

impl Lingering {
    pub fn new(region: SharedFrames, linger: Duration) -> Self {
        Self {
            region: Some(region),
            linger,
        }
    }

    pub fn region(&mut self) -> Option<&mut SharedFrames> {
        self.region.as_mut()
    }
}

impl Drop for Lingering {
    fn drop(&mut self) {
        let linger = self.linger;
        if let (Some(region), Ok(runtime)) =
            (self.region.take(), tokio::runtime::Handle::try_current())
        {
            runtime.spawn(async move {
                time::sleep(linger).await;
                drop(region);
            });
        }
    }
}

/// Bytes of an RGBA frame whose pixels the emulator wrote to the region.
pub(super) fn shared_frame_len(format: &Format, pixels: &[u8]) -> Option<usize> {
    let mapped = format
        .transport
        .as_ref()
        .is_some_and(|transport| transport.channel == MMAP);
    (mapped && pixels.is_empty() && format.format == 1)
        .then(|| usize::try_from(u64::from(format.width) * u64::from(format.height) * 4).ok())
        .flatten()
        .filter(|len| *len > 0)
}

/// A status the emulator itself answered with to reject the handle or the
/// region (`SharedMemoryLibrary` and `getScreenshot`). Other codes
/// (unavailable, cancelled, unknown...) and statuses made up on this side for
/// a broken connection (they carry the transport's error as their source) are
/// the connection or the VM going away.
pub(super) fn refusal(status: &Status) -> Option<Fallback> {
    (matches!(
        status.code(),
        Code::InvalidArgument
            | Code::PermissionDenied
            | Code::FailedPrecondition
            | Code::Internal
            | Code::ResourceExhausted
            | Code::Unimplemented
            | Code::NotFound
            | Code::AlreadyExists
            | Code::OutOfRange
    ) && std::error::Error::source(status).is_none())
    .then(|| Fallback::EngineRefused {
        code: format!("{:?}", status.code()),
        message: status.message().to_string(),
    })
}

type Item = Result<Option<Frame>, Status>;

fn shared_len(item: &Item) -> Option<usize> {
    match item {
        Ok(Some(frame)) => frame
            .format
            .as_ref()
            .and_then(|format| shared_frame_len(format, &frame.pixels)),
        _ => None,
    }
}

/// Width, height and rotation: how the bytes of a frame are laid out.
fn shape(item: &Item) -> Option<(u32, u32, i32)> {
    let Ok(Some(frame)) = item else { return None };
    let format = frame.format.as_ref()?;
    let rotation = format.rotation.as_ref().map_or(0, |turn| turn.value);
    Some((format.width, format.height, rotation))
}

/// A message the transport already received, without waiting.
fn poll_pending(frames: &mut Streaming<Frame>) -> Option<Item> {
    let mut context = Context::from_waker(Waker::noop());
    match Pin::new(frames).poll_next(&mut context) {
        Poll::Ready(Some(item)) => Some(item.map(Some)),
        Poll::Ready(None) => Some(Ok(None)),
        Poll::Pending => None,
    }
}

/// The newest frame in the region that the transport has already announced,
/// if there is one. A message of another kind stops the scan and waits in
/// `rest` to be handled in its turn.
fn newest_pending(frames: &mut Streaming<Frame>, rest: &mut Option<Item>) -> Option<Item> {
    let mut newest = None;
    // Only messages already received are pending; the bound only guards
    // against a source that never runs dry.
    for _ in 0..MAX_SKIP {
        if rest.is_some() {
            break;
        }
        match poll_pending(frames) {
            Some(item) if shared_len(&item).is_some() => newest = Some(item),
            Some(item) => *rest = Some(item),
            None => break,
        }
    }
    newest
}

/// Lets the transport take in what has arrived. The connection is read by a
/// task of its own (tonic spawns it) on the one thread of the video worker,
/// so while this task runs no message becomes visible to it. It steps aside
/// twice: once for the runtime to look at the socket, once for that task to
/// run.
async fn let_the_transport_run() {
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
}

/// One subscription through `region`. `Err` is the reason why the session
/// continues in-band (the caller repeats the request there); `Ok` is how the
/// subscription ended otherwise, with the meaning `receive` gives it.
#[allow(clippy::too_many_arguments)]
pub(super) async fn receive_shared(
    video: &mut Grpc<Channel>,
    token: &MetadataValue<Ascii>,
    ctx: &egui::Context,
    state: &Mutex<Shared>,
    request: Request,
    fixed_raster: Option<[u32; 2]>,
    region: &mut SharedFrames,
    timing: Timing,
) -> Result<DisplayResult<()>, Fallback> {
    // As in-band: only a reduced request has a limit for its first picture.
    let deadline = request
        .reduction
        .map(|_| time::Instant::now() + timing.reduced_frame);
    let [width, height] = request.reduction.map_or([0, 0], |reduction| reduction.size);
    let start = async {
        video.ready().await?;
        let reply = video
            .server_streaming(
                authorized(
                    Format {
                        format: 1,
                        width,
                        height,
                        transport: Some(Transport {
                            channel: MMAP,
                            handle: region.handle().to_string(),
                        }),
                        ..Default::default()
                    },
                    token,
                ),
                PathAndQuery::from_static(
                    "/android.emulation.control.EmulatorController/streamScreenshot",
                ),
                ProstCodec::<Format, Frame>::default(),
            )
            .await;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(reply)
    };
    let started = match deadline {
        Some(deadline) => match time::timeout_at(deadline, start).await {
            Ok(started) => started,
            Err(late) => return Ok(Err(late.into())),
        },
        None => start.await,
    };
    let mut frames = match started {
        Err(error) => return Ok(Err(error)),
        Ok(Err(status)) => {
            return match refusal(&status) {
                Some(fallback) => Err(fallback),
                None => Ok(Err(status.into())),
            };
        }
        Ok(Ok(reply)) => reply.into_inner(),
    };
    let scaled = request.reduction.is_some();
    let mut pictures = false;
    let mut first = deadline;
    // A frame announced while the copy before it was made, to be copied
    // next, and the message of another kind that came after the frames.
    let mut again: Option<Item> = None;
    let mut lookahead: Option<Item> = None;
    loop {
        let item = match (again.take().or_else(|| lookahead.take()), first) {
            (Some(item), _) => item,
            (None, Some(deadline)) => match time::timeout_at(deadline, frames.message()).await {
                Ok(item) => item,
                Err(late) => return Ok(Err(late.into())),
            },
            (None, None) => frames.message().await,
        };
        let item = if shared_len(&item).is_some() {
            let mut current = item;
            let mut copies = 0;
            let pixels = loop {
                // The region holds the frame the emulator wrote last, which
                // the newest message describes.
                if let Some(newer) = newest_pending(&mut frames, &mut lookahead) {
                    current = newer;
                }
                let pixels = match region.copy(shared_len(&current).unwrap_or(0)) {
                    Ok(pixels) => pixels,
                    Err(CopyError::TooLarge { frame, region }) => {
                        return Err(Fallback::FrameTooLarge { frame, region });
                    }
                    Err(CopyError::System(error)) => {
                        return Err(Fallback::RegionError(error.to_string()));
                    }
                };
                copies += 1;
                let_the_transport_run().await;
                let Some(newer) = newest_pending(&mut frames, &mut lookahead) else {
                    break Some(pixels);
                };
                // A frame was announced while we copied: the emulator wrote
                // into the region meanwhile, and the copy can be a mix of two
                // frames. It is made again for the newer one.
                if copies <= MAX_RECOPY {
                    current = newer;
                    continue;
                }
                // Enough copies in one go. This one is drawn, unless the two
                // frames differ in shape and its bytes may be laid out in
                // another way than its message says. The newer frame is
                // copied in its turn, so the last frame announced always is.
                let same_shape = shape(&newer) == shape(&current);
                again = Some(newer);
                break same_shape.then_some(pixels);
            };
            let Some(pixels) = pixels else {
                continue;
            };
            // A region the emulator never wrote has had its mark since before
            // the request: there one frame that is not in it is enough.
            let elsewhere = if region.written() {
                region.ignored(UNWRITTEN_FRAMES, timing.unwritten_span)
            } else {
                region.unwritten() > 0
            };
            if elsewhere {
                return Err(Fallback::NotWritten);
            }
            if !region.written() {
                // A frame too small to carry the mark proves nothing, and
                // nothing is shown without proof.
                continue;
            }
            match current {
                Ok(Some(mut frame)) => {
                    frame.pixels = pixels.into();
                    Ok(Some(frame))
                }
                other => other,
            }
        } else {
            item
        };
        let frame = match item {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(Err(StreamEnded.into())),
            Err(status) => {
                return match refusal(&status) {
                    Some(fallback) => Err(fallback),
                    None => Ok(Err(status.into())),
                };
            }
        };
        let mapped = frame
            .format
            .as_ref()
            .and_then(|format| format.transport.as_ref())
            .is_some_and(|transport| transport.channel == MMAP);
        if !frame.pixels.is_empty() && !mapped {
            // The emulator ignored the request; an in-band subscription works.
            return Err(Fallback::EngineSentInline);
        }
        if frame.pixels.is_empty() {
            let mut shared = state.lock().unwrap();
            if !shared.blank {
                shared.blank = true;
                shared.latest = None;
                shared.source.visibility_epoch += 1;
                drop(shared);
                ctx.request_repaint();
            }
            if scaled && first.is_none() {
                return Ok(Ok(()));
            }
            continue;
        }
        if let Some(reduction) = request.reduction {
            match reduction.accepts(&frame) {
                Ok(true) => {}
                Ok(false) => return Ok(Ok(())),
                Err(error) => return Ok(Err(error)),
            }
        }
        first = None;
        if !pictures {
            pictures = true;
            state
                .lock()
                .unwrap()
                .set_video_transport(VideoTransport::SharedMemory);
        }
        let can_adapt = fixed_raster.is_some_and(|raster| {
            scaled
                || frame
                    .format
                    .as_ref()
                    .and_then(NativeDisplay::conventional)
                    .is_some_and(|native| native.matches_raster(raster))
        });
        let mut shared = state.lock().unwrap();
        #[cfg(test)]
        {
            if let Some(record) = &mut shared.record {
                record.push(frame.clone());
            }
        }
        shared.latest = Some(frame);
        shared.delivered += 1;
        shared.source = Source {
            generation: request.generation,
            visibility_epoch: shared.source.visibility_epoch,
            scaled,
            can_adapt,
        };
        shared.blank = false;
        drop(shared);
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{
            Preference, Reduction, Setup,
            fake_engine::{FakeEngine, Files, picture},
            run,
        },
        *,
    };
    use crate::display::shared_frames::Moment;
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU32, Ordering},
        },
    };
    use tokio::sync::watch;

    const NATIVE: [u32; 2] = [16, 8];
    const NATIVE_BYTES: usize = 16 * 8 * 4;

    fn status(code: Code) -> Status {
        Status::new(code, "Rejected shared memory handle 'file:///x'")
    }

    fn quick() -> Timing {
        Timing {
            reduced_frame: Duration::from_millis(300),
            unwritten_span: Duration::from_millis(60),
            picture_poll: Duration::from_millis(10),
            linger: Duration::from_millis(100),
        }
    }

    fn setup(create: impl Fn(usize) -> std::io::Result<SharedFrames> + 'static) -> Setup {
        Setup {
            shared: Ok(()),
            timing: quick(),
            create: Box::new(create),
        }
    }

    /// A display session against the fake emulator, arranged as the real
    /// video worker is: `run`, the gRPC connection (the task tonic spawns for
    /// it) and the test's own steps share one thread and its runtime. The
    /// emulator has its own thread.
    struct Session {
        state: Arc<Mutex<Shared>>,
        engine: Arc<FakeEngine>,
        preference: Preference,
        requests: watch::Receiver<Request>,
    }

    impl Session {
        fn new(engine: Arc<FakeEngine>) -> Self {
            let (preference, requests) = Preference::new();
            Self {
                state: Arc::default(),
                engine,
                preference,
                requests,
            }
        }

        /// Runs `run` until `steps` finish; `Some` is what `run` returned if
        /// it ended first. Both sides are gone when this returns: the
        /// client's regions first, then the emulator's.
        fn play(
            &self,
            setup: Setup,
            fixed_raster: Option<[u32; 2]>,
            steps: impl AsyncFnOnce(&Self),
        ) -> Option<DisplayResult<()>> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let ended = runtime.block_on(async {
                let video = self.engine.serve().await;
                let token = "Bearer test".parse().unwrap();
                let ctx = egui::Context::default();
                let session = run(
                    video,
                    &token,
                    &ctx,
                    &self.state,
                    self.requests.clone(),
                    fixed_raster,
                    setup,
                );
                tokio::select! {
                    result = session => Some(result),
                    () = steps(self) => None,
                }
            });
            drop(runtime);
            self.engine.shut_down();
            ended
        }

        fn transport(&self) -> Option<VideoTransport> {
            self.state.lock().unwrap().video_transport.clone()
        }

        /// Waits (up to 5 s) until `done` holds for the shared state.
        async fn until(&self, what: &str, done: impl Fn(&Shared) -> bool) {
            let deadline = time::Instant::now() + Duration::from_secs(5);
            while !done(&self.state.lock().unwrap()) {
                assert!(
                    time::Instant::now() < deadline,
                    "timed out waiting for {what}"
                );
                time::sleep(Duration::from_millis(2)).await;
            }
        }

        /// Waits (up to 5 s) until `done` holds for the emulator.
        async fn until_engine(&self, what: &str, done: impl Fn(&FakeEngine) -> bool) {
            let deadline = time::Instant::now() + Duration::from_secs(5);
            while !done(&self.engine) {
                assert!(
                    time::Instant::now() < deadline,
                    "timed out waiting for {what}"
                );
                time::sleep(Duration::from_millis(2)).await;
            }
        }

        /// The emulator shows a new picture and the panel must get exactly
        /// those pixels (never an older frame, zeros or the mark).
        async fn shows(&self, value: u32) {
            self.engine.post(value);
            self.has(value).await;
        }

        /// The guest posts `first` and the panel ends up with `last`.
        async fn shows_at_last(&self, first: u32, last: u32) {
            self.engine.post(first);
            self.has(last).await;
        }

        /// The panel gets the picture `value` without the guest posting it.
        async fn has(&self, value: u32) {
            let expected = picture(value, self.engine.frame_bytes());
            self.until(&format!("picture {value} on the panel"), |shared| {
                shared
                    .latest
                    .as_ref()
                    .is_some_and(|frame| frame.pixels == expected)
            })
            .await;
        }

        async fn reports(&self, transport: VideoTransport) {
            self.until(&format!("{transport:?}"), |shared| {
                shared.video_transport.as_ref() == Some(&transport)
            })
            .await;
        }

        /// The panel asks for the same picture again, as after a resize.
        fn asks_again(&self) {
            self.state.lock().unwrap().latest = None;
            self.preference
                .sender
                .send_modify(|request| request.generation += 1);
        }

        /// The panel stops needing pixels, or needs them again.
        fn pauses(&self, paused: bool) {
            self.preference.sender.send_modify(|request| {
                request.generation += 1;
                request.reduction = None;
                request.paused = paused;
            });
        }

        fn asks_for_a_reduction(&self, size: [u32; 2]) {
            self.preference.sender.send_modify(|request| {
                request.generation += 1;
                request.reduction = Some(Reduction {
                    size,
                    native: NativeDisplay {
                        size: NATIVE,
                        rotation: 0,
                    },
                });
            });
        }
    }

    fn inline(fallback: Fallback) -> VideoTransport {
        VideoTransport::Inline {
            fallback: Some(fallback),
        }
    }

    /// A region in `directory`, as on Linux.
    fn file_regions(directory: PathBuf) -> Setup {
        setup(move |bytes| SharedFrames::in_directory(&directory, bytes))
    }

    fn file_of(handle: &str) -> PathBuf {
        PathBuf::from(handle.strip_prefix("file://").unwrap())
    }

    /// For a test that watches a copy: the guest does `what`, and the
    /// emulator writes the frame and announces it, all before this returns
    /// into the copy, with time for the message to reach this process.
    fn announced_meanwhile(engine: &FakeEngine, what: impl FnOnce(&FakeEngine)) {
        let before = engine.announced();
        what(engine);
        let started = std::time::Instant::now();
        while engine.announced() == before {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the frame was not announced"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    #[test]
    fn the_session_starts_in_band_and_moves_to_shared_memory_after_the_first_picture() {
        let dir = tempfile::tempdir().unwrap();
        for (files, setup) in [
            (Files::Posix, file_regions(dir.path().into())),
            #[cfg(windows)]
            (Files::Windows, setup(SharedFrames::create)),
        ] {
            let session = Session::new(FakeEngine::new(NATIVE, files, [dir.path().into()]));
            let ended = session.play(setup, Some(NATIVE), async |session| {
                // Android is still booting: no frame, and nothing is reported
                // however long it takes.
                time::sleep(Duration::from_millis(400)).await;
                assert_eq!(session.transport(), None);
                assert_eq!(session.engine.subscriptions(), [false]);
                session.shows(1).await;
                session.reports(VideoTransport::SharedMemory).await;
                // Frame after frame stays live: the emulator closed and
                // reopened the region after its first frame.
                for value in 2..40 {
                    session.shows(value).await;
                }
                assert_eq!(session.engine.subscriptions(), [false, true]);
                assert_eq!(session.transport(), Some(VideoTransport::SharedMemory));
            });
            assert!(ended.is_none());
        }
    }

    #[test]
    fn every_new_subscription_gets_a_fresh_region_and_its_first_picture_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                session.reports(VideoTransport::SharedMemory).await;
                let first = session.engine.handles();
                // The screen is still: a resize asks again and the picture
                // comes without a new frame of the guest.
                session.asks_again();
                session.has(1).await;
                let second = session.engine.handles();
                assert_eq!(second.len(), first.len() + 1);
                assert_ne!(second.last(), first.last());
                session.shows(2).await;
                assert_eq!(session.transport(), Some(VideoTransport::SharedMemory));
            },
        );
        assert!(ended.is_none());
        // Every region was removed when the session ended.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_paused_panel_holds_no_subscription_and_gets_the_screen_at_once_when_it_asks_again() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                session.reports(VideoTransport::SharedMemory).await;
                session.shows(2).await;
                let streams = session.engine.subscriptions().len();
                session.pauses(true);
                // The stream ends in the emulator: what the guest draws now
                // is announced to nobody.
                time::sleep(Duration::from_millis(300)).await;
                let announced = session.engine.announced();
                for value in 3..8 {
                    session.engine.post(value);
                    time::sleep(Duration::from_millis(20)).await;
                }
                assert_eq!(session.engine.announced(), announced);
                assert_eq!(session.engine.subscriptions().len(), streams);
                // A feature needs pixels: the picture of this moment comes
                // without a new frame of the guest, through a region again.
                session.state.lock().unwrap().latest = None;
                session.pauses(false);
                session.has(7).await;
                assert_eq!(session.engine.subscriptions().len(), streams + 1);
                assert_eq!(session.engine.subscriptions().last(), Some(&true));
                session.shows(8).await;
                assert_eq!(session.transport(), Some(VideoTransport::SharedMemory));
            },
        );
        assert!(ended.is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_region_of_a_replaced_subscription_stays_for_a_while_and_is_then_removed() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let mut lingering = file_regions(dir.path().into());
        lingering.timing.linger = Duration::from_millis(1000);
        let ended = session.play(lingering, Some(NATIVE), async |session| {
            session.shows(1).await;
            session.reports(VideoTransport::SharedMemory).await;
            let replaced = time::Instant::now();
            session.asks_again();
            session.has(1).await;
            let handles = session.engine.handles();
            let [old, new] = [0, 1].map(|index| file_of(&handles[index]));
            // The old stream may not have ended yet in the emulator: its
            // region keeps its name for the time given, and no longer.
            let deadline = replaced + Duration::from_secs(10);
            while old.exists() {
                assert!(time::Instant::now() < deadline, "the region was kept");
                time::sleep(Duration::from_millis(10)).await;
            }
            assert!(replaced.elapsed() >= Duration::from_millis(1000));
            // The session goes on in the new one.
            session.shows(2).await;
            assert!(new.exists());
        });
        assert!(ended.is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_subscription_that_shows_nothing_is_waited_for_as_in_band_and_keeps_shared_memory() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                session.reports(VideoTransport::SharedMemory).await;
                // The screen goes off and the window is resized: the new
                // subscription has no picture to send, for as long as it takes.
                session.engine.screen(false);
                session.asks_again();
                session
                    .until_engine("the new subscription", |engine| {
                        engine.subscriptions().len() == 3
                    })
                    .await;
                time::sleep(Duration::from_millis(800)).await;
                assert_eq!(session.transport(), Some(VideoTransport::SharedMemory));
                assert_eq!(session.engine.subscriptions(), [false, true, true]);
                assert!(session.state.lock().unwrap().latest.is_none());
                // The screen comes back: its picture arrives through the
                // region of that same subscription.
                session.engine.screen(true);
                session.has(1).await;
                session.shows(2).await;
                assert_eq!(session.engine.subscriptions(), [false, true, true]);
                assert_eq!(session.transport(), Some(VideoTransport::SharedMemory));
            },
        );
        assert!(ended.is_none());
    }

    #[test]
    fn a_reduced_request_that_shows_nothing_costs_the_reduction_and_not_shared_memory() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                session.reports(VideoTransport::SharedMemory).await;
                session.engine.screen(false);
                session.state.lock().unwrap().latest = None;
                session.asks_for_a_reduction([8, 4]);
                // The reduced subscription misses the reduction's limit; the
                // request is repeated unreduced, through a region again.
                session
                    .until_engine("the unreduced subscription", |engine| {
                        engine.subscriptions().len() == 4
                    })
                    .await;
                assert_eq!(session.engine.subscriptions(), [false, true, true, true]);
                session.engine.screen(true);
                session.has(1).await;
                assert!(!session.state.lock().unwrap().source.scaled);
                session.shows(2).await;
                assert_eq!(session.engine.subscriptions(), [false, true, true, true]);
                assert_eq!(session.transport(), Some(VideoTransport::SharedMemory));
            },
        );
        assert!(ended.is_none());
    }

    #[test]
    fn a_frame_of_another_shape_announced_during_the_copy_is_seen_and_the_copy_is_never_drawn() {
        // The emulator turns the screen and writes the turned frame while the
        // client is inside its copy of the frame before: the bytes it reads
        // are laid out the other way than its message says. The worker only
        // learns of the turned frame from a message that its own thread has
        // to take in after the copy.
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let during_the_copy = Arc::new(AtomicBool::new(false));
        let hooked = {
            let (armed, engine) = (during_the_copy.clone(), session.engine.clone());
            let directory = PathBuf::from(dir.path());
            setup(move |bytes| {
                let mut frames = SharedFrames::in_directory(&directory, bytes)?;
                let (armed, engine) = (armed.clone(), engine.clone());
                frames.watch_copies(move |moment| {
                    if moment == Moment::BeforeRead && armed.swap(false, Ordering::SeqCst) {
                        announced_meanwhile(&engine, |engine| engine.turn_and_post(1, 3));
                    }
                });
                Ok(frames)
            })
        };
        let ended = session.play(hooked, Some(NATIVE), async |session| {
            session.shows(1).await;
            session.reports(VideoTransport::SharedMemory).await;
            session.state.lock().unwrap().record = Some(vec![]);
            during_the_copy.store(true, Ordering::SeqCst);
            session.engine.post(2);
            session.has(3).await;
            assert!(!during_the_copy.load(Ordering::SeqCst), "no copy was made");
            session.shows(4).await;
            // Everything drawn since: the turned frame with its own shape and
            // the one after it. The copy made for frame 2 held frame 3.
            let drawn: Vec<_> = session
                .state
                .lock()
                .unwrap()
                .record
                .take()
                .unwrap()
                .into_iter()
                .map(|frame| {
                    let format = frame.format.unwrap();
                    let turn = format.rotation.unwrap().value;
                    let value = u32::from(frame.pixels[0]);
                    assert_eq!(frame.pixels, picture(value, NATIVE_BYTES));
                    (value, [format.width, format.height], turn)
                })
                .collect();
            assert_eq!(drawn, [(3, [8, 16], 1), (4, [8, 16], 1)]);
        });
        assert!(ended.is_none());
    }

    #[test]
    fn the_last_frame_announced_is_copied_although_every_copy_before_was_overtaken() {
        // The emulator writes and announces the next frame while the client
        // is inside each of its copies, more often than the client copies
        // again in one go, and then stops: a still screen.
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let overtakes = Arc::new(AtomicU32::new(0));
        let overtaken = {
            let (overtakes, engine) = (overtakes.clone(), session.engine.clone());
            let directory = PathBuf::from(dir.path());
            setup(move |bytes| {
                let mut frames = SharedFrames::in_directory(&directory, bytes)?;
                let (overtakes, engine) = (overtakes.clone(), engine.clone());
                frames.watch_copies(move |moment| {
                    let left = overtakes.load(Ordering::SeqCst);
                    if moment == Moment::AfterRead && left > 0 {
                        overtakes.store(left - 1, Ordering::SeqCst);
                        announced_meanwhile(&engine, |engine| engine.post(engine.drawn() + 1));
                    }
                });
                Ok(frames)
            })
        };
        let ended = session.play(overtaken, Some(NATIVE), async |session| {
            session.shows(1).await;
            session.reports(VideoTransport::SharedMemory).await;
            session.state.lock().unwrap().record = Some(vec![]);
            // Frame 10 and, during the copies, 11 to 15.
            overtakes.store(MAX_RECOPY + 1, Ordering::SeqCst);
            session.shows_at_last(10, 15).await;
            assert_eq!(overtakes.load(Ordering::SeqCst), 0);
            assert_eq!(session.engine.drawn(), 15);
            // Four copies were made again. The fifth was drawn, a whole frame
            // here, and frame 15 was copied in its turn, although it was
            // written just before the client's mark landed on it.
            let drawn: Vec<_> = session
                .state
                .lock()
                .unwrap()
                .record
                .take()
                .unwrap()
                .into_iter()
                .map(|frame| {
                    let value = u32::from(frame.pixels[0]);
                    assert_eq!(frame.pixels, picture(value, NATIVE_BYTES));
                    value
                })
                .collect();
            assert_eq!(drawn, [14, 15]);
            assert_eq!(session.transport(), Some(VideoTransport::SharedMemory));
            session.shows(16).await;
        });
        assert!(ended.is_none());
    }

    #[test]
    fn a_guest_that_draws_without_pause_from_its_own_thread_costs_no_shared_memory_and_no_colour() {
        // 128x64: a write and a copy take long enough to overlap.
        let native = [128, 64];
        let dir = tempfile::tempdir().unwrap();
        for (files, setup) in [
            (Files::Posix, file_regions(dir.path().into())),
            #[cfg(windows)]
            (Files::Windows, setup(SharedFrames::create)),
        ] {
            let session = Session::new(FakeEngine::new(native, files, [dir.path().into()]));
            let ended = session.play(setup, Some(native), async |session| {
                session.shows(1).await;
                session.reports(VideoTransport::SharedMemory).await;
                session.engine.draw(Duration::ZERO);
                let started = time::Instant::now();
                // What each pixel showed last: a pixel never goes back to an
                // older frame, and never shows what the guest has not drawn.
                let mut before = vec![1u32; (native[0] * native[1]) as usize];
                let mut pictures = 0;
                while pictures < 500 {
                    assert!(started.elapsed() < Duration::from_secs(30), "{pictures}");
                    let taken = session.state.lock().unwrap().latest.take();
                    let Some(frame) = taken else {
                        tokio::task::yield_now().await;
                        continue;
                    };
                    let newest = session.engine.drawn();
                    assert_eq!(frame.pixels.len(), before.len() * 4);
                    for (pixel, before) in frame.pixels.chunks_exact(4).zip(&mut before) {
                        let value = u32::from_le_bytes([pixel[0], pixel[1], pixel[2], 0]);
                        assert_eq!(pixel[3], 255, "an alpha the emulator never wrote");
                        assert!(
                            *before <= value && value <= newest,
                            "frame {value} after {before}, newest {newest}"
                        );
                        *before = value;
                    }
                    pictures += 1;
                    assert_eq!(
                        session.transport(),
                        Some(VideoTransport::SharedMemory),
                        "after {pictures} pictures"
                    );
                }
                // The guest rests: its last frame is the picture, whole.
                session.engine.rest();
                session.has(session.engine.drawn()).await;
                assert_eq!(session.engine.subscriptions(), [false, true]);
                assert_eq!(session.transport(), Some(VideoTransport::SharedMemory));
            });
            assert!(ended.is_none());
        }
    }

    #[test]
    fn frames_announced_but_written_elsewhere_fall_back_in_band_with_the_picture_live() {
        // The first shared-memory build on Windows: a file the Windows
        // emulator deletes when it closes the region after the first frame,
        // and re-creates under the same name for the rest of the stream. The
        // client still reads the file it created.
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Windows, [dir.path().into()]));
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                session.reports(VideoTransport::SharedMemory).await;
                let started = time::Instant::now();
                let mut value = 1;
                // The guest keeps drawing; the panel must not stay on frame 1.
                while session.transport() == Some(VideoTransport::SharedMemory) {
                    assert!(started.elapsed() < Duration::from_secs(2), "not detected");
                    value += 1;
                    session.engine.post(value);
                    time::sleep(Duration::from_millis(5)).await;
                }
                // Where the name disappears at once the frames are written to
                // the new file; where it only disappears when we close ours, the
                // emulator can not reopen it and says so. Both end in-band.
                let reason = session.transport();
                assert!(
                    matches!(
                        &reason,
                        Some(VideoTransport::Inline {
                            fallback: Some(Fallback::NotWritten | Fallback::EngineRefused { .. })
                        })
                    ),
                    "{reason:?}"
                );
                for value in 100..110 {
                    session.shows(value).await;
                }
                assert_eq!(session.engine.subscriptions().last(), Some(&false));
                assert_eq!(session.transport(), reason);
            },
        );
        assert!(ended.is_none());
    }

    #[test]
    fn a_region_the_emulator_can_not_find_is_never_shown_and_falls_back_at_its_first_frame() {
        // The handle names nothing the client created (as a path the
        // emulator reads differently): it creates its own region there. The
        // screen is still, so one frame is all that is announced.
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        session.engine.misread_handles();
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                session.reports(inline(Fallback::NotWritten)).await;
                // The in-band subscription that follows shows the still
                // screen again: the second frame handed to the panel. Nothing
                // was shown in between, neither zeros nor an older picture.
                session
                    .until("the still picture in-band", |shared| shared.delivered >= 2)
                    .await;
                {
                    let shared = session.state.lock().unwrap();
                    assert_eq!(shared.delivered, 2);
                    assert_eq!(
                        shared.latest.as_ref().unwrap().pixels,
                        picture(1, NATIVE_BYTES)
                    );
                }
                assert_eq!(session.engine.subscriptions(), [false, true, false]);
                assert_eq!(session.engine.announced(), 1);
                session.shows(200).await;
            },
        );
        assert!(ended.is_none());
    }

    #[test]
    fn an_engine_status_after_pictures_falls_back_instead_of_ending_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                session.reports(VideoTransport::SharedMemory).await;
                session.shows(2).await;
                session.engine.refuse_next(
                    Code::InvalidArgument,
                    "Unable to open or map shared memory region: x",
                );
                session.engine.post(3);
                session
                    .reports(inline(Fallback::EngineRefused {
                        code: "InvalidArgument".into(),
                        message: "Unable to open or map shared memory region: x".into(),
                    }))
                    .await;
                session.shows(4).await;
                assert_eq!(session.engine.subscriptions(), [false, true, false]);
            },
        );
        assert!(ended.is_none(), "the session ended: {ended:?}");
    }

    #[test]
    fn a_handle_the_emulator_rejects_falls_back_with_its_words() {
        // Regions outside the directories the emulator accepts.
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(
            NATIVE,
            Files::Posix,
            [elsewhere.path().into()],
        ));
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                session
                    .until("the refusal", |shared| {
                        matches!(
                            &shared.video_transport,
                            Some(VideoTransport::Inline {
                                fallback: Some(Fallback::EngineRefused { code, message })
                            }) if code == "PermissionDenied"
                                && message.starts_with("Rejected shared memory handle 'file://")
                        )
                    })
                    .await;
                session.shows(2).await;
            },
        );
        assert!(ended.is_none());
    }

    #[test]
    fn a_frame_larger_than_the_region_falls_back_naming_both_sizes() {
        // The POSIX emulator grows a file that is too small and writes the
        // whole frame; the client reads no further than what it created.
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let directory = PathBuf::from(dir.path());
        let small = setup(move |_| SharedFrames::in_directory(&directory, NATIVE_BYTES / 2));
        let ended = session.play(small, Some(NATIVE), async |session| {
            session.shows(1).await;
            session
                .reports(inline(Fallback::FrameTooLarge {
                    frame: NATIVE_BYTES as u64,
                    region: NATIVE_BYTES as u64 / 2,
                }))
                .await;
            session.shows(2).await;
        });
        assert!(ended.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn a_mapping_too_small_for_the_frame_is_refused_by_the_emulator_and_falls_back() {
        // The Windows emulator can not map more than the mapping holds
        // (a mapping holds whole pages, so the frame here needs four).
        let native = [64, 64];
        let session = Session::new(FakeEngine::new(native, Files::Windows, []));
        let small = setup(|_| SharedFrames::create(4096));
        let ended = session.play(small, Some(native), async |session| {
            session.shows(1).await;
            session
                .until("the refusal", |shared| {
                    matches!(
                        &shared.video_transport,
                        Some(VideoTransport::Inline {
                            fallback: Some(Fallback::EngineRefused { code, message })
                        }) if code == "InvalidArgument"
                            && message.starts_with("Unable to open or map shared memory region")
                    )
                })
                .await;
            session.shows(2).await;
        });
        assert!(ended.is_none());
    }

    #[test]
    fn an_emulator_that_keeps_the_pixels_in_the_messages_is_followed_in_band() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        session.engine.ignore_transport();
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                session.reports(inline(Fallback::EngineSentInline)).await;
                session.shows(2).await;
            },
        );
        assert!(ended.is_none());
    }

    #[test]
    fn a_region_that_can_not_be_created_falls_back_with_the_systems_message() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let expected = std::fs::read_dir(&missing).unwrap_err().to_string();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let ended = session.play(file_regions(missing), Some(NATIVE), async |session| {
            session.shows(1).await;
            session
                .reports(inline(Fallback::RegionError(expected.clone())))
                .await;
            session.shows(2).await;
            assert_eq!(session.engine.subscriptions(), [false, false]);
        });
        assert!(ended.is_none());
    }

    #[test]
    fn the_vm_stopping_ends_the_session_as_in_band_and_reports_no_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                session.reports(VideoTransport::SharedMemory).await;
                session.engine.stop();
                time::sleep(Duration::from_secs(5)).await;
            },
        );
        let error = ended.expect("the session ends").unwrap_err();
        assert!(error.is::<StreamEnded>(), "{error}");
        assert_eq!(session.transport(), Some(VideoTransport::SharedMemory));
    }

    #[test]
    fn shared_memory_is_not_tried_when_it_is_off_or_the_screen_can_resize() {
        let dir = tempfile::tempdir().unwrap();
        for (shared, raster, expected) in [
            (
                Err(Fallback::TurnedOff),
                Some(NATIVE),
                inline(Fallback::TurnedOff),
            ),
            (
                Err(Fallback::OffByDefault),
                Some(NATIVE),
                inline(Fallback::OffByDefault),
            ),
            (Ok(()), None, inline(Fallback::ResizableDisplay)),
            (
                Ok(()),
                Some([1 << 15, 1 << 15]),
                inline(Fallback::FrameTooLarge {
                    frame: 1 << 32,
                    region: crate::display::shared_frames::MAX_REGION,
                }),
            ),
        ] {
            let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
            let mut setup = file_regions(dir.path().into());
            setup.shared = shared;
            let ended = session.play(setup, raster, async |session| {
                session.reports(expected.clone()).await;
                session.shows(1).await;
                session.shows(2).await;
                assert_eq!(session.engine.subscriptions(), [false]);
                assert_eq!(session.transport(), Some(expected.clone()));
            });
            assert!(ended.is_none());
        }
    }

    #[test]
    fn a_reduced_raster_travels_through_the_region_too() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                session.reports(VideoTransport::SharedMemory).await;
                session.asks_for_a_reduction([8, 4]);
                session
                    .until("the reduced picture", |shared| {
                        shared.source.scaled
                            && shared
                                .latest
                                .as_ref()
                                .is_some_and(|f| f.pixels == picture(1, 8 * 4 * 4))
                    })
                    .await;
                assert_eq!(session.transport(), Some(VideoTransport::SharedMemory));
            },
        );
        assert!(ended.is_none());
    }

    #[test]
    fn only_the_emulators_own_handle_errors_are_refusals_and_they_keep_its_words() {
        for code in [
            Code::PermissionDenied,
            Code::InvalidArgument,
            Code::Internal,
            Code::FailedPrecondition,
        ] {
            assert_eq!(
                refusal(&status(code)),
                Some(Fallback::EngineRefused {
                    code: format!("{code:?}"),
                    message: "Rejected shared memory handle 'file:///x'".into()
                })
            );
        }
        for code in [
            Code::Unavailable,
            Code::Cancelled,
            Code::Unknown,
            Code::DeadlineExceeded,
            Code::Aborted,
            Code::Unauthenticated,
        ] {
            assert_eq!(refusal(&status(code)), None, "{code:?}");
        }
        // A status this side made up for a broken connection is not the
        // emulator's answer, whatever its code.
        let broken = std::io::Error::from(std::io::ErrorKind::ConnectionReset);
        let mut made_up = Status::new(Code::Internal, "h2 protocol error");
        made_up.set_source(Arc::new(broken));
        assert_eq!(refusal(&made_up), None);
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn shared_frames_are_recognized_from_their_format() {
        let format = Format {
            format: 1,
            width: 800,
            height: 450,
            transport: Some(Transport {
                channel: MMAP,
                handle: "file:///tmp/f".into(),
            }),
            ..Default::default()
        };
        let frame = |format: Format, pixels: Vec<u8>| -> Item {
            Ok(Some(Frame {
                format: Some(format),
                pixels: pixels.into(),
            }))
        };
        assert_eq!(
            shared_len(&frame(format.clone(), vec![])),
            Some(800 * 450 * 4)
        );
        assert_eq!(shape(&frame(format.clone(), vec![])), Some((800, 450, 0)));
        assert_eq!(shared_frame_len(&format, &[]), Some(800 * 450 * 4));
        assert_eq!(shared_len(&frame(format.clone(), vec![1, 2, 3, 4])), None);
        let mut blank = format.clone();
        blank.width = 0;
        assert_eq!(shared_len(&frame(blank, vec![])), None);
        let mut png = format.clone();
        png.format = 0;
        assert_eq!(shared_len(&frame(png, vec![])), None);
        let mut inline = format;
        inline.transport = None;
        assert_eq!(shared_len(&frame(inline, vec![])), None);
        assert_eq!(shared_len(&Ok(None)), None);
        assert_eq!(shape(&Ok(None)), None);
        assert_eq!(shared_len(&Err(status(Code::Unavailable))), None);
    }
}
