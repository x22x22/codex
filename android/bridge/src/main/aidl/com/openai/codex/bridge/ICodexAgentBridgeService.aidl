package com.openai.codex.bridge;

import com.openai.codex.bridge.BridgeHttpResponse;
import com.openai.codex.bridge.BridgeRuntimeStatus;

interface ICodexAgentBridgeService {
    BridgeRuntimeStatus getRuntimeStatus();
    BridgeHttpResponse sendResponsesRequest(String requestBody);
}
