//! JNI entry used by `app.notia.mobile.NotiaNative`.

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::jstring;

use crate::handle;

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_notia_mobile_NotiaNative_call(
    mut env: JNIEnv,
    _class: JClass,
    request: JString,
) -> jstring {
    let request = env
        .get_string(&request)
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_default();
    let response = handle(&request);
    match env.new_string(response) {
        Ok(text) => text.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}
