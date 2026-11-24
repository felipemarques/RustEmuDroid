use super::{Completed, Job, Mode, Shot, encode};
use prost::bytes::Bytes;
use std::{
    collections::VecDeque,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub(super) const FPS: u32 = 10;
const FRAME_PERIOD: Duration = Duration::from_millis(1_000 / FPS as u64);
const WINDOW: Duration = Duration::from_secs(3);
const FRAME_LIMIT: u64 = WINDOW.as_secs() * FPS as u64;
const SOURCE_GAP: Duration = Duration::from_millis(500);
const MAX_EDGE: u32 = 720;

enum Command {
    Sample {
        slot: u64,
        shot: Shot,
    },
    Reset,
    Save {
        slot: u64,
        dir: PathBuf,
        completed: Completed,
    },
}

pub(super) struct Replay {
    commands: mpsc::Sender<Command>,
    sampling: Arc<AtomicBool>,
    observed: Option<(Instant, (u32, u32))>,
    started: Option<Instant>,
    sampled: Option<Instant>,
}

impl Replay {
    pub(super) fn new() -> io::Result<Self> {
        let (commands, rx) = mpsc::channel();
        let sampling = Arc::new(AtomicBool::new(false));
        let worker_sampling = Arc::clone(&sampling);
        thread::Builder::new()
            .name("capture-replay-buffer".into())
            .spawn(move || run(rx, &worker_sampling))?;
        Ok(Self {
            commands,
            sampling,
            observed: None,
            started: None,
            sampled: None,
        })
    }

    pub(super) fn observe(&mut self, shot: &Shot, at: Instant) {
        let size = (shot.width, shot.height);
        if shot.validate().is_err() {
            self.reset();
            return;
        }
        if self
            .observed
            .is_some_and(|(last, previous)| at - last >= SOURCE_GAP || previous != size)
        {
            self.reset();
        }
        self.observed = Some((at, size));

        if self.sampled.is_some_and(|last| at - last < FRAME_PERIOD)
            || self.sampling.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let start = self.started.unwrap_or(at);
        let slot = slot_at(start, at);
        // Keep only one full-resolution sample in flight. The display never waits for scaling.
        if self
            .commands
            .send(Command::Sample {
                slot,
                shot: shot.clone(),
            })
            .is_ok()
        {
            self.started = Some(start);
            self.sampled = Some(at);
        } else {
            self.sampling.store(false, Ordering::Release);
            self.started = None;
        }
    }

    pub(super) fn reset(&mut self) {
        self.observed = None;
        if self.started.take().is_some() {
            let _ = self.commands.send(Command::Reset);
        }
    }

    fn current_slot(&self, at: Instant) -> Option<u64> {
        let (last, _) = self.observed?;
        let start = self.started?;
        (at - last < SOURCE_GAP && at - start >= WINDOW).then(|| slot_at(start, at))
    }

    pub(super) fn ready(&self, at: Instant) -> bool {
        self.current_slot(at).is_some()
    }

    pub(super) fn save(&self, at: Instant, dir: &Path, completed: Completed) -> io::Result<()> {
        let slot = self.current_slot(at).ok_or(io::ErrorKind::NotFound)?;
        self.commands
            .send(Command::Save {
                slot,
                dir: dir.to_owned(),
                completed,
            })
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
}

fn slot_at(start: Instant, at: Instant) -> u64 {
    ((at - start).as_millis() / FRAME_PERIOD.as_millis()) as u64
}

#[derive(Default)]
struct History {
    input: Option<Shot>,
    size: (u32, u32),
    frames: VecDeque<(u64, Bytes)>,
}

impl History {
    fn sample(&mut self, slot: u64, shot: Shot) {
        let pixels = if self
            .input
            .as_ref()
            .is_some_and(|input| input.pixels.as_ptr() == shot.pixels.as_ptr())
        {
            self.frames.back().unwrap().1.clone()
        } else {
            let (size, pixels) = compact(&shot);
            self.size = size;
            self.input = Some(shot);
            pixels
        };
        let oldest_slot = slot.saturating_sub(FRAME_LIMIT - 1);
        // Retain the predecessor when a slot was skipped so its image can fill the gap.
        while self
            .frames
            .get(1)
            .is_some_and(|(next, _)| *next <= oldest_slot)
        {
            self.frames.pop_front();
        }
        self.frames.push_back((slot, pixels));
    }

    fn clip(&self, slot: u64) -> Option<Clip> {
        let mut frames = self.frames.iter().peekable();
        let (first_slot, mut current) = frames.next().map(|(slot, pixels)| (*slot, pixels))?;
        let oldest_slot = first_slot.max(slot.saturating_sub(FRAME_LIMIT - 1));
        let pixels = (oldest_slot..=slot)
            .map(|frame_slot| {
                while frames.peek().is_some_and(|(next, _)| *next <= frame_slot) {
                    current = &frames.next().unwrap().1;
                }
                current.clone()
            })
            .collect();
        Some(Clip {
            size: self.size,
            pixels,
        })
    }
}

struct Clip {
    size: (u32, u32),
    pixels: Vec<Bytes>,
}

impl Clip {
    fn save(self, dir: &Path) -> io::Result<PathBuf> {
        encode(Mode::Moment, self.size, "rgb24", dir, |pipe, _| {
            for pixels in self.pixels {
                pipe.write_all(&pixels)?;
            }
            Ok(())
        })
    }
}

fn run(commands: mpsc::Receiver<Command>, sampling: &AtomicBool) {
    let mut history = History::default();
    while let Ok(command) = commands.recv() {
        match command {
            Command::Sample { slot, shot } => {
                history.sample(slot, shot);
                sampling.store(false, Ordering::Release);
            }
            Command::Reset => history = History::default(),
            Command::Save {
                slot,
                dir,
                completed,
            } => {
                // This runs after the last queued sample. The encoder shares compact pixels
                // and drains them independently so the buffer can continue following the display.
                let clip = history.clip(slot);
                let worker_completed = completed.clone();
                if let Err(error) = thread::Builder::new()
                    .name("capture-replay-encoder".into())
                    .spawn(move || {
                        let result = clip
                            .ok_or_else(|| io::ErrorKind::NotFound.into())
                            .and_then(|clip| clip.save(&dir));
                        let _ = worker_completed.send((Job::Replay, result));
                    })
                {
                    let _ = completed.send((Job::Replay, Err(error)));
                }
            }
        }
    }
}

fn compact(shot: &Shot) -> ((u32, u32), Bytes) {
    let long_edge = shot.width.max(shot.height);
    let scale = |edge: u32| {
        if long_edge <= MAX_EDGE {
            return edge;
        }
        let long_edge = u64::from(long_edge);
        ((u64::from(edge) * u64::from(MAX_EDGE) + long_edge / 2) / long_edge).max(1) as u32
    };
    let size = (scale(shot.width), scale(shot.height));
    let mut rgb = Vec::with_capacity((size.0 * size.1 * 3) as usize);
    if size == (shot.width, shot.height) {
        for pixel in shot.pixels.chunks_exact(4) {
            rgb.extend_from_slice(&pixel[..3]);
        }
    } else {
        let source_index = |index: u32, target: u32, source: u32| {
            (u64::from(2 * index + 1) * u64::from(source) / u64::from(2 * target)) as usize
        };
        let columns: Vec<_> = (0..size.0)
            .map(|x| source_index(x, size.0, shot.width) * 4)
            .collect();
        for y in 0..size.1 {
            let row = source_index(y, size.1, shot.height) * shot.width as usize * 4;
            for x in &columns {
                rgb.extend_from_slice(&shot.pixels[row + x..row + x + 3]);
            }
        }
    }
    (size, rgb.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Capture, Event, tests::decoded_motion};

    fn shot(width: u32, height: u32, value: u8) -> Shot {
        Shot {
            width,
            height,
            pixels: vec![value; (width * height * 4) as usize].into(),
        }
    }

    fn producer() -> (Replay, mpsc::Receiver<Command>) {
        let (commands, rx) = mpsc::channel();
        let replay = Replay {
            commands,
            sampling: Arc::new(false.into()),
            observed: None,
            started: None,
            sampled: None,
        };
        (replay, rx)
    }

    #[test]
    fn replay_requires_three_seconds_of_recent_input_and_has_a_strict_sample_limit() {
        let (mut replay, commands) = producer();
        let at = Instant::now();
        let landscape = shot(2, 1, 1);
        let dir = tempfile::tempdir().unwrap();
        let (completed, _) = mpsc::channel();
        for step in 0..30 {
            let tick = at + FRAME_PERIOD * step;
            replay.observe(&landscape, tick);
            assert!(
                matches!(commands.try_recv().unwrap(), Command::Sample { slot, .. } if slot == u64::from(step))
            );
            replay.sampling.store(false, Ordering::Release);
            // Many display frames at 60 FPS share the same replay interval.
            replay.observe(&landscape, tick + Duration::from_millis(99));
            assert!(commands.try_recv().is_err());
            assert!(!replay.ready(tick + Duration::from_millis(99)));
        }
        assert_eq!(
            replay
                .save(
                    at + WINDOW - Duration::from_millis(1),
                    dir.path(),
                    completed.clone()
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert!(commands.try_recv().is_err());
        assert_eq!(dir.path().read_dir().unwrap().count(), 0);
        assert!(replay.ready(at + WINDOW));
        replay.save(at + WINDOW, dir.path(), completed).unwrap();
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::Save { slot: 30, .. }
        ));
    }

    #[test]
    fn replay_resets_on_dimensions_and_discontinuous_observations() {
        let (mut replay, commands) = producer();
        let at = Instant::now();
        let landscape = shot(2, 1, 1);
        let portrait = shot(1, 2, 2);
        replay.observe(&landscape, at);
        replay.observe(&landscape, at + FRAME_PERIOD);
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::Sample { slot: 0, .. }
        ));
        assert!(
            commands.try_recv().is_err(),
            "only one sample may be in flight"
        );
        replay.sampling.store(false, Ordering::Release);

        replay.observe(&portrait, at + FRAME_PERIOD * 2);
        assert!(matches!(commands.try_recv().unwrap(), Command::Reset));
        assert!(
            matches!(commands.try_recv().unwrap(), Command::Sample { slot: 0, shot } if shot.height == 2)
        );
        replay.sampling.store(false, Ordering::Release);
        let resumed = at + FRAME_PERIOD * 2 + SOURCE_GAP;
        assert!(!replay.ready(resumed));
        replay.observe(&portrait, resumed);
        assert!(matches!(commands.try_recv().unwrap(), Command::Reset));
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::Sample { slot: 0, .. }
        ));
        assert_eq!(replay.started, Some(resumed));
    }

    #[test]
    fn replay_keeps_compact_frames_in_order_and_fills_missing_slots() {
        let (size, pixels) = compact(&shot(1920, 1080, 17));
        assert_eq!(size, (720, 405));
        assert_eq!(pixels.len(), 720 * 405 * 3);
        assert_eq!(compact(&shot(15, 1080, 18)).0, (10, 720));
        let mut history = History::default();
        for slot in 0..60 {
            if slot != 30 && slot != 45 {
                history.sample(slot, shot(2, 1, slot as u8));
            }
            assert!(history.frames.len() <= FRAME_LIMIT as usize);
        }
        let clip = history.clip(59).unwrap();
        let expected: Vec<_> = (30..60)
            .map(|slot| match slot {
                30 => 29,
                45 => 44,
                _ => slot,
            })
            .collect();
        let actual: Vec<_> = clip.pixels.iter().map(|pixels| pixels[0]).collect();
        assert_eq!(actual, expected);

        let still = shot(2, 1, 255);
        history.sample(60, still.clone());
        history.sample(61, still);
        let clip = history.clip(62).unwrap();
        assert!(
            clip.pixels[27..]
                .iter()
                .all(|pixels| pixels.as_ptr() == clip.pixels[27].as_ptr())
        );
    }

    #[test]
    #[ignore = "requires FFmpeg with libx264 and libjxl_anim encoders and decoders"]
    fn replay_drains_pending_frames_and_disarm_preserves_its_export() {
        let dir = tempfile::tempdir().unwrap();
        let dark = shot(16, 16, 0);
        let light = shot(16, 16, 190);
        let brightest = shot(16, 16, 255);
        let mut capture = Capture::default();
        assert!(!capture.replay_armed());
        capture.set_replay_armed(true).unwrap();
        let replay = capture.replay.as_mut().unwrap();
        let start = Instant::now() - WINDOW;
        for slot in 0..30 {
            replay.observe(
                if slot < 15 { &dark } else { &light },
                start + FRAME_PERIOD * slot,
            );
            let deadline = Instant::now() + Duration::from_secs(2);
            while replay.sampling.load(Ordering::Acquire) {
                assert!(Instant::now() < deadline, "replay buffering timed out");
                thread::yield_now();
            }
        }
        replay.observe(&brightest, start + WINDOW);
        assert!(capture.replay_ready());
        capture.save_replay(dir.path()).unwrap();
        assert!(capture.replay_saving());
        assert_eq!(
            capture.save_replay(dir.path()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        capture.set_replay_armed(false).unwrap();
        assert!(!capture.replay_armed());
        capture.start(Mode::Video, &brightest, dir.path()).unwrap();

        let await_save = |capture: &mut Capture| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match capture.poll() {
                    Some(Event::Saved(path)) => return path,
                    Some(Event::Failed(error)) => panic!("{error}"),
                    None => {}
                }
                assert!(Instant::now() < deadline, "capture worker timed out");
                thread::sleep(Duration::from_millis(5));
            }
        };
        let path = await_save(&mut capture);
        assert_eq!(path.extension().unwrap(), "jxl");
        assert!(!capture.replay_saving());
        assert_eq!(capture.mode(), Some(Mode::Video));
        capture.stop();
        assert_eq!(await_save(&mut capture).extension().unwrap(), "mp4");
        assert!(!capture.busy());
        assert_eq!(decoded_motion(&path), (3.0, 30, 3));
    }
}
