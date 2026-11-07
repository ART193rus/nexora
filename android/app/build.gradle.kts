import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.nexora.mobile"
    compileSdk = 36
    defaultConfig {
        applicationId = "com.nexora.mobile"
        minSdk = 29
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }
    // Релизная подпись для домашнего пользования: включается ключом -PlocalSign, ключ лежит в keystore/ и в Git не попадает.
    val ksFile = rootProject.file("keystore.properties")
    if (ksFile.exists() && project.hasProperty("localSign")) {
        val ks = Properties().apply { ksFile.inputStream().use { load(it) } }
        signingConfigs.create("local") {
            storeFile = rootProject.file(ks.getProperty("storeFile"))
            storePassword = ks.getProperty("storePassword")
            keyAlias = ks.getProperty("keyAlias")
            keyPassword = ks.getProperty("keyPassword")
        }
    }
    buildTypes {
        // Тестовая release-сборка подписывается отладочным ключом; для публикации нужен собственный ключ владельца.
        release { signingConfig = signingConfigs.findByName("local") ?: signingConfigs.getByName("debug"); isMinifyEnabled = false; // R8 ломал внутренности сканера QR (Google ML Kit); размер приложения не критичен
             proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro") }
    }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    buildFeatures { compose = true }
    lint { checkReleaseBuilds = false; disable += "PropertyEscape" }
    testOptions { unitTests.isReturnDefaultValues = true }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2025.09.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.core:core-ktx:1.16.0")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    implementation("com.google.android.gms:play-services-code-scanner:16.1.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20240303")
}
