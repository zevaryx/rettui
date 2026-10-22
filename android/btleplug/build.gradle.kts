// btleplug's Java classes (Bluetooth RNodes), which its Rust half calls.
// build-rust.sh copies them into java/ from the btleplug crate Cargo.lock
// has, so the two halves match.
plugins {
    id("com.android.library")
}

android {
    namespace = "com.nonpolynomial.btleplug.android"
    compileSdk = 35

    defaultConfig {
        minSdk = 26
    }

    sourceSets["main"].java.srcDir("java")

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
