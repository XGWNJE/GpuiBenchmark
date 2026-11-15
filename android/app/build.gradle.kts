plugins {
    id("com.android.application")
}

val labAbi = providers.gradleProperty("labAbi").orElse("arm64-v8a")
layout.buildDirectory.set(file("../../.build/gradle/app"))

android {
    namespace = "dev.gpui.lab"
    compileSdk = 36
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "dev.gpui.lab"
        minSdk = 26
        targetSdk = 36
        versionCode = 2
        versionName = "0.2.0"
        ndk.abiFilters += labAbi.get()
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Local testing APKs use Android's development signing key.
            signingConfig = signingConfigs.getByName("debug")
        }
        debug {
            isDebuggable = true
            isJniDebuggable = true
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets.getByName("main").jniLibs.directories.add("../../.build/jniLibs")
}
