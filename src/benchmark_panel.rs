use std::time::{Duration, Instant};

use gpui_kit::component::{
    button::{Button, ButtonCustomVariant, ButtonVariants},
    ActiveTheme, Sizable, Theme, ThemeMode,
};
use gpui_kit::{prelude::*, *};

use crate::android_host;
use crate::benchmark::{
    BenchmarkEvent, BenchmarkOptions, BenchmarkPhase, BenchmarkScene, BenchmarkSuite,
};
use crate::benchmark_view::{BenchmarkDashboard, BenchmarkViewData};
use crate::style_preview::StylePreview;

struct Preparation {
    run_id: String,
    options: BenchmarkOptions,
    requested_hz: f64,
    started: Instant,
}

pub struct BenchmarkPanel {
    suite: Option<BenchmarkSuite>,
    preparation: Option<Preparation>,
    running: bool,
    foreground: bool,
    frame_pending: bool,
    report_sent: bool,
    device: serde_json::Value,
    upload_status: String,
    original_dark: Option<bool>,
    scroll: UniformListScrollHandle,
    last_tick: Option<Instant>,
    last_style_tick: Instant,
    styles: Entity<StylePreview>,
    measurement_viewport: Option<Size<Pixels>>,
    measurement_insets: [i32; 4],
}

pub struct BenchmarkStateChanged;
impl EventEmitter<BenchmarkStateChanged> for BenchmarkPanel {}

