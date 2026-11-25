//! Android SurfaceFlinger presentations for the foreground game's SurfaceView.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use eframe::egui;

const PERIOD: Duration = Duration::from_millis(500);
const REDISCOVER: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(crate) struct SurfaceFps {
    package: String,
    pending: Option<Pending>,
    previous: Vec<Sample>,
    selected: Option<String>,
    reading: Option<(Instant, f32)>,
    next_poll: Option<Instant>,
    discovery: Option<Instant>,
}

struct Sample {
    layer: String,
    time: Instant,
    stamps: Vec<u64>,
}

struct Pending {
    reply: Receiver<Vec<Sample>>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl SurfaceFps {
    pub(crate) fn reset(&mut self) {
        if !self.package.is_empty() {
            if let Some(work) = &self.pending {
                work.cancelled.store(true, Ordering::Release);
            }
            self.package.clear();
            self.previous.clear();
            self.selected = None;
            self.reading = None;
            self.next_poll = None;
            self.discovery = None;
        }
    }

    pub(crate) fn value(&self) -> Option<f32> {
        self.reading
            .filter(|(when, _)| when.elapsed() < Duration::from_secs(2))
            .map(|(_, fps)| fps)
    }

    #[cfg(test)]
    pub(crate) fn sample_time(&self) -> Option<Instant> {
        self.reading.map(|(time, _)| time)
    }

    pub(crate) fn update(
        &mut self,
        package: Option<&str>,
        ctx: &egui::Context,
        mut query: impl FnMut(&str) -> Option<String> + Send + 'static,
    ) {
        let Some(package) = package.filter(|name| !name.is_empty()) else {
            self.reset();
            return;
        };
        if self.package != package {
            self.reset();
            self.package = package.to_owned();
        }
        if let Some(pending) = &self.pending {
            match pending.reply.try_recv() {
                Ok(current) => {
                    let relevant = !pending.cancelled.load(Ordering::Acquire);
                    self.pending = None;
                    if relevant {
                        if current.is_empty() {
                            self.next_poll = Some(Instant::now() + Duration::from_secs(2));
                        }
                        let reading = rate(&self.previous, &current);
                        self.selected = reading.map(|(sample, _)| sample.layer.clone());
                        self.reading = reading.map(|(sample, fps)| (sample.time, fps));
                        self.previous = current;
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.previous.clear();
                    self.selected = None;
                    self.reading = None;
                }
                Err(TryRecvError::Empty) => return,
            }
        }
        let now = Instant::now();
        if let Some(next) = self.next_poll.filter(|next| *next > now) {
            ctx.request_repaint_after(next - now);
            return;
        }
        let cached = self.selected.clone().filter(|_| {
            self.discovery.is_some_and(|last| now - last < REDISCOVER)
                && self.reading.is_some_and(|(_, fps)| fps > 0.)
        });
        if cached.is_none() {
            self.discovery = Some(now);
        }
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.pending = Some(Pending {
            reply: receiver,
            cancelled: cancelled.clone(),
        });
        self.next_poll = Some(now + PERIOD);
        let package = self.package.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            if cancelled.load(Ordering::Acquire) {
                let _ = sender.send(Vec::new());
                ctx.request_repaint();
                return;
            }
            let names = match cached {
                Some(layer) => vec![layer],
                None => query("dumpsys SurfaceFlinger --list")
                    .map(|listing| layers(&listing, &package))
                    .unwrap_or_default(),
            };
            let mut current = Vec::new();
            for layer in names {
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
                let quoted = layer.replace('\'', "'\\''");
                let start = Instant::now();
                if let Some(output) = query(&format!("dumpsys SurfaceFlinger --latency '{quoted}'"))
                {
                    let time = start + start.elapsed() / 2;
                    let stamps = timestamps(&output);
                    if !stamps.is_empty() {
                        current.push(Sample {
                            layer,
                            time,
                            stamps,
                        });
                    }
                }
            }
            let _ = sender.send(current);
            ctx.request_repaint();
        });
    }
}

