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
//!     │       new region and must show a picture within FIRST_PICTURE: with
//!     │       the screen up, the emulator sends the current picture at once
//!     └── any failure ────────────────────────────────────────▶ Inline(reason)
//!   Inline ── in-band for the rest of the session.
//! ```
//!
//! The failures, all of which repeat the same request in-band at once and are
//! reported with their reason: the emulator answers with a status of its own
//! at any moment of the subscription, it sends the pixels inside the messages,
//! the region can not be created or read, no picture arrives in time, or
//! frames are announced but not written into our region (see `shared_frames`
//! for how that is known). A picture counts, and shared memory is reported as
//! the transport, only once the emulator has provably written into the region.
//!
//! A stream that ends or a connection error is not a failure of shared
//! memory: it ends the subscription exactly as the in-band path does (normally
//! the VM stopping) and nothing is reported. On a reduced request the limit is
//! never shorter than the reduction's own first-frame limit and a miss is
//! charged to shared memory, not to the reduction; the in-band subscription
//! that follows tests the reduction on its own.
use super::{
    super::{Fallback, Format, Frame, Shared, StreamEnded, Transport, VideoTransport, authorized},
    DisplayResult, FIRST_REDUCED_FRAME, NativeDisplay, Request, Source,
};
use crate::display::shared_frames::{CopyError, MMAP, SharedFrames, copy_newest};
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

/// A shared subscription shows its first picture within this time or the
/// session continues in-band.
const FIRST_PICTURE: Duration = Duration::from_secs(3);
/// This many announced frames in a row that were not written into our region,
/// the first of them at least `UNWRITTEN_SPAN` ago, mean that the emulator
/// writes somewhere else. One such frame is normal (see `shared_frames`).
const UNWRITTEN_FRAMES: u32 = 4;
const UNWRITTEN_SPAN: Duration = Duration::from_millis(500);
/// How often the plan looks for the first in-band picture.
const PICTURE_POLL: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Plan {
    UntilFirstPicture,
    Shared,
    Inline,
}

