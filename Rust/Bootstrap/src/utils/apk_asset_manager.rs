// Copies the MelonLoader / dotnet payload that the installer embedded in the APK out to disk.
//
// betterlemon changes vs upstream LemonLoader 0.6.X:
//  * No unwrap()/expect() on file or JNI operations. Upstream panicked inside JNI_OnLoad on any
//    I/O error (e.g. a leftover file from a previous install that the re-signed app can no longer
//    delete), which aborts the process: the game "closes instantly" after "JNI initialized!".
//  * AAssetManager_open() result is null-checked (upstream fed a null AAsset* to AAsset_read -> SIGSEGV).
//  * JNI local references are released per directory (upstream leaked several local refs per file,
//    thousands in total, all inside a single native frame).
//  * The asset manager is resolved once instead of once per file.
//  * lemon_patch_date.txt is opened directly and trimmed before parsing (a trailing newline made
//    the RFC3339 parse fail -> unwrap -> abort).
//  * Every failure is written to logcat (tag "MelonLoader") and to Latest-Bootstrap.log with the
//    exact path and OS error, so the next crash is diagnosable.

use chrono::{DateTime, Utc};
use jni::{
    objects::{GlobalRef, JObject, JObjectArray, JString, JValueGen},
    JNIEnv,
};
use ndk_sys::AAssetManager;
use std::{
    ffi::CString,
    fs,
    io::Write,
    path::Path,
    time::UNIX_EPOCH,
};

use crate::{errors::DynErr, melonenv::paths};

const PATCH_DATE_ASSET: &str = "lemon_patch_date.txt";

struct AssetCtx {
    java: GlobalRef,
    native: *mut AAssetManager,
}

/// Default used when the APK has no (readable) patch date: forces every file to be re-copied.
pub fn force_copy_date() -> DateTime<Utc> {
    DateTime::from_timestamp_nanos(i64::MAX)
}

pub fn report(msg: &str) {
    logcat(msg);
    let _ = crate::error!("{}", msg);
}

fn logcat(msg: &str) {
    let tag = CString::new("MelonLoader").unwrap();
    let msg = CString::new(msg.replace('\0', "")).unwrap();
    unsafe {
        android_liblog_sys::__android_log_write(6, tag.as_ptr(), msg.as_ptr());
    }
}

/// Clears a pending Java exception (if any) so further JNI calls are legal, and prints it to logcat.
pub fn clear_java_exception(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

fn current_activity<'l>(env: &mut JNIEnv<'l>) -> Result<JObject<'l>, DynErr> {
    let cls = env.find_class("com/unity3d/player/UnityPlayer")?;
    let activity = env
        .get_static_field(&cls, "currentActivity", "Landroid/app/Activity;")?
        .l()?;
    env.delete_local_ref(cls)?;
    if activity.is_null() {
        return Err("UnityPlayer.currentActivity is null".into());
    }
    Ok(activity)
}

impl AssetCtx {
    fn new(env: &mut JNIEnv) -> Result<Self, DynErr> {
        let activity = current_activity(env)?;
        let am = env
            .call_method(&activity, "getAssets", "()Landroid/content/res/AssetManager;", &[])?
            .l()?;
        env.delete_local_ref(activity)?;
        if am.is_null() {
            return Err("Activity.getAssets() returned null".into());
        }

        let java = env.new_global_ref(&am)?;
        env.delete_local_ref(am)?;

        let native = unsafe {
            ndk_sys::AAssetManager_fromJava(env.get_native_interface(), java.as_obj().as_raw())
        };
        if native.is_null() {
            return Err("AAssetManager_fromJava returned null".into());
        }

        Ok(Self { java, native })
    }
}

fn list_assets(env: &mut JNIEnv, ctx: &AssetCtx, path: &str) -> Result<Vec<String>, DynErr> {
    env.with_local_frame(32, |env| -> Result<Vec<String>, DynErr> {
        let jpath = env.new_string(path)?;
        let arr: JObjectArray = env
            .call_method(
                ctx.java.as_obj(),
                "list",
                "(Ljava/lang/String;)[Ljava/lang/String;",
                &[JValueGen::from(&jpath)],
            )?
            .l()?
            .into();

        if arr.is_null() {
            return Ok(Vec::new());
        }

        let len = env.get_array_length(&arr)?;
        let mut out = Vec::with_capacity(len.max(0) as usize);
        for i in 0..len {
            let jstr: JString = env.get_object_array_element(&arr, i)?.into();
            let s: String = env.get_string(&jstr)?.into();
            env.delete_local_ref(jstr)?;
            out.push(s);
        }
        Ok(out)
    })
}

