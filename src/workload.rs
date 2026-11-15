use std::time::Duration;

const NANOS_PER_SECOND: u128 = 1_000_000_000;
const MAX_UPDATES_PER_TICK: usize = 8_192;
const RANDOM_SEED: u64 = 0x4750_5549_4c41_4201;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkloadPreset {
    Light,
    Medium,
    Heavy,
    Extreme,
}

impl WorkloadPreset {
    pub const ALL: [Self; 4] = [Self::Light, Self::Medium, Self::Heavy, Self::Extreme];

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "轻量",
            Self::Medium => "标准",
            Self::Heavy => "高压",
            Self::Extreme => "极限",
        }
    }

    pub fn row_count(self) -> usize {
        match self {
            Self::Light => 10_000,
            Self::Medium => 50_000,
            Self::Heavy => 100_000,
            Self::Extreme => 200_000,
        }
    }

    pub fn stream_count(self) -> usize {
        match self {
            Self::Light => 1,
            Self::Medium => 4,
            Self::Heavy => 8,
            Self::Extreme => 16,
        }
    }

    pub fn updates_per_stream_per_second(self) -> usize {
        match self {
            Self::Light => 20,
            Self::Medium => 100,
            Self::Heavy => 500,
            Self::Extreme => 1_000,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Medium => "medium",
            Self::Heavy => "heavy",
            Self::Extreme => "extreme",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub id: usize,
    pub source: usize,
    pub value: f64,
    pub previous: f64,
    pub trend: f64,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TickResult {
    pub applied: u64,
    pub skipped: u64,
}

#[derive(Debug)]
pub struct Workload {
    preset: WorkloadPreset,
    rows: Vec<Row>,
    random_state: u64,
    next_stream: usize,
    fractional_update_units: u128,
    total_updates: u64,
    skipped_updates: u64,
}

impl Workload {
    pub fn new(preset: WorkloadPreset) -> Self {
        let mut workload = Self {
            preset,
            rows: Vec::with_capacity(preset.row_count()),
            random_state: RANDOM_SEED,
            next_stream: 0,
            fractional_update_units: 0,
            total_updates: 0,
            skipped_updates: 0,
        };
        workload.apply_preset(preset);
        workload
    }

    pub fn preset(&self) -> WorkloadPreset {
        self.preset
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn row(&self, index: usize) -> Option<&Row> {
        self.rows.get(index)
    }

    pub fn stream_count(&self) -> usize {
        self.preset.stream_count()
    }

    pub fn updates_per_stream_per_second(&self) -> usize {
        self.preset.updates_per_stream_per_second()
    }

    pub fn updates_per_second(&self) -> usize {
        self.stream_count() * self.updates_per_stream_per_second()
    }

    pub fn total_updates(&self) -> u64 {
        self.total_updates
    }

    pub fn skipped_updates(&self) -> u64 {
        self.skipped_updates
    }

    pub fn apply_preset(&mut self, preset: WorkloadPreset) {
        self.preset = preset;
        self.rows.clear();
        self.rows.reserve(preset.row_count());
        self.rows.extend((0..preset.row_count()).map(|index| {
            let value = 50.0 + ((index * 37) % 9_500) as f64 / 100.0;
            Row {
                id: index + 1,
                source: index % preset.stream_count() + 1,
                value,
                previous: value,
                trend: 0.0,
                revision: 0,
            }
        }));
        self.random_state = RANDOM_SEED;
        self.next_stream = 0;
        self.fractional_update_units = 0;
        self.total_updates = 0;
        self.skipped_updates = 0;
    }

    /// Elapsed time determines the requested update count, independently of UI FPS.
    /// Old backlog is counted as skipped instead of growing a queue after a stall.
    pub fn tick(&mut self, elapsed: Duration) -> TickResult {
        let update_units = elapsed
            .as_nanos()
            .saturating_mul(self.updates_per_second() as u128)
            .saturating_add(self.fractional_update_units);
        let requested = update_units / NANOS_PER_SECOND;
        self.fractional_update_units = update_units % NANOS_PER_SECOND;
        let applied = requested.min(MAX_UPDATES_PER_TICK as u128) as usize;
        let skipped = requested.saturating_sub(applied as u128);
        let stream_count = self.stream_count();
        let rows_per_stream = self.rows.len() / stream_count;

        for _ in 0..applied {
            let random = self.next_random();
            let row_index = (random as usize % rows_per_stream) * stream_count + self.next_stream;
            let delta = ((random >> 32) % 151) as f64 / 100.0 - 0.75;
            let row = &mut self.rows[row_index];
            row.previous = row.value;
            row.value = (row.value + delta).max(0.01);
            row.trend = row.value - row.previous;
            row.revision = row.revision.saturating_add(1);
            self.next_stream = (self.next_stream + 1) % stream_count;
        }

        self.next_stream =
            (self.next_stream + (skipped % stream_count as u128) as usize) % stream_count;
        let applied = applied as u64;
        let skipped = skipped.min(u64::MAX as u128) as u64;
        self.total_updates = self.total_updates.saturating_add(applied);
        self.skipped_updates = self.skipped_updates.saturating_add(skipped);
        TickResult { applied, skipped }
    }

    pub fn summary_json(&self) -> String {
        format!(
            "{{\"preset\":\"{}\",\"rows\":{},\"streams\":{},\"updates_per_stream_per_second\":{},\"configured_updates_per_second\":{},\"applied_updates\":{},\"skipped_updates\":{}}}",
            self.preset.key(),
            self.len(),
            self.stream_count(),
            self.updates_per_stream_per_second(),
            self.updates_per_second(),
            self.total_updates,
            self.skipped_updates,
        )
    }

    fn next_random(&mut self) -> u64 {
        let mut value = self.random_state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.random_state = value;
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_ticks_preserve_update_rate_and_determinism() {
        let mut split = Workload::new(WorkloadPreset::Medium);
        let mut whole = Workload::new(WorkloadPreset::Medium);
        for _ in 0..1_000 {
            split.tick(Duration::from_millis(1));
        }
        whole.tick(Duration::from_secs(1));
        assert_eq!(split.total_updates(), 400);
        assert_eq!(split.total_updates(), whole.total_updates());
        for index in 0..split.len() {
            assert_eq!(
                split.row(index).unwrap().value,
                whole.row(index).unwrap().value
            );
            assert_eq!(
                split.row(index).unwrap().revision,
                whole.row(index).unwrap().revision
            );
        }
    }

    #[test]
    fn backlog_and_preset_changes_keep_storage_and_counters_bounded() {
        let mut workload = Workload::new(WorkloadPreset::Extreme);
        let result = workload.tick(Duration::from_secs(600));
        assert_eq!(result.applied, MAX_UPDATES_PER_TICK as u64);
        assert_eq!(result.skipped, 9_600_000 - result.applied);
        assert_eq!(workload.len(), 200_000);
        assert_eq!(workload.skipped_updates(), result.skipped);
        workload.apply_preset(WorkloadPreset::Light);
        assert_eq!(workload.len(), 10_000);
        assert_eq!(workload.total_updates(), 0);
        assert_eq!(workload.skipped_updates(), 0);
        assert_eq!(workload.tick(Duration::from_millis(50)).applied, 1);
    }
}
