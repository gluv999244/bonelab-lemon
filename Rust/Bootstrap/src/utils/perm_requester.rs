use jni::{
    objects::{JObject, JValueGen},
    sys::jint,
    JNIEnv,
};

use crate::melonenv::paths;

// this module is mostly based on the libmainloader from sc2ad
// https://github.com/sc2ad/LibMainLoader/blob/master/src/fileutils.cpp

pub fn ensure_perms(env: &mut JNIEnv) -> bool {
    let current_activity_obj: JObject = match env
        .get_static_field("com/unity3d/player/UnityPlayer", "currentActivity", "Landroid/app/Activity;")
        .and_then(|v| v.l())
    {
        Ok(obj) => obj,
        Err(e) => {
            clear_exception(env);
            direct_log(&format!("Failed to get UnityPlayer.currentActivity: {}", e));
            return false;
        }
    };

    if current_activity_obj.is_null() {
        direct_log("currentActivity is null");
        clear_exception(env);
        return false;
    }

    // causes the app to freeze for some reason
    /* if ensure_perms_with_unity(env, &current_activity_obj) {
        return true;
    }

    direct_log("Failed to request permissions with Unity. Check System.err.");
    if env.exception_check().unwrap() {
        env.exception_describe().unwrap();
        env.exception_clear().unwrap();
    } */

    if ensure_perms_with_package_name(env, &current_activity_obj) {
        return true;
    }

    direct_log("Failed to request permissions with intent. Check System.err.");
    clear_exception(env);

    false
}

fn clear_exception(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

fn sdk_int(env: &mut JNIEnv) -> i32 {
    let r = env
        .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
        .and_then(|v| v.i());
    match r {
        Ok(v) => v,
        Err(_) => {
            clear_exception(env);
            0
        }
    }
}

fn is_external_storage_manager(env: &mut JNIEnv) -> Option<bool> {
    let r = env
        .call_static_method("android/os/Environment", "isExternalStorageManager", "()Z", &[])
        .and_then(|v| v.z());
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            clear_exception(env);
            direct_log(&format!("isExternalStorageManager failed: {}", e));
            None
        }
    }
}

fn open_all_files_settings(env: &mut JNIEnv, current_activity_obj: &JObject) -> Result<(), String> {
    let package = paths::get_package_name().map_err(|e| e.to_string())?;
    let r = env.with_local_frame(16, |env| -> Result<(), jni::errors::Error> {
        let action_name =
            env.new_string("android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION")?;
        let package_name = env.new_string(format!("package:{}", package))?;
        let uri = env
            .call_static_method(
                "android/net/Uri",
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValueGen::from(&package_name)],
            )?
            .l()?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValueGen::from(&action_name), JValueGen::from(&uri)],
        )?;
        env.call_method(
            current_activity_obj,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValueGen::from(&intent)],
        )?;
        Ok(())
    });
    r.map_err(|e| {
        clear_exception(env);
        e.to_string()
    })
}

pub fn ensure_perms_with_package_name(env: &mut JNIEnv, current_activity_obj: &JObject) -> bool {
    // Upstream opened the settings screen up to 3 times, waited 5s each, then carried on as if
    // permission had been granted. On current Quest builds the user often needs longer than that,
    // so MelonLoader then ran without storage access and died. Instead: open the screen once and
    // poll until access is granted (or a generous timeout passes).
    const POLL_MS: u64 = 500;
    const TIMEOUT_MS: u64 = 120_000;

    if sdk_int(env) < 30 {
        // Pre-Android 11: legacy storage permissions apply, nothing to request here.
        return true;
    }

    match is_external_storage_manager(env) {
        Some(true) => return true,
        Some(false) => {}
        None => return false,
    }

    direct_log("Requesting 'All files access' - please allow it, then return to the game.");
    if let Err(e) = open_all_files_settings(env, current_activity_obj) {
        direct_log(&format!("Failed to open the All files access screen: {}", e));
        return false;
    }

    let mut waited = 0;
    while waited < TIMEOUT_MS {
        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
        waited += POLL_MS;
        if is_external_storage_manager(env) == Some(true) {
            direct_log("All files access granted.");
            return true;
        }
    }

    direct_log("Timed out waiting for All files access.");
    false
}

pub fn ensure_perms_with_unity(env: &mut JNIEnv, current_activity_obj: &JObject) -> bool {
    const PERMS_TO_REQUEST: [&str; 2] = [
        "android.permission.WRITE_EXTERNAL_STORAGE",
        "android.permission.READ_EXTERNAL_STORAGE",
    ];

    let unity_perms_class = env
        .find_class("com/unity3d/player/UnityPermissions")
        .expect("Failed to find class com/unity3d/player/UnityPermissions");

    let wait_perm_class = env
        .find_class("com/unity3d/player/UnityPermissions$ModalWaitForPermissionResponse")
        .expect("Failed to find class com/unity3d/player/UnityPermissions$ModalWaitForPermissionResponse");

    let wait_perm = env
        .new_object(wait_perm_class, "()V", &[])
        .expect("Failed to create new ModalWaitForPermissionResponse");

    let permissions_array = env
        .new_object_array(2, "java/lang/String", JObject::null())
        .expect("Failed to create new String array");

    for (i, perm) in PERMS_TO_REQUEST.iter().enumerate() {
        let perm_str = env.new_string(perm).expect("Failed to create new string");

        env.set_object_array_element(&permissions_array, i as jint, perm_str)
            .expect("Failed to set object array element");
    }

    env.call_static_method(
        unity_perms_class,
        "requestUserPermissions",
        "(Landroid/app/Activity;[Ljava/lang/String;Lcom/unity3d/player/IPermissionRequestCallbacks;)V",
        &[JValueGen::from(current_activity_obj), JValueGen::from(&permissions_array), JValueGen::from(&wait_perm)]
    ).expect("Failed to call requestUserPermissions");

    if env.exception_check().unwrap() {
        return false;
    }

    env.call_method(wait_perm, "waitForResponse", "()V", &[])
        .expect("Failed to call waitForResponse");

    !env.exception_check().unwrap()
}

fn direct_log(msg: &str) {
    use std::ffi::CString;
    let tag = CString::new("MelonLoader").unwrap();
    let msg = CString::new(msg).unwrap();
    unsafe {
        android_liblog_sys::__android_log_write(4, tag.as_ptr(), msg.as_ptr());
    }
}