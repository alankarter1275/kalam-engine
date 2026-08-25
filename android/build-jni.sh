#!/usr/bin/env bash
# Build the Rust side straight into the library module's jniLibs, then
# check the two things that otherwise fail on a device rather than here.
#
# cargo-ndk's -o writes the `<abi>/lib*.so` layout Gradle expects, so
# nothing is copied by hand and the .so is never checked in.
set -euo pipefail

: "${ANDROID_NDK_HOME:?set ANDROID_NDK_HOME to e.g. \$HOME/Android/Sdk/ndk/<version>}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here/.."

profile="${1:-debug}"
out=android/chapbook/src/main/jniLibs
build=(build)
[ "$profile" = "release" ] && build=(build --release)

cargo ndk -t arm64-v8a -t x86_64 -P 24 -o "$out" "${build[@]}" -p chapbook-jni

readelf="$(ls "$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/*/bin/llvm-readelf | head -1)"
native=android/chapbook/src/main/kotlin/com/ophymx/chapbook/Native.kt
status=0

for so in "$out"/*/libchapbook_jni.so; do
    abi="$(basename "$(dirname "$so")")"
    printf '%-10s %s  %s bytes\n' "$abi" "$so" "$(stat -c%s "$so")"

    # A missing `#[link(name = ...)]` leaves the AndroidBitmap symbols
    # undefined with nothing in DT_NEEDED to resolve them. The build stays
    # green and System.loadLibrary is where it goes wrong.
    if ! "$readelf" -d "$so" | grep -q 'libjnigraphics\.so'; then
        echo "  !! libjnigraphics is not in DT_NEEDED — this .so will fail to load" >&2
        status=1
    fi

    # Every `external fun` Kotlin declares must have a symbol to bind to,
    # or the first call throws UnsatisfiedLinkError. Names drift silently:
    # nothing on either side references the other at compile time.
    exported="$("$readelf" --dyn-syms "$so" | grep -o 'Java_com_ophymx_chapbook_Native_[A-Za-z0-9_]*' | sort -u)"
    while read -r fn; do
        [ -z "$fn" ] && continue
        if ! grep -qx "Java_com_ophymx_chapbook_Native_$fn" <<<"$exported"; then
            echo "  !! Native.$fn is declared in Kotlin but not exported by the .so" >&2
            status=1
        fi
    done < <(grep -oP 'external fun \K[a-zA-Z0-9_]+' "$native")
done

[ $status -eq 0 ] && echo "jniLibs ok: linkage and symbols both check out"
exit $status
