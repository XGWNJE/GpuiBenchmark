use std::collections::VecDeque;
use std::time::{Duration, Instant};

const MAX_SAMPLES: usize = 600;

/// CPU-side frame callback intervals in a bounded rolling window.
#[derive(Clone, Copy, Debug, Default)]
pub struct MetricsSnapshot {
    pub sample_count: usize,
    pub total_intervals: u64,
    pub elapsed_seconds: f64,
    pub refresh_hz: u32,
    pub budget_ms: f64,
    pub fps: f64,
    pub average_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub over_budget_count: usize,
    pub over_budget_ratio: f64,
}

pub struct Metrics {
    intervals: VecDeque<Duration>,
    last_frame: Option<Instant>,
    active: bool,
    refresh_hz: u32,
    total_intervals: u64,
    elapsed: Duration,
}

impl Metrics {
    pub fn new(refresh_hz: u32) -> Self {
        Self {
            intervals: VecDeque::with_capacity(MAX_SAMPLES),
            last_frame: None,
            active: false,
            refresh_hz: refresh_hz.max(1),
            total_intervals: 0,
            elapsed: Duration::ZERO,
        }
    }

    pub fn reset(&mut self) {
        self.intervals.clear();
        self.last_frame = None;
        self.total_intervals = 0;
        self.elapsed = Duration::ZERO;
    }

    pub fn pause(&mut self) {
        self.active = false;
        self.last_frame = None;
    }

    pub fn resume(&mut self) {
        self.active = true;
        self.last_frame = None;
    }

    pub fn set_refresh_hz(&mut self, refresh_hz: u32) {
        self.refresh_hz = refresh_hz.max(1);
    }

    pub fn frame(&mut self, now: Instant) {
        if !self.active {
            return;
        }
        if let Some(previous) = self.last_frame {
            if let Some(interval) = now.checked_duration_since(previous) {
                if !interval.is_zero() {
                    if self.intervals.len() == MAX_SAMPLES {
                        self.intervals.pop_front();
                    }
                    self.intervals.push_back(interval);
                    self.total_intervals = self.total_intervals.saturating_add(1);
                    self.elapsed = self.elapsed.saturating_add(interval);
                }
            }
        }
        self.last_frame = Some(now);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let sample_count = self.intervals.len();
        let budget_ms = 1_000.0 / self.refresh_hz as f64;
        let mut snapshot = MetricsSnapshot {
            sample_count,
            total_intervals: self.total_intervals,
            elapsed_seconds: self.elapsed.as_secs_f64(),
            refresh_hz: self.refresh_hz,
            budget_ms,
            ..MetricsSnapshot::default()
        };
        if sample_count == 0 {
            return snapshot;
        }
        let mut sorted: Vec<f64> = self.intervals.iter().map(Duration::as_secs_f64).collect();
        sorted.sort_unstable_by(f64::total_cmp);
        let total_seconds: f64 = sorted.iter().sum();
        snapshot.fps = sample_count as f64 / total_seconds;
        snapshot.average_ms = total_seconds * 1_000.0 / sample_count as f64;
        snapshot.p95_ms = percentile(&sorted, 95) * 1_000.0;
        snapshot.p99_ms = percentile(&sorted, 99) * 1_000.0;
        snapshot.over_budget_count = sorted
            .iter()
            .filter(|&&seconds| seconds * 1_000.0 > budget_ms)
            .count();
        snapshot.over_budget_ratio = snapshot.over_budget_count as f64 / sample_count as f64;
        snapshot
    }

