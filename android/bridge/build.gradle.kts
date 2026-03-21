import org.gradle.api.GradleException

plugins {
    id("com.android.library")
}

val minAndroidJavaVersion = 17
val maxAndroidJavaVersion = 21
val hostJavaMajorVersion = JavaVersion.current().majorVersion.toIntOrNull()
    ?: throw GradleException("Unable to determine Java version from ${JavaVersion.current()}.")
if (hostJavaMajorVersion < minAndroidJavaVersion) {
    throw GradleException(
        "Android bridge build requires Java ${minAndroidJavaVersion}+ (tested through Java ${maxAndroidJavaVersion}). Found Java ${hostJavaMajorVersion}."
    )
}
val androidJavaTargetVersion = hostJavaMajorVersion.coerceAtMost(maxAndroidJavaVersion)
val androidJavaVersion = JavaVersion.toVersion(androidJavaTargetVersion)

android {
    namespace = "com.openai.codex.bridge"
    compileSdk = 34

    defaultConfig {
        minSdk = 26
    }

    compileOptions {
        sourceCompatibility = androidJavaVersion
        targetCompatibility = androidJavaVersion
    }
}

dependencies {
    testImplementation("junit:junit:4.13.2")
}
