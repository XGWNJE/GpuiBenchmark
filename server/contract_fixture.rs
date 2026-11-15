// Compiles against the application's real serializers and cached host dependencies.
#[path = "../src/metrics.rs"]
mod metrics;
#[path = "../src/workload.rs"]
mod workload;

mod benchmark {
    include!("../src/benchmark.rs");

    pub fn serialized_contract_fixture() -> String {
        let options = BenchmarkOptions::default();
        let mut frames = SessionMetrics::new(120.0);
        let started = Instant::now();
        let mut now = started;
        frames.start(now);
        let mut index = 0;
        while now.duration_since(started) < Duration::from_secs(20) {
            now += Duration::from_micros(8_333 + if index % 20 == 0 { 4_000 } else { 0 });
            frames.frame(now);
            index += 1;
        }
        let mut latency = DurationDistribution::default();
        for value in 1..=100 {
            latency.record(Duration::from_micros(value * 100));
        }
        let results: Vec<_> = make_cases(options.repetitions)
            .into_iter()
            .map(|case| {
                let rate = case.preset.stream_count() * case.preset.updates_per_stream_per_second();
                let generated = rate as u64 * 20;
                let dropped = generated / 10;
                let pending = 17;
                CaseResult {
                    case_id: case.id,
                    preset: case.preset,
                    scene: case.scene,
                    repetition: case.repetition,
                    warmup_ms: options.warmup_ms,
                    measurement_ms: options.measurement_ms,
                    actual_duration_ms: now.duration_since(started).as_secs_f64() * 1000.0,
                    rows: case.preset.row_count(),
                    streams: case.preset.stream_count(),
                    configured_updates_per_second: rate,
                    generated_updates: generated,
                    applied_updates: generated - dropped - pending,
                    dropped_updates: dropped,
                    pending_updates: pending,
                    frame_stats: frames.snapshot(),
                    queue_latency_ms: latency.snapshot(),
                }
            })
            .collect();
        serde_json::to_string(&BenchmarkReport {
            schema_version: 2,
            protocol_id: PROTOCOL_ID,
            run_id: "rust-contract-fixture".into(),
            mode: options.mode(),
            status: "completed",
            options,
            refresh_hz: 120.0,
            score: calculate_score(&results),
            invalid_reason: None,
            results,
        })
        .unwrap()
    }
}

fn main() {
    println!("{}", benchmark::serialized_contract_fixture());
}
