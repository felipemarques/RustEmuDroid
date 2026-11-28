//! Whether the engine's picture on the screen is live, judged by its content.
//!
//! Every [`PERIOD`] the panel's area of the screen is read at a tiny size and
//! the engine is asked for its current frame at a tiny size. Each is compared
//! with its own previous sample only: a still picture gives byte-identical
//! samples on both sides, so counting the bytes that changed tells even a thin
//! moving line apart from a still picture. A screen area
//! that stays the same while the engine's frames change means the picture the
//! user sees is not the engine's any more (a surface that stopped presenting
//! or was hidden, a black panel). Window structure is not looked at. Samples are taken only while nothing covers the panel and the
//! client draws over little of it.

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
/// Consecutive periods with a moving engine and a still screen.
const STRIKES: u8 = 3;

pub(crate) type EngineSample = oneshot::Receiver<Option<Vec<u8>>>;

#[derive(Default)]
pub(crate) struct Liveness {
    next: Option<Instant>,
    pending: Option<(EngineSample, Vec<u8>)>,
    last: Option<(Vec<u8>, Vec<u8>)>,
    strikes: u8,
    /// Periods judged, and how many found the screen following the engine.
    pub(crate) judged: u32,
    pub(crate) followed: u32,
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

impl Liveness {
    /// Forget the previous samples (the panel moved, was covered, or showed
    /// copied frames meanwhile).
    pub(crate) fn reset(&mut self) {
        self.pending = None;
        self.last = None;
        self.strikes = 0;
    }

    /// One step; `true` when the picture on the screen is stale. `sample`
    /// asks the engine for a frame, `screen` reads the panel's screen area;
    /// both are called only when a period is due and `can_sample` holds.
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
        if let Some((receiver, _)) = &mut self.pending {
            match receiver.try_recv() {
                Err(oneshot::error::TryRecvError::Empty) => return false,
                Err(oneshot::error::TryRecvError::Closed) | Ok(None) => self.pending = None,
                Ok(Some(engine)) => {
                    let (_, shown) = self.pending.take().unwrap();
                    return self.judge(engine, shown);
                }
            }
        }
        if self.next.is_some_and(|next| now < next) {
            return false;
        }
        self.next = Some(now + PERIOD);
        let (Some(shown), Some(receiver)) = (screen(), sample()) else {
            self.reset();
            return false;
        };
        self.pending = Some((receiver, shown));
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
        let previous = self.last.replace((engine, shown));
        let Some((engine_before, shown_before)) = previous else {
            return false;
        };
        let (engine_now, shown_now) = self.last.as_ref().unwrap();
        let (Some(engine_moved), Some(screen_moved)) = (
            difference(&engine_before, engine_now),
            difference(&shown_before, shown_now),
        ) else {
            return false;
        };
        if screen_moved > SCREEN_MOVED {
            self.strikes = 0;
            if engine_moved > ENGINE_MOVED {
                self.judged += 1;
                self.followed += 1;
            }
        } else if engine_moved > ENGINE_MOVED {
            self.judged += 1;
            self.strikes += 1;
        }
        self.strikes >= STRIKES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(value: u8) -> Vec<u8> {
        vec![value; (SAMPLE.0 * SAMPLE.1 * 3) as usize]
    }

    /// Runs one period whose engine sample is `engine` and screen sample is `shown`.
    fn period(live: &mut Liveness, now: &mut Instant, engine: u8, shown: u8) -> bool {
        *now += PERIOD;
        let (sender, receiver) = oneshot::channel();
        assert!(!live.poll(*now, true, || Some(receiver), || Some(picture(shown))));
        assert!(live.waiting());
        sender.send(Some(picture(engine))).unwrap();
        live.poll(*now, true, || unreachable!(), || unreachable!())
    }

    #[test]
    fn a_screen_that_follows_the_engine_is_live_and_a_still_one_is_not() {
        let mut live = Liveness::default();
        let mut now = Instant::now();
        // A moving game on a live panel.
        for step in 0..6 {
            assert!(!period(&mut live, &mut now, step * 20, step * 20));
        }
        assert_eq!((live.judged, live.followed), (5, 5));
        // A still Android screen proves nothing either way.
        for _ in 0..6 {
            assert!(!period(&mut live, &mut now, 100, 100));
        }
        // The engine moves, the screen does not: stale on the third period.
        assert!(!period(&mut live, &mut now, 130, 100));
        assert!(!period(&mut live, &mut now, 160, 100));
        assert!(period(&mut live, &mut now, 190, 100));
    }

    #[test]
    fn covered_or_failed_samples_start_the_count_again() {
        let mut live = Liveness::default();
        let mut now = Instant::now();
        assert!(!period(&mut live, &mut now, 0, 50));
        assert!(!period(&mut live, &mut now, 40, 50));
        assert!(!period(&mut live, &mut now, 80, 50));
        // Covered: nothing is sampled and the strikes are forgotten.
        now += PERIOD;
        assert!(!live.poll(now, false, || unreachable!(), || unreachable!()));
        assert!(!period(&mut live, &mut now, 120, 50));
        assert!(!period(&mut live, &mut now, 160, 50));
        assert!(!period(&mut live, &mut now, 200, 50));
        assert!(period(&mut live, &mut now, 240, 50));
        // An engine without an answer is no evidence.
        let mut live = Liveness::default();
        now += PERIOD;
        let (sender, receiver) = oneshot::channel();
        assert!(!live.poll(now, true, || Some(receiver), || Some(picture(1))));
        sender.send(None).unwrap();
        assert!(!live.poll(now, true, || unreachable!(), || unreachable!()));
        assert!(!live.waiting());
        assert_eq!(difference(&[1, 2], &[1]), None);
        assert_eq!(difference(&[0, 10, 7, 7], &[10, 0, 9, 7]), Some(0.5));
        // A thin line crossing a tiny sample is motion; a still picture is not.
        let still = picture(40);
        let mut line = still.clone();
        for index in 0..30 {
            line[index * 50] = 200;
        }
        assert!(difference(&still, &line).unwrap() > ENGINE_MOVED);
        assert_eq!(difference(&still, &still), Some(0.0));
    }
}
