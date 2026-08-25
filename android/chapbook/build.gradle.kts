plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.ophymx.chapbook"
    compileSdk = 36

    defaultConfig {
        minSdk = 24
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    // cargo-ndk writes straight into this layout, so the Rust build output
    // is the source of truth and nothing is copied by hand. See
    // `../tools/build-jni.sh`.
    sourceSets["main"].jniLibs.srcDirs("src/main/jniLibs")
}
