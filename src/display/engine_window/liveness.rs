//! Whether the engine's picture on the screen is live, judged by its content.
//!
//! Every [`PERIOD`] the engine is asked for its current frame at a tiny size
//! and, as soon as it answers, the panel's area of the screen is read at a
//! tiny size, so both samples show the same moment as closely as the screen
//! allows. Two things count against the picture on the screen:
//!
//! - it stands still while the engine's frames change: each side is compared
//!   with its own previous sample only, and a still picture gives
//!   byte-identical samples, so counting the bytes that changed tells even a
//!   thin moving line apart from a still picture;
//! - it is blank (one flat colour) while the engine's frame has a picture of
//!   another colour: a surface that presents nothing is noticed even while
//!   Android itself stands still.
//!
//! Window structure is not looked at. Samples are taken only while the
//! client's window is shown, nothing covers the panel and the client draws
//! over little of it; a period in which the panel cannot be seen is no
//! evidence either way and starts the count again.

use std::time::{Duration, Instant};
use tokio::sync::oneshot;

pub(crate) const PERIOD: Duration = Duration::from_millis(1500);
/// Tiny samples: enough to see a game move, cheap to read and compare.
pub(crate) const SAMPLE: (u32, u32) = (32, 18);
/// Share of the bytes that changed (by more than [`NOISE`]) above which a
/// sample moved.
const ENGINE_MOVED: f32 = 0.01;
const SCREEN_MOVED: f32 = 0.002;
const NOISE: u8 = 3;
/// A screen sample whose colour channels each span at most this much is one
/// flat colour.
const FLAT: u8 = 12;
/// An engine frame whose colour channels span more than this has a picture.
const DETAIL: u8 = 48;
/// A blank screen counts only when its colour is this far (on some channel)
/// from the average colour of the engine's frame.
const OTHER_COLOUR: u8 = 40;
/// Consecutive periods with evidence against the picture on the screen.
const STRIKES: u8 = 3;

/// The engine's samples are RGBA, the screen's RGB.
const ENGINE_CHANNELS: usize = 4;
const SCREEN_CHANNELS: usize = 3;

pub(crate) type EngineSample = oneshot::Receiver<Option<Vec<u8>>>;

/// What one period says about the picture on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Evidence {
    /// Nothing to go by: the engine's frame did not change and the screen
    /// is not blank, or a sample was unusable.
    Nothing,
    /// The screen changed.
    Follows,
    /// The engine's frames changed and the screen stood still.
    StoodStill,
    /// The screen is one flat colour while the engine's frame has a picture
    /// of another colour.
    Blank,
}

#[derive(Default)]
pub(crate) struct Liveness {
    next: Option<Instant>,
    pending: Option<EngineSample>,
    last: Option<(Vec<u8>, Vec<u8>)>,
    strikes: u8,
    /// Periods judged, and how many found the screen following the engine.
    pub(crate) judged: u32,
    pub(crate) followed: u32,
    /// What the last period found, for the frame statistics.
    pub(crate) last_evidence: Option<Evidence>,
}

/// The share of the bytes that differ by more than [`NOISE`], or `None` for
/// samples of other sizes.
pub(crate) fn difference(a: &[u8], b: &[u8]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let changed = a
        .iter()
        .zip(b)
        .filter(|(x, y)| x.abs_diff(**y) > NOISE)
        .count();
    Some(changed as f32 / a.len() as f32)
}

/// Per colour channel: the smallest and largest value and the mean, or
/// `None` for an empty or ragged sample.
fn colours(sample: &[u8], channels: usize) -> Option<[(u8, u8, f32); 3]> {
    if sample.is_empty() || sample.len() % channels != 0 {
        return None;
    }
    let pixels = sample.len() / channels;
    let mut out = [(u8::MAX, u8::MIN, 0.0f32); 3];
    for (channel, (low, high, mean)) in out.iter_mut().enumerate() {
        let mut sum = 0u64;
        for value in sample.iter().skip(channel).step_by(channels) {
            *low = (*low).min(*value);
            *high = (*high).max(*value);
            sum += u64::from(*value);
        }
        *mean = sum as f32 / pixels as f32;
    }
    Some(out)
}

