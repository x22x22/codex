package com.openai.codex.bridge;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

public final class HostedCodexConfig {
    public static final String ANDROID_HTTP_PROVIDER_ID = "android-openai-http";
    private static final String AGENTS_FILENAME = "AGENTS.md";

    private HostedCodexConfig() {}

    public static void write(File codexHome, String baseUrl) throws IOException {
        ensureCodexHome(codexHome);
        installAgentsFile(codexHome);

        String escapedBaseUrl = baseUrl
                .replace("\\", "\\\\")
                .replace("\"", "\\\"");
        String configToml = "model_provider = \"" + ANDROID_HTTP_PROVIDER_ID + "\"\n\n"
                + "[model_providers." + ANDROID_HTTP_PROVIDER_ID + "]\n"
                + "name = \"Android OpenAI HTTP\"\n"
                + "base_url = \"" + escapedBaseUrl + "\"\n"
                + "wire_api = \"responses\"\n"
                + "requires_openai_auth = true\n"
                + "supports_websockets = false\n";
        Files.write(
                new File(codexHome, "config.toml").toPath(),
                configToml.getBytes(StandardCharsets.UTF_8));
    }

    public static void installAgentsFile(File codexHome) throws IOException {
        ensureCodexHome(codexHome);
        Files.write(
                new File(codexHome, AGENTS_FILENAME).toPath(),
                buildAgentsMarkdown().getBytes(StandardCharsets.UTF_8));
    }

    static String buildAgentsMarkdown() {
        return """
                # Android Agent/Genie Runtime Notes

                This Codex runtime is operating on an Android device through the Agent Platform.

                ## If you are the Agent

                - The user interacts only with the Agent.
                - Plan the work, choose the target package or packages, and start one Genie session per target app that needs to be driven.
                - Delegate objectives, not tool choices. Tell each Genie what outcome it must achieve in its paired app and let the Genie choose its own tools.
                - Answer Genie questions directly when you can. If the answer depends on user intent or missing constraints, ask the user.
                - Keep auth, upstream access, and any internet-facing model traffic on the Agent side.

                ## If you are a Genie

                - You are paired with exactly one target app sandbox for this session.
                - Solve the delegated objective inside that sandbox by using the normal Codex tool path and the Android tools that are available on-device.
                - Ask the Agent a concise free-form question only when you are blocked on missing intent, missing constraints, or a framework-owned action.
                - Do not assume you can reach the internet directly. Model and auth traffic are Agent-owned.
                - Do not rely on direct cross-app `bindService(...)` or raw local sockets to reach the Agent. Use the framework-managed session bridge.

                ## Shell and device tooling

                - Prefer standard Android shell tools first: `cmd`, `am`, `pm`, `input`, `uiautomator`, `dumpsys`, `wm`, `settings`, `content`, `logcat`.
                - Do not assume desktop/Linux extras such as `python3`, GNU `date -d`, or other non-stock userland tools are present.
                - When a command affects app launch or user-visible state, prefer an explicit `--user 0` when the tool supports it.
                - Keep temporary artifacts in app-private storage such as the current app `files/` or `cache/` directories, or under `$CODEX_HOME`. Do not rely on shared storage.

                ## UI inspection and files

                - In self-target Genie mode, prefer `uiautomator dump /proc/self/fd/1` or `uiautomator dump /dev/stdout` when stdout capture is acceptable.
                - Plain `uiautomator dump` writes to the app-private dump directory.
                - Explicit shared-storage targets such as `/sdcard/...` are redirected back into app-private storage in self-target mode.
                - Do not assume `/sdcard` or `/data/local/tmp` are readable or writable from the paired app sandbox.

                ## Presentation semantics

                - Detached launch, shown-detached, and attached are different states.
                - `targetDetached=true` means the target is still detached even if it is visible in a detached or mirrored presentation.
                - If the task says the app should be visible to the user, do not claim success until the target is attached unless the task explicitly allows detached presentation.
                - Treat framework session state as the source of truth for presentation state.

                ## Working style

                - Prefer solving tasks with normal shell/tool use before reverse-engineering APK contents.
                - When you need to ask a question, make it specific and short so the Agent can either answer directly or escalate it to the user.
                """;
    }

    private static void ensureCodexHome(File codexHome) throws IOException {
        if (!codexHome.isDirectory() && !codexHome.mkdirs()) {
            throw new IOException("failed to create codex home at " + codexHome.getAbsolutePath());
        }
    }
}
