package com.emiac.airnote.android

import android.text.InputType
import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertFalse
import kotlin.test.assertTrue

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

    @Test
    fun runtimeVoicePayloadIncludesOnlySupportedAndroidFields() {
        val payload = runtimeVoicePayloadFields(
            wavBytes = byteArrayOf(1, 2, 3),
            clientRunId = "android-test-run",
            deviceId = "android-device",
            outputLanguage = "hinglish",
            selectedModel = "smart",
            safeVocabTerms = listOf("N8N", "  EMIAC  ", "N8N"),
            mode = "message-polish",
        )

        assertEquals("hinglish", payload.outputLanguage)
        assertEquals("smart", payload.selectedModel)
        assertEquals("android-test-run", payload.clientRunId)
        assertEquals("android", payload.platform)
        assertEquals(listOf("N8N", "EMIAC"), payload.safeVocabTerms)
        assertEquals("message_polish", payload.mode)
    }

    @Test
    fun runtimeTextPolishPayloadMatchesIosRewriteContract() {
        val payload = runtimeTextPolishPayloadFields(
            text = "  make this sharper  ",
            clientRunId = "android-rewrite-run",
            outputLanguage = "english",
            tonePreset = "work",
            screenContext = "x".repeat(700),
            safeVocabTerms = listOf("  N8N  ", "N8N", "EMIAC"),
        )

        assertEquals("make this sharper", payload.transcript)
        assertEquals("english", payload.outputLanguage)
        assertEquals("smart", payload.selectedModel)
        assertEquals("professional", payload.tonePreset)
        assertEquals(500, payload.screenContext?.length)
        assertEquals(listOf("N8N", "EMIAC"), payload.safeVocabTerms)
        assertEquals("android-rewrite-run", payload.clientRunId)
    }

    @Test
    fun vocabTermsAreSanitizedForSafeHints() {
        assertEquals("EMIAC Tech", sanitizeVocabTerm(" EMIAC   Tech "))
        assertNull(sanitizeVocabTerm("x"))
        assertEquals("bad term", sanitizeVocabTerm("bad\nterm"))
        assertNull(sanitizeVocabTerm("bad\u0001term"))
    }

    @Test
    fun gatewayUrlsAreNormalizedForAndroidSettings() {
        assertEquals("https://airnote-dev.103.180.163.41.sslip.io", normalizeGatewayUrl("https://airnote-dev.103.180.163.41.sslip.io/"))
        assertEquals("https://airnote.emiactech.com", normalizeGatewayUrl("airnote.emiactech.com"))
    }

    @Test
    fun diagnosticsSummaryAvoidsRawDictationData() {
        val summary = AndroidDiagnosticsSnapshot(
            serverUrl = "https://airnote-dev.103.180.163.41.sslip.io",
            authState = "signed_in",
            micPermission = "granted",
            accessibilityEnabled = true,
            audioRoute = "Phone mic",
            lastRequestId = "android-test",
            lastLatencyMs = 123,
            lastInsertionResult = "inserted",
            lastFailure = "",
        ).redactedSummary

        assertTrue(summary.contains("server=https://airnote-dev.103.180.163.41.sslip.io"))
        assertTrue(summary.contains("request=android-test"))
        assertFalse(summary.contains("transcript"))
        assertFalse(summary.contains("output="))
        assertFalse(summary.contains("wav"))
    }

    @Test
    fun accessibilityServiceDetectionAcceptsAndroidShortAndFullComponentNames() {
        val expectedPackage = "com.emiac.airnote.android"
        val expectedClass = "com.emiac.airnote.android.AirNoteBubbleAccessibilityService"

        assertTrue(
            isAirNoteAccessibilityServiceListed(
                "com.emiac.airnote.android/.AirNoteBubbleAccessibilityService",
                expectedPackage,
                expectedClass,
            ),
        )
        assertTrue(
            isAirNoteAccessibilityServiceListed(
                "com.other/.Service:com.emiac.airnote.android/com.emiac.airnote.android.AirNoteBubbleAccessibilityService",
                expectedPackage,
                expectedClass,
            ),
        )
        assertFalse(
            isAirNoteAccessibilityServiceListed(
                "com.emiac.airnote.android/.OtherService",
                expectedPackage,
                expectedClass,
            ),
        )
    }

    @Test
    fun fieldSafetyBlocksPasswordAndOtpLikeFields() {
        assertTrue(
            AndroidFieldSafety.isSensitiveField(
                inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD,
                isPassword = false,
                text = null,
                hint = null,
                className = null,
            ),
        )
        assertTrue(
            AndroidFieldSafety.isSensitiveField(
                inputType = InputType.TYPE_CLASS_NUMBER,
                isPassword = false,
                text = null,
                hint = "Enter OTP",
                className = "android.widget.EditText",
            ),
        )
        assertFalse(
            AndroidFieldSafety.isSensitiveField(
                inputType = InputType.TYPE_CLASS_TEXT,
                isPassword = false,
                text = "Write a message",
                hint = "Message",
                className = "android.widget.EditText",
            ),
        )
    }

    @Test
    fun androidRewriteTargetPrefersSelectionThenCursorSentence() {
        val text = "First sentence. Polish this one please. Last bit"
        val selectedText = "Polish this one please"
        val selected = resolveAndroidRewriteTarget(
            fullText = text,
            selectionStart = text.indexOf(selectedText),
            selectionEnd = text.indexOf(selectedText) + selectedText.length,
        )

        assertEquals(AndroidRewriteScope.Selection, selected?.scope)
        assertEquals("Polish this one please", selected?.text)

        val cursorText = "First sentence. Polish this one please"
        val cursor = resolveAndroidRewriteTarget(
            fullText = cursorText,
            selectionStart = cursorText.length,
            selectionEnd = cursorText.length,
        )

        assertEquals(AndroidRewriteScope.CursorSentence, cursor?.scope)
        assertEquals("Polish this one please", cursor?.text)
    }

    @Test
    fun androidRewriteReplacementRequiresExactTarget() {
        val target = resolveAndroidRewriteTarget(
            fullText = "Before. rewrite me",
            selectionStart = 18,
            selectionEnd = 18,
        ) ?: error("expected target")

        assertEquals("Before. Rewritten.", replaceAndroidRewriteTarget(target, "Rewritten."))
        assertNull(replaceAndroidRewriteTarget(target.copy(fullText = "Before. edited"), "Rewritten."))
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