/// The screen sample is one flat colour while the engine's frame has a
/// picture whose average colour is clearly another.
pub(crate) fn blank(engine: &[u8], shown: &[u8]) -> bool {
    let (Some(engine), Some(shown)) = (
        colours(engine, ENGINE_CHANNELS),
        colours(shown, SCREEN_CHANNELS),
    ) else {
        return false;
    };
    let flat = shown.iter().all(|(low, high, _)| high - low <= FLAT);
    let detailed = engine.iter().any(|(low, high, _)| high - low > DETAIL);
    let other = engine
        .iter()
        .zip(&shown)
        .any(|((_, _, a), (_, _, b))| (a - b).abs() > f32::from(OTHER_COLOUR));
    flat && detailed && other
}

/// What a period with the samples `now` (engine, screen) says, given the
/// samples of the period before, if that one was judged too.
pub(crate) fn evidence(before: Option<(&[u8], &[u8])>, now: (&[u8], &[u8])) -> Evidence {
    let (engine, shown) = now;
    if blank(engine, shown) {
        return Evidence::Blank;
    }
    let Some((engine_before, shown_before)) = before else {
        return Evidence::Nothing;
    };
    let (Some(engine_moved), Some(screen_moved)) = (
        difference(engine_before, engine),
        difference(shown_before, shown),
    ) else {
        return Evidence::Nothing;
    };
    if screen_moved > SCREEN_MOVED {
        Evidence::Follows
    } else if engine_moved > ENGINE_MOVED {
        Evidence::StoodStill
    } else {
        Evidence::Nothing
    }
}

/// The strikes after a period with `evidence`, starting from `strikes`.
pub(crate) fn count(strikes: u8, evidence: Evidence) -> u8 {
    match evidence {
        Evidence::Follows => 0,
        Evidence::StoodStill | Evidence::Blank => strikes.saturating_add(1),
        Evidence::Nothing => strikes,
    }
}

impl Liveness {
    /// Forget the previous samples (the panel moved, was covered, or showed
    /// copied frames meanwhile).
    pub(crate) fn reset(&mut self) {
        self.pending = None;
        self.last = None;
        self.strikes = 0;
    }

    /// One step; `true` when the picture on the screen is stale. `sample`
    /// asks the engine for a frame when a period is due; `screen` reads the
    /// panel's screen area once the engine answered. Neither is called
    /// unless `can_sample` holds.
    pub(crate) fn poll(
        &mut self,
        now: Instant,
        can_sample: bool,
        sample: impl FnOnce() -> Option<EngineSample>,
        screen: impl FnOnce() -> Option<Vec<u8>>,
    ) -> bool {
        if !can_sample {
            self.reset();
            self.next = Some(now + PERIOD);
            return false;
        }
        if let Some(receiver) = &mut self.pending {
            return match receiver.try_recv() {
                Err(oneshot::error::TryRecvError::Empty) => false,
                Err(oneshot::error::TryRecvError::Closed) | Ok(None) => {
                    self.pending = None;
                    false
                }
                Ok(Some(engine)) => {
                    self.pending = None;
                    match screen() {
                        Some(shown) => self.judge(engine, shown),
                        None => {
                            self.reset();
                            false
                        }
                    }
                }
            };
        }
        if self.next.is_some_and(|next| now < next) {
            return false;
        }
        self.next = Some(now + PERIOD);
        match sample() {
            Some(receiver) => self.pending = Some(receiver),
            None => self.reset(),
        }
        false
    }

