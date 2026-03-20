package com.openai.codexd

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AgentFrameworkToolBridgeTest {
    @Test
    fun parseStartDirectSessionArgumentsExtractsTargetsReasonAndDetachedMode() {
        val request = AgentFrameworkToolBridge.parseStartDirectSessionArguments(
            arguments = JSONObject(
                """
                {
                  "targets": [
                    {
                      "packageName": "com.android.deskclock",
                      "objective": "Start the requested timer in Clock."
                    }
                  ],
                  "reason": "Clock is the installed timer app.",
                  "allowDetachedMode": false
                }
                """.trimIndent(),
            ),
            userObjective = "Start a 5-minute timer.",
            isEligibleTargetPackage = linkedSetOf("com.android.deskclock", "com.android.contacts")::contains,
        )

        assertEquals("Start a 5-minute timer.", request.plan.originalObjective)
        assertEquals("Clock is the installed timer app.", request.plan.rationale)
        assertEquals(false, request.plan.usedOverride)
        assertEquals(false, request.allowDetachedMode)
        assertEquals(1, request.plan.targets.size)
        assertEquals("com.android.deskclock", request.plan.targets.single().packageName)
        assertEquals("Start the requested timer in Clock.", request.plan.targets.single().objective)
    }

    @Test
    fun parseStartDirectSessionArgumentsFallsBackToUserObjectiveWhenDelegatedObjectiveMissing() {
        val request = AgentFrameworkToolBridge.parseStartDirectSessionArguments(
            arguments = JSONObject(
                """
                {
                  "targets": [
                    {
                      "packageName": "com.android.deskclock"
                    }
                  ]
                }
                """.trimIndent(),
            ),
            userObjective = "Start a 5-minute timer.",
            isEligibleTargetPackage = linkedSetOf("com.android.deskclock")::contains,
        )

        assertEquals("Start a 5-minute timer.", request.plan.targets.single().objective)
        assertEquals(true, request.allowDetachedMode)
    }

    @Test
    fun parseStartDirectSessionArgumentsRejectsUnknownPackages() {
        val err = runCatching {
            AgentFrameworkToolBridge.parseStartDirectSessionArguments(
                arguments = JSONObject(
                    """
                    {
                      "targets": [
                        {
                          "packageName": "com.unknown.app",
                          "objective": "Do the task."
                        }
                      ]
                    }
                    """.trimIndent(),
                ),
                userObjective = "Start a timer.",
                isEligibleTargetPackage = linkedSetOf("com.android.deskclock")::contains,
            )
        }.exceptionOrNull()

        assertTrue(err is java.io.IOException)
        assertEquals(
            "Framework session tool selected missing or disallowed package(s): com.unknown.app",
            err?.message,
        )
    }

}
