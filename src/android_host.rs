use gpui_mobile::android::jni::{activity, get_string, with_env};
use jni::{jni_sig, jni_str, objects::JValue};
use std::sync::{Mutex, OnceLock};

static SAFE_AREA: Mutex<[i32; 4]> = Mutex::new([0; 4]);
static INSET_UPDATES: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> =
    OnceLock::new();
static HOST_UPDATES: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> =
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

fn notify_host_update() {
    let _ = HOST_UPDATES
        .get_or_init(|| async_channel::bounded(1))
        .0
        .try_send(());
}

pub fn host_updates() -> async_channel::Receiver<()> {
    HOST_UPDATES
        .get_or_init(|| async_channel::bounded(1))
        .1
        .clone()
}

#[no_mangle]
pub extern "system" fn Java_dev_gpui_lab_MainActivity_publishHostUpdate(
    _env: *mut jni::sys::JNIEnv,
    _class: jni::sys::jclass,
) {
    notify_host_update();
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

fn read_host_string(method: &jni::strings::JNIStr) -> Result<String, String> {
    with_env(|env| {
        let host = activity(env)?;
        let value = env
            .call_method(&host, method, jni_sig!("()Ljava/lang/String;"), &[])
            .and_then(|value| value.l())
            .map_err(|error| describe_jni_error(env, error))?;
        Ok(get_string(env, &value))
    })
}

pub fn benchmark_settings() -> Result<String, String> {
    read_host_string(jni_str!("getBenchmarkSettings"))
}

pub fn prepare_benchmark() -> Result<serde_json::Value, String> {
    let value = read_host_string(jni_str!("prepareBenchmark"))?;
    serde_json::from_str(&value).map_err(|error| error.to_string())
}

pub fn benchmark_device() -> Result<serde_json::Value, String> {
    let value = read_host_string(jni_str!("getBenchmarkDeviceInfo"))?;
    serde_json::from_str(&value).map_err(|error| error.to_string())
}

pub fn confirm_benchmark_device() -> Result<serde_json::Value, String> {
    let value = read_host_string(jni_str!("confirmBenchmarkDevice"))?;
    serde_json::from_str(&value).map_err(|error| error.to_string())
}

pub fn benchmark_upload_status() -> String {
    read_host_string(jni_str!("getBenchmarkUploadStatus")).unwrap_or_else(|error| error)
}

pub fn save_benchmark_report(report: &str) -> Result<(), String> {
    call_host_text(jni_str!("saveBenchmarkReport"), report)
}

pub fn finish_benchmark() -> Result<(), String> {
    call_host_void(jni_str!("finishBenchmark"))
}

pub fn open_leaderboard() -> Result<(), String> {
    call_host_void(jni_str!("openLeaderboard"))
}

fn call_host_void(method: &jni::strings::JNIStr) -> Result<(), String> {
    with_env(|env| {
        let host = activity(env)?;
        env.call_method(&host, method, jni_sig!("()V"), &[])
            .map(|_| ())
            .map_err(|error| describe_jni_error(env, error))
    })
}

fn call_host_text(method: &jni::strings::JNIStr, text: &str) -> Result<(), String> {
    with_env(|env| {
        let host = activity(env)?;
        let text = env
            .new_string(text)
            .map_err(|error| describe_jni_error(env, error))?;
        env.call_method(
            &host,
            method,
            jni_sig!("(Ljava/lang/String;)V"),
            &[JValue::Object(&text)],
        )
        .map(|_| ())
        .map_err(|error| describe_jni_error(env, error))
    })
}

pub fn benchmark_marker(marker: &str, case_index: usize) -> Result<(), String> {
    with_env(|env| {
        let host = activity(env)?;
        let marker = env
            .new_string(marker)
            .map_err(|error| describe_jni_error(env, error))?;
        env.call_method(
            &host,
            jni_str!("benchmarkMarker"),
            jni_sig!("(Ljava/lang/String;I)V"),
            &[JValue::Object(&marker), JValue::Int(case_index as i32)],
        )
        .map(|_| ())
        .map_err(|error| describe_jni_error(env, error))
    })
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
    format!("Android 调用失败 {error}")
}
