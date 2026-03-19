package com.openai.codex.bridge;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

public final class HostedCodexConfig {
    public static final String ANDROID_HTTP_PROVIDER_ID = "android-openai-http";

    private HostedCodexConfig() {}

    public static void write(File codexHome, String baseUrl) throws IOException {
        if (!codexHome.isDirectory() && !codexHome.mkdirs()) {
            throw new IOException("failed to create codex home at " + codexHome.getAbsolutePath());
        }

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
}
