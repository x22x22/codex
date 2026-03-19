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
        sessionControls[sessionId]?.userResponses?.offer(response)
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
                "Genie is headless. It hosts codex app-server locally, routes model traffic through the Agent Binder bridge, uses normal Android shell commands for package/app driving, and reserves dynamic tools for framework-only target controls.",
            )

            if (request.isDetachedModeAllowed) {
                callback.requestLaunchDetachedTargetHidden(sessionId)
                callback.publishTrace(sessionId, "Requested detached target launch for ${request.targetPackage}.")
            }

            AgentBridgeClient(this).use { bridgeClient ->
                val runtimeStatus = bridgeClient.getRuntimeStatus()
                val accountSuffix = runtimeStatus.accountEmail?.let { " ($it)" } ?: ""
                callback.publishTrace(
                    sessionId,
                    "Reached Agent Binder bridge; authenticated=${runtimeStatus.authenticated}${accountSuffix}, provider=${runtimeStatus.modelProviderId}, model=${runtimeStatus.effectiveModel ?: "unknown"}, clients=${runtimeStatus.clientCount}.",
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
            callback.publishError(sessionId, "Interrupted: ${err.message}")
            callback.updateState(sessionId, AgentSessionInfo.STATE_FAILED)
        } catch (err: IOException) {
            if (control.cancelled) {
                callback.publishError(sessionId, "Cancelled")
                callback.updateState(sessionId, AgentSessionInfo.STATE_CANCELLED)
            } else {
                callback.publishError(sessionId, err.message ?: err::class.java.simpleName)
                callback.updateState(sessionId, AgentSessionInfo.STATE_FAILED)
            }
        } catch (err: RuntimeException) {
            callback.publishError(sessionId, "${err::class.java.simpleName}: ${err.message}")
            callback.updateState(sessionId, AgentSessionInfo.STATE_FAILED)
        } finally {
            sessionControls.remove(sessionId)
            control.cancel()
        }
    }
}