    pub fn summary_json(&self) -> String {
        let s = self.snapshot();
        format!(
            "{{\"measurement\":\"frame_callback_intervals\",\"window_limit\":{},\"sample_count\":{},\"total_intervals\":{},\"elapsed_seconds\":{:.3},\"refresh_hz\":{},\"budget_ms\":{:.3},\"fps\":{:.2},\"average_ms\":{:.3},\"p95_ms\":{:.3},\"p99_ms\":{:.3},\"over_budget_count\":{},\"over_budget_ratio\":{:.5}}}",
            MAX_SAMPLES, s.sample_count, s.total_intervals, s.elapsed_seconds, s.refresh_hz,
            s.budget_ms, s.fps, s.average_ms, s.p95_ms, s.p99_ms, s.over_budget_count, s.over_budget_ratio
        )
    }
}

fn percentile(sorted: &[f64], percentage: usize) -> f64 {
    let rank = (sorted.len() * percentage).div_ceil(100);
    sorted[rank.saturating_sub(1)]
}

pub const MAX_SESSION_SAMPLES: usize = 100_000;

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct LatencySnapshot {
    pub sample_count: usize,
    pub average_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub sample_overflow: bool,
}

/// Full measurement-window samples. Overflow invalidates a benchmark result.
#[derive(Default)]
pub struct DurationDistribution {
    samples: Vec<Duration>,
    count: usize,
    total: Duration,
    maximum: Duration,
    overflow: bool,
}

impl DurationDistribution {
    pub fn record(&mut self, duration: Duration) {
        self.count = self.count.saturating_add(1);
        self.total = self.total.saturating_add(duration);
        self.maximum = self.maximum.max(duration);
        if self.samples.len() < MAX_SESSION_SAMPLES {
            self.samples.push(duration);
        } else {
            self.overflow = true;
        }
    }

