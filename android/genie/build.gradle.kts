import org.gradle.api.GradleException
import org.gradle.api.tasks.Sync

plugins {
    id("com.android.application")
}

val minAndroidJavaVersion = 17
val maxAndroidJavaVersion = 21
val hostJavaMajorVersion = JavaVersion.current().majorVersion.toIntOrNull()
    ?: throw GradleException("Unable to determine Java version from ${JavaVersion.current()}.")
if (hostJavaMajorVersion < minAndroidJavaVersion) {
    throw GradleException(
        "Android Genie build requires Java ${minAndroidJavaVersion}+ (tested through Java ${maxAndroidJavaVersion}). Found Java ${hostJavaMajorVersion}."
    )
}
val androidJavaTargetVersion = hostJavaMajorVersion.coerceAtMost(maxAndroidJavaVersion)
val androidJavaVersion = JavaVersion.toVersion(androidJavaTargetVersion)
val agentPlatformStubSdkZip = providers
    .gradleProperty("agentPlatformStubSdkZip")
    .orElse(providers.environmentVariable("ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP"))
val extractedAgentPlatformJar = layout.buildDirectory.file(
    "generated/agent-platform/android-agent-platform-stub-sdk.jar"
)

android {
    namespace = "com.openai.codex.genie"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.openai.codex.genie"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    compileOptions {
        sourceCompatibility = androidJavaVersion
        targetCompatibility = androidJavaVersion
    }
}

val extractAgentPlatformStubSdk = tasks.register<Sync>("extractAgentPlatformStubSdk") {
    val sdkZip = agentPlatformStubSdkZip.orNull
        ?: throw GradleException(
            "Set ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP or -PagentPlatformStubSdkZip to the Android Agent Platform stub SDK zip."
        )
    val outputDir = extractedAgentPlatformJar.get().asFile.parentFile
    from(zipTree(sdkZip)) {
        include("payloads/compile_only/android-agent-platform-stub-sdk.jar")
        eachFile { path = name }
        includeEmptyDirs = false
    }
    into(outputDir)
}

tasks.named("preBuild").configure {
    dependsOn(extractAgentPlatformStubSdk)
}

dependencies {
    implementation(project(":bridge"))
    compileOnly(files(extractedAgentPlatformJar))
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20240303")
}
