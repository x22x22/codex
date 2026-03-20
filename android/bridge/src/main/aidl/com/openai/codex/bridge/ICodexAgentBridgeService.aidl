package com.openai.codex.bridge;

import android.os.ParcelFileDescriptor;
import com.openai.codex.bridge.BridgeHttpResponse;
import com.openai.codex.bridge.BridgeRuntimeStatus;

interface ICodexAgentBridgeService {
    BridgeRuntimeStatus getRuntimeStatus();
    BridgeHttpResponse sendResponsesRequest(String requestBody);
    ParcelFileDescriptor openResponsesStream(String requestBody);
}
