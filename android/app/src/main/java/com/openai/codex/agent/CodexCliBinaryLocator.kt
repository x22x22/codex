package com.openai.codex.agent

import android.content.Context
import java.io.File
import java.io.IOException

object CodexCliBinaryLocator {
    fun resolve(context: Context): File {
        val binary = File(context.applicationInfo.nativeLibraryDir, "libcodex.so")
        if (!binary.exists()) {
            throw IOException("codex binary missing at ${binary.absolutePath}")
        }
        return binary
    }
}
