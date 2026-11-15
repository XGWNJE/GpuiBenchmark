use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::metrics::{DurationDistribution, LatencySnapshot, SessionMetrics, SessionSnapshot};
use crate::workload::{PreparedUpdate, Workload, WorkloadPreset};

pub const PROTOCOL_ID: &str = "gpui-benchmark-v1";
const BATCH_CAPACITY: usize = 128;
const QUEUE_CAPACITY: usize = 8;
const MAX_UPDATES_PER_FRAME: usize = 8_192;
const MAX_GENERATED_PER_TICK: u64 = 8_192;
const PRODUCER_INTERVAL: Duration = Duration::from_millis(10);
const CHART_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct BenchmarkOptions {
    pub warmup_ms: u64,
    pub measurement_ms: u64,
    pub repetitions: u32,
}

impl Default for BenchmarkOptions {
    fn default() -> Self {
        Self {
            warmup_ms: 3_000,
            measurement_ms: 20_000,
            repetitions: 3,
        }
    }
}

impl BenchmarkOptions {
    pub fn demo() -> Self {
        Self {
            warmup_ms: 1_000,
            measurement_ms: 3_000,
            repetitions: 1,
        }
    }

    pub fn is_standard(self) -> bool {
        self == Self::default()
    }

    fn validate(self) -> Result<(), String> {
        if self.warmup_ms > 30_000
            || !(1_000..=120_000).contains(&self.measurement_ms)
            || !(1..=5).contains(&self.repetitions)
        {
            return Err("测试参数超出范围".into());
        }
        Ok(())
    }