fn layers(listing: &str, package: &str) -> Vec<String> {
    let needle = format!("{package}/");
    let mut names = Vec::new();
    for line in listing.lines() {
        let name = line
            .split_once('{')
            .and_then(|(_, tail)| tail.split_once(" parentId="))
            .map_or(line, |(name, _)| name)
            .trim();
        let base = name
            .split_once(' ')
            .filter(|(prefix, _)| {
                !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_hexdigit())
            })
            .map_or(name, |(_, rest)| rest);
        if name.len() <= 1024
            && !name.chars().any(char::is_control)
            && base
                .strip_prefix("SurfaceView")
                .is_some_and(|tail| tail.starts_with([' ', '(', '[', '-']))
            && base.match_indices(&needle).any(|(offset, _)| {
                !base[..offset]
                    .ends_with(|c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_')
            })
            && !names.iter().any(|s| s == name)
        {
            names.push(name.to_owned());
        }
    }
    names
}

fn timestamps(output: &str) -> Vec<u64> {
    let mut stamps: Vec<_> = output
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_ascii_whitespace();
            cols.next()?.parse::<u64>().ok()?;
            let actual = cols.next()?.parse::<u64>().ok()?;
            cols.next()?.parse::<u64>().ok()?;
            (cols.next().is_none() && (1..i64::MAX as u64).contains(&actual)).then_some(actual)
        })
        .collect();
    stamps.sort_unstable();
    stamps.dedup();
    stamps
}

fn rate<'a>(previous: &[Sample], current: &'a [Sample]) -> Option<(&'a Sample, f32)> {
    if current.len() > 1
        && current
            .iter()
            .any(|new| !previous.iter().any(|old| old.layer == new.layer))
    {
        return None;
    }
    let mut candidates = current.iter().filter_map(|sample| {
        let before = previous.iter().find(|old| old.layer == sample.layer)?;
        let latest = before.stamps.last()?;
        // The last old frame must still exist in the finite history ring.
        let newest_old = sample.stamps.binary_search(latest).ok()?;
        let count = sample.stamps.len() - newest_old - 1;
        let elapsed = sample.time.checked_duration_since(before.time)?;
        if elapsed.is_zero()
            || elapsed > Duration::from_secs(2)
            || (current.len() > 1 && count == 0)
        {
            return None;
        }
        Some((sample, count as f32 / elapsed.as_secs_f32()))
    });
    let selected = candidates.next()?;
    candidates.next().is_none().then_some(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_exact_game_layers_on_pie_and_modern_android() {
        let list = "SurfaceView - com.tencent.ig/.GameActivity#0\n\
            Layer [418] {cafe SurfaceView[com.tencent.ig/.GameActivity](BLAST)#82 parentId=42}\n\
            Background for SurfaceView - com.tencent.ig/.GameActivity#0\n\
            SurfaceView - other.com.tencent.ig/.Activity#0\n\
            SurfaceView - com.tencent.ig/.GameActivity#0";
        assert_eq!(
            layers(list, "com.tencent.ig"),
            [
                "SurfaceView - com.tencent.ig/.GameActivity#0",
                "cafe SurfaceView[com.tencent.ig/.GameActivity](BLAST)#82",
            ]
        );
        assert_eq!(
            timestamps(
                "16666666\n4 20 6\n4 20 6\n0 0 0\n3 9223372036854775807 8\n3 10 4\nwrong 30 0"
            ),
            [10, 20]
        );
    }

    #[test]
    fn cadence_tracks_one_layer_across_overlapping_history_only() {
        let start = Instant::now();
        let sample = |name: &str, ms, stamps: &[u64]| Sample {
            layer: name.into(),
            time: start + Duration::from_millis(ms),
            stamps: stamps.to_vec(),
        };
        let before = [sample("game", 0, &[100, 200]), sample("popup", 0, &[20])];
        let now = |stamps: &[u64]| sample("game", 500, stamps);
        assert_eq!(rate(&before, &[now(&[200, 300, 400])]).unwrap().1, 4.);
        assert_eq!(rate(&before, &[now(&[100, 200])]).unwrap().1, 0.);
        assert!(rate(&before, &[now(&[300, 400])]).is_none());
        assert!(rate(&before, &[sample("replacement", 500, &[100, 200])]).is_none());
        assert!(
            rate(
                &before,
                &[now(&[200, 300]), sample("popup", 500, &[20, 30])]
            )
            .is_none()
        );
        assert_eq!(
            rate(&before, &[now(&[200, 300]), sample("popup", 500, &[20])])
                .unwrap()
                .0
                .layer,
            "game"
        );
    }
}
