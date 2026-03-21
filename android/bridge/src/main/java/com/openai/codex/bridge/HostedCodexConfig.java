package com.openai.codex.bridge;

import android.content.Context;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

public final class HostedCodexConfig {
    public static final String ANDROID_HTTP_PROVIDER_ID = "android-openai-http";
    public static final String AGENTS_FILENAME = "AGENTS.md";
    private static final String BUNDLED_AGENTS_ASSET_PATH = AGENTS_FILENAME;

    private HostedCodexConfig() {}

    public static void write(Context context, File codexHome, String baseUrl) throws IOException {
        ensureCodexHome(codexHome);
        installBundledAgentsFile(context, codexHome);

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

    public static void installBundledAgentsFile(Context context, File codexHome) throws IOException {
        installAgentsFile(codexHome, readBundledAgentsMarkdown(context));
    }

    public static void installAgentsFile(File codexHome, String agentsMarkdown) throws IOException {
        ensureCodexHome(codexHome);
        Files.write(
                new File(codexHome, AGENTS_FILENAME).toPath(),
                agentsMarkdown.getBytes(StandardCharsets.UTF_8));
    }

    public static String readBundledAgentsMarkdown(Context context) throws IOException {
        try (InputStream inputStream = context.getAssets().open(BUNDLED_AGENTS_ASSET_PATH)) {
            return new String(inputStream.readAllBytes(), StandardCharsets.UTF_8);
        }
    }

    public static String readInstalledAgentsMarkdown(File codexHome) throws IOException {
        return new String(
                Files.readAllBytes(new File(codexHome, AGENTS_FILENAME).toPath()),
                StandardCharsets.UTF_8);
    }

    private static void ensureCodexHome(File codexHome) throws IOException {
        if (!codexHome.isDirectory() && !codexHome.mkdirs()) {
            throw new IOException("failed to create codex home at " + codexHome.getAbsolutePath());
        }
    }
}
