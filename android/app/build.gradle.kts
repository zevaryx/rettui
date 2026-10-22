plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// The app's version is rettui's, from Cargo.toml; CI numbers each build.
val rettuiVersion = Regex("""(?m)^version = "([^"]+)"""")
    .find(rootDir.resolve("../Cargo.toml").readText())?.groupValues?.get(1) ?: "0.0.0"

android {
    namespace = "org.zevaryx.rettui"
    compileSdk = 35
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "org.zevaryx.rettui"
        minSdk = 26
        targetSdk = 35
        versionCode = System.getenv("RETTUI_VERSION_CODE")?.toIntOrNull() ?: 1
        versionName = rettuiVersion
    }

    signingConfigs {
        // One key for every build (RETTUI_KEYSTORE), so a new APK installs
        // over the last one; without it, the machine's own debug key.
        fun env(name: String) = System.getenv(name)?.takeIf { it.isNotBlank() }
        env("RETTUI_KEYSTORE")?.let { path ->
            create("shared") {
                storeFile = file(path)
                storePassword = env("RETTUI_KEYSTORE_PASSWORD") ?: "android"
                keyAlias = env("RETTUI_KEY_ALIAS") ?: "androiddebugkey"
                keyPassword = env("RETTUI_KEY_PASSWORD") ?: storePassword
            }
        }
    }

    buildTypes {
        debug {
            signingConfigs.findByName("shared")?.let { signingConfig = it }
        }
    }

    packaging {
        // rettui runs as a program, so it has to be a file on disk.
        jniLibs.useLegacyPackaging = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}
