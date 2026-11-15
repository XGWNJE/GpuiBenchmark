use gpui_kit::{prelude::*, *};

/// A bounded presentation snapshot. The benchmark owner supplies every value.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct BenchmarkViewData {
    /// Available content height in logical platform pixels, after the app shell and insets.
    pub available_height: f32,
    pub running: bool,
    pub phase_label: String,
    pub case_label: String,
    pub case_index: usize,
    pub case_count: usize,
    pub progress: f32,
    pub remaining_seconds: f64,
    pub callback_fps: f64,
    pub p95_ms: f64,
    pub budget_ms: f64,
    pub rows: usize,
    pub streams: usize,
    pub updates_per_second: u64,
    pub total_updates: u64,
    pub dropped_updates: u64,
    pub frame_intervals: Vec<f32>,
    /// Per-stream applied update counts for the same recent interval.
    pub stream_values: Vec<f32>,
    pub score: Option<f64>,
    pub upload_status: String,
}

// Product-owned color roles keep the benchmark's video presentation stable
// while the app's component catalogue exercises its light and dark themes.
#[derive(Clone, Copy)]
struct BenchmarkPalette {
    background: Hsla,
    surface: Hsla,
    track: Hsla,
    border: Hsla,
    foreground: Hsla,
    secondary: Hsla,
    activity: Hsla,
    result: Hsla,
    slow: Hsla,
}

impl Default for BenchmarkPalette {
    fn default() -> Self {
        Self {
            background: rgb(0x080d14).into(),
            surface: rgb(0x101b29).into(),
            track: rgb(0x1a2c3d).into(),
            border: rgb(0x253b4c).into(),
            foreground: rgb(0xf2f7fa).into(),
            secondary: rgb(0x91a9bb).into(),
            activity: rgb(0x55e4d4).into(),
            result: rgb(0xc8f26b).into(),
            slow: rgb(0xff785b).into(),
        }
    }
}

/// The parent owns commands, scheduling, sampling and upload state.
#[derive(IntoElement)]
pub struct BenchmarkDashboard {
    snapshot: BenchmarkViewData,
    controls: AnyElement,
    surface: Option<AnyElement>,
}

impl BenchmarkDashboard {
    pub fn new(snapshot: BenchmarkViewData, controls: AnyElement) -> Self {
        Self {
            snapshot,
            controls,
            surface: None,
        }
    }

    /// The caller supplies the real, bounded list or component surface being measured.
    pub fn surface(mut self, surface: AnyElement) -> Self {
        self.surface = Some(surface);
        self
    }
}

impl RenderOnce for BenchmarkDashboard {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let palette = BenchmarkPalette::default();
        let snapshot = self.snapshot;
        let rem_size = f32::from(window.rem_size());
        let controls_height = (rem_size * 3.0).max(48.0);
        let telemetry_height = (rem_size * 4.0).max(48.0);
        // The benchmark's measured viewport is a fixed protocol target. Compact
        // containers retain one complete row and keep the telemetry scrollable.
        let preferred_surface_height = 190.0;
        let minimum_surface_height = (rem_size * 3.0).max(48.0);
        let full_header_height = rem_size * 8.25 + controls_height;
        let compact = snapshot.available_height
            < full_header_height + preferred_surface_height + telemetry_height + rem_size * 0.75;
        let header_height = if compact {
            rem_size * 4.5 + controls_height
        } else {
            full_header_height
        };
        let surface_height =
            (snapshot.available_height - header_height - telemetry_height - rem_size * 0.75).clamp(
                minimum_surface_height,
                preferred_surface_height.max(minimum_surface_height),
            );
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .font_family("Roboto")
            .bg(palette.background)
            .text_color(palette.foreground)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .gap_3()
                    .when(compact, |view| view.px_3().pt_2().pb_2().gap_2())
                    .child(hero(&snapshot, palette, compact))
                    .child(self.controls),
            )
            .when_some(self.surface, |view, surface| {
                view.child(
                    div()
                        .px_4()
                        .pb_3()
                        .flex_shrink_0()
                        .when(compact, |view| view.px_3())
                        .child(
                            div()
                                .h(px(surface_height))
                                .w_full()
                                .min_h_0()
                                .overflow_hidden()
                                .child(surface),
                        ),
                )
            })
            .child(
                div()
                    .id("benchmark-telemetry")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .px_4()
                    .pb_4()
                    .gap_4()
                    .when(compact, |view| view.px_3().gap_3())
                    .child(run_progress(&snapshot, palette))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(160.))
                                    .child(frame_chart(&snapshot, palette)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(160.))
                                    .child(stream_activity(&snapshot, palette)),
                            ),
                    )
                    .child(workload_summary(&snapshot, palette))
                    .when(!snapshot.upload_status.is_empty(), |view| {
                        view.child(
                            div()
                                .text_sm()
                                .text_color(palette.secondary)
                                .child(snapshot.upload_status.clone()),
                        )
                    }),
            )
    }
}

