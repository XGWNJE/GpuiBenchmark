use gpui_kit::component::{
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    progress::Progress,
    scroll::ScrollableElement,
    switch::Switch,
    ActiveTheme, Disableable, Selectable, Sizable, Theme, ThemeMode,
};
use gpui_kit::{prelude::*, *};

// Android's minimum touch target is a physical platform boundary. Relative
// spacing and typography can grow around this floor when the base font changes.
const MIN_TOUCH_TARGET: Pixels = px(48.0);

pub struct StylePreview {
    title: Entity<InputState>,
    preview_title: SharedString,
    enabled: bool,
    selected: bool,
    compact: bool,
    text_size: usize,
    activation_count: u64,
    benchmark_mode: bool,
    _input_subscription: Subscription,
}

impl StylePreview {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("输入卡片标题"));
        let input_subscription = cx.subscribe_in(&title, window, |this, input, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.preview_title = input.read(cx).value();
                cx.notify();
            }
        });
        Self {
            title,
            preview_title: "".into(),
            enabled: true,
            selected: false,
            compact: false,
            text_size: 1,
            activation_count: 0,
            benchmark_mode: false,
            _input_subscription: input_subscription,
        }
    }

    pub fn new_benchmark(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut preview = Self::new(window, cx);
        preview.benchmark_mode = true;
        preview
    }

    pub fn benchmark_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.activation_count = self.activation_count.saturating_add(1);
        self.enabled = self.activation_count % 2 == 0;
        self.selected = self.activation_count % 3 == 0;
        let title = format!("GPUI stream · {}", self.activation_count);
        self.title
            .update(cx, |state, cx| state.set_value(title, window, cx));
        cx.notify();
    }

    fn render_benchmark(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .size_full()
            .p_3()
            .gap_3()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                Input::new(&self.title)
                    .large()
                    .min_h(MIN_TOUCH_TARGET)
                    .w_full(),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        Button::new("benchmark-component-button")
                            .label("主要按钮")
                            .primary()
                            .large()
                            .min_h(MIN_TOUCH_TARGET),
                    )
                    .child(
                        Switch::new("benchmark-component-switch")
                            .checked(self.enabled)
                            .label("启用"),
                    )
                    .child(
                        Checkbox::new("benchmark-component-check")
                            .checked(self.selected)
                            .label("选中")
                            .min_h(MIN_TOUCH_TARGET),
                    ),
            )
            .child(
                Progress::new("benchmark-component-progress")
                    .value(((self.activation_count % 11) * 10) as f32),
            )
    }

    fn activate_sample(&mut self, cx: &mut Context<Self>) {
        self.activation_count = self.activation_count.saturating_add(1);
        cx.notify();
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = cx.theme().is_dark();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("外观"),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("style-light")
                            .label("浅色")
                            .selected(!dark)
                            .large()
                            .min_h(MIN_TOUCH_TARGET)
                            .flex_1()
                            .on_click(|_, window, cx| {
                                Theme::change(ThemeMode::Light, Some(window), cx);
                            }),
                    )
                    .child(
                        Button::new("style-dark")
                            .label("深色")
                            .selected(dark)
                            .large()
                            .min_h(MIN_TOUCH_TARGET)
                            .flex_1()
                            .on_click(|_, window, cx| {
                                Theme::change(ThemeMode::Dark, Some(window), cx);
                            }),
                    ),
            )
    }

    fn render_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("按钮"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("style-default")
                            .label("默认")
                            .large()
                            .min_h(MIN_TOUCH_TARGET)
                            .on_click(cx.listener(|this, _, _, cx| this.activate_sample(cx))),
                    )
                    .child(
                        Button::new("style-primary")
                            .label("主要")
                            .primary()
                            .large()
                            .min_h(MIN_TOUCH_TARGET)
                            .on_click(cx.listener(|this, _, _, cx| this.activate_sample(cx))),
                    )
                    .child(
                        Button::new("style-outline")
                            .label("描边")
                            .outline()
                            .large()
                            .min_h(MIN_TOUCH_TARGET)
                            .on_click(cx.listener(|this, _, _, cx| this.activate_sample(cx))),
                    )
                    .child(
                        Button::new("style-ghost")
                            .label("轻量")
                            .ghost()
                            .large()
                            .min_h(MIN_TOUCH_TARGET)
                            .on_click(cx.listener(|this, _, _, cx| this.activate_sample(cx))),
                    )
                    .child(
                        Button::new("style-disabled")
                            .label("禁用")
                            .disabled(true)
                            .large()
                            .min_h(MIN_TOUCH_TARGET),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("已点 {} 次", self.activation_count)),
            )
    }

    fn render_input(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("卡片标题"),
            )
            .child(
                Input::new(&self.title)
                    .aria_label("卡片标题")
                    .large()
                    .min_h(MIN_TOUCH_TARGET)
                    .w_full(),
            )
    }

    fn render_toggles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("状态"),
            )
            // Kit applies Switch's Styled geometry to its outer wrapper. The
            // surrounding hit target extends the same command to the full row.
            .child(
                div()
                    .id("style-switch-touch-target")
                    .flex()
                    .items_center()
                    .min_h(MIN_TOUCH_TARGET)
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.enabled = !this.enabled;
                        cx.notify();
                    }))
                    .child(
                        Switch::new("style-enabled")
                            .checked(self.enabled)
                            .label("启用预览状态")
                            .on_change(cx.listener(|this, next, _, cx| {
                                this.enabled = *next;
                                cx.stop_propagation();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Checkbox::new("style-selected")
                    .label("选中卡片")
                    .checked(self.selected)
                    .min_h(MIN_TOUCH_TARGET)
                    .py_3()
                    .w_full()
                    .on_change(cx.listener(|this, next, _, cx| {
                        this.selected = *next;
                        cx.notify();
                    })),
            )
    }

    fn render_cards(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title: SharedString = if self.preview_title.is_empty() {
            "实时数据监测".into()
        } else {
            self.preview_title.clone()
        };
        let progress = ((self.activation_count % 11) * 10) as f32;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("卡片"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .border_1()
                    .border_color(if self.selected {
                        cx.theme().primary
                    } else {
                        cx.theme().border
                    })
                    .rounded(cx.theme().radius)
                    .bg(if self.selected {
                        cx.theme().accent
                    } else {
                        cx.theme().group_box
                    })
                    .child(title)
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap_2()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(if self.enabled {
                                "已开启"
                            } else {
                                "已关闭"
                            })
                            .child(format!("{progress:.0}%")),
                    )
                    .child(
                        Progress::new("style-progress")
                            .value(progress)
                            .accessibility_label("按钮点击进度"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_4()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().muted)
                    .child("多行内容")
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("中文、English、1234567890。长内容随可用宽度换行。"),
                    ),
            )
    }

    fn render_density(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("数据行"),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("style-comfortable")
                            .label("舒适")
                            .selected(!self.compact)
                            .large()
                            .min_h(MIN_TOUCH_TARGET)
                            .flex_1()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.compact = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("style-compact")
                            .label("紧凑")
                            .selected(self.compact)
                            .large()
                            .min_h(MIN_TOUCH_TARGET)
                            .flex_1()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.compact = true;
                                cx.notify();
                            })),
                    ),
            )
            .children(
                [
                    ("Stream 01", "128.40"),
                    ("Stream 02", "96.18"),
                    ("Stream 03", "204.62"),
                ]
                .into_iter()
                .map(|(name, value)| {
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_3()
                        .when(self.compact, |row| row.py_2().text_sm())
                        .when(!self.compact, |row| row.py_4().text_base())
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(name)
                        .child(value)
                }),
            )
    }

    fn render_typography(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("字号"),
            )
            .child(
                div().flex().gap_2().children(
                    [("small", "小"), ("normal", "中"), ("large", "大")]
                        .into_iter()
                        .enumerate()
                        .map(|(size, (_, label))| {
                            Button::new(("style-text-size", size))
                                .label(label)
                                .selected(self.text_size == size)
                                .large()
                                .min_h(MIN_TOUCH_TARGET)
                                .flex_1()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.text_size = size;
                                    cx.notify();
                                }))
                        }),
                ),
            )
            .child(
                div()
                    .py_3()
                    .when(self.text_size == 0, |text| text.text_sm())
                    .when(self.text_size == 1, |text| text.text_base())
                    .when(self.text_size == 2, |text| text.text_lg())
                    .child("流畅的界面，让数据清晰呈现。GPUI 0123456789"),
            )
    }
}

impl Render for StylePreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.benchmark_mode {
            return self.render_benchmark(cx).into_any_element();
        }
        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .p_4()
                    .child(self.render_appearance(cx))
                    .child(self.render_input(cx))
                    .child(self.render_buttons(cx))
                    .child(self.render_toggles(cx))
                    .child(self.render_cards(cx))
                    .child(self.render_density(cx))
                    .child(self.render_typography(cx)),
            )
            .overflow_y_scrollbar()
            .into_any_element()
    }
}