    /// Whether a sample is on its way (the caller should look again soon).
    pub(crate) fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    fn judge(&mut self, engine: Vec<u8>, shown: Vec<u8>) -> bool {
        if self.last.is_none() && std::env::var_os("RUSTEMUDROID_FRAME_STATS").is_some() {
            println!(
                "engine_liveness sample engine_bytes={} screen_bytes={}",
                engine.len(),
                shown.len()
            );
        }
        let found = evidence(
            self.last
                .as_ref()
                .map(|(engine, shown)| (engine.as_slice(), shown.as_slice())),
            (&engine, &shown),
        );
        self.last = Some((engine, shown));
        self.last_evidence = Some(found);
        match found {
            Evidence::Follows => {
                self.judged += 1;
                self.followed += 1;
            }
            Evidence::StoodStill | Evidence::Blank => self.judged += 1,
            Evidence::Nothing => {}
        }
        self.strikes = count(self.strikes, found);
        self.strikes >= STRIKES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIXELS: usize = (SAMPLE.0 * SAMPLE.1) as usize;

    /// An engine frame (RGBA) of one grey level with a darker band, so that
    /// it has a picture; `shift` moves the band.
    fn frame(level: u8, shift: usize) -> Vec<u8> {
        let mut pixels = vec![255u8; PIXELS * 4];
        for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            let dark = (index + shift) % 32 < 4;
            let value = if dark { level / 3 } else { level };
            pixel[..3].fill(value);
        }
        pixels
    }

    /// The screen (RGB) showing that frame.
    fn screen_of(level: u8, shift: usize) -> Vec<u8> {
        frame(level, shift)
            .chunks_exact(4)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect()
    }

    fn flat_screen(value: u8) -> Vec<u8> {
        vec![value; PIXELS * 3]
    }

    /// One period of a sequence: what the engine has, what the screen shows,
    /// and whether the panel can be seen at all.
    struct Step {
        engine: Vec<u8>,
        shown: Vec<u8>,
        seen: bool,
    }

    fn seen(engine: Vec<u8>, shown: Vec<u8>) -> Step {
        Step {
            engine,
            shown,
            seen: true,
        }
    }

    fn unseen() -> Step {
        Step {
            engine: Vec::new(),
            shown: Vec::new(),
            seen: false,
        }
    }

    /// Runs `steps` through the check, one period each; the periods at which
    /// it found the picture stale.
    fn run(steps: Vec<Step>) -> Vec<usize> {
        let mut live = Liveness::default();
        let mut now = Instant::now();
        let mut stale = Vec::new();
        for (index, step) in steps.into_iter().enumerate() {
            now += PERIOD;
            if !step.seen {
                assert!(!live.poll(now, false, || unreachable!(), || unreachable!()));
                continue;
            }
            let (sender, receiver) = oneshot::channel();
            assert!(!live.poll(now, true, || Some(receiver), || unreachable!()));
            assert!(live.waiting());
            sender.send(Some(step.engine)).unwrap();
            let shown = step.shown;
            if live.poll(now, true, || unreachable!(), || Some(shown)) {
                stale.push(index);
            }
        }
        stale
    }

    #[test]
    fn a_screen_that_follows_the_engine_is_live() {
        // A moving game on a live panel, then a still one.
        let mut steps: Vec<_> = (0..8)
            .map(|step| seen(frame(200, step * 3), screen_of(200, step * 3)))
            .collect();
        steps.extend((0..8).map(|_| seen(frame(200, 0), screen_of(200, 0))));
        assert!(run(steps).is_empty());
    }

    #[test]
    fn a_screen_that_stands_still_while_the_engine_moves_is_stale() {
        let steps = (0..4)
            .map(|step| seen(frame(200, step * 3), screen_of(200, 0)))
            .collect();
        // Judged from the second period on: stale on the fourth.
        assert_eq!(run(steps), [3]);
    }

    #[test]
    fn a_blank_screen_is_stale_even_while_android_stands_still() {
        // A black panel over a still Settings page, and over a moving one.
        let still = (0..3)
            .map(|_| seen(frame(240, 0), flat_screen(0)))
            .collect();
        assert_eq!(run(still), [2]);
        let moving = (0..3)
            .map(|step| seen(frame(240, step * 5), flat_screen(0)))
            .collect();
        assert_eq!(run(moving), [2]);
    }

    #[test]
    fn a_blank_android_screen_is_no_evidence() {
        // Android itself shows one colour (a black loading screen, a splash):
        // the screen showing it is flat too, and that is right.
        let black = vec![0u8; PIXELS * 4];
        let steps = (0..6)
            .map(|_| seen(black.clone(), flat_screen(0)))
            .collect();
        assert!(run(steps).is_empty());
        // A mostly dark frame whose average the flat screen matches is no
        // proof either: a small light dot may be averaged away on the screen.
        let mut dot = vec![0u8; PIXELS * 4];
        dot[..12].fill(255);
        let steps = (0..6).map(|_| seen(dot.clone(), flat_screen(2))).collect();
        assert!(run(steps).is_empty());
    }

