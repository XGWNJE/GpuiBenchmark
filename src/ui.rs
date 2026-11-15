use std::time::{Duration, Instant};

use gpui_kit::component::{
    button::{Button, ButtonVariants},
    ActiveTheme, Selectable, Sizable,
};
use gpui_kit::{prelude::*, *};

use crate::metrics::{Metrics, MetricsSnapshot};
use crate::style_preview::StylePreview;
use crate::workload::{Workload, WorkloadPreset};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Stress,
    Styles,
    Help,
}

pub struct Lab {
    page: Page,
    workload: Workload,
    metrics: Metrics,
    snapshot: MetricsSnapshot,
    running: bool,
    foreground: bool,
    frame_pending: bool,
    last_tick: Option<Instant>,
    last_readout: Instant,
    list_scroll: UniformListScrollHandle,
    auto_scroll: bool,
    controls_open: bool,
    compact: bool,
    share_status: Option<String>,
    styles: Entity<StylePreview>,
    light_system_bars: Option<bool>,
}

impl Lab {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let inset_updates = crate::android_host::inset_updates();
        cx.spawn(async move |this, cx| {
            while inset_updates.recv().await.is_ok() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            this.update_foreground(window, cx);
        })
        .detach();
        cx.observe_window_visibility(window, |this, _, window, cx| {
            this.update_foreground(window, cx);
        })
        .detach();
        let metrics = Metrics::new(60);
        let snapshot = metrics.snapshot();
        Self {
            page: Page::Stress,
            workload: Workload::new(WorkloadPreset::Light),
            metrics,
            snapshot,
            running: false,
            foreground: window.is_window_active() && window.is_visible(),
            frame_pending: false,
            last_tick: None,
            last_readout: Instant::now(),
            list_scroll: UniformListScrollHandle::new(),
            auto_scroll: false,
            controls_open: false,
            compact: false,
            share_status: None,
            styles: cx.new(|cx| StylePreview::new(window, cx)),
            light_system_bars: None,
        }
    }

    fn update_foreground(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.foreground = window.is_window_active() && window.is_visible();
        self.last_tick = None;
        if self.running && self.foreground {
            self.metrics.resume();
            self.schedule_frame(window, cx);
        } else {
            self.metrics.pause();
        }
        cx.notify();
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
            let now = Instant::now();
            this.metrics.frame(now);
            if let Some(previous) = this.last_tick.replace(now) {
                let elapsed = now.saturating_duration_since(previous);
                this.workload.tick(elapsed);
                if this.auto_scroll && this.page == Page::Stress && !this.controls_open {
                    let handle = this.list_scroll.0.borrow().base_handle.clone();
                    let offset = handle.offset();
                    if this.list_scroll.is_scrolled_to_end() == Some(true) {
                        handle.set_offset(point(px(0.), px(0.)));
                    } else {
                        handle.set_offset(point(
                            offset.x,
                            offset.y - px(240.0 * elapsed.as_secs_f32()),
                        ));
                    }
                }
            }
            if now.duration_since(this.last_readout) >= Duration::from_millis(250) {
                this.snapshot = this.metrics.snapshot();
                this.last_readout = now;
            }
            cx.notify();
            this.schedule_frame(window, cx);
        });
    }

    fn toggle_running(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.running = !self.running;
        self.last_tick = None;
        self.share_status = None;
        if self.running {
            self.foreground = window.is_window_active() && window.is_visible();
            self.metrics.resume();
            self.schedule_frame(window, cx);
        } else {
            self.metrics.pause();
            self.snapshot = self.metrics.snapshot();
        }
        cx.notify();
    }

    fn reset_run(&mut self, preset: WorkloadPreset, cx: &mut Context<Self>) {
        self.workload.apply_preset(preset);
        self.metrics.reset();
        self.snapshot = self.metrics.snapshot();
        self.last_tick = None;
        self.share_status = None;
        cx.notify();
    }

    fn stress(&self, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        let frame_value = if self.snapshot.sample_count == 0 {
            "—".into()
        } else {
            format!("{:.0}", self.snapshot.fps)
        };
        let p95_value = if self.snapshot.sample_count == 0 {
            "—".into()
        } else {
            format!("{:.1} ms", self.snapshot.p95_ms)
        };
        let metrics = div()
            .flex()
            .flex_wrap()
            .gap_4()
            .px_4()
            .py_3()
            .child(readout("帧回调 / 秒", frame_value, cx))
            .child(readout("P95 间隔", p95_value, cx))
            .child(readout(
                "累计更新",
                short_number(self.workload.total_updates()),
                cx,
            ));
        let toolbar = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .px_4()
            .pb_3()
            .child(
                command("run", if self.running { "暂停" } else { "开始" })
                    .primary()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_running(window, cx);
                    })),
            )
            .child(
                command("parameters", "参数")
                    .selected(self.controls_open)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.controls_open = !this.controls_open;
                        cx.notify();
                    })),
            )
            .child(
                command("auto-scroll", "自动滚动")
                    .selected(self.auto_scroll)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.auto_scroll = !this.auto_scroll;
                        cx.notify();
                    })),
            );
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_4()
            .py_2()
            .bg(theme.muted)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(format!(
                "{} 行 · {} 流 · {} 次/秒",
                short_number(self.workload.len() as u64),
                self.workload.stream_count(),
                short_number(self.workload.updates_per_second() as u64)
            ))
            .child(if self.running {
                "运行中"
            } else {
                "已暂停"
            });
        let entity = cx.entity();
        let list = uniform_list("stress-list", self.workload.len(), move |range, _, cx| {
            let lab = entity.read(cx);
            let theme = cx.theme();
            range
                .filter_map(|index| lab.workload.row(index))
                .map(|row| {
                    div()
                        .id(("record", row.id))
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .when(lab.compact, |view| view.h_12())
                        .when(!lab.compact, |view| view.h_16())
                        .px_4()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(format!("Stream {:02} / {:06}", row.source, row.id))
                                .when(!lab.compact, |view| {
                                    view.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(format!("修订 {}", row.revision)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_end()
                                .child(format!("{:.2}", row.value))
                                .when(!lab.compact, |view| {
                                    view.child(
                                        div()
                                            .text_xs()
                                            .text_color(if row.trend >= 0.0 {
                                                theme.success
                                            } else {
                                                theme.danger
                                            })
                                            .child(format!("{:+.2}", row.trend)),
                                    )
                                }),
                        )
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.list_scroll)
        .size_full()
        .flex_1()
        .min_h_0();
        let body = if self.controls_open {
            self.controls(cx).into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .child(header)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(list),
                )
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(metrics)
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(body))
    }

    fn controls(&self, cx: &Context<Self>) -> impl IntoElement {
        div()
            .id("load-controls")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .bg(cx.theme().muted)
            .child(
                div().flex().flex_wrap().gap_2().children(
                    WorkloadPreset::ALL
                        .into_iter()
                        .enumerate()
                        .map(|(index, preset)| {
                            command(("preset", index), preset.label())
                                .selected(self.workload.preset() == preset)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.reset_run(preset, cx);
                                    this.list_scroll
                                        .scroll_to_item_strict(0, ScrollStrategy::Top);
                                }))
                        }),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children([60, 90, 120].into_iter().map(|hz| {
                        command(("refresh-budget", hz as usize), format!("{hz} Hz"))
                            .selected(self.snapshot.refresh_hz == hz)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.metrics.set_refresh_hz(hz);
                                this.snapshot = this.metrics.snapshot();
                                cx.notify();
                            }))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        command("compact-rows", "紧凑行")
                            .selected(self.compact)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.compact = !this.compact;
                                cx.notify();
                            })),
                    )
                    .child(
                        command("reset", "重置").on_click(cx.listener(|this, _, _, cx| {
                            this.reset_run(this.workload.preset(), cx);
                        })),
                    ),
            )
    }

    fn share_report(&mut self, cx: &mut Context<Self>) {
        let report = format!(
            "{{\"app\":\"GPUI Lab\",\"version\":\"0.1.0\",\"abi\":\"{}\",\"workload\":{},\"frames\":{}}}",
            std::env::consts::ARCH, self.workload.summary_json(), self.metrics.summary_json()
        );
        log::info!("GPUI_LAB_REPORT {}", report);
        self.share_status = Some(match crate::android_host::share_report(&report) {
            Ok(()) => "选择分享目标".into(),
            Err(error) => format!("分享入口打开失败，{error}"),
        });
        cx.notify();
    }

    fn help(&self, cx: &Context<Self>) -> impl IntoElement {
        div().id("help-scroll").flex().flex_col().size_full().overflow_y_scroll().p_4().gap_4()
            .child(div().text_xl().child("测试方法"))
            .child("选择负载，点击开始，手动滑动列表或启用自动滚动。保持相同主题、屏幕刷新率和温度，再比较不同负载。暂停后可分享结果。")
            .child(div().text_lg().child("负载"))
            .child("轻量 1 万行 / 1 流 / 20 次每秒。标准 5 万行 / 4 流 / 400 次每秒。高压 10 万行 / 8 流 / 4,000 次每秒。极限 20 万行 / 16 流 / 16,000 次每秒。")
            .child("数据在本机按固定种子模拟，原位更新，只为屏幕内的行创建界面。卡顿时超出单次处理上限的更新会计入跳过量。")
            .child(div().text_lg().child("测量范围"))
            .child("帧回调 / 秒、平均间隔、P95、P99 和超预算比例来自最近 600 个帧回调间隔。它们反映 UI 线程调度与负载，GPU 呈现需结合 Android 系统追踪测量。")
            .child("60 / 90 / 120 Hz 选项用于设定统计预算。设备实际刷新率由系统控制。App 退到后台时暂停更新与采样，返回后继续。样式页可在负载运行期间切换。")
            .child(div().text_lg().child("当前结果"))
            .child(format!("{} 个样本 · {:.1} 秒\n平均 {:.2} ms · P95 {:.2} ms · P99 {:.2} ms\n超预算 {:.1}% · 跳过 {} 次更新",
                self.snapshot.sample_count, self.snapshot.elapsed_seconds, self.snapshot.average_ms,
                self.snapshot.p95_ms, self.snapshot.p99_ms, self.snapshot.over_budget_ratio * 100.0,
                self.workload.skipped_updates()))
            .child(command("share-results", "分享结果").on_click(cx.listener(|this, _, _, cx| {
                this.share_report(cx);
            })))
            .when_some(self.share_status.clone(), |view, status| view.child(div().text_sm().child(status)))
            .child(div().text_sm().text_color(cx.theme().muted_foreground)
                .child("GPUI Lab 0.1.0 · GPUI 0.3.6\nAndroid 移动适配来自社区 gpui-mobile。样式组件来自 GPUI Kit。"))
    }
}

