use build_target::Os;

// betterlemon: no network download at build time. Set NETHOST_LIB_DIR to a folder containing
// libnethost (e.g. runtimes/linux-bionic-arm64/native from the
// runtime.linux-bionic-arm64.microsoft.netcore.dotnetapphost nuget package).
// libBootstrap.so does not actually end up depending on libnethost; it only has to be findable
// at link time.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=NETHOST_LIB_DIR");
    match std::env::var("NETHOST_LIB_DIR") {
        Ok(dir) => cargo_emit::rustc_link_search!(dir),
        Err(_) => println!("cargo:warning=NETHOST_LIB_DIR not set; linking may fail to find libnethost"),
    }

    // NOTE: for some reason we need the rustc argument here, but the link attribute in lib.rs for other os.
    // For more information see https://github.com/OpenByteDev/netcorehost/issues/2.
    if build_target::target_os() == Ok(Os::Windows) {
        cargo_emit::rustc_link_lib!("libnethost");
    }

    Ok(())
}
