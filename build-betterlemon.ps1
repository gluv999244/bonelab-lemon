# Builds the patched libBootstrap.so (arm64 Android) and repacks it into a melon_data.zip
# that the LemonLoader installer picks up instead of downloading the official one.
#
# Expects (all under ..\..\..\tools relative to this file, i.e. C:\Users\maks\betterlemon\tools):
#   rustup/ + cargo/           isolated Rust toolchain with target aarch64-linux-android
#   android-ndk-r26d/          Android NDK
#   nethost/pkg/...            runtime.linux-bionic-arm64.microsoft.netcore.dotnetapphost 8.0.6 nupkg, extracted
# and the official melon_data.zip at ..\..\official\melon_data.zip

$ErrorActionPreference = 'Stop'
$root  = $PSScriptRoot
$tools = (Resolve-Path (Join-Path $root '..\..\..\tools')).Path
$official = (Resolve-Path (Join-Path $root '..\..\official\melon_data.zip')).Path
$out   = Join-Path (Resolve-Path (Join-Path $root '..\..\..')).Path 'output'

$env:RUSTUP_HOME = "$tools\rustup"
$env:CARGO_HOME  = "$tools\cargo"
$env:RUSTUP_AUTO_INSTALL = '0'
$ndkBin = "$tools\android-ndk-r26d\toolchains\llvm\prebuilt\windows-x86_64\bin"
$api = 21   # same API level as the official 0.6.5 build
$env:ANDROID_NDK_HOME = "$tools\android-ndk-r26d"
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "$ndkBin\aarch64-linux-android$api-clang.cmd"
$env:CC_aarch64_linux_android  = "$ndkBin\aarch64-linux-android$api-clang.cmd"
$env:CXX_aarch64_linux_android = "$ndkBin\aarch64-linux-android$api-clang++.cmd"
$env:AR_aarch64_linux_android  = "$ndkBin\llvm-ar.exe"
$env:NETHOST_LIB_DIR = "$tools\nethost\pkg\runtimes\linux-bionic-arm64\native"

Push-Location $root
try {
    # cargo writes progress to stderr; don't let Windows PowerShell 5.1 treat that as a terminating error
    $ErrorActionPreference = 'Continue'
    & "$tools\cargo\bin\cargo.exe" build --release -p Bootstrap --target aarch64-linux-android 2>&1 | ForEach-Object { "$_" }
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($code -ne 0) { throw "cargo build failed ($code)" }
} finally { Pop-Location }

$so = Join-Path $root 'target\aarch64-linux-android\release\libBootstrap.so'
& "$ndkBin\llvm-strip.exe" --strip-unneeded -o "$so.stripped" $so

New-Item -ItemType Directory -Force $out | Out-Null
$zipOut = Join-Path $out 'melon_data.zip'
Copy-Item $official $zipOut -Force

Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::Open($zipOut, 'Update')
try {
    $zip.GetEntry('native/libBootstrap.so').Delete()
    [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, "$so.stripped", 'native/libBootstrap.so')
} finally { $zip.Dispose() }

Get-Item $zipOut, "$so.stripped" | Select-Object FullName, Length
& "$ndkBin\llvm-readelf.exe" -d "$so.stripped" | Select-String 'NEEDED'
