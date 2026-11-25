plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// versionName comes from the workspace version in ../Cargo.toml, so bumping
// the release version there is the only edit a release needs. Keep that value
// numeric (one to four dot-separated integers); Android rejects suffixes such
// as -rc.1. versionCode is deliberately not derived from it: the Play Store
// needs a plain integer that grows with every upload, so it moves on its own.
val notiaVersion: String = run {
    val manifest = rootProject.projectDir.parentFile.resolve("Cargo.toml")
    require(manifest.isFile) { "Missing ${manifest.path}; run Gradle from the Notia checkout." }
    var inWorkspacePackage = false
    var found: String? = null
    manifest.readLines().forEach { raw ->
        val line = raw.trim()
        if (line.startsWith("[")) {
            inWorkspacePackage = line == "[workspace.package]"
        } else if (inWorkspacePackage && found == null) {
            found = Regex("^version\\s*=\\s*\"([^\"]+)\"")
                .find(line)
                ?.groupValues
                ?.get(1)
        }
    }
    found ?: error("No [workspace.package] version found in ${manifest.path}.")
}

android {
    namespace = "app.notia.mobile"
    compileSdk = 35

    defaultConfig {
        applicationId = "app.notia.mobile"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = notiaVersion
        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
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

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
    }
}

tasks.register<Exec>("buildNotiaRust") {
    group = "build"
    description = "Compile the Notia vault library for Android."
    workingDir = rootProject.projectDir.parentFile
    val shell =
        if (System.getProperty("os.name").lowercase().contains("windows")) {
            val gitBash = File("C:/Program Files/Git/bin/bash.exe")
            if (gitBash.isFile) gitBash.absolutePath else "bash"
        } else {
            "bash"
        }
    commandLine(shell, "scripts/package-android.sh")
    val localAppData = System.getenv("LOCALAPPDATA")
    if (localAppData != null) {
        environment("LOCALAPPDATA", localAppData)
    }
    val sdk =
        System.getenv("ANDROID_HOME")
            ?: System.getenv("ANDROID_SDK_ROOT")
            ?: localAppData?.let { "$it\\Android\\Sdk" }
    if (sdk != null) {
        environment("ANDROID_HOME", sdk)
        if (System.getenv("ANDROID_NDK_HOME") != null) {
            environment("ANDROID_NDK_HOME", System.getenv("ANDROID_NDK_HOME")!!)
        } else {
            val ndkRoot = file("$sdk/ndk")
            if (ndkRoot.isDirectory) {
                ndkRoot
                    .listFiles()
                    ?.filter { it.isDirectory }
                    ?.maxByOrNull { it.name }
                    ?.let { environment("ANDROID_NDK_HOME", it.absolutePath) }
            }
        }
    }
}

tasks.named("preBuild").configure {
    dependsOn("buildNotiaRust")
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
