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
}
