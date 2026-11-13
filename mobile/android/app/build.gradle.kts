plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Rust core, sherpa-onnx wake libraries and UniFFI Kotlin bindings, built
// by scripts/build-core.sh into ../build-core (ignored).
val core = rootProject.layout.projectDirectory.dir("build-core")
val repo = rootProject.layout.projectDirectory.dir("../..")

// The desktop's wake models (keyword spotter, plus the Whisper backup model
// unless built with -PnotdotWhisper=false), copied into generated assets so
// nothing is duplicated in the source tree.
val whisper = (findProperty("notdotWhisper") as String?)?.toBoolean() ?: true
val wakeAssets = layout.buildDirectory.dir("generated/wake-assets")
val copyWakeModels by tasks.registering(Copy::class) {
    from(repo.dir("src-tauri/resources/wake")) {
        include("phonetic/**", "bpe.model", "encoder.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt", "LICENSE", "NOTICE.md")
        if (whisper) include("ggml-tiny.en-q5_1.bin", "WHISPER-LICENSE")
    }
    into(wakeAssets.map { it.dir("wake") })
}
// The desktop's wake test clips, for instrumented tests only.
val wakeFixtures = layout.buildDirectory.dir("generated/wake-fixtures")
val copyWakeFixtures by tasks.registering(Copy::class) {
    from(repo.dir("src-tauri/tests/fixtures")) {
        include("hey-ava.wav", "hey-ava-uk.wav", "hey-ava-us.wav", "ava.wav", "negative.wav")
    }
    into(wakeFixtures.map { it.dir("fixtures") })
}
// Generate the launcher asset from the canonical desktop icon, so a
// future brand update cannot silently leave mobile behind.
val iconResources = layout.buildDirectory.dir("generated/icon-resources")
val copyNotdotIcon by tasks.registering(Copy::class) {
    from(repo.file("src-tauri/icons/icon.png")) { rename { "notdot_mark.png" } }
    into(iconResources.map { it.dir("drawable-nodpi") })
}
tasks.named("preBuild") { dependsOn(copyWakeModels, copyWakeFixtures, copyNotdotIcon) }

android {
    namespace = "dev.notdot.mobile"
    compileSdk = 36
    defaultConfig {
        applicationId = "dev.notdot.mobile.prototype"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk { abiFilters += "arm64-v8a" }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    androidResources {
        // The quantised model does not compress; store it as is.
        noCompress += "bin"
    }
    sourceSets {
        getByName("main") {
            java.srcDir(core.dir("kotlin"))
            jniLibs.srcDir(core.dir("jniLibs"))
            assets.srcDir(wakeAssets)
            res.srcDir(iconResources)
        }
        getByName("androidTest") {
            // Golden orb frames shared with iOS and the desktop contract.
            assets.srcDir(repo.dir("crates/notdot-orb/fixtures"))
            assets.srcDir(wakeFixtures)
        }
    }
    testOptions {
        unitTests.all {
            // JVM unit tests load the host build of the same Rust library.
            it.systemProperty("jna.library.path", core.dir("host").asFile.absolutePath)
            it.systemProperty("notdot.repo", repo.asFile.absolutePath)
        }
    }
    packaging {
        jniLibs { useLegacyPackaging = true }
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.03.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.11.0")
    implementation("androidx.core:core-ktx:1.17.0")
    implementation("net.java.dev.jna:jna:5.19.1@aar")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    testImplementation("junit:junit:4.13.2")
    testImplementation("net.java.dev.jna:jna:5.19.1")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
}