impl BenchmarkPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_window_activation(window, |this, window, cx| {
            this.activation_changed(window, cx)
        })
        .detach();
        cx.observe_window_visibility(window, |this, _, window, cx| {
            this.activation_changed(window, cx)
        })
        .detach();
        let updates = android_host::host_updates();
        cx.spawn(async move |this, cx| {
            while updates.recv().await.is_ok() {
                if this
                    .update(cx, |this, cx| {
                        this.upload_status = android_host::benchmark_upload_status();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        if let Ok(settings) = android_host::benchmark_settings() {
            if let Ok(settings) = serde_json::from_str::<serde_json::Value>(&settings) {
                let options = BenchmarkOptions {
                    warmup_ms: settings["warmup_ms"].as_u64().unwrap_or(3_000),
                    measurement_ms: settings["measurement_ms"].as_u64().unwrap_or(20_000),
                    repetitions: settings["repetitions"]
                        .as_u64()
                        .unwrap_or(3)
                        .min(u32::MAX as u64) as u32,
                };
                let run_id = settings["run_id"].as_str().map(str::to_owned);
                cx.on_next_frame(window, move |this, window, cx| {
                    this.start(options, run_id, window, cx)
                });
            }
        }
        Self {
            suite: None,
            preparation: None,
            running: false,
            foreground: window.is_window_active() && window.is_visible(),
            frame_pending: false,
            report_sent: false,
            device: serde_json::Value::Null,
            upload_status: String::new(),
            original_dark: None,
            scroll: UniformListScrollHandle::new(),
            last_tick: None,
            last_style_tick: Instant::now(),
            styles: cx.new(|cx| StylePreview::new_benchmark(window, cx)),
            measurement_viewport: None,
            measurement_insets: [0; 4],
        }
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    fn start(
        &mut self,
        options: BenchmarkOptions,
        run_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.running {
            return;
        }
        match android_host::prepare_benchmark() {
            Ok(device) => {
                let id = run_id.or_else(|| device["run_id"].as_str().map(str::to_owned));
                let Some(id) = id else {
                    self.upload_status = "无法创建测试编号".into();
                    let _ = android_host::finish_benchmark();
                    cx.notify();
                    return;
                };
                self.preparation = Some(Preparation {
                    run_id: id,
                    options,
                    requested_hz: device["refresh_hz"].as_f64().unwrap_or(60.0),
                    started: Instant::now(),
                });
                self.device = device;
                self.suite = None;
                self.report_sent = false;
                self.running = true;
                cx.emit(BenchmarkStateChanged);
                self.foreground = window.is_window_active() && window.is_visible();
                self.last_tick = None;
                self.measurement_viewport = None;
                self.upload_status.clear();
                self.original_dark = Some(cx.theme().is_dark());
                Theme::change(ThemeMode::Dark, Some(window), cx);
                self.schedule_frame(window, cx);
            }
            Err(error) => {
                let _ = android_host::finish_benchmark();
                self.upload_status = error;
            }
        }
        cx.notify();
    }

    fn activation_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let foreground = window.is_window_active() && window.is_visible();
        if self.running && self.foreground && !foreground && self.preparation.is_none() {
            self.cancel("测试被后台或其他窗口打断", window, cx);
        }
        self.foreground = foreground;
        if self.running && foreground {
            self.schedule_frame(window, cx);
        }
    }

    fn schedule_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.frame_pending || !self.running || !self.foreground {
            return;
        }
        self.frame_pending = true;
        cx.on_next_frame(window, |this, window, cx| {
            this.frame_pending = false;
            if !this.running || !this.foreground {
                return;
            }
            this.advance(Instant::now(), window, cx);
            cx.notify();
            this.schedule_frame(window, cx);
        });
    }

    fn advance(&mut self, now: Instant, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(preparation) = self.preparation.as_ref() {
            let elapsed = now.saturating_duration_since(preparation.started);
            if elapsed < Duration::from_millis(500) {
                return;
            }
            let actual = android_host::benchmark_device();
            let actual_hz = actual
                .as_ref()
                .ok()
                .and_then(|value| value["refresh_hz"].as_f64());
            let settled = actual_hz.is_some_and(|hz| (hz - preparation.requested_hz).abs() < 0.5);
            if !settled && elapsed < Duration::from_secs(2) {
                return;
            }
            let preparation = self.preparation.take().unwrap();
            match android_host::confirm_benchmark_device() {
                Ok(device) => {
                    let refresh_hz = device["refresh_hz"].as_f64().unwrap_or(60.0);
                    self.device = device;
                    self.measurement_viewport = Some(window.viewport_size());
                    self.measurement_insets = android_host::safe_area();
                    self.suite = Some(BenchmarkSuite::new(
                        preparation.run_id,
                        preparation.options,
                        refresh_hz,
                        now,
                    ));
                }
                Err(error) => {
                    let mut suite =
                        BenchmarkSuite::new(preparation.run_id, preparation.options, 60.0, now);
                    suite.invalidate(&error);
                    self.suite = Some(suite);
                    self.finish(window, cx);
                    return;
                }
            }
        }
        let Some(suite) = self.suite.as_mut() else {
            return;
        };
        if self.measurement_viewport != Some(window.viewport_size())
            || self.measurement_insets != android_host::safe_area()
        {
            suite.invalidate("测量期间可用窗口或安全区发生变化");
            self.finish(window, cx);
            return;
        }
        let events = suite.advance(now);
        let snapshot = suite.snapshot(now);
        let elapsed = self
            .last_tick
            .replace(now)
            .map(|previous| now.saturating_duration_since(previous));
        let scene = snapshot.case.as_ref().map(|case| case.scene);
        if scene == Some(BenchmarkScene::Scroll) {
            if let Some(elapsed) = elapsed {
                let handle = self.scroll.0.borrow().base_handle.clone();
                let offset = handle.offset();
                if self.scroll.is_scrolled_to_end() == Some(true) {
                    handle.set_offset(point(px(0.), px(0.)));
                } else {
                    handle.set_offset(point(
                        offset.x,
                        offset.y - px(240.0 * elapsed.as_secs_f32()),
                    ));
                }
            }
        }
        if scene.is_some_and(BenchmarkScene::is_styles)
            && now.saturating_duration_since(self.last_style_tick) >= Duration::from_millis(250)
        {
            self.styles
                .update(cx, |styles, cx| styles.benchmark_step(window, cx));
            self.last_style_tick = now;
        }
        for event in events {
            let (marker, index) = match event {
                BenchmarkEvent::CaseStarted(case) => {
                    self.scroll.scroll_to_item_strict(0, ScrollStrategy::Top);
                    let mode = if case.scene == BenchmarkScene::StylesLight {
                        ThemeMode::Light
                    } else {
                        ThemeMode::Dark
                    };
                    Theme::change(mode, Some(window), cx);
                    ("started", snapshot.case_index)
                }
                BenchmarkEvent::MeasurementStarted(case) => {
                    self.verify_display(&case.id);
                    ("measuring", snapshot.case_index)
                }
                BenchmarkEvent::CaseCompleted(result) => {
                    self.verify_display(&result.case_id);
                    ("case_complete", snapshot.case_index.saturating_sub(1))
                }
                BenchmarkEvent::Finished => {
                    self.finish(window, cx);
                    continue;
                }
            };
            if let Err(error) = android_host::benchmark_marker(marker, index) {
                log::warn!("{error}");
            }
        }
        if self.suite.as_ref().is_some_and(BenchmarkSuite::is_finished) {
            self.finish(window, cx);
        }
    }

    fn verify_display(&mut self, case_id: &str) {
        let expected = self.device["refresh_hz"].as_f64().unwrap_or(60.0);
        match android_host::benchmark_device() {
            Ok(device)
                if device["refresh_hz"]
                    .as_f64()
                    .is_some_and(|hz| (hz - expected).abs() < 0.5) => {}
            _ => {
                if let Some(suite) = self.suite.as_mut() {
                    suite.invalidate(&format!("测量期间屏幕刷新模式发生变化 · {case_id}"));
                }
            }
        }
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.report_sent {
            return;
        }
        self.running = false;
        cx.emit(BenchmarkStateChanged);
        self.preparation = None;
        self.last_tick = None;
        self.report_sent = true;
        if let Err(error) = android_host::finish_benchmark() {
            log::warn!("{error}");
        }
        if let Some(suite) = self.suite.as_ref() {
            let mut report = serde_json::to_value(suite.report()).unwrap_or_default();
            if let Some(viewport) = self.measurement_viewport {
                report["render_context"] = serde_json::json!({
                    "viewport_width_dp": f32::from(viewport.width),
                    "viewport_height_dp": f32::from(viewport.height),
                    "safe_area_px": self.measurement_insets,
                    "scale_factor": window.scale_factor(),
                    "rem_size_dp": f32::from(window.rem_size())
                });
            }
            match serde_json::to_string(&report)
                .map_err(|error| error.to_string())
                .and_then(|report| android_host::save_benchmark_report(&report))
            {
                Ok(()) => self.upload_status = "正在保存完整报告".into(),
                Err(error) => self.upload_status = error,
            }
        }
        if let Some(dark) = self.original_dark.take() {
            Theme::change(
                if dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                },
                Some(window),
                cx,
            );
        }
        cx.notify();
    }

    fn cancel(&mut self, reason: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(preparation) = self.preparation.take() {
            let mut suite = BenchmarkSuite::new(
                preparation.run_id,
                preparation.options,
                60.0,
                Instant::now(),
            );
            suite.cancel(reason);
            self.suite = Some(suite);
        } else if let Some(suite) = self.suite.as_mut() {
            suite.cancel(reason);
        }
        self.finish(window, cx);
    }

    fn view_data(&self) -> BenchmarkViewData {
        let Some(suite) = self.suite.as_ref() else {
            return BenchmarkViewData {
                running: self.running,
                phase_label: if self.preparation.is_some() {
                    "准备测试环境".into()
                } else {
                    "准备就绪".into()
                },
                case_label: "30 场景 · 全自动 · 约 12 分钟".into(),
                case_count: 30,
                rows: 200_000,
                streams: 16,
                updates_per_second: 16_000,
                upload_status: self.upload_status.clone(),
                ..Default::default()
            };
        };
        let snapshot = suite.snapshot(Instant::now());
        let report = if suite.is_finished() {
            Some(suite.report())
        } else {
            None
        };
        let phase_label = match snapshot.phase {
            BenchmarkPhase::Warmup => "预热",
            BenchmarkPhase::Measuring => "正在测量",
            BenchmarkPhase::Finished
                if report
                    .as_ref()
                    .is_some_and(|report| report.status == "completed") =>
            {
                "测试完成"
            }
            BenchmarkPhase::Finished | BenchmarkPhase::Cancelled => "测试已停止",
        };
        BenchmarkViewData {
            available_height: 0.0,
            running: self.running,
            phase_label: phase_label.into(),
            case_label: snapshot
                .case
                .as_ref()
                .map(|case| format!("{} · 第 {} 轮", case.label(), case.repetition))
                .or_else(|| {
                    report
                        .as_ref()
                        .and_then(|report| report.invalid_reason.clone())
                })
                .unwrap_or_else(|| "完整报告已生成".into()),
            case_index: if suite.is_finished() {
                snapshot.case_index
            } else {
                snapshot.case_index + 1
            },
            case_count: snapshot.total_cases,
            progress: snapshot.overall_progress,
            remaining_seconds: snapshot.remaining_seconds as f64,
            callback_fps: snapshot.frames.fps,
            p95_ms: report.as_ref().map_or(snapshot.frames.p95_ms, |report| {
                report
                    .results
                    .iter()
                    .map(|case| case.frame_stats.p95_ms)
                    .fold(0.0, f64::max)
            }),
            budget_ms: snapshot.frames.budget_ms,
            rows: suite.workload().len(),
            streams: suite.workload().stream_count(),
            updates_per_second: suite.workload().updates_per_second() as u64,
            total_updates: report.as_ref().map_or(snapshot.applied_updates, |report| {
                report.results.iter().map(|case| case.applied_updates).sum()
            }),
            dropped_updates: report.as_ref().map_or(snapshot.dropped_updates, |report| {
                report.results.iter().map(|case| case.dropped_updates).sum()
            }),
            frame_intervals: snapshot
                .recent_intervals_ms
                .into_iter()
                .map(|value| value as f32)
                .collect(),
            stream_values: snapshot
                .stream_activity
                .into_iter()
                .map(|value| value as f32)
                .collect(),
            score: report
                .filter(|report| report.status == "completed")
                .map(|report| report.score),
            upload_status: self.upload_status.clone(),
        }
    }

    fn data_surface(&self, cx: &mut Context<Self>) -> AnyElement {
        if self
            .suite
            .as_ref()
            .and_then(BenchmarkSuite::current_case)
            .is_some_and(|case| case.scene.is_styles())
        {
            return self.styles.clone().into_any_element();
        }
        let entity = cx.entity();
        let count = self
            .suite
            .as_ref()
            .map_or(0, |suite| suite.workload().len());
        uniform_list("benchmark-data", count, move |range, _, cx| {
            let panel = entity.read(cx);
            let Some(suite) = panel.suite.as_ref() else {
                return Vec::new();
            };
            range
                .filter_map(|index| suite.workload().row(index))
                .map(|row| {
                    div()
                        .w_full()
                        .h_12()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .px_3()
                        .border_b_1()
                        .border_color(rgb(0x253b4c))
                        .bg(rgb(0x101b29))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0x91a9bb))
                                        .child(format!("流 {:02} / {:06}", row.source, row.id)),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(rgb(0x91a9bb))
                                        .child(format!("更新 {}", row.revision)),
                                ),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0x55e4d4))
                                .child(format!("{:.2}", row.value)),
                        )
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .size_full()
        .into_any_element()
    }
}

