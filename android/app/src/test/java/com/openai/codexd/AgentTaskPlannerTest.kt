package com.openai.codexd

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AgentTaskPlannerTest {
    @Test
    fun parsePlanResponseExtractsTargetsAndReason() {
        val plan = AgentTaskPlanner.parsePlanResponse(
            responseText = """
                {"targets":[{"packageName":"com.android.deskclock","objective":"Start the requested timer in Clock."}],"reason":"Clock is the installed timer app."}
            """.trimIndent(),
            userObjective = "Start a 5-minute timer.",
            allowedPackageNames = linkedSetOf("com.android.deskclock", "com.android.contacts"),
        )

        assertEquals("Start a 5-minute timer.", plan.originalObjective)
        assertEquals("Clock is the installed timer app.", plan.rationale)
        assertEquals(false, plan.usedOverride)
        assertEquals(1, plan.targets.size)
        assertEquals("com.android.deskclock", plan.targets.single().packageName)
        assertEquals("Start the requested timer in Clock.", plan.targets.single().objective)
    }

    @Test
    fun parsePlanResponseFallsBackToUserObjectiveWhenDelegatedObjectiveMissing() {
        val plan = AgentTaskPlanner.parsePlanResponse(
            responseText = """
                {"targets":[{"packageName":"com.android.deskclock"}],"reason":"Clock matches the request."}
            """.trimIndent(),
            userObjective = "Start a 5-minute timer.",
            allowedPackageNames = linkedSetOf("com.android.deskclock"),
        )

        assertEquals("Start a 5-minute timer.", plan.targets.single().objective)
    }

    @Test
    fun parsePlanResponseRejectsUnknownPackages() {
        val err = runCatching {
            AgentTaskPlanner.parsePlanResponse(
                responseText = """
                    {"targets":[{"packageName":"com.unknown.app","objective":"Do the task."}]}
                """.trimIndent(),
                userObjective = "Start a timer.",
                allowedPackageNames = linkedSetOf("com.android.deskclock"),
            )
        }.exceptionOrNull()

        assertTrue(err is java.io.IOException)
        assertEquals("Planner response did not select an installed package", err?.message)
    }
}
