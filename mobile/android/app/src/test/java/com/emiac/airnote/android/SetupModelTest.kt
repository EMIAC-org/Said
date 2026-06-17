package com.emiac.airnote.android

import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class SetupModelTest {
    @Test
    fun androidSetupOrderMatchesProductFlow() {
        assertEquals(
            listOf(
                AndroidSetupStep.Welcome,
                AndroidSetupStep.Account,
                AndroidSetupStep.Privacy,
                AndroidSetupStep.Microphone,
                AndroidSetupStep.Bubble,
                AndroidSetupStep.Preview,
            ),
            AndroidSetupStep.ordered,
        )
    }

    @Test
    fun setupStepNavigationIsLinear() {
        assertEquals(AndroidSetupStep.Account, AndroidSetupStep.Welcome.next())
        assertEquals(AndroidSetupStep.Bubble, AndroidSetupStep.Microphone.next())
        assertEquals(AndroidSetupStep.Bubble, AndroidSetupStep.Preview.previous())
        assertNull(AndroidSetupStep.Preview.next())
        assertNull(AndroidSetupStep.Welcome.previous())
    }

    @Test
    fun previewStatesExposeBubbleAndKeyboardOutcomes() {
        assertEquals(listOf("Ready", "Listening", "Insert", "Copy"), AndroidPreviewState.entries.map { it.label })
    }

    @Test
    fun voicePhasesExposeShippingSessionStates() {
        assertEquals(listOf("Ready", "Listening", "Polishing", "Ready", "Retry"), AndroidVoicePhase.entries.map { it.label })
    }

    @Test
    fun runtimeStatusLabelsCredentialAndMemoryStates() {
        assertEquals(
            "server_ready",
            runtimeStatus(activeCredentials = 1, memoryReady = false).readinessLabel,
        )
        assertEquals(
            "server_memory_ready",
            runtimeStatus(activeCredentials = 1, memoryReady = true).readinessLabel,
        )
        assertEquals(
            "needs_credentials",
            runtimeStatus(activeCredentials = 0, memoryReady = false).readinessLabel,
        )
    }

    @Test
    fun historyItemFallsBackToTranscriptWhenFinalTextIsMissing() {
        val item = RuntimeHistoryItem(
            id = "history-1",
            runId = "run-1",
            clientRunId = "client-1",
            transcript = "raw transcript",
            finalText = "",
            source = "server_wav",
            platform = "android",
            createdAt = "2026-06-08T12:00:00Z",
        )

        assertEquals("raw transcript", item.displayText)
    }

    private fun runtimeStatus(activeCredentials: Int, memoryReady: Boolean): RuntimeStatus =
        RuntimeStatus(
            credentialEncryptionConfigured = true,
            activeCredentialCount = activeCredentials,
            runtimeSessionCount = 0,
            learningEventCount = 0,
            personalReplacementCount = 0,
            personalVocabCount = 0,
            personalAliasCount = 0,
            activeEditPolicyCount = 0,
            serverMemoryReady = memoryReady,
        )
}
