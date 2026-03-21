package com.openai.codex.agent

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AgentTaskPlannerTest {
    @Test
    fun parsePlannerResponseExtractsStructuredPlan() {
        val request = AgentTaskPlanner.parsePlannerResponse(
            responseText =
                """
                {
                  "targets": [
                    {
                      "packageName": "com.android.deskclock",
                      "objective": "Start the requested timer in Clock."
                    }
                  ],
                  "reason": "DeskClock is the installed timer handler.",
                  "allowDetachedMode": true
                }
                """.trimIndent(),
            userObjective = "Start a 5-minute timer.",
            isEligibleTargetPackage = linkedSetOf("com.android.deskclock")::contains,
        )

        assertEquals("DeskClock is the installed timer handler.", request.plan.rationale)
        assertEquals(true, request.allowDetachedMode)
        assertEquals(1, request.plan.targets.size)
        assertEquals("com.android.deskclock", request.plan.targets.single().packageName)
        assertEquals("Start the requested timer in Clock.", request.plan.targets.single().objective)
    }

    @Test
    fun parsePlannerResponseAcceptsMarkdownFences() {
        val request = AgentTaskPlanner.parsePlannerResponse(
            responseText =
                """
                ```json
                {
                  "targets": [
                    {
                      "packageName": "com.android.deskclock"
                    }
                  ]
                }
                ```
                """.trimIndent(),
            userObjective = "Start a 5-minute timer.",
            isEligibleTargetPackage = linkedSetOf("com.android.deskclock")::contains,
        )

        assertEquals("Start a 5-minute timer.", request.plan.targets.single().objective)
        assertEquals(true, request.allowDetachedMode)
    }

    @Test
    fun parsePlannerResponseRejectsMissingJson() {
        val err = runCatching {
            AgentTaskPlanner.parsePlannerResponse(
                responseText = "DeskClock seems right.",
                userObjective = "Start a timer.",
                isEligibleTargetPackage = linkedSetOf("com.android.deskclock")::contains,
            )
        }.exceptionOrNull()

        assertTrue(err is java.io.IOException)
        assertEquals("Planner did not return a valid JSON object", err?.message)
    }
}