    pub fn snapshot(&self) -> LatencySnapshot {
        let mut snapshot = LatencySnapshot {
            sample_count: self.count,
            max_ms: self.maximum.as_secs_f64() * 1_000.0,
            sample_overflow: self.overflow,
            ..LatencySnapshot::default()
        };
        if self.count > 0 {
            let mut sorted: Vec<f64> = self.samples.iter().map(Duration::as_secs_f64).collect();
            sorted.sort_unstable_by(f64::total_cmp);
            snapshot.average_ms = self.total.as_secs_f64() * 1_000.0 / self.count as f64;
            snapshot.p95_ms = percentile(&sorted, 95) * 1_000.0;
            snapshot.p99_ms = percentile(&sorted, 99) * 1_000.0;
        }
        snapshot
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct SessionSnapshot {
    pub measurement: &'static str,
    pub sample_count: usize,
    pub elapsed_seconds: f64,
    pub refresh_hz: f64,
    pub budget_ms: f64,
    pub fps: f64,
    pub average_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub over_budget_count: usize,
    pub over_budget_ratio: f64,
    pub sample_overflow: bool,
}

pub struct SessionMetrics {
    distribution: DurationDistribution,
    last_frame: Option<Instant>,
    refresh_hz: f64,
    over_budget_count: usize,
    recent: VecDeque<f64>,
}

impl SessionMetrics {
    pub fn new(refresh_hz: f64) -> Self {
        Self {
            distribution: DurationDistribution::default(),
            last_frame: None,
            refresh_hz: if refresh_hz.is_finite() && refresh_hz > 0.0 {
                refresh_hz
            } else {
                60.0
            },
            over_budget_count: 0,
            recent: VecDeque::with_capacity(120),
        }
    }

    pub fn start(&mut self, now: Instant) {
        self.last_frame = Some(now);
    }

    pub fn frame(&mut self, now: Instant) {
        if let Some(previous) = self.last_frame {
            if let Some(interval) = now.checked_duration_since(previous) {
                if !interval.is_zero() {
                    self.distribution.record(interval);
                    let milliseconds = interval.as_secs_f64() * 1_000.0;
                    if milliseconds > 1_000.0 / self.refresh_hz {
                        self.over_budget_count += 1;
                    }
                    if self.recent.len() == 120 {
                        self.recent.pop_front();
                    }
                    self.recent.push_back(milliseconds);
                }
            }
        }
        self.last_frame = Some(now);
    }

    pub fn recent_intervals_ms(&self) -> Vec<f64> {
        self.recent.iter().copied().collect()
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let distribution = self.distribution.snapshot();
        let elapsed_seconds = self.distribution.total.as_secs_f64();
        let count = distribution.sample_count;
        SessionSnapshot {
            measurement: "frame_callback_intervals",
            sample_count: count,
            elapsed_seconds,
            refresh_hz: self.refresh_hz,
            budget_ms: 1_000.0 / self.refresh_hz,
            fps: if elapsed_seconds > 0.0 {
                count as f64 / elapsed_seconds
            } else {
                0.0
            },
            average_ms: distribution.average_ms,
            p95_ms: distribution.p95_ms,
            p99_ms: distribution.p99_ms,
            max_ms: distribution.max_ms,
            over_budget_count: self.over_budget_count,
            over_budget_ratio: if count > 0 {
                self.over_budget_count as f64 / count as f64
            } else {
                0.0
            },
            sample_overflow: distribution.sample_overflow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_and_over_budget_ratio_use_callback_intervals() {
        let mut metrics = Metrics::new(60);
        metrics.resume();
        let mut time = Instant::now();
        metrics.frame(time);
        for milliseconds in 1..=100 {
            time += Duration::from_millis(milliseconds);
            metrics.frame(time);
        }
        let result = metrics.snapshot();
        assert_eq!(result.sample_count, 100);
        assert_eq!(result.p95_ms, 95.0);
        assert_eq!(result.p99_ms, 99.0);
        assert_eq!(result.over_budget_count, 84);
        assert!((result.average_ms - 50.5).abs() < 0.0001);
        assert!((result.over_budget_ratio - 0.84).abs() < 0.0001);
    }

    #[test]
    fn rolling_samples_and_pause_do_not_include_background_time() {
        let mut metrics = Metrics::new(120);
        metrics.resume();
        let mut time = Instant::now();
        metrics.frame(time);
        for _ in 0..700 {
            time += Duration::from_millis(8);
            metrics.frame(time);
        }
        assert_eq!(metrics.snapshot().sample_count, MAX_SAMPLES);
        assert_eq!(metrics.snapshot().total_intervals, 700);
        metrics.pause();
        time += Duration::from_secs(60);
        metrics.frame(time);
        metrics.resume();
        metrics.frame(time);
        time += Duration::from_millis(8);
        metrics.frame(time);
        let result = metrics.snapshot();
        assert_eq!(result.total_intervals, 701);
        assert!((result.elapsed_seconds - 5.608).abs() < 0.0001);
        assert_eq!(result.p99_ms, 8.0);
        metrics.reset();
        assert_eq!(metrics.snapshot().sample_count, 0);
    }

    #[test]
    fn session_statistics_preserve_early_stalls_and_every_interval() {
        let mut metrics = SessionMetrics::new(120.0);
        let mut time = Instant::now();
        metrics.start(time);
        time += Duration::from_millis(200);
        metrics.frame(time);
        for _ in 0..1_200 {
            time += Duration::from_millis(8);
            metrics.frame(time);
        }
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.sample_count, 1_201);
        assert_eq!(snapshot.max_ms, 200.0);
        assert_eq!(snapshot.over_budget_count, 1);
        assert_eq!(metrics.recent_intervals_ms().len(), 120);
        assert!((snapshot.elapsed_seconds - 9.8).abs() < 0.0001);
    }

    #[test]
    fn session_overflow_is_reported_without_unbounded_storage() {
        let mut distribution = DurationDistribution::default();
        for _ in 0..=MAX_SESSION_SAMPLES {
            distribution.record(Duration::from_millis(1));
        }
        assert_eq!(distribution.samples.len(), MAX_SESSION_SAMPLES);
        assert!(distribution.snapshot().sample_overflow);
        assert_eq!(
            distribution.snapshot().sample_count,
            MAX_SESSION_SAMPLES + 1
        );
    }
}
