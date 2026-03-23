import org.gradle.api.tasks.Exec
import org.gradle.api.tasks.PathSensitivity

plugins {
    id("com.android.application") version "9.0.0" apply false
}

val repoRoot = rootProject.projectDir.parentFile
val skipAndroidLto = providers
    .gradleProperty("codexAndroidSkipLto")
    .orElse(providers.environmentVariable("CODEX_ANDROID_SKIP_LTO"))
    .orNull
    ?.let { it == "1" || it.equals("true", ignoreCase = true) }
    ?: false
val codexCargoProfileDir = if (skipAndroidLto) "android-release-no-lto" else "release"
val codexTargets = mapOf(
    "arm64-v8a" to "aarch64-linux-android",
    "x86_64" to "x86_64-linux-android",
)

tasks.register<Exec>("buildCodexCliNative") {
    group = "build"
    description = "Build the Android codex binary packaged into the Agent and Genie APKs."
    workingDir = repoRoot
    commandLine("just", "android-build")
    if (skipAndroidLto) {
        environment("CODEX_ANDROID_SKIP_LTO", "1")
    }
    inputs.files(
        fileTree(repoRoot.resolve("codex-rs")) {
            exclude("target/**")
        },
    ).withPathSensitivity(PathSensitivity.RELATIVE)
    inputs.file(repoRoot.resolve("justfile"))
        .withPathSensitivity(PathSensitivity.RELATIVE)
    outputs.files(
        codexTargets.values.map { triple ->
            repoRoot.resolve("codex-rs/target/android/${triple}/${codexCargoProfileDir}/codex")
        },
    )
}
