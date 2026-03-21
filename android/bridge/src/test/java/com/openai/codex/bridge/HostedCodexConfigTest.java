package com.openai.codex.bridge;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import org.junit.Test;

public final class HostedCodexConfigTest {
    @Test
    public void writeInstallsConfigAndAgentsFile() throws Exception {
        File codexHome = Files.createTempDirectory("hosted-codex-home").toFile();

        HostedCodexConfig.write(codexHome, "http://127.0.0.1:8080");

        String configToml =
                new String(
                        Files.readAllBytes(new File(codexHome, "config.toml").toPath()),
                        StandardCharsets.UTF_8);
        String agentsMarkdown =
                new String(
                        Files.readAllBytes(new File(codexHome, "AGENTS.md").toPath()),
                        StandardCharsets.UTF_8);

        assertTrue(configToml.contains("model_provider = \"android-openai-http\""));
        assertTrue(configToml.contains("base_url = \"http://127.0.0.1:8080\""));
        assertEquals(HostedCodexConfig.buildAgentsMarkdown(), agentsMarkdown);
    }

    @Test
    public void installAgentsFileWritesExpectedGuidance() throws Exception {
        File codexHome = Files.createTempDirectory("hosted-codex-agents").toFile();

        HostedCodexConfig.installAgentsFile(codexHome);

        String agentsMarkdown =
                new String(
                        Files.readAllBytes(new File(codexHome, "AGENTS.md").toPath()),
                        StandardCharsets.UTF_8);

        assertEquals(HostedCodexConfig.buildAgentsMarkdown(), agentsMarkdown);
        assertTrue(agentsMarkdown.contains("The user interacts only with the Agent."));
        assertTrue(
                agentsMarkdown.contains(
                        "Do not rely on direct cross-app `bindService(...)` or raw local sockets"));
        assertTrue(
                agentsMarkdown.contains(
                        "If the task says the app should be visible to the user, do not claim success until the target is attached"));
    }
}
