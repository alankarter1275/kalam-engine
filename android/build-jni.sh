#!/usr/bin/env bash
# Build the Rust side straight into the library module's jniLibs.
#
# cargo-ndk's -o writes the `<abi>/lib*.so` layout Gradle expects, so
# nothing is copied by hand and the .so is never checked in.
set -euo pipefail

: "${ANDROID_NDK_HOME:?set ANDROID_NDK_HOME to e.g. \$HOME/Android/Sdk/ndk/<version>}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here/.."

profile="${1:-debug}"
flags=(-t arm64-v8a -t x86_64 -P 24 -o android/chapbook/src/main/jniLibs)
[ "$profile" = "release" ] && build=(build --release) || build=(build)

cargo ndk "${flags[@]}" "${build[@]}" -p chapbook-jni
find android/chapbook/src/main/jniLibs -name '*.so' -printf '%p  %sB\n'
