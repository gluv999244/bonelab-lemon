use crate::{log, melonenv::paths, utils::{apk_asset_manager::{self, copy_melonloader_data, get_apk_modification_date}, perm_requester}};
use jni::{
    sys::{jint, JNI_VERSION_1_6},
    JNIEnv, JavaVM,
};
use lazy_static::lazy_static;
use std::{os::raw::c_void, panic::catch_unwind, sync::Mutex, ffi::{CString, c_char}};

const INVALID_JNI_VERSION: jint = 0;

lazy_static! {
    pub static ref JAVA_VM: Mutex<Option<JavaVM>> = Mutex::new(None);
}

#[no_mangle]
pub extern "system" fn JNI_OnLoad(vm: JavaVM, _: *mut c_void) -> jint {
    let mut vm_mutex = JAVA_VM.lock().unwrap();
    *vm_mutex = Some(vm);

    let mut env: JNIEnv = vm_mutex.as_ref().unwrap().get_env().expect("Cannot get reference to the JNIEnv");
    vm_mutex.as_ref().unwrap().attach_current_thread()
        .expect("Unable to attach current thread to the JVM");

    paths::cache_data_dir(&mut env);

    if !perm_requester::ensure_perms(&mut env) {
        apk_asset_manager::report(
            "All-files access was NOT granted. Open Settings > Apps > (this game) > Permissions and allow \
             'All files access' / 'Files and media', then start the game again.",
        );
    }

    let melonloader_path = paths::MELONLOADER_FOLDER.clone().0;
    if std::fs::metadata(&melonloader_path).is_err() {
        if let Err(e) = std::fs::create_dir_all(&melonloader_path) {
            // Without this folder nothing can work; say exactly why before giving up.
            let msg = format!(
                "Cannot create {}: {}. Storage permission missing, or a folder left over from an older \
                 install of this game is blocking it - delete /sdcard/MelonLoader/<package> and try again.",
                melonloader_path.display(),
                e
            );
            apk_asset_manager::report(&msg);
            panic!("{}", msg);
        }
    }

    crate::logging::logger::init().expect("Failed to initialize logger!");

    #[cfg(debug_assertions)]
    std::thread::spawn(|| unsafe {
        crate::dotnet_trace::redirect_stderr();
    });
    #[cfg(debug_assertions)]
    std::thread::spawn(|| unsafe {
        crate::dotnet_trace::redirect_stdout();
    });

    log!("JNI initialized!");
    
    let lemon_patch_date = match get_apk_modification_date(&mut env) {
        Ok(date) => date,
        Err(e) => {
            apk_asset_manager::clear_java_exception(&mut env);
            apk_asset_manager::report(&format!(
                "Could not read the patch date from the APK ({}); re-copying all files.",
                e
            ));
            apk_asset_manager::force_copy_date()
        }
    };

    match copy_melonloader_data(&mut env, lemon_patch_date) {
        Ok(_) => log!("APK assets copied!"),
        Err(e) => {
            apk_asset_manager::clear_java_exception(&mut env);
            apk_asset_manager::report(&format!(
                "Copying APK assets FAILED: {}. If this mentions 'Permission denied', delete the folder \
                 /sdcard/MelonLoader/<package> (SideQuest file manager) and launch again.",
                e
            ));
        }
    }

    catch_unwind(|| JNI_VERSION_1_6).unwrap_or(INVALID_JNI_VERSION)
}

pub unsafe fn get_raw_java_vm() -> *mut *const c_void {
    let mutex = JAVA_VM.lock().unwrap();
    let vm = mutex.as_ref().expect("JavaVM not initialized");
    vm.get_java_vm_pointer() as *mut *const c_void
}

#[no_mangle]
pub extern "C" fn melonloader_print_string(input: *const c_char) {
    unsafe {
        let tag = CString::new("MelonLoader").unwrap();
        android_liblog_sys::__android_log_write(4, tag.as_ptr(), input);
    }
}