impl Render for Lab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let light = !theme.is_dark();
        if self.light_system_bars != Some(light) {
            crate::android_host::set_light_system_bars(light);
            self.light_system_bars = Some(light);
        }
        let [left, top, right, bottom] =
            crate::android_host::safe_area().map(|inset| px(inset as f32 / window.scale_factor()));
        let content = match self.page {
            Page::Stress => self.stress(cx).into_any_element(),
            Page::Styles => self.styles.clone().into_any_element(),
            Page::Help => self.help(cx).into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .pl(left)
            .pt(top)
            .pr(right)
            .pb(bottom)
            .text_color(theme.foreground)
            .text_sm()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .h_12()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("GPUI Lab"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Android"),
                    ),
            )
            .child(div().flex_1().min_h_0().min_w_0().child(content))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .p_2()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(theme.border)
                    .children(
                        [
                            (Page::Stress, "压力"),
                            (Page::Styles, "样式"),
                            (Page::Help, "帮助"),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, (page, label))| {
                            command(("navigation", index), label)
                                .flex_1()
                                .selected(self.page == page)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.page = page;
                                    if page != Page::Styles {
                                        window.dismiss_virtual_keyboard();
                                    }
                                    cx.notify();
                                }))
                        }),
                    ),
            )
    }
}

fn command(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id).label(label).large().h_12().min_w_12()
}

fn readout(label: &'static str, value: String, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_20()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
}

fn short_number(value: u64) -> String {
    if value >= 10_000 {
        format!("{:.1}万", value as f64 / 10_000.0)
    } else {
        value.to_string()
    }
}
