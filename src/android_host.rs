use gpui_mobile::android::jni::{activity, with_env};
use jni::{jni_sig, jni_str, objects::JValue};
use std::sync::{Mutex, OnceLock};

static SAFE_AREA: Mutex<[i32; 4]> = Mutex::new([0; 4]);
static INSET_UPDATES: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> =
    OnceLock::new();

pub fn safe_area() -> [i32; 4] {
    *SAFE_AREA.lock().unwrap()
}

pub fn inset_updates() -> async_channel::Receiver<()> {
    INSET_UPDATES
        .get_or_init(|| async_channel::bounded(1))
        .1
        .clone()
}

/// WindowInsets supplies system bars, cutouts and the keyboard in physical pixels.
#[no_mangle]
pub extern "system" fn Java_dev_gpui_lab_MainActivity_publishInsets(
    _env: *mut jni::sys::JNIEnv,
    _class: jni::sys::jclass,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) {
    let next = [left, top, right, bottom];
    let mut current = SAFE_AREA.lock().unwrap();
    if *current != next {
        *current = next;
        drop(current);
        let _ = INSET_UPDATES
            .get_or_init(|| async_channel::bounded(1))
            .0
            .try_send(());
    }
}

pub fn set_light_system_bars(light: bool) {
    if let Err(error) = with_env(|env| {
        let host = activity(env)?;
        env.call_method(
            &host,
            jni_str!("setLightSystemBars"),
            jni_sig!("(Z)V"),
            &[JValue::Bool(light.into())],
        )
        .map(|_| ())
        .map_err(|error| describe_jni_error(env, error))
    }) {
        log::warn!("System bar appearance failed: {error}");
    }
}

/// Schedule Android's share chooser. The user completes the share in Android.
pub fn share_report(report: &str) -> Result<(), String> {
    with_env(|env| {
        let host = activity(env)?;
        let text = env
            .new_string(report)
            .map_err(|error| describe_jni_error(env, error))?;
        env.call_method(
            &host,
            jni_str!("shareReport"),
            jni_sig!("(Ljava/lang/String;)V"),
            &[JValue::Object(&text)],
        )
        .map(|_| ())
        .map_err(|error| describe_jni_error(env, error))
    })
}

fn describe_jni_error(env: &mut jni::Env<'_>, error: jni::errors::Error) -> String {
    let error = env.exception_catch().err().unwrap_or(error);
    format!("无法打开 Android 分享 {error}")
}
