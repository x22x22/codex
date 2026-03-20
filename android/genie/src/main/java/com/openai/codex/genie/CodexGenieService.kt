package com.openai.codex.genie

import android.app.agent.AgentSessionInfo
import android.app.agent.GenieRequest
import android.app.agent.GenieService
import android.util.Log
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap

class CodexGenieService : GenieService() {
    companion object {
        private const val TAG = "CodexGenieService"
    }

    private val sessionControls = ConcurrentHashMap<String, GenieSessionControl>()

    override fun onStartGenieSession(request: GenieRequest, callback: Callback) {
        val control = GenieSessionControl()
        sessionControls[request.sessionId] = control
        Thread {
            runSession(request, callback, control)
        }.apply {
            name = "CodexGenie-${request.sessionId}"
            start()
        }
    }

    override fun onCancelGenieSession(sessionId: String) {
        sessionControls.remove(sessionId)?.cancel()
        Log.i(TAG, "Cancelled session $sessionId")
    }

    override fun onUserResponse(sessionId: String, response: String) {
        sessionControls[sessionId]?.recordResponse(response)
        Log.i(TAG, "Received Agent response for $sessionId")
    }

    private fun runSession(
        request: GenieRequest,
        callback: Callback,
        control: GenieSessionControl,
    ) {
        val sessionId = request.sessionId
        try {
            callback.updateState(sessionId, AgentSessionInfo.STATE_RUNNING)
            callback.publishTrace(
                sessionId,
                "Codex Genie started for target=${request.targetPackage} prompt=${request.prompt}",
            )
            callback.publishTrace(
                sessionId,
                "Genie is headless. It hosts codex app-server locally, routes model traffic through the Agent bridge, uses normal Android shell commands for package/app driving, and reserves dynamic tools for framework-only target controls.",
            )

            if (request.isDetachedModeAllowed) {
                callback.requestLaunchDetachedTargetHidden(sessionId)
                callback.publishTrace(sessionId, "Requested detached target launch for ${request.targetPackage}.")
            }

            AgentBridgeClient(
                callback = callback,
                sessionId = sessionId,
            ).use { bridgeClient ->
                val runtimeStatus = bridgeClient.getRuntimeStatus()
                val accountSuffix = runtimeStatus.accountEmail?.let { " ($it)" } ?: ""
                callback.publishTrace(
                    sessionId,
                    "Reached Agent bridge; authenticated=${runtimeStatus.authenticated}${accountSuffix}, provider=${runtimeStatus.modelProviderId}, model=${runtimeStatus.effectiveModel ?: "unknown"}, clients=${runtimeStatus.clientCount}.",
                )
                if (!runtimeStatus.authenticated) {
                    callback.publishResult(
                        sessionId,
                        "Reached the Agent bridge, but the Agent runtime was not authenticated for ${request.targetPackage}.",
                    )
                    callback.updateState(sessionId, AgentSessionInfo.STATE_COMPLETED)
                    return
                }

                CodexAppServerHost(
                    context = this,
                    request = request,
                    callback = callback,
                    control = control,
                    bridgeClient = bridgeClient,
                    runtimeStatus = runtimeStatus,
                ).use { host ->
                    host.run()
                }
            }
        } catch (err: InterruptedException) {
            Thread.currentThread().interrupt()
            Log.w(TAG, "Interrupted Genie session $sessionId", err)
            safeCallback("publish interrupted error") {
                callback.publishError(sessionId, "Interrupted: ${err.message}")
            }
            safeCallback("publish interrupted state") {
                callback.updateState(sessionId, AgentSessionInfo.STATE_FAILED)
            }
        } catch (err: IOException) {
            Log.w(TAG, "I/O failure in Genie session $sessionId", err)
            if (control.cancelled) {
                safeCallback("publish cancelled error") {
                    callback.publishError(sessionId, "Cancelled")
                }
                safeCallback("publish cancelled state") {
                    callback.updateState(sessionId, AgentSessionInfo.STATE_CANCELLED)
                }
            } else {
                safeCallback("publish I/O error") {
                    callback.publishError(sessionId, err.message ?: err::class.java.simpleName)
                }
                safeCallback("publish failed state") {
                    callback.updateState(sessionId, AgentSessionInfo.STATE_FAILED)
                }
            }
        } catch (err: RuntimeException) {
            Log.w(TAG, "Runtime failure in Genie session $sessionId", err)
            safeCallback("publish runtime error") {
                callback.publishError(sessionId, "${err::class.java.simpleName}: ${err.message}")
            }
            safeCallback("publish runtime failed state") {
                callback.updateState(sessionId, AgentSessionInfo.STATE_FAILED)
            }
        } finally {
            sessionControls.remove(sessionId)
            control.cancel()
        }
    }

    private fun safeCallback(
        operation: String,
        block: () -> Unit,
    ) {
        runCatching(block).onFailure { err ->
            Log.w(TAG, "Ignoring Genie callback failure during $operation", err)
        }
    }
}