fn hero(snapshot: &BenchmarkViewData, palette: BenchmarkPalette, compact: bool) -> Div {
    let (label, value, color) = match snapshot.score {
        Some(score) => ("综合得分", format!("{score:.0}"), palette.result),
        None => (
            "UI 回调 / 秒",
            measured_value(snapshot.callback_fps, 1),
            palette.activity,
        ),
    };
    if compact {
        return div()
            .flex()
            .flex_wrap()
            .items_baseline()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_24()
                    .child(div().text_xs().text_color(palette.secondary).child(label))
                    .child(
                        div()
                            .text_size(rems(2.0))
                            .line_height(relative(1.0))
                            .font_weight(FontWeight::BLACK)
                            .text_color(color)
                            .child(value),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(palette.secondary)
                    .child(format!("P95 {} ms", measured_value(snapshot.p95_ms, 2))),
            );
    }
    div()
        .flex()
        .flex_wrap()
        .items_end()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_40()
                .gap_1()
                .child(div().text_xs().text_color(palette.secondary).child(label))
                .child(
                    div()
                        .text_size(rems(4.25))
                        .line_height(relative(1.0))
                        .font_weight(FontWeight::BLACK)
                        .text_color(color)
                        .child(value),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_24()
                .gap_1()
                .pb_1()
                .child(div().text_xs().text_color(palette.secondary).child(
                    if snapshot.score.is_some() {
                        "最慢场 P95"
                    } else {
                        "P95 间隔"
                    },
                ))
                .child(
                    div()
                        .text_size(rems(1.75))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(measured_value(snapshot.p95_ms, 2)),
                )
                .child(div().text_xs().text_color(palette.secondary).child("ms")),
        )
}

fn run_progress(snapshot: &BenchmarkViewData, palette: BenchmarkPalette) -> Div {
    let phase = if snapshot.phase_label.is_empty() {
        "准备就绪".to_string()
    } else {
        snapshot.phase_label.clone()
    };
    let timing = if snapshot.running {
        format!("{:.0} 秒", snapshot.remaining_seconds.ceil().max(0.0))
    } else if snapshot.case_count > 0 {
        format!("{} / {}", snapshot.case_index, snapshot.case_count)
    } else {
        String::new()
    };
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap_1()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(phase))
                        .when(!snapshot.case_label.is_empty(), |view| {
                            view.child(
                                div()
                                    .text_xs()
                                    .text_color(palette.secondary)
                                    .child(snapshot.case_label.clone()),
                            )
                        }),
                )
                .child(div().text_sm().text_color(palette.activity).child(timing)),
        )
        .child(
            div().h_1().w_full().bg(palette.track).child(
                div()
                    .h_full()
                    .w(relative(snapshot.progress.clamp(0.0, 1.0)))
                    .bg(palette.activity),
            ),
        )
}

