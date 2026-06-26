package com.emiac.airnote.android

enum class AndroidSetupStep(
    val eyebrow: String,
    val title: String,
    val subtitle: String,
) {
    Welcome(
        eyebrow = "Get started",
        title = "Set up AirNote",
        subtitle = "Account, privacy, microphone, and floating bubble in one guided pass.",
    ),
    Account(
        eyebrow = "Account",
        title = "Account",
        subtitle = "Connect your AirNote workspace before recording.",
    ),
    Privacy(
        eyebrow = "Privacy",
        title = "Privacy",
        subtitle = "Review storage and recovery defaults before recording.",
    ),
    Microphone(
        eyebrow = "Microphone",
        title = "Microphone",
        subtitle = "Confirm the recording surface is ready.",
    ),
    Bubble(
        eyebrow = "Floating bubble",
        title = "Floating bubble",
        subtitle = "Prepare the AirNote Bubble above your existing keyboard.",
    ),
    Preview(
        eyebrow = "Bubble preview",
        title = "Bubble preview",
        subtitle = "Run the bubble and keyboard states before leaving setup.",
    );

    val progressIndex: Int get() = ordinal + 1

    companion object {
        val ordered: List<AndroidSetupStep> = entries
    }
}

enum class AndroidPreviewState(
    val label: String,
    val title: String,
    val subtitle: String,
) {
    Ready(
        label = "Ready",
        title = "AirNote ready",
        subtitle = "Bubble waits above your keyboard.",
    ),
    Listening(
        label = "Listening",
        title = "Listening",
        subtitle = "Speak naturally. Tap stop when done.",
    ),
    Insert(
        label = "Insert",
        title = "Ready to insert",
        subtitle = "Review, insert, copy, or save.",
    ),
    CopyOnly(
        label = "Copy",
        title = "Copy ready",
        subtitle = "Secure field detected. Copy polished text instead.",
    );
}

enum class AndroidVoicePhase(
    val label: String,
) {
    Idle("Ready"),
    Recording("Listening"),
    Uploading("Polishing"),
    Complete("Ready"),
    Error("Retry");
}

fun AndroidSetupStep.next(): AndroidSetupStep? =
    AndroidSetupStep.ordered.getOrNull(ordinal + 1)

fun AndroidSetupStep.previous(): AndroidSetupStep? =
    AndroidSetupStep.ordered.getOrNull(ordinal - 1)
