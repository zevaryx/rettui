plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// The app's version is rettui's, from Cargo.toml. CI numbers each build
// (RETTUI_VERSION_CODE) by the minutes since late 2023, so every build,
// test or release, installs over any earlier one.
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
            // Installs beside the release app, so a test build never
            // replaces it (or its identity).
            applicationIdSuffix = ".debug"
            versionNameSuffix = "-debug"
            signingConfigs.findByName("shared")?.let { signingConfig = it }
        }
        // Releases: not debuggable (adb can't read the app's files, the
        // identity among them), signed with the same key as test builds,
        // so either installs over the other. With no key, packaging one
        // stops (see below): never with the debug key, which anyone can make.
        release {
            signingConfig = signingConfigs.findByName("shared")
            isMinifyEnabled = false
        }
    }

    packaging {
        // rettui compressed in the APK (half the download), and unpacked
        // when it's installed.
        jniLibs.useLegacyPackaging = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

// Stop before a release APK is made with no key to sign it with. Checked when
// the tasks to run are known, so building a debug APK, or compiling the
// release code, still works without one.
gradle.taskGraph.whenReady {
    val making = setOf("assembleRelease", "packageRelease", "bundleRelease", "packageReleaseBundle", "packageReleaseUniversalApk", "signReleaseBundle", "installRelease")
    val packaging = allTasks.any { it.project == project && it.name in making }
    if (packaging && android.signingConfigs.findByName("shared") == null) {
        throw GradleException(
            "No release signing key. Set RETTUI_KEYSTORE to a keystore file (and RETTUI_KEYSTORE_PASSWORD, " +
                "RETTUI_KEY_ALIAS and RETTUI_KEY_PASSWORD if they aren't the defaults: see wiki/Android.md for making one). " +
                "A release is never signed with the Android debug key. For a test build, run assembleDebug instead.",
        )
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    implementation(project(":btleplug"))
}