    #[test]
    fn a_panel_that_cannot_be_seen_is_no_evidence_and_starts_the_count_again() {
        // Covered, minimised, locked or off the desktop between strikes.
        let steps = vec![
            seen(frame(200, 0), screen_of(200, 0)),
            seen(frame(200, 3), screen_of(200, 0)),
            seen(frame(200, 6), screen_of(200, 0)),
            unseen(),
            seen(frame(200, 9), flat_screen(0)),
            seen(frame(200, 12), flat_screen(0)),
            unseen(),
            unseen(),
            seen(frame(200, 15), flat_screen(0)),
            seen(frame(200, 18), flat_screen(0)),
            seen(frame(200, 21), flat_screen(0)),
        ];
        assert_eq!(run(steps), [10]);
    }

    #[test]
    fn noise_and_small_changes_are_not_motion() {
        // The clock or a cursor changes a pixel of the engine's frame and the
        // screen, averaged, barely changes: nothing either way.
        let mut clock = frame(200, 0);
        clock[0] = 90;
        let steps = vec![
            seen(frame(200, 0), screen_of(200, 0)),
            seen(clock.clone(), screen_of(200, 0)),
            seen(frame(200, 0), screen_of(200, 0)),
            seen(clock, screen_of(200, 0)),
            seen(frame(200, 0), screen_of(200, 0)),
        ];
        assert!(run(steps).is_empty());
        let mut screen = screen_of(200, 0);
        screen[7] = screen[7].saturating_add(NOISE);
        assert_eq!(difference(&screen_of(200, 0), &screen), Some(0.0));
    }

    #[test]
    fn an_engine_or_screen_without_an_answer_is_no_evidence() {
        let mut live = Liveness::default();
        let now = Instant::now() + PERIOD;
        let (sender, receiver) = oneshot::channel();
        assert!(!live.poll(now, true, || Some(receiver), || unreachable!()));
        sender.send(None).unwrap();
        assert!(!live.poll(now, true, || unreachable!(), || unreachable!()));
        assert!(!live.waiting());
        let now = now + PERIOD;
        let (sender, receiver) = oneshot::channel();
        assert!(!live.poll(now, true, || Some(receiver), || unreachable!()));
        sender.send(Some(frame(200, 0))).unwrap();
        assert!(!live.poll(now, true, || unreachable!(), || None));
        assert_eq!(live.judged, 0);
    }

    #[test]
    fn evidence_and_counting() {
        assert_eq!(difference(&[1, 2], &[1]), None);
        assert_eq!(difference(&[0, 10, 7, 7], &[10, 0, 9, 7]), Some(0.5));
        // A thin line crossing a tiny sample is motion; a still picture is not.
        let still = vec![40u8; PIXELS * 3];
        let mut line = still.clone();
        for index in 0..30 {
            line[index * 50] = 200;
        }
        assert!(difference(&still, &line).unwrap() > ENGINE_MOVED);
        assert_eq!(difference(&still, &still), Some(0.0));
        let (engine, shown) = (frame(200, 0), screen_of(200, 0));
        assert_eq!(evidence(None, (&engine, &shown)), Evidence::Nothing);
        assert_eq!(evidence(None, (&engine, &flat_screen(0))), Evidence::Blank);
        let moved = frame(200, 4);
        assert_eq!(
            evidence(Some((&engine, &shown)), (&moved, &shown)),
            Evidence::StoodStill
        );
        assert_eq!(
            evidence(Some((&engine, &shown)), (&moved, &screen_of(200, 4))),
            Evidence::Follows
        );
        assert!(!blank(&[], &flat_screen(0)));
        assert!(!blank(&engine, &[0, 0]));
        assert_eq!(count(2, Evidence::Follows), 0);
        assert_eq!(count(2, Evidence::Nothing), 2);
        assert_eq!(count(2, Evidence::Blank), 3);
        assert_eq!(count(u8::MAX, Evidence::StoodStill), u8::MAX);
    }
}
