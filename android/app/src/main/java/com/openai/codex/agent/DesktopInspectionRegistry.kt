package com.openai.codex.agent

object DesktopInspectionRegistry {
    private val lock = Any()
    private val attachedPlannerSessions = linkedSetOf<String>()
    private val heldChildrenByParent = mutableMapOf<String, MutableSet<String>>()
    private val parentByHeldChild = mutableMapOf<String, String>()

    fun markPlannerAttached(parentSessionId: String) {
        synchronized(lock) {
            attachedPlannerSessions += parentSessionId
        }
    }

    fun markPlannerDetached(parentSessionId: String): Set<String> {
        return synchronized(lock) {
            attachedPlannerSessions.remove(parentSessionId)
            val releasedChildren = heldChildrenByParent.remove(parentSessionId).orEmpty().toSet()
            releasedChildren.forEach(parentByHeldChild::remove)
            releasedChildren
        }
    }

    fun holdChildrenForAttachedPlanner(
        parentSessionId: String,
        childSessionIds: Collection<String>,
    ): Boolean {
        return synchronized(lock) {
            if (parentSessionId !in attachedPlannerSessions) {
                return false
            }
            val heldChildren = heldChildrenByParent.getOrPut(parentSessionId, ::linkedSetOf)
            childSessionIds.forEach { childSessionId ->
                if (childSessionId.isNotBlank()) {
                    heldChildren += childSessionId
                    parentByHeldChild[childSessionId] = parentSessionId
                }
            }
            true
        }
    }

    fun isPlannerAttached(parentSessionId: String): Boolean {
        return synchronized(lock) {
            parentSessionId in attachedPlannerSessions
        }
    }

    fun isSessionHeldForInspection(sessionId: String): Boolean {
        return synchronized(lock) {
            sessionId in parentByHeldChild
        }
    }

    fun removeSession(sessionId: String) {
        synchronized(lock) {
            if (attachedPlannerSessions.remove(sessionId)) {
                heldChildrenByParent.remove(sessionId).orEmpty().forEach(parentByHeldChild::remove)
            }
            parentByHeldChild.remove(sessionId)?.let { parentSessionId ->
                heldChildrenByParent[parentSessionId]?.remove(sessionId)
                if (heldChildrenByParent[parentSessionId].isNullOrEmpty()) {
                    heldChildrenByParent.remove(parentSessionId)
                }
            }
        }
    }
}