/// Reads an asset fully into memory. Returns Ok(None) if the asset does not exist.
fn read_asset(ctx: &AssetCtx, asset_path: &str) -> Result<Option<Vec<u8>>, DynErr> {
    let cpath = CString::new(asset_path)?;
    let asset = unsafe {
        ndk_sys::AAssetManager_open(
            ctx.native,
            cpath.as_ptr(),
            ndk_sys::AASSET_MODE_STREAMING as i32,
        )
    };
    if asset.is_null() {
        return Ok(None);
    }

    let mut data = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    let result = loop {
        let n = unsafe {
            ndk_sys::AAsset_read(asset, buf.as_mut_ptr() as *mut std::ffi::c_void, buf.len())
        };
        if n < 0 {
            break Err(format!("AAsset_read failed for '{}'", asset_path).into());
        }
        if n == 0 {
            break Ok(Some(data));
        }
        data.extend_from_slice(&buf[..n as usize]);
    };

    unsafe { ndk_sys::AAsset_close(asset) };
    result
}

fn is_up_to_date(dest: &Path, patch_date: DateTime<Utc>) -> bool {
    let Ok(meta) = fs::metadata(dest) else { return false };
    let Ok(modified) = meta.modified() else { return false };
    let Ok(since) = modified.duration_since(UNIX_EPOCH) else { return false };
    (since.as_secs() as i64) > patch_date.timestamp()
}

fn copy_file(ctx: &AssetCtx, asset_path: &str, dest: &Path, patch_date: DateTime<Utc>) -> Result<(), DynErr> {
    if is_up_to_date(dest, patch_date) {
        return Ok(());
    }

    let data = match read_asset(ctx, asset_path)? {
        Some(d) => d,
        None => return Err(format!("asset '{}' could not be opened (missing or compressed?)", asset_path).into()),
    };

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create folder {}: {}", parent.display(), e))?;
    }

    if dest.exists() {
        if let Err(e) = fs::remove_file(dest) {
            // Usually a file left behind by an earlier install of the game, owned by the old app UID.
            // Try to overwrite it in place before giving up.
            report(&format!(
                "warning: could not delete old {} ({}), trying to overwrite it",
                dest.display(),
                e
            ));
        }
    }

    let mut out = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dest)
        .map_err(|e| format!("cannot write {}: {}", dest.display(), e))?;
    out.write_all(&data)
        .map_err(|e| format!("cannot write {}: {}", dest.display(), e))?;
    out.flush()?;
    Ok(())
}

fn copy_tree(
    env: &mut JNIEnv,
    ctx: &AssetCtx,
    asset_path: &str,
    dest_root: &Path,
    patch_date: DateTime<Utc>,
    failures: &mut usize,
) {
    let children = match list_assets(env, ctx, asset_path) {
        Ok(c) => c,
        Err(e) => {
            clear_java_exception(env);
            report(&format!("Failed to list APK asset folder '{}': {}", asset_path, e));
            *failures += 1;
            return;
        }
    };

    let dest = dest_root.join(asset_path);

    // AssetManager.list() returns an empty array for plain files.
    if children.is_empty() {
        if let Err(e) = copy_file(ctx, asset_path, &dest, patch_date) {
            report(&format!("Failed to copy '{}': {}", asset_path, e));
            *failures += 1;
        }
        return;
    }

    if let Err(e) = fs::create_dir_all(&dest) {
        report(&format!("Failed to create folder {}: {}", dest.display(), e));
        *failures += 1;
        return;
    }

    for child in children {
        let child_path = format!("{}/{}", asset_path, child);
        copy_tree(env, ctx, &child_path, dest_root, patch_date, failures);
    }
}

pub fn copy_melonloader_data(env: &mut JNIEnv, patch_date: DateTime<Utc>) -> Result<bool, DynErr> {
    let ctx = AssetCtx::new(env).map_err(|e| {
        clear_java_exception(env);
        e
    })?;

    let base = paths::BASE_DIR.clone().0;
    let internal = paths::get_internal_data_path()?;

    let mut failures = 0usize;
    copy_tree(env, &ctx, "MelonLoader", &base, patch_date, &mut failures);
    copy_tree(env, &ctx, "dotnet", &internal, patch_date, &mut failures);

    if failures > 0 {
        return Err(format!(
            "{} item(s) failed to copy (details above / in logcat tag 'MelonLoader')",
            failures
        )
        .into());
    }
    Ok(true)
}

pub fn get_apk_modification_date(env: &mut JNIEnv) -> Result<DateTime<Utc>, DynErr> {
    let ctx = AssetCtx::new(env).map_err(|e| {
        clear_java_exception(env);
        e
    })?;

    let Some(bytes) = read_asset(&ctx, PATCH_DATE_ASSET)? else {
        return Ok(force_copy_date());
    };

    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let date = DateTime::parse_from_rfc3339(text)
        .map_err(|e| format!("bad {} contents '{}': {}", PATCH_DATE_ASSET, text, e))?;
    Ok(date.with_timezone(&Utc))
}