    fn mode(self) -> &'static str {
        if self.is_standard() {
            "standard"
        } else if self == Self::demo() {
            "demo"
        } else {
            "custom"
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkScene {
    Hot,
    Scroll,
    StylesLight,
    StylesDark,
}

impl BenchmarkScene {
    pub fn key(self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Scroll => "scroll",
            Self::StylesLight => "styles_light",
            Self::StylesDark => "styles_dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Hot => "热点更新",
            Self::Scroll => "滚动更新",
            Self::StylesLight => "浅色组件",
            Self::StylesDark => "深色组件",
        }
    }

    pub fn is_styles(self) -> bool {
        matches!(self, Self::StylesLight | Self::StylesDark)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BenchmarkCase {
    pub id: String,
    pub preset: WorkloadPreset,
    pub scene: BenchmarkScene,
    pub repetition: u32,
}

impl BenchmarkCase {
    pub fn label(&self) -> String {
        if self.scene.is_styles() {
            self.scene.label().into()
        } else {
            format!("{} · {}", self.preset.label(), self.scene.label())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkPhase {
    Warmup,
    Measuring,
    Finished,
    Cancelled,
}

#[derive(Clone, Debug, Serialize)]
pub struct CaseResult {
    pub case_id: String,
    pub preset: WorkloadPreset,
    pub scene: BenchmarkScene,
    pub repetition: u32,
    pub warmup_ms: u64,
    pub measurement_ms: u64,
    pub actual_duration_ms: f64,
    pub rows: usize,
    pub streams: usize,
    pub configured_updates_per_second: usize,
    pub generated_updates: u64,
    pub applied_updates: u64,
    pub dropped_updates: u64,
    pub pending_updates: u64,
    pub frame_stats: SessionSnapshot,
    pub queue_latency_ms: LatencySnapshot,
}

impl CaseResult {
    fn score_contribution(&self) -> f64 {
        if self.generated_updates == 0 || self.frame_stats.p95_ms <= 0.0 {
            return 0.0;
        }
        let smoothness = (self.frame_stats.budget_ms / self.frame_stats.p95_ms).min(1.0);
        let delivered = (self.applied_updates as f64 / self.generated_updates as f64).min(1.0);
        self.frame_stats.fps * smoothness * delivered
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BenchmarkReport {
    pub schema_version: u32,
    pub protocol_id: &'static str,
    pub run_id: String,
    pub mode: &'static str,
    pub status: &'static str,
    pub options: BenchmarkOptions,
    pub refresh_hz: f64,
    pub score: f64,
    pub invalid_reason: Option<String>,
    pub results: Vec<CaseResult>,
}

pub fn calculate_score(results: &[CaseResult]) -> f64 {
    if results.is_empty() {
        return 0.0;
    }
    let total: f64 = results.iter().map(CaseResult::score_contribution).sum();
    let score = total / results.len() as f64 / 60.0 * 1_000.0;
    (score * 100.0).round() / 100.0
}

#[derive(Clone, Debug)]
pub enum BenchmarkEvent {
    CaseStarted(BenchmarkCase),
    MeasurementStarted(BenchmarkCase),
    CaseCompleted(CaseResult),
    Finished,
}

#[derive(Clone, Debug)]
pub struct BenchmarkSnapshot {
    pub case: Option<BenchmarkCase>,
    pub phase: BenchmarkPhase,
    /// Zero-based case position, capped at total_cases after completion.
    pub case_index: usize,
    pub total_cases: usize,
    pub overall_progress: f32,
    pub remaining_seconds: f32,
    pub frames: SessionSnapshot,
    pub recent_intervals_ms: Vec<f64>,
    /// Applied payload count per logical stream in the last consumed frame.
    pub stream_activity: Vec<f64>,
    pub applied_updates: u64,
    pub dropped_updates: u64,
}

struct UpdateBatch {
    generated_at: Instant,
    updates: Vec<PreparedUpdate>,
}

#[derive(Default)]
struct ProducerCounters {
    generated: AtomicU64,
    dropped: AtomicU64,
}

struct Producer {
    receiver: Receiver<UpdateBatch>,
    counters: Arc<ProducerCounters>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Producer {
    fn new(preset: WorkloadPreset, scene: BenchmarkScene) -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let counters = Arc::new(ProducerCounters::default());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_counters = Arc::clone(&counters);
        let worker_stop = Arc::clone(&stop);
        let started = Instant::now();
        let worker = thread::Builder::new()
            .name("gpui-benchmark-data".into())
            .spawn(move || {
                produce_updates(preset, scene, sender, worker_counters, worker_stop, started);
            })
            .map_err(|error| format!("无法启动数据生成线程，{error}"))?;
        Ok(Self {
            receiver,
            counters,
            stop,
            worker: Some(worker),
        })
    }

    fn stop(&mut self) -> bool {
        self.stop.store(true, Ordering::Release);
        match self.worker.take() {
            Some(worker) => {
                worker.thread().unpark();
                worker.join().is_ok()
            }
            None => true,
        }
    }
}

impl Drop for Producer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn produce_updates(
    preset: WorkloadPreset,
    scene: BenchmarkScene,
    sender: SyncSender<UpdateBatch>,
    counters: Arc<ProducerCounters>,
    stop: Arc<AtomicBool>,
    started: Instant,
) {
    let mut previous = started;
    let mut fractional = 0u128;
    let mut random_state = 0x4750_5549_4c41_4201u64;
    let mut stream = 0usize;
    let stream_count = preset.stream_count();
    let target_rows = if scene == BenchmarkScene::Hot {
        32.min(preset.row_count())
    } else {
        preset.row_count()
    };
    let rows_per_stream = target_rows / stream_count;
    let rate = stream_count * preset.updates_per_stream_per_second();

    while !stop.load(Ordering::Acquire) {
        thread::park_timeout(PRODUCER_INTERVAL);
        if stop.load(Ordering::Acquire) {
            break;
        }
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(previous);
        previous = now;
        let units = elapsed.as_nanos() * rate as u128 + fractional;
        let requested = (units / 1_000_000_000).min(u64::MAX as u128) as u64;
        fractional = units % 1_000_000_000;
        counters.generated.fetch_add(requested, Ordering::Relaxed);
        let produced = requested.min(MAX_GENERATED_PER_TICK);
        counters
            .dropped
            .fetch_add(requested - produced, Ordering::Relaxed);
        let mut remaining = produced as usize;
        while remaining > 0 {
            let count = remaining.min(BATCH_CAPACITY);
            let mut updates = Vec::with_capacity(count);
            for _ in 0..count {
                random_state ^= random_state << 13;
                random_state ^= random_state >> 7;
                random_state ^= random_state << 17;
                let row_index = (random_state as usize % rows_per_stream) * stream_count + stream;
                let delta = ((random_state >> 32) % 151) as f64 / 100.0 - 0.75;
                updates.push(PreparedUpdate { row_index, delta });
                stream = (stream + 1) % stream_count;
            }
            let batch = UpdateBatch {
                generated_at: now,
                updates,
            };
            if !enqueue_batch(&sender, &counters, batch) {
                return;
            }
            remaining -= count;
        }
    }
}

fn enqueue_batch(
    sender: &SyncSender<UpdateBatch>,
    counters: &ProducerCounters,
    batch: UpdateBatch,
) -> bool {
    match sender.try_send(batch) {
        Ok(()) => true,
        Err(TrySendError::Full(batch)) => {
            counters
                .dropped
                .fetch_add(batch.updates.len() as u64, Ordering::Relaxed);
            true
        }
        Err(TrySendError::Disconnected(batch)) => {
            counters
                .dropped
                .fetch_add(batch.updates.len() as u64, Ordering::Relaxed);
            false
        }
    }
}

pub struct BenchmarkSuite {
    run_id: String,
    options: BenchmarkOptions,
    refresh_hz: f64,
    cases: Vec<BenchmarkCase>,
    case_index: usize,
    phase: BenchmarkPhase,
    phase_started: Instant,
    pending_case_event: bool,
    pending_finish_event: bool,
    workload: Workload,
    producer: Option<Producer>,
    metrics: SessionMetrics,
    frame_snapshot: SessionSnapshot,
    last_chart: Instant,
    latency: DurationDistribution,
    stream_activity: [u64; 16],
    results: Vec<CaseResult>,
    invalid_reason: Option<String>,
    spawn_producers: bool,
}

impl BenchmarkSuite {
    pub fn new(run_id: String, options: BenchmarkOptions, refresh_hz: f64, now: Instant) -> Self {
        Self::create(run_id, options, refresh_hz, now, true)
    }

    fn create(
        run_id: String,
        options: BenchmarkOptions,
        refresh_hz: f64,
        now: Instant,
        spawn_producers: bool,
    ) -> Self {
        let metrics = SessionMetrics::new(refresh_hz);
        let frame_snapshot = metrics.snapshot();
        let mut suite = Self {
            run_id,
            options,
            refresh_hz: frame_snapshot.refresh_hz,
            cases: Vec::new(),
            case_index: 0,
            phase: BenchmarkPhase::Warmup,
            phase_started: now,
            pending_case_event: true,
            pending_finish_event: false,
            workload: Workload::new(WorkloadPreset::Light),
            producer: None,
            metrics,
            frame_snapshot,
            last_chart: now,
            latency: DurationDistribution::default(),
            stream_activity: [0; 16],
            results: Vec::new(),
            invalid_reason: None,
            spawn_producers,
        };
        let validation = options.validate().and_then(|_| {
            if suite.run_id.is_empty()
                || suite.run_id.len() > 64
                || !suite
                    .run_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            {
                Err("测试编号格式无效".into())
            } else if !refresh_hz.is_finite() || !(1.0..=1_000.0).contains(&refresh_hz) {
                Err("屏幕刷新率无效".into())
            } else {
                Ok(())
            }
        });
        if let Err(reason) = validation {
            suite.fail(reason);
            return suite;
        }
        suite.cases = make_cases(options.repetitions);
        suite.start_case(now);
        suite
    }

    pub fn current_case(&self) -> Option<&BenchmarkCase> {
        self.cases.get(self.case_index)
    }

    pub fn workload(&self) -> &Workload {
        &self.workload
    }

    pub fn is_finished(&self) -> bool {
        matches!(
            self.phase,
            BenchmarkPhase::Finished | BenchmarkPhase::Cancelled
        )
    }

    pub fn cancel(&mut self, reason: &str) {
        if !self.is_finished() {
            self.producer = None;
            self.phase = BenchmarkPhase::Cancelled;
            self.invalid_reason = Some(reason.to_owned());
        }
    }

    /// Invalidate external conditions even after the final case has completed.
    pub fn invalidate(&mut self, reason: &str) {
        self.fail(reason.to_owned());
    }

    pub fn advance(&mut self, now: Instant) -> Vec<BenchmarkEvent> {
        let mut events = Vec::new();
        if self.pending_finish_event {
            self.pending_finish_event = false;
            return vec![BenchmarkEvent::Finished];
        }
        if self.is_finished() {
            return events;
        }
        if self.pending_case_event {
            self.pending_case_event = false;
            if let Some(case) = self.current_case() {
                events.push(BenchmarkEvent::CaseStarted(case.clone()));
            }
        }
        self.consume_updates();
        let elapsed = now.saturating_duration_since(self.phase_started);
        match self.phase {
            BenchmarkPhase::Warmup => {
                self.metrics.frame(now);
                if elapsed >= Duration::from_millis(self.options.warmup_ms) {
                    self.producer = None;
                    self.workload.reset_counters();
                    self.latency = DurationDistribution::default();
                    self.metrics = SessionMetrics::new(self.refresh_hz);
                    self.metrics.start(now);
                    self.frame_snapshot = self.metrics.snapshot();
                    self.phase = BenchmarkPhase::Measuring;
                    self.phase_started = now;
                    self.start_producer();
                    if self.phase == BenchmarkPhase::Measuring {
                        if let Some(case) = self.current_case() {
                            events.push(BenchmarkEvent::MeasurementStarted(case.clone()));
                        }
                    }
                }
            }
            BenchmarkPhase::Measuring => {
                self.metrics.frame(now);
                if elapsed >= Duration::from_millis(self.options.measurement_ms) {
                    let result = self.finish_case(now);
                    events.push(BenchmarkEvent::CaseCompleted(result));
                    self.case_index += 1;
                    if self.case_index == self.cases.len() {
                        self.phase = BenchmarkPhase::Finished;
                        events.push(BenchmarkEvent::Finished);
                    } else {
                        self.start_case(now);
                    }
                }
            }
            BenchmarkPhase::Finished | BenchmarkPhase::Cancelled => {}
        }
        if now.saturating_duration_since(self.last_chart) >= CHART_INTERVAL {
            self.frame_snapshot = self.metrics.snapshot();
            self.last_chart = now;
        }
        if self.pending_finish_event {
            self.pending_finish_event = false;
            events.push(BenchmarkEvent::Finished);
        }
        events
    }

    pub fn snapshot(&self, now: Instant) -> BenchmarkSnapshot {
        let elapsed = now.saturating_duration_since(self.phase_started);
        let case_duration = (self.options.warmup_ms + self.options.measurement_ms) as f32;
        let case_elapsed = if self.phase == BenchmarkPhase::Warmup {
            elapsed.as_secs_f32() * 1_000.0
        } else {
            self.options.warmup_ms as f32 + elapsed.as_secs_f32() * 1_000.0
        };
        let case_fraction = (case_elapsed / case_duration).min(1.0);
        let total_cases = self.cases.len();
        let overall_progress =
            if self.phase == BenchmarkPhase::Finished && self.invalid_reason.is_none() {
                1.0
            } else if total_cases == 0 {
                0.0
            } else {
                ((self.case_index as f32 + case_fraction) / total_cases as f32).min(1.0)
            };
        let remaining_seconds = if self.is_finished() {
            0.0
        } else {
            ((1.0 - overall_progress) * total_cases as f32 * case_duration / 1_000.0).max(0.0)
        };
        let (_, dropped_updates) = self.producer_totals();
        BenchmarkSnapshot {
            case: self.current_case().cloned(),
            phase: self.phase,
            case_index: self.case_index,
            total_cases,
            overall_progress,
            remaining_seconds,
            frames: self.frame_snapshot.clone(),
            recent_intervals_ms: self.metrics.recent_intervals_ms(),
            stream_activity: self
                .stream_activity
                .iter()
                .map(|&count| count as f64)
                .collect(),
            applied_updates: self.workload.total_updates(),
            dropped_updates,
        }
    }

    pub fn report(&self) -> BenchmarkReport {
        let status = if self.phase == BenchmarkPhase::Cancelled {
            "cancelled"
        } else if self.invalid_reason.is_some() {
            "invalid"
        } else if self.phase == BenchmarkPhase::Finished {
            "completed"
        } else {
            "running"
        };
        BenchmarkReport {
            schema_version: 2,
            protocol_id: PROTOCOL_ID,
            run_id: self.run_id.clone(),
            mode: self.options.mode(),
            status,
            options: self.options,
            refresh_hz: self.refresh_hz,
            score: calculate_score(&self.results),
            invalid_reason: self.invalid_reason.clone(),
            results: self.results.clone(),
        }
    }

    fn start_case(&mut self, now: Instant) {
        self.producer = None;
        let case = self.cases[self.case_index].clone();
        self.workload.apply_preset(case.preset);
        self.phase = BenchmarkPhase::Warmup;
        self.phase_started = now;
        self.pending_case_event = true;
        self.stream_activity = [0; 16];
        self.metrics = SessionMetrics::new(self.refresh_hz);
        self.metrics.start(now);
        self.frame_snapshot = self.metrics.snapshot();
        self.last_chart = now;
        self.latency = DurationDistribution::default();
        self.start_producer();
    }

    fn start_producer(&mut self) {
        if self.spawn_producers {
            let Some(case) = self.current_case() else {
                return;
            };
            match Producer::new(case.preset, case.scene) {
                Ok(producer) => self.producer = Some(producer),
                Err(reason) => self.fail(reason),
            }
        }
    }

    fn consume_updates(&mut self) {
        self.stream_activity = [0; 16];
        let Some(producer) = self.producer.as_ref() else {
            return;
        };
        let mut consumed = 0;
        while consumed < MAX_UPDATES_PER_FRAME {
            let Ok(batch) = producer.receiver.try_recv() else {
                break;
            };
            self.latency
                .record(Instant::now().saturating_duration_since(batch.generated_at));
            let stream_count = self.workload.stream_count();
            for update in &batch.updates {
                self.stream_activity[update.row_index % stream_count] += 1;
            }
            consumed += batch.updates.len();
            self.workload.apply_prepared(&batch.updates);
        }
    }

    fn producer_totals(&self) -> (u64, u64) {
        self.producer.as_ref().map_or((0, 0), |producer| {
            (
                producer.counters.generated.load(Ordering::Relaxed),
                producer.counters.dropped.load(Ordering::Relaxed),
            )
        })
    }

    fn finish_case(&mut self, now: Instant) -> CaseResult {
        if let Some(producer) = self.producer.as_mut() {
            if !producer.stop() {
                self.invalid_reason = Some("数据生成线程异常退出".into());
            }
        }
        let (generated_updates, dropped_updates) = self.producer_totals();
        let applied_updates = self.workload.total_updates();
        let frame_stats = self.metrics.snapshot();
        let queue_latency_ms = self.latency.snapshot();
        if frame_stats.sample_overflow || queue_latency_ms.sample_overflow {
            self.invalid_reason = Some("采样数量超过单场景上限".into());
        } else if frame_stats.sample_count == 0 || generated_updates == 0 {
            self.invalid_reason = Some("本场景缺少有效采样".into());
        }
        self.frame_snapshot = frame_stats.clone();
        let case = self.cases[self.case_index].clone();
        let result = CaseResult {
            case_id: case.id,
            preset: case.preset,
            scene: case.scene,
            repetition: case.repetition,
            warmup_ms: self.options.warmup_ms,
            measurement_ms: self.options.measurement_ms,
            actual_duration_ms: now
                .saturating_duration_since(self.phase_started)
                .as_secs_f64()
                * 1_000.0,
            rows: self.workload.len(),
            streams: self.workload.stream_count(),
            configured_updates_per_second: self.workload.updates_per_second(),
            generated_updates,
            applied_updates,
            dropped_updates,
            pending_updates: generated_updates
                .saturating_sub(applied_updates)
                .saturating_sub(dropped_updates),
            frame_stats,
            queue_latency_ms,
        };
        self.results.push(result.clone());
        result
    }

    fn fail(&mut self, reason: String) {
        self.producer = None;
        self.invalid_reason = Some(reason);
        self.phase = BenchmarkPhase::Finished;
        self.pending_finish_event = true;
    }
}

fn make_cases(repetitions: u32) -> Vec<BenchmarkCase> {
    let mut cases = Vec::with_capacity(repetitions as usize * 10);
    for repetition in 1..=repetitions {
        for preset in WorkloadPreset::ALL {
            for scene in [BenchmarkScene::Hot, BenchmarkScene::Scroll] {
                cases.push(BenchmarkCase {
                    id: format!("{}_{}_r{repetition}", preset.key(), scene.key()),
                    preset,
                    scene,
                    repetition,
                });
            }
        }
        for scene in [BenchmarkScene::StylesLight, BenchmarkScene::StylesDark] {
            cases.push(BenchmarkCase {
                id: format!("{}_r{repetition}", scene.key()),
                preset: WorkloadPreset::Extreme,
                scene,
                repetition,
            });
        }
    }
    cases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_cases_exclude_warmup_and_finish_without_wall_clock_waits() {
        let options = BenchmarkOptions {
            warmup_ms: 10,
            measurement_ms: 1_000,
            repetitions: 1,
        };
        let mut time = Instant::now();
        let mut suite = BenchmarkSuite::create("test-run".into(), options, 120.0, time, false);
        assert_eq!(suite.snapshot(time).total_cases, 10);
        for index in 0..10 {
            let start_events = suite.advance(time);
            assert!(matches!(&start_events[0], BenchmarkEvent::CaseStarted(_)));
            time += Duration::from_millis(10);
            let events = suite.advance(time);
            assert!(matches!(&events[0], BenchmarkEvent::MeasurementStarted(_)));
            for _ in 0..100 {
                time += Duration::from_millis(10);
                let events = suite.advance(time);
                if !events.is_empty() {
                    assert!(matches!(&events[0], BenchmarkEvent::CaseCompleted(_)));
                }
            }
            let result = &suite.results[index];
            assert_eq!(result.frame_stats.sample_count, 100);
            assert!((result.frame_stats.elapsed_seconds - 1.0).abs() < 0.0001);
            assert!((result.frame_stats.fps - 100.0).abs() < 0.0001);
        }
        assert!(suite.is_finished());
        assert_eq!(suite.report().results.len(), 10);
        assert_eq!(suite.snapshot(time).overall_progress, 1.0);
        assert_eq!(suite.report().status, "invalid");
    }

    #[test]
    fn protocol_cases_and_scoring_are_stable() {
        let cases = make_cases(3);
        assert_eq!(cases.len(), 30);
        assert_eq!(cases[0].id, "light_hot_r1");
        assert_eq!(cases[9].id, "styles_dark_r1");
        assert_eq!(cases[29].id, "styles_dark_r3");
        let mut frames = SessionMetrics::new(60.0);
        let time = Instant::now();
        frames.start(time);
        frames.frame(time + Duration::from_millis(10));
        let result = CaseResult {
            case_id: "light_hot_r1".into(),
            preset: WorkloadPreset::Light,
            scene: BenchmarkScene::Hot,
            repetition: 1,
            warmup_ms: 3_000,
            measurement_ms: 20_000,
            actual_duration_ms: 20_000.0,
            rows: 10_000,
            streams: 1,
            configured_updates_per_second: 20,
            generated_updates: 400,
            applied_updates: 200,
            dropped_updates: 100,
            pending_updates: 100,
            frame_stats: frames.snapshot(),
            queue_latency_ms: LatencySnapshot::default(),
        };
        assert_eq!(calculate_score(&[result]), 833.33);
    }

    #[test]
    fn bounded_producer_reports_drops_and_stops_promptly() {
        let counters = ProducerCounters::default();
        let (sender, receiver) = mpsc::sync_channel(1);
        let batch = || UpdateBatch {
            generated_at: Instant::now(),
            updates: vec![PreparedUpdate {
                row_index: 0,
                delta: 1.0,
            }],
        };
        counters.generated.store(2, Ordering::Relaxed);
        assert!(enqueue_batch(&sender, &counters, batch()));
        assert!(enqueue_batch(&sender, &counters, batch()));
        assert_eq!(receiver.try_recv().unwrap().updates.len(), 1);
        assert!(receiver.try_recv().is_err());
        assert_eq!(counters.generated.load(Ordering::Relaxed), 2);
        assert_eq!(counters.dropped.load(Ordering::Relaxed), 1);
        let mut producer = Producer::new(WorkloadPreset::Extreme, BenchmarkScene::Hot).unwrap();
        let started = Instant::now();
        assert!(producer.stop());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn cancellation_rejects_partial_results_and_options_are_bounded() {
        let time = Instant::now();
        let mut suite = BenchmarkSuite::create(
            "cancel-run".into(),
            BenchmarkOptions::default(),
            120.0,
            time,
            false,
        );
        suite.cancel("应用退到后台");
        assert!(suite.is_finished());
        assert_eq!(suite.report().status, "cancelled");
        assert!(suite.advance(time + Duration::from_secs(60)).is_empty());
        let invalid = BenchmarkSuite::create(
            "invalid-run".into(),
            BenchmarkOptions {
                repetitions: 1_000_000,
                ..BenchmarkOptions::default()
            },
            120.0,
            time,
            false,
        );
        assert_eq!(invalid.report().status, "invalid");
        assert!(invalid.cases.is_empty());
    }

    #[test]
    fn measurement_counters_conserve_payloads_after_producer_freeze() {
        let time = Instant::now();
        let options = BenchmarkOptions {
            warmup_ms: 0,
            measurement_ms: 1_000,
            repetitions: 1,
        };
        let mut suite = BenchmarkSuite::create("counter-run".into(), options, 60.0, time, false);
        suite.advance(time);
        let (sender, receiver) = mpsc::sync_channel(1);
        let counters = Arc::new(ProducerCounters::default());
        counters.generated.store(3, Ordering::Relaxed);
        counters.dropped.store(1, Ordering::Relaxed);
        sender
            .send(UpdateBatch {
                generated_at: Instant::now(),
                updates: vec![
                    PreparedUpdate {
                        row_index: 0,
                        delta: 1.0,
                    },
                    PreparedUpdate {
                        row_index: 1,
                        delta: -0.5,
                    },
                ],
            })
            .unwrap();
        suite.producer = Some(Producer {
            receiver,
            counters,
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        });
        suite.advance(time + Duration::from_millis(16));
        let events = suite.advance(time + Duration::from_secs(1));
        let BenchmarkEvent::CaseCompleted(result) = &events[0] else {
            panic!("expected completed case");
        };
        assert_eq!(result.generated_updates, 3);
        assert_eq!(result.applied_updates, 2);
        assert_eq!(result.dropped_updates, 1);
        assert_eq!(result.pending_updates, 0);
        assert_eq!(result.queue_latency_ms.sample_count, 1);
        assert_eq!(
            result.generated_updates,
            result.applied_updates + result.dropped_updates + result.pending_updates
        );
    }

    #[test]
    fn completed_final_case_can_be_invalidated_by_external_conditions() {
        let mut time = Instant::now();
        let options = BenchmarkOptions {
            warmup_ms: 0,
            measurement_ms: 1_000,
            repetitions: 1,
        };
        let mut suite = BenchmarkSuite::create("refresh-run".into(), options, 60.0, time, false);
        for _ in 0..10 {
            suite.advance(time);
            let (sender, receiver) = mpsc::sync_channel(1);
            let counters = Arc::new(ProducerCounters::default());
            counters.generated.store(1, Ordering::Relaxed);
            sender
                .send(UpdateBatch {
                    generated_at: Instant::now(),
                    updates: vec![PreparedUpdate {
                        row_index: 0,
                        delta: 0.5,
                    }],
                })
                .unwrap();
            suite.producer = Some(Producer {
                receiver,
                counters,
                stop: Arc::new(AtomicBool::new(false)),
                worker: None,
            });
            time += Duration::from_secs(1);
            suite.advance(time);
        }
        assert!(suite.is_finished());
        assert_eq!(suite.report().status, "completed");
        assert_eq!(suite.report().results.len(), 10);
        suite.invalidate("屏幕刷新率在最后一个场景变化");
        assert!(suite.is_finished());
        assert_eq!(suite.report().status, "invalid");
        assert_eq!(
            suite.report().invalid_reason.as_deref(),
            Some("屏幕刷新率在最后一个场景变化")
        );
        assert!(suite.producer.is_none());
    }
}