fn frame_chart(snapshot: &BenchmarkViewData, palette: BenchmarkPalette) -> Div {
    // Maximums retain short spikes when several frame intervals share a column.
    let chunk_size = snapshot.frame_intervals.len().div_ceil(32).max(1);
    let columns: Vec<f32> = snapshot
        .frame_intervals
        .chunks(chunk_size)
        .map(|chunk| chunk.iter().copied().fold(0.0, f32::max))
        .collect();
    let maximum = columns.iter().copied().fold(0.0, f32::max);
    let slow_count = snapshot
        .frame_intervals
        .iter()
        .filter(|&&interval| interval as f64 > snapshot.budget_ms)
        .count();
    let scale = maximum.max(snapshot.budget_ms as f32 * 2.0).max(1.0);
    let budget_label = if snapshot.budget_ms > 0.0 {
        format!("预算 {:.2} ms", snapshot.budget_ms)
    } else {
        "等待采样".to_string()
    };
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .bg(palette.surface)
        .child(section_heading("帧间隔", budget_label, palette))
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(1.))
                .h_12()
                .w_full()
                .border_b_1()
                .border_color(palette.border)
                .children(columns.into_iter().enumerate().map(|(column, interval)| {
                    let color = if interval as f64 > snapshot.budget_ms {
                        palette.slow
                    } else {
                        palette.activity
                    };
                    div()
                        .id(("benchmark-frame-column", column))
                        .flex_1()
                        .h(rems(3.0 * (interval / scale).clamp(0.0, 1.0)))
                        .bg(color)
                })),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .justify_between()
                .gap_2()
                .text_xs()
                .text_color(palette.secondary)
                .child(format!("{} 个近期回调", snapshot.frame_intervals.len()))
                .child(format!("峰值 {maximum:.2} ms · 超预算 {slow_count}")),
        )
}

fn stream_activity(snapshot: &BenchmarkViewData, palette: BenchmarkPalette) -> Div {
    let stream_count = snapshot.streams.min(16);
    let maximum = snapshot
        .stream_values
        .iter()
        .take(stream_count)
        .copied()
        .fold(0.0, f32::max)
        .max(1.0);
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .bg(palette.surface)
        .child(section_heading(
            "流活动",
            format!("{} 路", snapshot.streams),
            palette,
        ))
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(1.))
                .h_12()
                .children((0..stream_count).map(|stream| {
                    let count = snapshot.stream_values.get(stream).copied().unwrap_or(0.0);
                    div()
                        .id(("benchmark-stream", stream))
                        .flex()
                        .flex_col()
                        .flex_1()
                        .h_full()
                        .justify_end()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .w_full()
                                .h(rems(2.5 * (count / maximum).clamp(0.0, 1.0)))
                                .bg(palette.result),
                        )
                })),
        )
}

fn workload_summary(snapshot: &BenchmarkViewData, palette: BenchmarkPalette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .pt_3()
        .border_t_1()
        .border_color(palette.border)
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap_2()
                .child(
                    div()
                        .text_size(rems(2.5))
                        .line_height(relative(1.0))
                        .font_weight(FontWeight::BOLD)
                        .child(format_count(snapshot.rows as u64)),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(palette.secondary)
                        .child("数据行"),
                )
                .child(div().text_sm().text_color(palette.activity).child(format!(
                    "{} 次 / 秒",
                    format_count(snapshot.updates_per_second)
                ))),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_4()
                .text_xs()
                .text_color(palette.secondary)
                .child(format!(
                    "已应用 {} 次",
                    format_count(snapshot.total_updates)
                ))
                .child(
                    div()
                        .when(snapshot.dropped_updates > 0, |view| {
                            view.text_color(palette.slow)
                        })
                        .child(format!(
                            "丢弃 {} 次",
                            format_count(snapshot.dropped_updates)
                        )),
                ),
        )
}

fn section_heading(title: &'static str, value: String, palette: BenchmarkPalette) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .justify_between()
        .gap_2()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(div().text_xs().text_color(palette.secondary).child(value))
}

fn measured_value(value: f64, decimals: usize) -> String {
    if value.is_finite() && value > 0.0 {
        format!("{value:.decimals$}")
    } else {
        "—".to_string()
    }
}

fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let first_group = digits.len() % 3;
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && index % 3 == first_group {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}
