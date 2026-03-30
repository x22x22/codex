package com.openai.codex.genie

import java.io.File

internal object DetachedSessionCommandShims {
    private const val PATH_SEPARATOR = ":"
    private const val DETACHED_MODE_ALLOWED_ENV = "CODEX_ANDROID_DETACHED_MODE_ALLOWED"
    private const val DETACHED_TARGET_PACKAGE_ENV = "CODEX_ANDROID_DETACHED_TARGET_PACKAGE"
    private const val DEFAULT_SYSTEM_PATH = "/system/bin:/system/xbin"

    fun installAndConfigureEnvironment(
        codexHome: File,
        environment: MutableMap<String, String>,
        targetPackage: String,
    ) {
        val shimDirectory = File(codexHome, "bin").apply {
            mkdirs()
        }
        writeShim(
            directory = shimDirectory,
            command = "am",
            realBinary = "/system/bin/am",
            body = """
                if [ "${'$'}1" = "start" ] || [ "${'$'}1" = "start-activity" ]; then
                  prev=""
                  for arg in "${'$'}@"; do
                    if [ "${'$'}prev" = "-n" ]; then
                      case "${'$'}arg" in
                        "${'$'}TARGET_PKG"/*)
                          detached_session_violation "am ${'$'}*"
                          ;;
                      esac
                    elif [ "${'$'}prev" = "-p" ] || [ "${'$'}prev" = "--package" ]; then
                      if [ "${'$'}arg" = "${'$'}TARGET_PKG" ]; then
                        detached_session_violation "am ${'$'}*"
                      fi
                    fi
                    prev="${'$'}arg"
                  done
                fi
            """.trimIndent(),
        )
        writeShim(
            directory = shimDirectory,
            command = "cmd",
            realBinary = "/system/bin/cmd",
            body = """
                if [ "${'$'}1" = "activity" ] && [ "${'$'}2" = "start-activity" ]; then
                  prev=""
                  for arg in "${'$'}@"; do
                    if [ "${'$'}prev" = "-n" ]; then
                      case "${'$'}arg" in
                        "${'$'}TARGET_PKG"/*)
                          detached_session_violation "cmd ${'$'}*"
                          ;;
                      esac
                    elif [ "${'$'}prev" = "-p" ] || [ "${'$'}prev" = "--package" ]; then
                      if [ "${'$'}arg" = "${'$'}TARGET_PKG" ]; then
                        detached_session_violation "cmd ${'$'}*"
                      fi
                    fi
                    prev="${'$'}arg"
                  done
                fi
            """.trimIndent(),
        )
        writeShim(
            directory = shimDirectory,
            command = "monkey",
            realBinary = "/system/bin/monkey",
            body = """
                prev=""
                for arg in "${'$'}@"; do
                  if [ "${'$'}prev" = "-p" ] && [ "${'$'}arg" = "${'$'}TARGET_PKG" ]; then
                    detached_session_violation "monkey ${'$'}*"
                  fi
                  prev="${'$'}arg"
                done
            """.trimIndent(),
        )
        environment[DETACHED_MODE_ALLOWED_ENV] = "1"
        environment[DETACHED_TARGET_PACKAGE_ENV] = targetPackage
        val existingPath = environment["PATH"].orEmpty().ifBlank { DEFAULT_SYSTEM_PATH }
        environment["PATH"] = "${shimDirectory.absolutePath}$PATH_SEPARATOR$existingPath"
    }

    private fun writeShim(
        directory: File,
        command: String,
        realBinary: String,
        body: String,
    ) {
        val script = File(directory, command)
        script.writeText(
            """
                #!/system/bin/sh
                TARGET_PKG="${'$'}{$DETACHED_TARGET_PACKAGE_ENV:-}"

                detached_session_violation() {
                  echo "Detached session contract violated: attempted to relaunch ${'$'}TARGET_PKG with shell command '${'$'}1'. The framework already launched the target hidden; use android_target_ensure_hidden/android_target_show/android_target_capture_frame plus UI inspection/input instead." >&2
                  exit 64
                }

                if [ "${'$'}{$DETACHED_MODE_ALLOWED_ENV:-0}" = "1" ] && [ -n "${'$'}TARGET_PKG" ]; then
                $body
                fi

                exec $realBinary "${'$'}@"
            """.trimIndent() + "\n",
        )
        check(script.setExecutable(true, /* ownerOnly = */ true)) {
            "Failed to mark detached-session shim executable: ${script.absolutePath}"
        }
    }
}
