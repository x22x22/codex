package com.openai.codex.agent

import org.junit.Assert.assertEquals
import org.junit.Test

class AgentParentSessionAggregatorTest {
    @Test
    fun rollupRequestsAttachWhenAttachedPresentationIsRequired() {
        val rollup = AgentParentSessionAggregator.rollup(
            listOf(
                ParentSessionChildSummary(
                    sessionId = "child-1",
                    targetPackage = "com.android.deskclock",
                    state = AgentSessionStateValues.COMPLETED,
                    targetPresentation = AgentTargetPresentationValues.DETACHED_SHOWN,
                    requiredFinalPresentationPolicy = SessionFinalPresentationPolicy.ATTACHED,
                    latestResult = "Started the stopwatch.",
                    latestError = null,
                ),
            ),
        )

        assertEquals(AgentSessionStateValues.RUNNING, rollup.state)
        assertEquals(listOf("child-1"), rollup.sessionsToAttach)
        assertEquals(null, rollup.resultMessage)
        assertEquals(null, rollup.errorMessage)
    }

    @Test
    fun rollupFailsWhenDetachedShownIsRequiredButTargetIsHidden() {
        val rollup = AgentParentSessionAggregator.rollup(
            listOf(
                ParentSessionChildSummary(
                    sessionId = "child-1",
                    targetPackage = "com.android.deskclock",
                    state = AgentSessionStateValues.COMPLETED,
                    targetPresentation = AgentTargetPresentationValues.DETACHED_HIDDEN,
                    requiredFinalPresentationPolicy = SessionFinalPresentationPolicy.DETACHED_SHOWN,
                    latestResult = "Started the stopwatch.",
                    latestError = null,
                ),
            ),
        )

        assertEquals(AgentSessionStateValues.FAILED, rollup.state)
        assertEquals(emptyList<String>(), rollup.sessionsToAttach)
        assertEquals(
            "Delegated session completed without the required final presentation; com.android.deskclock: required DETACHED_SHOWN, actual DETACHED_HIDDEN",
            rollup.errorMessage,
        )
    }

    @Test
    fun rollupCompletesWhenRequiredPresentationMatches() {
        val rollup = AgentParentSessionAggregator.rollup(
            listOf(
                ParentSessionChildSummary(
                    sessionId = "child-1",
                    targetPackage = "com.android.deskclock",
                    state = AgentSessionStateValues.COMPLETED,
                    targetPresentation = AgentTargetPresentationValues.ATTACHED,
                    requiredFinalPresentationPolicy = SessionFinalPresentationPolicy.ATTACHED,
                    latestResult = "Started the stopwatch.",
                    latestError = null,
                ),
            ),
        )

        assertEquals(AgentSessionStateValues.COMPLETED, rollup.state)
        assertEquals(emptyList<String>(), rollup.sessionsToAttach)
        assertEquals(
            "Completed delegated session; com.android.deskclock: Started the stopwatch.",
            rollup.resultMessage,
        )
    }
}