/// The time limits of the plan; the tests shorten them.
#[derive(Clone, Copy, Debug)]
pub(super) struct Timing {
    pub first_picture: Duration,
    pub unwritten_span: Duration,
    pub picture_poll: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            first_picture: FIRST_PICTURE,
            unwritten_span: UNWRITTEN_SPAN,
            picture_poll: PICTURE_POLL,
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

/// A message the transport already received, without waiting.
fn poll_pending(frames: &mut Streaming<Frame>) -> Option<Item> {
    let mut context = Context::from_waker(Waker::noop());
    match Pin::new(frames).poll_next(&mut context) {
        Poll::Ready(Some(item)) => Some(item.map(Some)),
        Poll::Ready(None) => Some(Ok(None)),
        Poll::Pending => None,
    }
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
    let limit = match request.reduction {
        Some(_) => timing.first_picture.max(FIRST_REDUCED_FRAME),
        None => timing.first_picture,
    };
    let picture_deadline = time::Instant::now() + limit;
    // Frames that were announced but not written are the more exact reason.
    let no_picture = |region: &SharedFrames| match region.unwritten() {
        0 => Fallback::NoPicture {
            seconds: limit.as_secs(),
        },
        _ => Fallback::NotWritten,
    };
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
    let mut frames = match time::timeout_at(picture_deadline, start).await {
        Err(_) => return Err(no_picture(region)),
        Ok(Err(error)) => return Ok(Err(error)),
        Ok(Ok(Err(status))) => {
            return match refusal(&status) {
                Some(fallback) => Err(fallback),
                None => Ok(Err(status.into())),
            };
        }
        Ok(Ok(Ok(reply))) => reply.into_inner(),
    };
    let scaled = request.reduction.is_some();
    let mut pictures = false;
    let mut first = request.reduction.map(|_| ());
    let mut lookahead: Option<Item> = None;
    loop {
        let item = match lookahead.take() {
            Some(item) => item,
            None if !pictures => match time::timeout_at(picture_deadline, frames.message()).await {
                Ok(item) => item,
                Err(_) => return Err(no_picture(region)),
            },
            None => frames.message().await,
        };
        let item = if shared_len(&item).is_some() {
            // Jump to the newest received message, copy, and redo the copy if
            // an even newer one arrived meanwhile; its size and rotation then
            // describe exactly the pixels copied.
            let (newest, pixels, rest, _) = copy_newest(
                item,
                || poll_pending(&mut frames),
                |item| shared_len(item).is_some(),
                |item| region.copy(shared_len(item).unwrap_or(0)),
            );
            lookahead = rest;
            let pixels = match pixels {
                Ok(pixels) => pixels,
                Err(CopyError::TooLarge { frame, region }) => {
                    return Err(Fallback::FrameTooLarge { frame, region });
                }
                Err(CopyError::System(error)) => {
                    return Err(Fallback::RegionError(error.to_string()));
                }
            };
            if region.ignored(UNWRITTEN_FRAMES, timing.unwritten_span) {
                return Err(Fallback::NotWritten);
            }
            if !region.written() {
                // Nothing of the emulator's is in the region yet: there is no
                // picture to show, only our own zeros.
                continue;
            }
            match newest {
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
    use std::{path::PathBuf, sync::Arc};
    use tokio::sync::watch;

    const NATIVE: [u32; 2] = [16, 8];
    const NATIVE_BYTES: usize = 16 * 8 * 4;

    fn status(code: Code) -> Status {
        Status::new(code, "Rejected shared memory handle 'file:///x'")
    }

    fn quick() -> Timing {
        Timing {
            first_picture: Duration::from_millis(400),
            unwritten_span: Duration::from_millis(60),
            picture_poll: Duration::from_millis(10),
        }
    }

    fn setup(create: impl Fn(usize) -> std::io::Result<SharedFrames> + 'static) -> Setup {
        Setup {
            shared: true,
            timing: quick(),
            create: Box::new(create),
        }
    }

    /// A display session against the fake emulator: `run` on one side, the
    /// test's own steps on the other, in one thread like the real worker.
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
        /// it ended first.
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
            runtime.block_on(async {
                let video = self.engine.clone().serve().await;
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
            })
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

        /// The emulator shows a new picture and the panel must get exactly
        /// those pixels (never an older frame, zeros or the mark).
        async fn shows(&self, value: u8) {
            self.engine.post(value);
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
                // although far more than the first-picture limit goes by.
                time::sleep(quick().first_picture * 2).await;
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
                session.state.lock().unwrap().latest = None;
                session
                    .preference
                    .sender
                    .send_modify(|request| request.generation += 1);
                session
                    .until("the current picture on the new subscription", |shared| {
                        shared
                            .latest
                            .as_ref()
                            .is_some_and(|frame| frame.pixels == picture(1, NATIVE_BYTES))
                    })
                    .await;
                let second = session.engine.handles();
                assert_eq!(second.len(), first.len() + 1);
                assert_ne!(second.last(), first.last());
                session.shows(2).await;
                assert_eq!(session.transport(), Some(VideoTransport::SharedMemory));
            },
        );
        assert!(ended.is_none());
        // Every region was removed with its subscription.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
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
    fn a_region_the_emulator_can_not_find_is_never_shown_and_falls_back() {
        // The handle names nothing the client created (as a path the
        // emulator reads differently): it creates its own region there.
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        session.engine.misread_handles();
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                let started = time::Instant::now();
                let mut value = 1;
                while session.transport().is_none() {
                    assert!(started.elapsed() < Duration::from_secs(2), "not detected");
                    value += 1;
                    session.engine.post(value);
                    time::sleep(Duration::from_millis(5)).await;
                    // Never zeros, never an older picture than in-band showed.
                    let shared = session.state.lock().unwrap();
                    if let Some(frame) = &shared.latest {
                        assert!(frame.pixels[0] >= 1 && frame.pixels[3] == 255);
                    }
                }
                assert_eq!(session.transport(), Some(inline(Fallback::NotWritten)));
                session.shows(200).await;
            },
        );
        assert!(ended.is_none());
    }

    #[test]
    fn a_still_screen_whose_only_frame_was_not_written_falls_back_at_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        session.engine.misread_handles();
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                let started = time::Instant::now();
                session.reports(inline(Fallback::NotWritten)).await;
                // One announced frame is not enough to tell; the limit is.
                assert!(started.elapsed() >= quick().first_picture / 2);
                // The in-band subscription that follows shows the still
                // screen again: the second frame handed to the panel.
                session
                    .until("the still picture in-band", |shared| shared.delivered >= 2)
                    .await;
                let shared = session.state.lock().unwrap();
                assert_eq!(shared.delivered, 2);
                assert_eq!(
                    shared.latest.as_ref().unwrap().pixels,
                    picture(1, NATIVE_BYTES)
                );
                assert_eq!(session.engine.subscriptions(), [false, true, false]);
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
    fn no_picture_through_the_region_within_the_limit_falls_back() {
        // The emulator has no frame for the new stream (as while the guest
        // has not posted one) and sends nothing at all.
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(FakeEngine::new(NATIVE, Files::Posix, [dir.path().into()]));
        session.engine.starve_shared();
        let ended = session.play(
            file_regions(dir.path().into()),
            Some(NATIVE),
            async |session| {
                session.shows(1).await;
                let started = time::Instant::now();
                session
                    .reports(inline(Fallback::NoPicture { seconds: 0 }))
                    .await;
                assert!(started.elapsed() >= quick().first_picture / 2);
                session.shows(2).await;
                assert_eq!(session.engine.subscriptions(), [false, true, false]);
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
    fn shared_memory_is_not_tried_when_inline_is_forced_or_the_screen_can_resize() {
        let dir = tempfile::tempdir().unwrap();
        for (shared, raster, expected) in [
            (
                false,
                Some(NATIVE),
                VideoTransport::Inline { fallback: None },
            ),
            (true, None, inline(Fallback::ResizableDisplay)),
            (
                true,
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
                session.preference.sender.send_modify(|request| {
                    request.generation += 1;
                    request.reduction = Some(Reduction {
                        size: [8, 4],
                        native: NativeDisplay {
                            size: NATIVE,
                            rotation: 0,
                        },
                    });
                });
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
        assert_eq!(shared_len(&Err(status(Code::Unavailable))), None);
    }
}
