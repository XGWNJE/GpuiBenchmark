mod benchmark;
mod metrics;
mod workload;

#[cfg(target_os = "android")]
mod android_host;
#[cfg(target_os = "android")]
mod benchmark_panel;
#[cfg(target_os = "android")]
mod benchmark_view;
#[cfg(target_os = "android")]
mod style_preview;
#[cfg(target_os = "android")]
mod ui;

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: android_activity::AndroidApp) {
    use gpui::{App, Application, WindowOptions};
    use gpui_kit::prelude::*;
    use gpui_mobile::android::jni;

    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("GpuiBenchmark"),
    );
    jni::install_panic_hook();
    let _platform = jni::init_platform(&app);
    let Some(shared) = jni::shared_platform() else {
        log::error!("Android GPUI platform initialization failed");
        return;
    };
    Application::with_platform(shared.into_rc()).run(|cx: &mut App| {
        gpui_kit::init(cx);
        cx.open_window(WindowOptions::default(), |window, cx| {
            let lab = cx.new(|cx| ui::Lab::new(window, cx));
            cx.new(|cx| gpui_kit::component::Root::new(lab, window, cx))
        })
        .expect("open GPUI benchmark window");
        cx.activate(true);
    });
}
