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
        "Android service build requires Java ${minAndroidJavaVersion}+ (tested through Java ${maxAndroidJavaVersion}). Found Java ${hostJavaMajorVersion}."
    )
}
val androidJavaTargetVersion = hostJavaMajorVersion.coerceAtMost(maxAndroidJavaVersion)
val androidJavaVersion = JavaVersion.toVersion(androidJavaTargetVersion)

android {
    namespace = "com.openai.codexd"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.openai.codexd"
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

    packaging {
        jniLibs.useLegacyPackaging = true
    }
}

val repoRoot = rootProject.projectDir.parentFile
val agentPlatformStubSdkZip = providers
    .gradleProperty("agentPlatformStubSdkZip")
    .orElse(providers.environmentVariable("ANDROID_AGENT_PLATFORM_STUB_SDK_ZIP"))
val extractedAgentPlatformJar = layout.buildDirectory.file(
    "generated/agent-platform/android-agent-platform-stub-sdk.jar"
)
val codexdTargets = mapOf(
    "arm64-v8a" to "aarch64-linux-android",
    "x86_64" to "x86_64-linux-android",
)
val codexdJniDir = layout.buildDirectory.dir("generated/codexd-jni")

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

val syncCodexdJniLibs = tasks.register<Sync>("syncCodexdJniLibs") {
    val outputDir = codexdJniDir
    into(outputDir)

    codexdTargets.forEach { (abi, triple) ->
        val binary = file("${repoRoot}/codex-rs/target/android/${triple}/release/codexd")
        from(binary) {
            into(abi)
            rename { "libcodexd.so" }
        }
    }

    doFirst {
        codexdTargets.forEach { (abi, triple) ->
            val binary = file("${repoRoot}/codex-rs/target/android/${triple}/release/codexd")
            if (!binary.exists()) {
                throw GradleException(
                    "Missing codexd binary for ${abi} at ${binary}. Run `just android-service-build` from the repo root."
                )
            }
        }
    }
}

android.sourceSets["main"].jniLibs.srcDir(codexdJniDir.get().asFile)

tasks.named("preBuild").configure {
    dependsOn(syncCodexdJniLibs)
    dependsOn(extractAgentPlatformStubSdk)
}

dependencies {
    implementation(project(":bridge"))
    compileOnly(files(extractedAgentPlatformJar))
}
