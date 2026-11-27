//! Shared-file ("MMAP") subscriptions of `streamScreenshot` and the session
//! plan that decides when to use them. The in-band subscription (`receive` in
//! the parent) is untouched; this path is used only while the plan wants it.
//!
//! Plan, per display session (a "picture" is a frame with pixels; during boot
//! the emulator sends only empty frames, sometimes for a long time):
//!
//! ```text
//!   Probe ──picture through the file within PROBE_LIMIT─────────▶ On
//!     ├── emulator refuses the file / file error ───────────────▶ Off(reason)
//!     └── no picture within PROBE_LIMIT ─▶ InlineUntilPictures (report NoPicture)
//!   InlineUntilPictures ── in-band pictures flow ──────────────▶ Retry
//!   Retry ──picture within RETRY_LIMIT──────────────────────────▶ On
//!     └── refusal / file error / no picture ────────────────────▶ Off(reason)
//!   On ── every later subscription (resize, rotation) uses the file with
//!     │   RETRY_LIMIT, because the emulator sends the current picture at once
//!     │   on a new subscription once the screen is up
//!     └── refusal / file error / no picture ────────────────────▶ Off(reason)
//!   Off ── in-band for the rest of the session.
//! ```
//!
//! The panel can not stay black: every shared subscription has a limit for its
//! first picture, the only in-band phase after a failure has none (as today),
//! and only one retry is made. A stream that ends or a connection error is not
//! a transition: it ends the subscription exactly as the in-band path does
//! (normally the VM stopping) and nothing is reported. On a reduced request the
//! limit is never shorter than the reduction's own first-frame limit and a
//! miss is charged to the file, not to the reduction; the in-band subscription
//! that follows tests the reduction on its own.
use super::{
    super::{Fallback, Format, Frame, Shared, Transport, VideoTransport, authorized},
    DisplayResult, FIRST_REDUCED_FRAME, NativeDisplay, Request, Source,
};
use crate::display::shared_frames::{MMAP, SharedFrames, copy_newest};
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

pub(super) const PROBE_LIMIT: Duration = Duration::from_secs(20);
pub(super) const RETRY_LIMIT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Plan {
    Probe,
    InlineUntilPictures,
    Retry,
    On,
    Off,
}

/// How a shared subscription ended, for the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// At least one picture came through the file.
    Pictures,
    /// The file can not be used: refusal, file error, or the emulator sending
    /// the pixels inside the messages.
    Unusable(Fallback),
    /// No picture within the limit.
    NoPicture(Duration),
    /// The stream ended or the connection failed before any picture.
    Ended,
}

impl Plan {
    pub fn limit(self) -> Option<Duration> {
        match self {
            Self::Probe => Some(PROBE_LIMIT),
            Self::Retry | Self::On => Some(RETRY_LIMIT),
            Self::InlineUntilPictures | Self::Off => None,
        }
    }

    /// The next plan and, when it changes how frames arrive, what to report.
    pub fn after(self, outcome: &Outcome) -> (Self, Option<VideoTransport>) {
        let inline = |fallback| VideoTransport::Inline {
            fallback: Some(fallback),
        };
        match (self, outcome) {
            (_, Outcome::Ended) => (self, None),
            (Self::Probe | Self::Retry | Self::On, Outcome::Pictures) => (Self::On, None),
            (Self::Probe, Outcome::NoPicture(limit)) => (
                Self::InlineUntilPictures,
                Some(inline(Fallback::NoPicture {
                    seconds: limit.as_secs(),
                })),
            ),
            (Self::Retry | Self::On, Outcome::NoPicture(limit)) => (
                Self::Off,
                Some(inline(Fallback::NoPicture {
                    seconds: limit.as_secs(),
                })),
            ),
            (_, Outcome::Unusable(fallback)) => (Self::Off, Some(inline(fallback.clone()))),
            (plan, _) => (plan, None),
        }
    }

