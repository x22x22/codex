package com.openai.codexd

import android.net.LocalSocketAddress

object CodexSocketConfig {
    const val DEFAULT_SOCKET_PATH = "@com.openai.codexd.codexd"

    fun toLocalSocketAddress(socketPath: String): LocalSocketAddress {
        val trimmed = socketPath.trim()
        return when {
            trimmed.startsWith("@") -> {
                LocalSocketAddress(trimmed.removePrefix("@"), LocalSocketAddress.Namespace.ABSTRACT)
            }
            trimmed.startsWith("abstract:") -> {
                LocalSocketAddress(
                    trimmed.removePrefix("abstract:"),
                    LocalSocketAddress.Namespace.ABSTRACT,
                )
            }
            else -> LocalSocketAddress(trimmed, LocalSocketAddress.Namespace.FILESYSTEM)
        }
    }
}
