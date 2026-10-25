#!/usr/bin/env bash
# Cross-compile the Rust log reader (tools/tel-jni) for the phone and drop the
# .so into app/src/main/jniLibs. Needs: rustup target aarch64-linux-android,
# cargo-ndk, and an Android NDK (set ANDROID_NDK_HOME, or it uses the default below).
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
: "${ANDROID_NDK_HOME:=$HOME/Android/ndk-telemetry/android-ndk-r29}"
export ANDROID_NDK_HOME
cd ..   # software/ (the Cargo workspace)
cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build --release -p tel-jni