    /// In-band pictures flowed: time for the single retry.
    pub fn after_inline_pictures(self) -> Self {
        match self {
            Self::InlineUntilPictures => Self::Retry,
            plan => plan,
        }
    }
}

/// Bytes of an RGBA frame whose pixels the emulator wrote to the shared file.
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

/// Status codes with which the emulator rejects the handle or the region
/// (`SharedMemoryLibrary` and `getScreenshot`); anything else (unavailable,
/// cancelled, unknown...) is the connection or the VM going away.
pub(super) fn refusal(status: &Status) -> Option<Fallback> {
    matches!(
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
    )
    .then(|| Fallback::EngineRefused {
        code: format!("{:?}", status.code()),
        message: status.message().to_string(),
    })
}

type Item = Result<Option<Frame>, Status>;

fn is_shared_frame(item: &Item) -> bool {
    matches!(item, Ok(Some(frame)) if frame
        .format
        .as_ref()
        .and_then(|format| shared_frame_len(format, &frame.pixels))
        .is_some())
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

fn ended(
    error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> (Outcome, DisplayResult<()>) {
    (Outcome::Ended, Err(error.into()))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn receive_shared(
    video: &mut Grpc<Channel>,
    token: &MetadataValue<Ascii>,
    ctx: &egui::Context,
    state: &Mutex<Shared>,
    request: Request,
    fixed_raster: Option<[u32; 2]>,
    file: &SharedFrames,
    limit: Duration,
) -> (Outcome, DisplayResult<()>) {
    let limit = match request.reduction {
        Some(_) => limit.max(FIRST_REDUCED_FRAME),
        None => limit,
    };
    let picture_deadline = time::Instant::now() + limit;
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
                            handle: file.handle(),
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
            .await
            .map_err(|status| match refusal(&status) {
                Some(fallback) => Err(fallback),
                None => Ok(Box::<dyn std::error::Error + Send + Sync>::from(status)),
            });
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(reply)
    };
    let mut frames = match time::timeout_at(picture_deadline, start).await {
        Err(_) => return (Outcome::NoPicture(limit), Ok(())),
        Ok(Err(error)) => return ended(error),
        Ok(Ok(Err(Err(fallback)))) => return (Outcome::Unusable(fallback), Ok(())),
        Ok(Ok(Err(Ok(error)))) => return ended(error),
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
                Err(_) => return (Outcome::NoPicture(limit), Ok(())),
            },
            None => frames.message().await,
        };
        let item = if is_shared_frame(&item) {
            // Jump to the newest received message, copy, and redo the copy if
            // an even newer one arrived meanwhile; its size and rotation then
            // describe exactly the pixels copied.
            let (newest, pixels, rest, _) = copy_newest(
                item,
                || poll_pending(&mut frames),
                is_shared_frame,
                |item| match item {
                    Ok(Some(frame)) => file.read(
                        frame
                            .format
                            .as_ref()
                            .and_then(|format| shared_frame_len(format, &frame.pixels))
                            .unwrap_or(0),
                    ),
                    _ => Ok(vec![]),
                },
            );
            lookahead = rest;
            match (newest, pixels) {
                (_, Err(error)) => {
                    return (
                        Outcome::Unusable(Fallback::FileError(error.to_string())),
                        Ok(()),
                    );
                }
                (Ok(Some(mut frame)), Ok(pixels)) => {
                    frame.pixels = pixels.into();
                    Ok(Some(frame))
                }
                (other, _) => other,
            }
        } else {
            item
        };
        let frame = match item {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                let outcome = if pictures {
                    Outcome::Pictures
                } else {
                    Outcome::Ended
                };
                return (outcome, Err("O motor encerrou o fluxo de vídeo.".into()));
            }
            Err(status) => {
                return match (pictures, refusal(&status)) {
                    (false, Some(fallback)) => (Outcome::Unusable(fallback), Ok(())),
                    (true, _) => (Outcome::Pictures, Err(status.into())),
                    (false, None) => ended(status),
                };
            }
        };
        let mapped = frame
            .format
            .as_ref()
            .and_then(|format| format.transport.as_ref())
            .is_some_and(|transport| transport.channel == MMAP);
        if !frame.pixels.is_empty() && !mapped {
            // The emulator ignored the request; this subscription still works.
            return (Outcome::Unusable(Fallback::EngineSentInline), Ok(()));
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
                return (
                    if pictures {
                        Outcome::Pictures
                    } else {
                        Outcome::Ended
                    },
                    Ok(()),
                );
            }
            continue;
        }
        if let Some(reduction) = request.reduction {
            match reduction.accepts(&frame) {
                Ok(true) => {}
                Ok(false) => return (Outcome::Pictures, Ok(())),
                Err(error) => return (Outcome::Pictures, Err(error)),
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
    use super::*;

    fn status(code: Code) -> Status {
        Status::new(code, "Rejected shared memory handle 'file:///x'")
    }

    #[test]
    fn plan_falls_back_late_during_boot_retries_once_and_never_stays_black() {
        use Plan::*;
        let no_picture = |limit| Outcome::NoPicture(limit);
        // Boot is slow: the probe gives up only after the generous limit.
        assert_eq!(Probe.limit(), Some(PROBE_LIMIT));
        assert!(PROBE_LIMIT >= Duration::from_secs(15));
        let (plan, report) = Probe.after(&no_picture(PROBE_LIMIT));
        assert_eq!(plan, InlineUntilPictures);
        assert_eq!(
            report,
            Some(VideoTransport::Inline {
                fallback: Some(Fallback::NoPicture { seconds: 20 })
            })
        );
        // In-band has no limit, as before; once pictures flow, one short retry.
        assert_eq!(plan.limit(), None);
        let plan = plan.after_inline_pictures();
        assert_eq!((plan, plan.limit()), (Retry, Some(RETRY_LIMIT)));
        // A failed retry turns the file off for the session.
        let (off, report) = plan.after(&no_picture(RETRY_LIMIT));
        assert_eq!(off, Off);
        assert!(report.is_some());
        assert_eq!(off.after_inline_pictures(), Off);
        assert_eq!(off.limit(), None);
        // A successful retry or probe turns it on; later subscriptions keep a limit.
        assert_eq!(Retry.after(&Outcome::Pictures), (On, None));
        assert_eq!(Probe.after(&Outcome::Pictures), (On, None));
        assert_eq!(On.limit(), Some(RETRY_LIMIT));
        assert_eq!(On.after(&no_picture(RETRY_LIMIT)).0, Off);
        // A refusal is final at once.
        let refused = Outcome::Unusable(refusal(&status(Code::PermissionDenied)).unwrap());
        assert_eq!(Probe.after(&refused).0, Off);
        assert_eq!(On.after(&refused).0, Off);
        // The VM stopping is not a transition and reports nothing.
        for plan in [Probe, InlineUntilPictures, Retry, On, Off] {
            assert_eq!(plan.after(&Outcome::Ended), (plan, None));
        }
    }

    #[test]
    fn only_handle_errors_are_refusals_and_they_keep_the_engines_words() {
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
        ] {
            assert_eq!(refusal(&status(code)), None, "{code:?}");
        }
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
        assert!(is_shared_frame(&frame(format.clone(), vec![])));
        assert_eq!(shared_frame_len(&format, &[]), Some(800 * 450 * 4));
        assert!(!is_shared_frame(&frame(format.clone(), vec![1, 2, 3, 4])));
        let mut blank = format.clone();
        blank.width = 0;
        assert!(!is_shared_frame(&frame(blank, vec![])));
        let mut png = format.clone();
        png.format = 0;
        assert!(!is_shared_frame(&frame(png, vec![])));
        let mut inline = format;
        inline.transport = None;
        assert!(!is_shared_frame(&frame(inline, vec![])));
        assert!(!is_shared_frame(&Ok(None)));
        assert!(!is_shared_frame(&Err(status(Code::Unavailable))));
    }
}
