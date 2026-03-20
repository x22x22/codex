package com.openai.codexd

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AgentParentSessionAggregatorTest {
    @Test
    fun rollupReturnsCompletedSummaryWhenChildrenComplete() {
        val rollup = AgentParentSessionAggregator.rollup(
            listOf(
                ParentSessionChildSummary(
                    sessionId = "child-1",
                    targetPackage = "com.android.deskclock",
                    state = AgentSessionStateValues.COMPLETED,
                    latestResult = "Alarm set for 2:07 PM.",
                    latestError = null,
                ),
            ),
        )

        assertEquals(AgentSessionStateValues.COMPLETED, rollup.state)
        assertEquals(
            "Completed delegated session; com.android.deskclock: Alarm set for 2:07 PM.",
            rollup.resultMessage,
        )
        assertNull(rollup.errorMessage)
    }

    @Test
    fun rollupReturnsWaitingWhenAnyChildWaitsForUser() {
        val rollup = AgentParentSessionAggregator.rollup(
            listOf(
                ParentSessionChildSummary(
                    sessionId = "child-1",
                    targetPackage = "com.android.deskclock",
                    state = AgentSessionStateValues.WAITING_FOR_USER,
                    latestResult = null,
                    latestError = null,
                ),
                ParentSessionChildSummary(
                    sessionId = "child-2",
                    targetPackage = "com.android.settings",
                    state = AgentSessionStateValues.COMPLETED,
                    latestResult = "Completed task.",
                    latestError = null,
                ),
            ),
        )

        assertEquals(AgentSessionStateValues.WAITING_FOR_USER, rollup.state)
        assertNull(rollup.resultMessage)
        assertNull(rollup.errorMessage)
    }

    @Test
    fun rollupReturnsFailedSummaryWhenAnyChildFails() {
        val rollup = AgentParentSessionAggregator.rollup(
            listOf(
                ParentSessionChildSummary(
                    sessionId = "child-1",
                    targetPackage = "com.android.deskclock",
                    state = AgentSessionStateValues.FAILED,
                    latestResult = null,
                    latestError = "Permission denied.",
                ),
                ParentSessionChildSummary(
                    sessionId = "child-2",
                    targetPackage = "com.android.settings",
                    state = AgentSessionStateValues.COMPLETED,
                    latestResult = "Completed task.",
                    latestError = null,
                ),
            ),
        )

        assertEquals(AgentSessionStateValues.FAILED, rollup.state)
        assertNull(rollup.resultMessage)
        assertEquals(
            "Delegated session failed; com.android.deskclock: Permission denied.",
            rollup.errorMessage,
        )
    }
}
