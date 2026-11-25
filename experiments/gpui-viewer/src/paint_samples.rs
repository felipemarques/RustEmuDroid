use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct PaintSamples {
    saw_image: bool,
    pub first: Option<u64>,
    pub last: Option<u64>,
    lags: Vec<f64>,
}

impl PaintSamples {
    pub fn observe(&mut self, sequence: u64, produced_at: Instant, measured: bool) {
        self.saw_image = true;
        if !measured || self.last.is_some_and(|last| sequence <= last) {
            return;
        }
        self.first.get_or_insert(sequence);
        self.last = Some(sequence);
        self.lags.push(produced_at.elapsed().as_secs_f64() * 1000.);
    }

    pub fn count(&self, produced: u64, elapsed: Duration, requested: u32) -> Result<usize, String> {
        if !self.saw_image || elapsed.is_zero() {
            return Err("No source image was painted or the measurement window was empty".into());
        }
        if self.lags.is_empty()
            && (produced != 0 || elapsed < Duration::from_secs(requested.into()))
        {
            return Err("No measured image was painted before the input ended".into());
        }
        Ok(self.lags.len())
    }

    pub fn latency(&mut self) -> Value {
        self.lags.sort_unstable_by(f64::total_cmp);
        let count = self.lags.len();
        let percentile =
            |percent: usize| (count != 0).then(|| self.lags[(count * percent).div_ceil(100) - 1]);
        json!({
            "mean": (count != 0).then(|| self.lags.iter().sum::<f64>() / count as f64),
            "p50": percentile(50), "p95": percentile(95), "max": self.lags.last()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_needs_a_painted_source_and_a_complete_observation() {
        let mut samples = PaintSamples::default();
        let window = Duration::from_secs(3);
        assert!(samples.count(0, window, 3).is_err());
        samples.observe(0, Instant::now(), false);
        assert!(
            samples
                .count(0, window - Duration::from_millis(1), 3)
                .is_err()
        );
        assert!(samples.count(1, window, 3).is_err());
        assert_eq!(samples.count(0, window, 3), Ok(0));
        assert_eq!((samples.first, samples.last), (None, None));
        assert_eq!(
            samples.latency(),
            json!({"mean":null,"p50":null,"p95":null,"max":null})
        );
    }
}