impl Render for BenchmarkPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let controls = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(
                Button::new("benchmark-run")
                    .large()
                    .min_h(px(48.))
                    .flex_1()
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(rgb(0x55e4d4).into())
                            .foreground(rgb(0x080d14).into())
                            .hover(rgb(0x7af0e3).into())
                            .active(rgb(0x37cabb).into()),
                    )
                    .label(if self.running {
                        "停止测试"
                    } else {
                        "开始跑分"
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.running {
                            this.cancel("用户停止测试", window, cx);
                        } else {
                            this.start(BenchmarkOptions::default(), None, window, cx);
                        }
                    })),
            )
            .child(
                Button::new("benchmark-leaderboard")
                    .large()
                    .min_h(px(48.))
                    .label("排行榜")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Err(error) = android_host::open_leaderboard() {
                            this.upload_status = error;
                            cx.notify();
                        }
                    })),
            )
            .when(
                self.suite.as_ref().is_some_and(BenchmarkSuite::is_finished),
                |view| {
                    view.child(
                        Button::new("benchmark-share")
                            .large()
                            .min_h(px(48.))
                            .label("分享")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(suite) = this.suite.as_ref() {
                                    let mut report =
                                        serde_json::to_value(suite.report()).unwrap_or_default();
                                    report["device"] = this.device.clone();
                                    if let Err(error) =
                                        android_host::share_report(&report.to_string())
                                    {
                                        this.upload_status = error;
                                        cx.notify();
                                    }
                                }
                            })),
                    )
                },
            )
            .into_any_element();
        let surface = div()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(self.data_surface(cx))
            .when(self.running, |view| {
                view.relative().child(
                    div()
                        .id("benchmark-surface-input-shield")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
                )
            });
        let insets = android_host::safe_area();
        let safe_height = px((insets[1] + insets[3]) as f32 / window.scale_factor());
        let mut data = self.view_data();
        data.available_height = f32::from(
            (window.viewport_size().height - safe_height - window.rem_size() * 7.).max(px(0.)),
        );
        BenchmarkDashboard::new(data, controls).surface(surface.into_any_element())
    }
}
