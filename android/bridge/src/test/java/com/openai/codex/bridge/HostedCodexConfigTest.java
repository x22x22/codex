package com.openai.codex.bridge;

import static org.junit.Assert.assertEquals;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import org.junit.Test;

public final class HostedCodexConfigTest {
    @Test
    public void installAgentsFileWritesExpectedGuidance() throws Exception {
        File codexHome = Files.createTempDirectory("hosted-codex-home").toFile();
        String agentsMarkdown = "# Runtime Notes\n\n- prefer `cmd`\n";

        HostedCodexConfig.installAgentsFile(codexHome, agentsMarkdown);

        String installedMarkdown =
                new String(
                        Files.readAllBytes(new File(codexHome, "AGENTS.md").toPath()),
                        StandardCharsets.UTF_8);

        assertEquals(agentsMarkdown, installedMarkdown);
    }

    @Test
    public void readInstalledAgentsMarkdownReadsExistingFile() throws Exception {
        File codexHome = Files.createTempDirectory("hosted-codex-agents").toFile();
        HostedCodexConfig.installAgentsFile(codexHome, "# Agent file\n");

        String installedMarkdown = HostedCodexConfig.readInstalledAgentsMarkdown(codexHome);

        assertEquals("# Agent file\n", installedMarkdown);
    }
}
