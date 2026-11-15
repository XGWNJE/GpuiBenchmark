use gpui_mobile::android::jni::{activity, with_env};
use jni::{jni_sig, jni_str, objects::JValue};

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
