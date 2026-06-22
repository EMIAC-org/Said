package com.emiac.airnote.android

import android.Manifest
import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.net.Uri
import android.provider.Settings
import android.view.View
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.ArrowForward
import androidx.compose.material.icons.rounded.AccountCircle
import androidx.compose.material.icons.rounded.Bolt
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.ContentCopy
import androidx.compose.material.icons.rounded.DarkMode
import androidx.compose.material.icons.rounded.Delete
import androidx.compose.material.icons.rounded.History
import androidx.compose.material.icons.rounded.Keyboard
import androidx.compose.material.icons.rounded.Language
import androidx.compose.material.icons.rounded.LightMode
import androidx.compose.material.icons.rounded.Lock
import androidx.compose.material.icons.rounded.Mic
import androidx.compose.material.icons.rounded.Person
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material.icons.rounded.Tune
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.util.UUID
import kotlinx.coroutines.launch

private data class AirNoteColors(
    val background: Color,
    val background2: Color,
    val surface: Color,
    val surfaceRaised: Color,
    val surfaceHover: Color,
    val foreground: Color,
    val muted: Color,
    val border: Color,
    val borderStrong: Color,
    val accent: Color,
    val success: Color,
    val danger: Color,
    val ink: Color,
    val primaryButtonFill: Color,
    val primaryButtonContent: Color,
    val keyboardWell: Color,
)

private val DarkAirNoteColors = AirNoteColors(
    background = Color(0xFF060609),
    background2 = Color(0xFF09090C),
    surface = Color(0xFF0E0E13),
    surfaceRaised = Color(0xFF16161C),
    surfaceHover = Color(0xFF1F1F28),
    foreground = Color(0xFFEDEDF5),
    muted = Color(0xFF9396A3),
    border = Color.White.copy(alpha = 0.07f),
    borderStrong = Color.White.copy(alpha = 0.12f),
    accent = Color(0xFF9EB3FA),
    success = Color(0xFF87D19B),
    danger = Color(0xFFF04D5C),
    ink = Color(0xFF0B0B0F),
    primaryButtonFill = Color.White.copy(alpha = 0.98f),
    primaryButtonContent = Color(0xFF0B0B0F),
    keyboardWell = Color(0xFF09090D),
)

private val LightAirNoteColors = AirNoteColors(
    background = Color(0xFFF5F6FA),
    background2 = Color(0xFFFCFCFE),
    surface = Color.White,
    surfaceRaised = Color(0xFFEFF2F8),
    surfaceHover = Color(0xFFE1E6F0),
    foreground = Color(0xFF12131A),
    muted = Color(0xFF676D7C),
    border = Color(0x1712131A),
    borderStrong = Color(0x2612131A),
    accent = Color(0xFF5B6FD6),
    success = Color(0xFF2F8D4E),
    danger = Color(0xFFD83A4B),
    ink = Color(0xFF0B0B0F),
    primaryButtonFill = Color(0xFF0B0B0F),
    primaryButtonContent = Color.White,
    keyboardWell = Color(0xFFE3E7F0),
)

private val LocalAirNoteColors = staticCompositionLocalOf { DarkAirNoteColors }

private object AirNotePalette {
    val Background: Color
        @Composable get() = LocalAirNoteColors.current.background
    val Background2: Color
        @Composable get() = LocalAirNoteColors.current.background2
    val Surface: Color
        @Composable get() = LocalAirNoteColors.current.surface
    val SurfaceRaised: Color
        @Composable get() = LocalAirNoteColors.current.surfaceRaised
    val SurfaceHover: Color
        @Composable get() = LocalAirNoteColors.current.surfaceHover
    val ForegroundFixed: Color
        @Composable get() = LocalAirNoteColors.current.foreground
    val Muted: Color
        @Composable get() = LocalAirNoteColors.current.muted
    val Border: Color
        @Composable get() = LocalAirNoteColors.current.border
    val BorderStrong: Color
        @Composable get() = LocalAirNoteColors.current.borderStrong
    val Accent: Color
        @Composable get() = LocalAirNoteColors.current.accent
    val Success: Color
        @Composable get() = LocalAirNoteColors.current.success
    val Danger: Color
        @Composable get() = LocalAirNoteColors.current.danger
    val Ink: Color
        @Composable get() = LocalAirNoteColors.current.ink
    val PrimaryButtonFill: Color
        @Composable get() = LocalAirNoteColors.current.primaryButtonFill
    val PrimaryButtonContent: Color
        @Composable get() = LocalAirNoteColors.current.primaryButtonContent
    val KeyboardWell: Color
        @Composable get() = LocalAirNoteColors.current.keyboardWell
}

private enum class AndroidAppearanceMode(val label: String, val detail: String) {
    System("Phone", "Match the phone theme"),
    Light("Light", "Keep AirNote light"),
    Dark("Dark", "Keep AirNote dark"),
}

@Composable
fun AirNoteAndroidApp(
    oauthToken: String? = null,
    onOAuthTokenConsumed: () -> Unit = {},
) {
    val context = LocalContext.current
    val configuration = LocalConfiguration.current
    val sessionStore = remember { AndroidSecureSessionStore(context.applicationContext) }
    val settingsStore = remember { AndroidSettingsStore(context.applicationContext) }
    val diagnosticsStore = remember { AndroidDiagnosticsStore(context.applicationContext) }
    val recorder = remember { AndroidVoiceRecorder() }
    val scope = rememberCoroutineScope()
    var gatewaySession by remember { mutableStateOf(sessionStore.read()) }
    var polishPrefs by remember { mutableStateOf(settingsStore.readPolishPreferences()) }
    var setupComplete by rememberSaveable { mutableStateOf(false) }
    var appearanceModeRaw by rememberSaveable { mutableStateOf(AndroidAppearanceMode.System.name) }
    val appearanceMode = AndroidAppearanceMode.entries
        .firstOrNull { it.name == appearanceModeRaw }
        ?: AndroidAppearanceMode.System
    val systemDark = (configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
    val lightMode = when (appearanceMode) {
        AndroidAppearanceMode.System -> !systemDark
        AndroidAppearanceMode.Light -> true
        AndroidAppearanceMode.Dark -> false
    }
    var runtimeStatus by rememberSaveable { mutableStateOf(if (BuildConfig.USE_MOCK_GATEWAY) "Preview" else "Unknown") }
    var voicePhase by rememberSaveable { mutableStateOf(AndroidVoicePhase.Idle) }
    var voiceMessage by rememberSaveable { mutableStateOf("Tap to record a short dictation.") }
    var voiceLevel by rememberSaveable { mutableStateOf(0f) }
    var voiceResult by rememberSaveable { mutableStateOf<String?>(null) }
    var diagnosticsSnapshot by remember {
        mutableStateOf(
            diagnosticsStore.snapshot(
                serverUrl = polishPrefs.gatewayBaseUrl,
                authState = if (gatewaySession == null) "signed_out" else "signed_in",
                micPermission = if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) "granted" else "missing",
            ),
        )
    }
    var historyMessage by rememberSaveable { mutableStateOf(if (BuildConfig.USE_MOCK_GATEWAY) "Preview history" else "History not loaded") }
    val history = remember { mutableStateListOf<RuntimeHistoryItem>() }
    var learningItem by remember { mutableStateOf<RuntimeHistoryItem?>(null) }
    var learningText by rememberSaveable { mutableStateOf("") }
    var learningMessage by rememberSaveable { mutableStateOf("Pick a saved dictation to review an edit.") }
    var learningWorking by rememberSaveable { mutableStateOf(false) }
    val learningCandidates = remember { mutableStateListOf<RuntimeLearningCandidate>() }
    val gateway = remember(gatewaySession?.token, polishPrefs.gatewayBaseUrl) {
        if (BuildConfig.USE_MOCK_GATEWAY) {
            MockGatewayClient()
        } else {
            HttpGatewayClient(polishPrefs.gatewayBaseUrl) {
                gatewaySession?.token ?: sessionStore.read()?.token
            }
        }
    }

    fun refreshDiagnostics() {
        diagnosticsSnapshot = diagnosticsStore.snapshot(
            serverUrl = polishPrefs.gatewayBaseUrl,
            authState = if (gatewaySession == null) "signed_out" else "signed_in",
            micPermission = if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) "granted" else "missing",
        )
    }

    fun reloadPolishPrefs() {
        val nextPrefs = settingsStore.readPolishPreferences()
        polishPrefs = nextPrefs
        diagnosticsSnapshot = diagnosticsStore.snapshot(
            serverUrl = nextPrefs.gatewayBaseUrl,
            authState = if (gatewaySession == null) "signed_out" else "signed_in",
            micPermission = if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) "granted" else "missing",
        )
    }

    fun cancelLearningReview() {
        learningItem = null
        learningText = ""
        learningMessage = "Pick a saved dictation to review an edit."
        learningCandidates.clear()
        learningWorking = false
    }

    fun startLearningReview(item: RuntimeHistoryItem) {
        learningItem = item
        learningText = item.displayText
        learningMessage = "Edit the kept text, then analyze the correction."
        learningCandidates.clear()
        learningWorking = false
    }

    fun analyzeLearning() {
        if (!polishPrefs.learningEnabled) {
            learningMessage = "Learning review is off in Android settings."
            learningCandidates.clear()
            return
        }
        val item = learningItem ?: return
        val kept = learningText.trim()
        if (kept.isBlank()) {
            learningMessage = "Kept text cannot be empty."
            return
        }
        scope.launch {
            learningWorking = true
            learningCandidates.clear()
            val result = runCatching {
                gateway.analyzeEdit(
                    recordingId = item.learningRecordingId(),
                    transcript = item.transcript,
                    aiOutput = item.learningAiOutput(),
                    userKept = kept,
                )
            }
            result.fold(
                onSuccess = { analysis ->
                    learningCandidates.addAll(analysis.candidates.filter { it.learnable })
                    learningMessage = when {
                        !analysis.changed -> "No edit detected."
                        learningCandidates.isEmpty() -> "No safe learning candidates found."
                        else -> "${learningCandidates.size} learning candidate${if (learningCandidates.size == 1) "" else "s"} ready."
                    }
                },
                onFailure = { error ->
                    learningMessage = error.message ?: "Could not analyze this correction."
                },
            )
            learningWorking = false
        }
    }

    fun confirmLearning() {
        if (!polishPrefs.learningEnabled) {
            learningMessage = "Learning review is off in Android settings."
            learningCandidates.clear()
            return
        }
        val item = learningItem ?: return
        val items = learningCandidates.filter { it.learnable }
        if (items.isEmpty()) {
            learningMessage = "Analyze a correction before confirming."
            return
        }
        scope.launch {
            learningWorking = true
            val result = runCatching {
                gateway.confirmLearning(
                    recordingId = item.learningRecordingId(),
                    items = items,
                )
            }
            result.fold(
                onSuccess = { confirmation ->
                    learningMessage = "Learned ${confirmation.learnedCount}, blocked ${confirmation.blockedCount}. ${confirmation.status}"
                    learningCandidates.clear()
                },
                onFailure = { error ->
                    learningMessage = error.message ?: "Could not confirm learning."
                },
            )
            learningWorking = false
        }
    }

    fun refreshHistory() {
        scope.launch {
            if (!BuildConfig.USE_MOCK_GATEWAY && gatewaySession?.token == null) {
                history.clear()
                historyMessage = "Sign in to sync server history."
                return@launch
            }
            val result = runCatching { gateway.listHistory(limit = 20) }
            result.fold(
                onSuccess = { rows ->
                    history.clear()
                    history.addAll(rows)
                    historyMessage = if (rows.isEmpty()) "No server history yet." else "Server history synced."
                },
                onFailure = { error ->
                    historyMessage = error.message ?: "Could not load server history."
                },
            )
        }
    }

    fun deleteHistory(item: RuntimeHistoryItem) {
        scope.launch {
            val result = runCatching { gateway.deleteHistory(item.id) }
            result.fold(
                onSuccess = {
                    history.removeAll { it.id == item.id }
                    if (learningItem?.id == item.id) {
                        cancelLearningReview()
                    }
                    historyMessage = "Deleted from server history."
                },
                onFailure = { error ->
                    historyMessage = error.message ?: "Could not delete history item."
                },
            )
        }
    }

    LaunchedEffect(gatewaySession?.token) {
        if (BuildConfig.USE_MOCK_GATEWAY) {
            runtimeStatus = "Preview"
            refreshHistory()
        } else if (gatewaySession?.token != null) {
            runtimeStatus = runCatching { gateway.runtimeStatus().readinessLabel }
                .getOrElse { "unreachable" }
            refreshHistory()
        } else {
            history.clear()
            historyMessage = "Sign in to sync server history."
        }
    }

    LaunchedEffect(oauthToken) {
        val token = oauthToken?.trim().orEmpty()
        if (token.isEmpty()) return@LaunchedEffect
        val result = runCatching { gateway.restoreSession(token) }
        result.onSuccess { response ->
            val saved = GatewaySession(response.token, response.account)
            sessionStore.write(saved)
            gatewaySession = saved
            runtimeStatus = runCatching { gateway.runtimeStatus().readinessLabel }
                .getOrElse { "unreachable" }
            refreshHistory()
        }.onFailure {
            runtimeStatus = "auth_failed"
        }
        onOAuthTokenConsumed()
    }

    fun beginVoiceRecording() {
        if (!BuildConfig.USE_MOCK_GATEWAY && gatewaySession?.token == null) {
            voicePhase = AndroidVoicePhase.Error
            voiceMessage = "Sign in before recording on the live Gateway."
            refreshDiagnostics()
            return
        }
        val route = AndroidAudioRoute.current(context.applicationContext)
        voiceResult = null
        voiceMessage = "Listening on ${route.label}. Speak naturally, then tap Stop."
        val started = recorder.start(context.applicationContext, scope) { level ->
            scope.launch { voiceLevel = level }
        }
        if (started) {
            voicePhase = AndroidVoicePhase.Recording
        } else {
            voicePhase = AndroidVoicePhase.Error
            voiceMessage = "Microphone is unavailable. Check Android permissions."
            diagnosticsStore.recordFailure("microphone_unavailable")
            refreshDiagnostics()
        }
    }

    fun finishVoiceRecording() {
        if (!recorder.isRecording) {
            return
        }
        scope.launch {
            voicePhase = AndroidVoicePhase.Uploading
            voiceMessage = "Sending audio to AirNote Gateway."
            voiceLevel = 0f
            val clientRunId = "android-${UUID.randomUUID()}"
            diagnosticsStore.recordRequestStarted(clientRunId)
            refreshDiagnostics()
            val result = runCatching {
                val wav = recorder.stop()
                require(wav.size > 44) { "No audio was captured. Try again." }
                gateway.polishWav(
                    wavBytes = wav,
                    clientRunId = clientRunId,
                    deviceId = androidDeviceId(context.applicationContext),
                    outputLanguage = polishPrefs.outputLanguage.wireValue,
                    selectedModel = polishPrefs.selectedModel.wireValue,
                    safeVocabTerms = polishPrefs.safeVocabTerms,
                )
            }
            result.fold(
                onSuccess = { response ->
                    voicePhase = AndroidVoicePhase.Complete
                    voiceResult = response.output
                    diagnosticsStore.recordVoiceSuccess(response.runId.ifBlank { clientRunId }, response.totalLatencyMs)
                    val routeNote = if (recorder.routeChanged) " Route changed: ${recorder.routeSummary}." else ""
                    voiceMessage = "Polished in ${response.totalLatencyMs.takeIf { it > 0 } ?: 0} ms.$routeNote"
                    refreshDiagnostics()
                    refreshHistory()
                },
                onFailure = { error ->
                    voicePhase = AndroidVoicePhase.Error
                    voiceResult = null
                    voiceMessage = error.message ?: "AirNote could not finish this recording."
                    diagnosticsStore.recordFailure(voiceMessage)
                    refreshDiagnostics()
                },
            )
        }
    }

    fun cancelVoiceRecording() {
        recorder.cancel()
        voicePhase = AndroidVoicePhase.Idle
        voiceMessage = "Recording cancelled."
        voiceLevel = 0f
        diagnosticsStore.recordFailure("")
        refreshDiagnostics()
    }

    val micPermissionLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.RequestPermission(),
    ) { granted ->
        if (granted) {
            refreshDiagnostics()
            beginVoiceRecording()
        } else {
            voicePhase = AndroidVoicePhase.Error
            voiceMessage = "Microphone permission is required for dictation."
            diagnosticsStore.recordFailure("microphone_permission_denied")
            refreshDiagnostics()
        }
    }

    fun handleVoiceAction() {
        when (voicePhase) {
            AndroidVoicePhase.Recording -> finishVoiceRecording()
            AndroidVoicePhase.Uploading -> Unit
            else -> {
                if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
                    beginVoiceRecording()
                } else {
                    micPermissionLauncher.launch(Manifest.permission.RECORD_AUDIO)
                }
            }
        }
    }

    AirNoteTheme(lightMode = lightMode) {
        if (setupComplete) {
            HomeScreen(
                history = history,
                historyMessage = historyMessage,
                accountEmail = gatewaySession?.account?.email,
                runtimeStatus = runtimeStatus,
                voicePhase = voicePhase,
                voiceMessage = voiceMessage,
                voiceLevel = voiceLevel,
                voiceResult = voiceResult,
                polishPrefs = polishPrefs,
                diagnosticsSnapshot = diagnosticsSnapshot,
                appearanceMode = appearanceMode,
                onAppearanceModeChange = { appearanceModeRaw = it.name },
                onGatewayPresetChange = { preset ->
                    settingsStore.setGatewayBaseUrl(preset.url)
                    reloadPolishPrefs()
                    runtimeStatus = if (BuildConfig.USE_MOCK_GATEWAY) "Preview" else "Unknown"
                },
                onOutputLanguageChange = { language ->
                    settingsStore.setOutputLanguage(language)
                    reloadPolishPrefs()
                },
                onSelectedModelChange = { model ->
                    settingsStore.setSelectedModel(model)
                    reloadPolishPrefs()
                },
                onLearningEnabledChange = { enabled ->
                    settingsStore.setLearningEnabled(enabled)
                    reloadPolishPrefs()
                },
                onAddSafeVocabTerm = { term ->
                    val added = settingsStore.addSafeVocabTerm(term)
                    reloadPolishPrefs()
                    added
                },
                onRemoveSafeVocabTerm = { term ->
                    settingsStore.removeSafeVocabTerm(term)
                    reloadPolishPrefs()
                },
                onVoiceAction = ::handleVoiceAction,
                onCancelVoice = ::cancelVoiceRecording,
                onRefreshHistory = ::refreshHistory,
                onDeleteHistory = ::deleteHistory,
                learningItem = learningItem,
                learningText = learningText,
                learningMessage = learningMessage,
                learningCandidates = learningCandidates,
                learningWorking = learningWorking,
                onLearningTextChange = { learningText = it },
                onStartLearningReview = ::startLearningReview,
                onAnalyzeLearning = ::analyzeLearning,
                onConfirmLearning = ::confirmLearning,
                onCancelLearningReview = ::cancelLearningReview,
                onReplaySetup = {
                    history.clear()
                    cancelLearningReview()
                    cancelVoiceRecording()
                    sessionStore.clear()
                    gatewaySession = null
                    runtimeStatus = if (BuildConfig.USE_MOCK_GATEWAY) "Preview" else "Unknown"
                    setupComplete = false
                },
            )
        } else {
            SetupFlowScreen(
                accountEmail = gatewaySession?.account?.email,
                runtimeStatus = runtimeStatus,
                onAuthenticate = { email, password, signup ->
                    runCatching {
                        val response = gateway.authenticate(email, password, signup)
                        val saved = GatewaySession(response.token, response.account)
                        sessionStore.write(saved)
                        gatewaySession = saved
                        runtimeStatus = runCatching { gateway.runtimeStatus().readinessLabel }
                            .getOrElse { "unreachable" }
                        response
                    }
                },
                onFinish = {
                    setupComplete = true
                    refreshHistory()
                },
            )
        }
    }
}

@Composable
private fun AirNoteTheme(
    lightMode: Boolean = false,
    content: @Composable () -> Unit,
) {
    val palette = if (lightMode) LightAirNoteColors else DarkAirNoteColors
    val colorScheme = if (lightMode) {
        lightColorScheme(
            background = palette.background,
            surface = palette.surface,
            primary = palette.accent,
            onPrimary = palette.primaryButtonContent,
            onSurface = palette.foreground,
        )
    } else {
        darkColorScheme(
            background = palette.background,
            surface = palette.surface,
            primary = palette.accent,
            onPrimary = palette.primaryButtonContent,
            onSurface = palette.foreground,
        )
    }

    val view = LocalView.current
    SideEffect {
        if (!view.isInEditMode) {
            (view.context as? Activity)?.window?.let { window ->
                window.statusBarColor = palette.background.toArgb()
                window.navigationBarColor = palette.background.toArgb()
                window.decorView.systemUiVisibility = if (lightMode) {
                    View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR or View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR
                } else {
                    0
                }
            }
        }
    }

    CompositionLocalProvider(LocalAirNoteColors provides palette) {
        MaterialTheme(
            colorScheme = colorScheme,
            content = content,
        )
    }
}

@Composable
private fun AirNoteBackground(content: @Composable () -> Unit) {
    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(
                Brush.linearGradient(
                    colors = listOf(
                        AirNotePalette.Background,
                        AirNotePalette.Background2,
                        AirNotePalette.Background,
                    ),
                    start = Offset(900f, 0f),
                    end = Offset(0f, 1800f),
                ),
            ),
    ) {
        content()
    }
}

@Composable
private fun SetupFlowScreen(
    accountEmail: String?,
    runtimeStatus: String,
    onAuthenticate: suspend (String, String, Boolean) -> Result<GatewayAuthResponse>,
    onFinish: () -> Unit,
) {
    var step by rememberSaveable { mutableStateOf(AndroidSetupStep.Welcome) }
    var email by rememberSaveable { mutableStateOf("") }
    var password by rememberSaveable { mutableStateOf("") }
    var signup by rememberSaveable { mutableStateOf(false) }
    var authWorking by rememberSaveable { mutableStateOf(false) }
    var authError by rememberSaveable { mutableStateOf<String?>(null) }
    var privacyAccepted by rememberSaveable { mutableStateOf(false) }
    var micChecked by rememberSaveable { mutableStateOf(false) }
    var bubbleEnabled by rememberSaveable { mutableStateOf(false) }
    var previewState by rememberSaveable { mutableStateOf(AndroidPreviewState.Ready) }

    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val canContinue = when (step) {
        AndroidSetupStep.Account -> BuildConfig.USE_MOCK_GATEWAY || accountEmail != null
        AndroidSetupStep.Privacy -> privacyAccepted
        AndroidSetupStep.Bubble -> bubbleEnabled
        else -> true
    }

    AirNoteBackground {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .statusBarsPadding()
                .navigationBarsPadding()
                .padding(horizontal = 16.dp)
                .padding(top = 18.dp, bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            AppHeader(
                label = if (BuildConfig.USE_MOCK_GATEWAY) "Preview" else "Live",
            )
            ProgressRail(step = step)

            AirNoteCard(padding = 18.dp) {
                Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    SectionLabel(step.eyebrow)
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(
                            text = step.title,
                            color = AirNotePalette.ForegroundFixed,
                            fontSize = 28.sp,
                            fontWeight = FontWeight.SemiBold,
                            lineHeight = 33.sp,
                        )
                        Text(
                            text = step.subtitle,
                            color = AirNotePalette.Muted,
                            fontSize = 15.sp,
                            lineHeight = 22.sp,
                        )
                    }

                    when (step) {
                        AndroidSetupStep.Welcome -> WelcomeStep()
                        AndroidSetupStep.Account -> AccountStep(
                            accountEmail = accountEmail,
                            runtimeStatus = runtimeStatus,
                            email = email,
                            password = password,
                            signup = signup,
                            authWorking = authWorking,
                            authError = authError,
                            onEmailChange = {
                                email = it
                                authError = null
                            },
                            onPasswordChange = {
                                password = it
                                authError = null
                            },
                            onSignupChange = { signup = it },
                            onOpenLarkAuth = {
                                val url = BuildConfig.GATEWAY_BASE_URL.trimEnd('/') + "/auth/lark"
                                context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
                            },
                            onAuthenticate = {
                                authWorking = true
                                authError = null
                                scope.launch {
                                    val result = onAuthenticate(email, password, signup)
                                    authWorking = false
                                    authError = result.exceptionOrNull()?.message
                                }
                            },
                        )
                        AndroidSetupStep.Privacy -> PrivacyStep(
                            accepted = privacyAccepted,
                            onAcceptedChange = { privacyAccepted = it },
                        )
                        AndroidSetupStep.Microphone -> MicrophoneStep(
                            checked = micChecked,
                        )
                        AndroidSetupStep.Bubble -> BubbleStep(
                            enabled = bubbleEnabled,
                            onEnabledChange = { bubbleEnabled = it },
                            onOpenAccessibility = {
                                context.startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
                            },
                        )
                        AndroidSetupStep.Preview -> PreviewStep(
                            state = previewState,
                            onStateChange = { previewState = it },
                        )
                    }
                }
            }

            FooterActions(
                step = step,
                micChecked = micChecked,
                canContinue = canContinue,
                onBack = { step.previous()?.let { step = it } },
                onPrimary = {
                    when (step) {
                        AndroidSetupStep.Welcome,
                        AndroidSetupStep.Privacy,
                        AndroidSetupStep.Bubble,
                        -> step.next()?.let { step = it }
                        AndroidSetupStep.Account -> {
                            if (BuildConfig.USE_MOCK_GATEWAY) {
                                scope.launch {
                                    val result = onAuthenticate("anugra@airnote.preview", "preview-password", false)
                                    authError = result.exceptionOrNull()?.message
                                    if (result.isSuccess) step.next()?.let { step = it }
                                }
                            } else if (accountEmail != null) {
                                step.next()?.let { step = it }
                            }
                        }
                        AndroidSetupStep.Microphone -> {
                            if (micChecked) {
                                step.next()?.let { step = it }
                            } else {
                                micChecked = true
                            }
                        }
                        AndroidSetupStep.Preview -> onFinish()
                    }
                },
            )
        }
    }
}

@Composable
private fun WelcomeStep() {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        SetupRow(Icons.Rounded.Person, "Workspace", "AirNote account, Lark identity, and mobile runtime.", "Ready")
        SetupRow(Icons.Rounded.Mic, "Microphone", "Recording surface and route check.", "Ready")
        SetupRow(Icons.Rounded.Keyboard, "Floating bubble", "Dictate above your existing keyboard.", "Ready")
    }
}

@Composable
private fun AccountStep(
    accountEmail: String?,
    runtimeStatus: String,
    email: String,
    password: String,
    signup: Boolean,
    authWorking: Boolean,
    authError: String?,
    onEmailChange: (String) -> Unit,
    onPasswordChange: (String) -> Unit,
    onSignupChange: (Boolean) -> Unit,
    onOpenLarkAuth: () -> Unit,
    onAuthenticate: () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        SetupRow(
            Icons.Rounded.AccountCircle,
            if (BuildConfig.USE_MOCK_GATEWAY) "Preview workspace" else "AirNote workspace",
            accountEmail ?: "Sign in with your AirNote or Lark workspace account.",
            if (accountEmail == null) "Required" else "Signed",
        )
        SetupRow(Icons.Rounded.Bolt, "Runtime Gateway", "Same control-plane runtime contract as desktop.", runtimeStatus)
        if (!BuildConfig.USE_MOCK_GATEWAY && accountEmail == null) {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .background(AirNotePalette.SurfaceRaised.copy(alpha = 0.52f), RoundedCornerShape(10.dp))
                    .border(1.dp, AirNotePalette.Border, RoundedCornerShape(10.dp))
                    .padding(12.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                OutlinedTextField(
                    value = email,
                    onValueChange = onEmailChange,
                    modifier = Modifier.fillMaxWidth(),
                    singleLine = true,
                    label = { Text("Email") },
                    keyboardOptions = KeyboardOptions(
                        keyboardType = KeyboardType.Email,
                        imeAction = ImeAction.Next,
                    ),
                )
                OutlinedTextField(
                    value = password,
                    onValueChange = onPasswordChange,
                    modifier = Modifier.fillMaxWidth(),
                    singleLine = true,
                    label = { Text("Password") },
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(
                        keyboardType = KeyboardType.Password,
                        imeAction = ImeAction.Done,
                    ),
                )
                ToggleRow(
                    label = "Create new account",
                    checked = signup,
                    onCheckedChange = onSignupChange,
                )
                OutlinedButton(
                    onClick = onOpenLarkAuth,
                    enabled = !authWorking,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(44.dp),
                    shape = RoundedCornerShape(10.dp),
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.Accent),
                    border = BorderStroke(1.dp, AirNotePalette.Accent.copy(alpha = 0.30f)),
                ) {
                    Icon(Icons.Rounded.AccountCircle, contentDescription = null)
                    Spacer(Modifier.width(8.dp))
                    Text("Continue with Lark", fontWeight = FontWeight.SemiBold)
                }
                if (authError != null) {
                    Text(
                        text = authError,
                        color = AirNotePalette.Danger,
                        fontSize = 12.sp,
                        lineHeight = 17.sp,
                    )
                }
                Button(
                    onClick = onAuthenticate,
                    enabled = !authWorking && email.isNotBlank() && password.length >= 8,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(44.dp),
                    shape = RoundedCornerShape(10.dp),
                    colors = ButtonDefaults.buttonColors(
                        containerColor = AirNotePalette.PrimaryButtonFill,
                        contentColor = AirNotePalette.PrimaryButtonContent,
                    ),
                ) {
                    Icon(Icons.Rounded.AccountCircle, contentDescription = null)
                    Spacer(Modifier.width(8.dp))
                    Text(if (authWorking) "Connecting" else if (signup) "Create account" else "Sign in", fontWeight = FontWeight.SemiBold)
                }
            }
        }
    }
}

@Composable
private fun PrivacyStep(
    accepted: Boolean,
    onAcceptedChange: (Boolean) -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SetupRow(Icons.Rounded.Mic, "Visible recording only", "The app records after a user action, never silently.", "Required")
        SetupRow(Icons.Rounded.ContentCopy, "Secure field recovery", "Password, OTP, and payment fields use copy-only recovery.", "Safe")
        SetupRow(Icons.Rounded.History, "Async learning", "Learning never delays text insertion.", "0 ms")
        ToggleRow(
            label = "I agree to these defaults",
            checked = accepted,
            onCheckedChange = onAcceptedChange,
        )
    }
}

@Composable
private fun MicrophoneStep(checked: Boolean) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        AirNoteCard(padding = 14.dp) {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    StatusPill(
                        icon = if (checked) Icons.Rounded.CheckCircle else Icons.Rounded.Mic,
                        text = if (checked) "Mic ready" else "Awaiting check",
                        color = if (checked) AirNotePalette.Success else AirNotePalette.Accent,
                    )
                    Spacer(modifier = Modifier.weight(1f))
                    Text(
                        text = "16 kHz PCM",
                        color = AirNotePalette.Muted,
                        fontSize = 12.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                }
                Waveform(level = if (checked) 0.52f else 0.12f, active = checked)
            }
        }
        SetupRow(Icons.Rounded.Tune, "Audio route", "Phone mic now, headset and route changes in device QA.", if (checked) "OK" else "Preview")
    }
}

@Composable
private fun BubbleStep(
    enabled: Boolean,
    onEnabledChange: (Boolean) -> Unit,
    onOpenAccessibility: () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        SetupRow(Icons.Rounded.Lock, "Accessibility service", "Lets AirNote find focused text fields and insert results.", "Step 1")
        SetupRow(Icons.Rounded.Keyboard, "Bubble over keyboard", "AirNote appears above the keyboard instead of replacing it.", if (enabled) "On" else "Off")
        SetupRow(Icons.Rounded.Bolt, "Battery unrestricted", "Keeps the bubble available after the phone sleeps.", "Step 3")
        ToggleRow(
            label = "Bubble preview enabled",
            checked = enabled,
            onCheckedChange = onEnabledChange,
        )
        OutlinedButton(
            onClick = onOpenAccessibility,
            modifier = Modifier
                .fillMaxWidth()
                .height(44.dp),
            shape = RoundedCornerShape(10.dp),
            colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.ForegroundFixed),
            border = BorderStroke(1.dp, AirNotePalette.BorderStrong),
            contentPadding = ButtonDefaults.ButtonWithIconContentPadding,
        ) {
            Icon(Icons.Rounded.Settings, contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(8.dp))
            Text("Open Android settings", fontWeight = FontWeight.SemiBold)
        }
    }
}

@Composable
private fun PreviewStep(
    state: AndroidPreviewState,
    onStateChange: (AndroidPreviewState) -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SegmentedPreviewControl(state = state, onStateChange = onStateChange)
        AndroidBubbleKeyboardPreview(state = state)
    }
}

@Composable
private fun FooterActions(
    step: AndroidSetupStep,
    micChecked: Boolean,
    canContinue: Boolean,
    onBack: () -> Unit,
    onPrimary: () -> Unit,
) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (step.ordinal > 0) {
            OutlinedButton(
                onClick = onBack,
                modifier = Modifier
                    .width(58.dp)
                    .height(44.dp),
                shape = RoundedCornerShape(10.dp),
                colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.ForegroundFixed),
                border = BorderStroke(1.dp, AirNotePalette.BorderStrong),
                contentPadding = PaddingValues(0.dp),
            ) {
                Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = "Back")
            }
        }

        Button(
            onClick = onPrimary,
            enabled = canContinue,
            modifier = Modifier
                .weight(1f)
                .height(44.dp),
            shape = RoundedCornerShape(10.dp),
            colors = ButtonDefaults.buttonColors(
                containerColor = AirNotePalette.PrimaryButtonFill,
                contentColor = AirNotePalette.PrimaryButtonContent,
                disabledContainerColor = AirNotePalette.PrimaryButtonFill.copy(alpha = 0.42f),
                disabledContentColor = AirNotePalette.PrimaryButtonContent.copy(alpha = 0.62f),
            ),
        ) {
            val icon = when (step) {
                AndroidSetupStep.Microphone -> if (micChecked) Icons.AutoMirrored.Rounded.ArrowForward else Icons.Rounded.Mic
                AndroidSetupStep.Preview -> Icons.Rounded.CheckCircle
                else -> Icons.AutoMirrored.Rounded.ArrowForward
            }
            Icon(icon, contentDescription = null)
            Spacer(Modifier.width(8.dp))
            Text(primaryTitle(step, micChecked), fontWeight = FontWeight.SemiBold)
        }
    }
}

private fun primaryTitle(step: AndroidSetupStep, micChecked: Boolean): String =
    when (step) {
        AndroidSetupStep.Welcome -> "Start setup"
        AndroidSetupStep.Account -> "Use account"
        AndroidSetupStep.Privacy -> "Continue"
        AndroidSetupStep.Microphone -> if (micChecked) "Continue" else "Run mic check"
        AndroidSetupStep.Bubble -> "Preview bubble"
        AndroidSetupStep.Preview -> "Finish setup"
    }

private fun voiceHeadline(phase: AndroidVoicePhase): String =
    when (phase) {
        AndroidVoicePhase.Idle -> "Ready to dictate"
        AndroidVoicePhase.Recording -> "Listening"
        AndroidVoicePhase.Uploading -> "Polishing your words"
        AndroidVoicePhase.Complete -> "Ready to insert"
        AndroidVoicePhase.Error -> "Let's recover that"
    }

private fun voiceButtonTitle(phase: AndroidVoicePhase): String =
    when (phase) {
        AndroidVoicePhase.Recording -> "Stop and polish"
        AndroidVoicePhase.Uploading -> "Polishing..."
        AndroidVoicePhase.Complete -> "Record another"
        AndroidVoicePhase.Error -> "Retry voice session"
        AndroidVoicePhase.Idle -> "Open voice session"
    }

private fun voiceStatusIcon(phase: AndroidVoicePhase): ImageVector =
    when (phase) {
        AndroidVoicePhase.Recording -> Icons.Rounded.Stop
        AndroidVoicePhase.Uploading -> Icons.Rounded.Bolt
        AndroidVoicePhase.Complete -> Icons.Rounded.CheckCircle
        AndroidVoicePhase.Error -> Icons.Rounded.Lock
        AndroidVoicePhase.Idle -> Icons.Rounded.Mic
    }

@Composable
private fun voiceStatusColor(phase: AndroidVoicePhase): Color =
    when (phase) {
        AndroidVoicePhase.Recording -> AirNotePalette.Danger
        AndroidVoicePhase.Uploading -> AirNotePalette.Accent
        AndroidVoicePhase.Complete -> AirNotePalette.Success
        AndroidVoicePhase.Error -> AirNotePalette.Danger
        AndroidVoicePhase.Idle -> AirNotePalette.Accent
    }

private fun androidDeviceId(context: android.content.Context): String =
    Settings.Secure.getString(context.contentResolver, Settings.Secure.ANDROID_ID)
        ?.takeIf { it.isNotBlank() }
        ?: "android-${UUID.randomUUID()}"

private fun RuntimeHistoryItem.learningRecordingId(): String =
    clientRunId ?: runId ?: id

private fun RuntimeHistoryItem.learningAiOutput(): String =
    finalText.ifBlank { displayText }

@Composable
private fun HomeScreen(
    history: List<RuntimeHistoryItem>,
    historyMessage: String,
    accountEmail: String?,
    runtimeStatus: String,
    voicePhase: AndroidVoicePhase,
    voiceMessage: String,
    voiceLevel: Float,
    voiceResult: String?,
    polishPrefs: AndroidPolishPreferences,
    diagnosticsSnapshot: AndroidDiagnosticsSnapshot,
    appearanceMode: AndroidAppearanceMode,
    onAppearanceModeChange: (AndroidAppearanceMode) -> Unit,
    onGatewayPresetChange: (AndroidGatewayPreset) -> Unit,
    onOutputLanguageChange: (AndroidOutputLanguage) -> Unit,
    onSelectedModelChange: (AndroidPolishModel) -> Unit,
    onLearningEnabledChange: (Boolean) -> Unit,
    onAddSafeVocabTerm: (String) -> Boolean,
    onRemoveSafeVocabTerm: (String) -> Unit,
    onVoiceAction: () -> Unit,
    onCancelVoice: () -> Unit,
    onRefreshHistory: () -> Unit,
    onDeleteHistory: (RuntimeHistoryItem) -> Unit,
    learningItem: RuntimeHistoryItem?,
    learningText: String,
    learningMessage: String,
    learningCandidates: List<RuntimeLearningCandidate>,
    learningWorking: Boolean,
    onLearningTextChange: (String) -> Unit,
    onStartLearningReview: (RuntimeHistoryItem) -> Unit,
    onAnalyzeLearning: () -> Unit,
    onConfirmLearning: () -> Unit,
    onCancelLearningReview: () -> Unit,
    onReplaySetup: () -> Unit,
) {
    val context = LocalContext.current
    var vocabDraft by rememberSaveable { mutableStateOf("") }
    var vocabMessage by rememberSaveable { mutableStateOf("Safe vocab terms are sent as existing server hints.") }

    AirNoteBackground {
        Column(
            modifier = Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .statusBarsPadding()
                .navigationBarsPadding()
                .padding(horizontal = 16.dp)
                .padding(top = 18.dp, bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            AppHeader(
                label = if (BuildConfig.USE_MOCK_GATEWAY) "Preview" else "Live",
                trailing = {
                Text(
                    text = "Replay setup",
                    color = AirNotePalette.Muted,
                    fontSize = 11.sp,
                    fontWeight = FontWeight.Bold,
                    modifier = Modifier.clickable(onClick = onReplaySetup),
                )
            })

            AirNoteCard(padding = 18.dp) {
                Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Column(verticalArrangement = Arrangement.spacedBy(5.dp), modifier = Modifier.weight(1f)) {
                            SectionLabel("Dashboard")
                            Text(voiceHeadline(voicePhase), color = AirNotePalette.ForegroundFixed, fontSize = 28.sp, fontWeight = FontWeight.SemiBold)
                            Text(accountEmail ?: "Bubble, mic, and Gateway are ready", color = AirNotePalette.Muted, fontSize = 15.sp)
                        }
                        StatusPill(voiceStatusIcon(voicePhase), voicePhase.label, voiceStatusColor(voicePhase))
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        MiniStat("Runtime", runtimeStatus, Modifier.weight(1f))
                        MiniStat("Model", polishPrefs.selectedModel.label, Modifier.weight(1f))
                        MiniStat("Lang", polishPrefs.outputLanguage.label, Modifier.weight(1f))
                    }
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .background(AirNotePalette.SurfaceRaised.copy(alpha = 0.52f), RoundedCornerShape(10.dp))
                            .border(1.dp, AirNotePalette.Border, RoundedCornerShape(10.dp))
                            .padding(14.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                StatusPill(voiceStatusIcon(voicePhase), voicePhase.label, voiceStatusColor(voicePhase))
                                Spacer(Modifier.weight(1f))
                                Text("16 kHz WAV", color = AirNotePalette.Muted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                            }
                            Waveform(level = voiceLevel.coerceAtLeast(if (voicePhase == AndroidVoicePhase.Recording) 0.08f else 0f), active = voicePhase == AndroidVoicePhase.Recording)
                            Text(
                                text = voiceMessage,
                                color = if (voicePhase == AndroidVoicePhase.Error) AirNotePalette.Danger else AirNotePalette.Muted,
                                fontSize = 13.sp,
                                lineHeight = 18.sp,
                            )
                            if (!voiceResult.isNullOrBlank()) {
                                Text(
                                    text = voiceResult,
                                    color = AirNotePalette.ForegroundFixed,
                                    fontSize = 15.sp,
                                    lineHeight = 21.sp,
                                    fontWeight = FontWeight.Medium,
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .background(AirNotePalette.SurfaceRaised, RoundedCornerShape(10.dp))
                                        .border(1.dp, AirNotePalette.Border, RoundedCornerShape(10.dp))
                                        .padding(10.dp),
                                )
                            }
                        }
                    }
                    Button(
                        onClick = onVoiceAction,
                        enabled = voicePhase != AndroidVoicePhase.Uploading,
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(44.dp),
                        shape = RoundedCornerShape(10.dp),
                        colors = ButtonDefaults.buttonColors(
                            containerColor = AirNotePalette.PrimaryButtonFill,
                            contentColor = AirNotePalette.PrimaryButtonContent,
                        ),
                    ) {
                        Icon(if (voicePhase == AndroidVoicePhase.Recording) Icons.Rounded.Stop else Icons.Rounded.Mic, contentDescription = null)
                        Spacer(Modifier.width(8.dp))
                        Text(voiceButtonTitle(voicePhase), fontWeight = FontWeight.SemiBold)
                    }
                    if (voicePhase == AndroidVoicePhase.Recording) {
                        OutlinedButton(
                            onClick = onCancelVoice,
                            modifier = Modifier
                                .fillMaxWidth()
                                .height(40.dp),
                            shape = RoundedCornerShape(10.dp),
                            colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.Danger),
                            border = BorderStroke(1.dp, AirNotePalette.Danger.copy(alpha = 0.35f)),
                        ) {
                            Text("Cancel recording", fontWeight = FontWeight.SemiBold)
                        }
                    }
                }
            }

            AirNoteCard(padding = 14.dp) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        SectionLabel("Polish Controls")
                        Spacer(Modifier.weight(1f))
                        Text("Bubble + app", color = AirNotePalette.Muted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                    }
                    OutputLanguageControl(
                        selected = polishPrefs.outputLanguage,
                        onSelected = onOutputLanguageChange,
                    )
                    PolishModelControl(
                        selected = polishPrefs.selectedModel,
                        onSelected = onSelectedModelChange,
                    )
                    SetupRow(
                        Icons.Rounded.Tune,
                        "Tone and rewrite",
                        "The WAV endpoint uses the account's server-default tone. Shorter/formal rewrite stays disabled until this endpoint supports it.",
                        "Server",
                    )
                }
            }

            AirNoteCard(padding = 14.dp) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        SectionLabel("Words")
                        Spacer(Modifier.weight(1f))
                        Text("${polishPrefs.safeVocabTerms.size}/50", color = AirNotePalette.Accent, fontSize = 12.sp, fontWeight = FontWeight.Bold)
                    }
                    Text(vocabMessage, color = AirNotePalette.Muted, fontSize = 12.sp, lineHeight = 17.sp)
                    OutlinedTextField(
                        value = vocabDraft,
                        onValueChange = { vocabDraft = it },
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                        label = { Text("Name, acronym, or word") },
                        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                    )
                    Button(
                        onClick = {
                            val added = onAddSafeVocabTerm(vocabDraft)
                            vocabMessage = if (added) {
                                "Vocabulary hint added for future Android requests."
                            } else {
                                "Enter a new term between 2 and 80 characters."
                            }
                            if (added) vocabDraft = ""
                        },
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(40.dp),
                        shape = RoundedCornerShape(10.dp),
                        colors = ButtonDefaults.buttonColors(
                            containerColor = AirNotePalette.PrimaryButtonFill,
                            contentColor = AirNotePalette.PrimaryButtonContent,
                        ),
                    ) {
                        Icon(Icons.Rounded.Language, contentDescription = null, modifier = Modifier.size(17.dp))
                        Spacer(Modifier.width(8.dp))
                        Text("Add vocabulary hint", fontWeight = FontWeight.SemiBold)
                    }
                    if (polishPrefs.safeVocabTerms.isEmpty()) {
                        SetupRow(Icons.Rounded.Language, "No local Android hints", "Server vocabulary still applies through the existing account memory.", "Empty")
                    } else {
                        polishPrefs.safeVocabTerms.forEach { term ->
                            VocabTermRow(term = term, onRemove = { onRemoveSafeVocabTerm(term) })
                        }
                    }
                }
            }

            AirNoteCard(padding = 14.dp) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        SectionLabel("Settings")
                        Spacer(Modifier.weight(1f))
                        Text(appearanceMode.detail, color = AirNotePalette.Muted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                    }
                    SetupRow(
                        Icons.Rounded.Settings,
                        "Appearance",
                        "Match the phone theme or choose AirNote's theme.",
                        appearanceMode.label,
                    )
                    AppearancePreferenceControl(
                        selected = appearanceMode,
                        onSelected = onAppearanceModeChange,
                    )
                    GatewayPresetControl(
                        selectedUrl = polishPrefs.gatewayBaseUrl,
                        onSelected = onGatewayPresetChange,
                    )
                    ToggleRow(
                        label = "Explicit learning review enabled",
                        checked = polishPrefs.learningEnabled,
                        onCheckedChange = onLearningEnabledChange,
                    )
                }
            }

            AirNoteCard(padding = 14.dp) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        SectionLabel("Setup")
                        Spacer(Modifier.weight(1f))
                        Text("4/4", color = AirNotePalette.Accent, fontSize = 12.sp, fontWeight = FontWeight.Bold)
                    }
                    SetupRow(Icons.Rounded.Person, "Workspace", "AirNote workspace ready.", "Done")
                    SetupRow(Icons.Rounded.Mic, "Microphone", "Health check completed.", "Done")
                    SetupRow(Icons.Rounded.Keyboard, "Floating bubble", "Accessibility preview completed.", "Done")
                    OutlinedButton(
                        onClick = onReplaySetup,
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(44.dp),
                        shape = RoundedCornerShape(10.dp),
                        colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.ForegroundFixed),
                        border = BorderStroke(1.dp, AirNotePalette.BorderStrong),
                    ) {
                        Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = null)
                        Spacer(Modifier.width(8.dp))
                        Text("Run setup flow from the beginning", fontWeight = FontWeight.SemiBold)
                    }
                }
            }

            AirNoteCard(padding = 14.dp) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        SectionLabel("Server History")
                        Spacer(Modifier.weight(1f))
                        Text(
                            text = if (history.isEmpty()) "0" else history.size.toString(),
                            color = AirNotePalette.Accent,
                            fontSize = 12.sp,
                            fontWeight = FontWeight.Bold,
                        )
                    }
                    Text(
                        text = historyMessage,
                        color = AirNotePalette.Muted,
                        fontSize = 12.sp,
                        lineHeight = 17.sp,
                    )
                    if (history.isEmpty()) {
                        SetupRow(Icons.Rounded.History, "No saved dictations", "Server-retained results appear here after dictation.", "Empty")
                    } else {
                        if (learningItem != null) {
                            LearningReviewPanel(
                                item = learningItem,
                                text = learningText,
                                message = learningMessage,
                                candidates = learningCandidates,
                                working = learningWorking,
                                onTextChange = onLearningTextChange,
                                onAnalyze = onAnalyzeLearning,
                                onConfirm = onConfirmLearning,
                                onCancel = onCancelLearningReview,
                            )
                        }
                        history.take(3).forEach { item ->
                            HistoryRow(
                                item = item,
                                onCopy = {
                                    val clipboard = context.getSystemService(ClipboardManager::class.java)
                                    clipboard?.setPrimaryClip(ClipData.newPlainText("AirNote", item.displayText))
                                },
                                onLearn = {
                                    if (polishPrefs.learningEnabled) {
                                        onStartLearningReview(item)
                                    } else {
                                        vocabMessage = "Learning review is off in Android settings."
                                    }
                                },
                                onDelete = { onDeleteHistory(item) },
                            )
                        }
                    }
                    OutlinedButton(
                        onClick = onRefreshHistory,
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(40.dp),
                        shape = RoundedCornerShape(10.dp),
                        colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.ForegroundFixed),
                        border = BorderStroke(1.dp, AirNotePalette.BorderStrong),
                    ) {
                        Icon(Icons.Rounded.History, contentDescription = null, modifier = Modifier.size(17.dp))
                        Spacer(Modifier.width(8.dp))
                        Text("Refresh server history", fontWeight = FontWeight.SemiBold)
                    }
                }
            }

            if (history.isNotEmpty()) {
                SectionLabel("Recent")
                history.take(2).forEach { item ->
                    AirNoteCard(padding = 14.dp) {
                        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(item.displayText, color = AirNotePalette.ForegroundFixed, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                StatusPill(Icons.Rounded.CheckCircle, item.source, AirNotePalette.Success)
                                Spacer(Modifier.weight(1f))
                                Text(item.createdAt.ifBlank { "Server" }, color = AirNotePalette.Muted, fontSize = 12.sp)
                            }
                        }
                    }
                }
            }

            AirNoteCard(padding = 14.dp) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        SectionLabel("Diagnostics")
                        Spacer(Modifier.weight(1f))
                        Text(BuildConfig.VERSION_NAME, color = AirNotePalette.Accent, fontSize = 12.sp, fontWeight = FontWeight.Bold)
                    }
                    DiagnosticRow("Server", diagnosticsSnapshot.serverUrl)
                    DiagnosticRow("Auth", diagnosticsSnapshot.authState)
                    DiagnosticRow("Mic", diagnosticsSnapshot.micPermission)
                    DiagnosticRow("Accessibility", if (diagnosticsSnapshot.accessibilityEnabled) "enabled" else "missing")
                    DiagnosticRow("Audio route", diagnosticsSnapshot.audioRoute)
                    DiagnosticRow("Last request", diagnosticsSnapshot.lastRequestId.ifBlank { "none" })
                    DiagnosticRow("Last latency", "${diagnosticsSnapshot.lastLatencyMs} ms")
                    DiagnosticRow("Last insert", diagnosticsSnapshot.lastInsertionResult)
                    DiagnosticRow("Last failure", diagnosticsSnapshot.lastFailure.ifBlank { "none" })
                }
            }

            SectionLabel("Tools")
            ToolRow(Icons.Rounded.Tune, "Language and model", "${polishPrefs.outputLanguage.label}, ${polishPrefs.selectedModel.label}")
            ToolRow(Icons.Rounded.History, "History", "Copy, retry, and recover")
            ToolRow(Icons.Rounded.Language, "Vocabulary", "Names, aliases, and terms")
            ToolRow(Icons.Rounded.Settings, "Settings", "Privacy, Gateway, diagnostics")
        }
    }
}

@Composable
private fun OutputLanguageControl(
    selected: AndroidOutputLanguage,
    onSelected: (AndroidOutputLanguage) -> Unit,
) {
    PreferenceSegmentedControl(
        title = "Language",
        items = AndroidOutputLanguage.entries,
        selected = selected,
        label = { it.label },
        detail = { it.detail },
        onSelected = onSelected,
    )
}

@Composable
private fun PolishModelControl(
    selected: AndroidPolishModel,
    onSelected: (AndroidPolishModel) -> Unit,
) {
    PreferenceSegmentedControl(
        title = "Model",
        items = AndroidPolishModel.entries,
        selected = selected,
        label = { it.label },
        detail = { it.detail },
        onSelected = onSelected,
    )
}

@Composable
private fun GatewayPresetControl(
    selectedUrl: String,
    onSelected: (AndroidGatewayPreset) -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SetupRow(
            Icons.Rounded.Bolt,
            "Gateway",
            normalizeGatewayUrl(selectedUrl),
            AndroidGatewayPreset.fromUrl(selectedUrl)?.label ?: "Custom",
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
            AndroidGatewayPreset.entries.forEach { preset ->
                val active = normalizeGatewayUrl(preset.url) == normalizeGatewayUrl(selectedUrl)
                OutlinedButton(
                    onClick = { onSelected(preset) },
                    modifier = Modifier
                        .weight(1f)
                        .height(38.dp),
                    shape = RoundedCornerShape(9.dp),
                    colors = ButtonDefaults.outlinedButtonColors(
                        contentColor = if (active) AirNotePalette.PrimaryButtonContent else AirNotePalette.ForegroundFixed,
                        containerColor = if (active) AirNotePalette.PrimaryButtonFill else Color.Transparent,
                    ),
                    border = BorderStroke(1.dp, if (active) AirNotePalette.PrimaryButtonFill else AirNotePalette.BorderStrong),
                    contentPadding = PaddingValues(horizontal = 6.dp, vertical = 0.dp),
                ) {
                    Text(preset.label, fontSize = 12.sp, fontWeight = FontWeight.Bold, maxLines = 1)
                }
            }
        }
    }
}

@Composable
private fun <T> PreferenceSegmentedControl(
    title: String,
    items: List<T>,
    selected: T,
    label: (T) -> String,
    detail: (T) -> String,
    onSelected: (T) -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(title, color = AirNotePalette.ForegroundFixed, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.weight(1f))
            Text(detail(selected), color = AirNotePalette.Muted, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
            items.forEach { item ->
                val active = item == selected
                OutlinedButton(
                    onClick = { onSelected(item) },
                    modifier = Modifier
                        .weight(1f)
                        .height(38.dp),
                    shape = RoundedCornerShape(9.dp),
                    colors = ButtonDefaults.outlinedButtonColors(
                        contentColor = if (active) AirNotePalette.PrimaryButtonContent else AirNotePalette.ForegroundFixed,
                        containerColor = if (active) AirNotePalette.PrimaryButtonFill else Color.Transparent,
                    ),
                    border = BorderStroke(1.dp, if (active) AirNotePalette.PrimaryButtonFill else AirNotePalette.BorderStrong),
                    contentPadding = PaddingValues(horizontal = 4.dp, vertical = 0.dp),
                ) {
                    Text(label(item), fontSize = 12.sp, fontWeight = FontWeight.Bold, maxLines = 1)
                }
            }
        }
    }
}

@Composable
private fun VocabTermRow(
    term: String,
    onRemove: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(AirNotePalette.SurfaceRaised.copy(alpha = 0.52f), RoundedCornerShape(10.dp))
            .border(1.dp, AirNotePalette.Border, RoundedCornerShape(10.dp))
            .padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(
            modifier = Modifier
                .size(32.dp)
                .background(AirNotePalette.Accent.copy(alpha = 0.12f), RoundedCornerShape(8.dp)),
            contentAlignment = Alignment.Center,
        ) {
            Text(term.take(1).uppercase(), color = AirNotePalette.Accent, fontWeight = FontWeight.Bold)
        }
        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(term, color = AirNotePalette.ForegroundFixed, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            Text("Sent as safe_vocab_terms on Android voice requests", color = AirNotePalette.Muted, fontSize = 12.sp)
        }
        OutlinedButton(
            onClick = onRemove,
            modifier = Modifier.height(32.dp),
            shape = RoundedCornerShape(8.dp),
            colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.Danger),
            border = BorderStroke(1.dp, AirNotePalette.Danger.copy(alpha = 0.35f)),
            contentPadding = PaddingValues(horizontal = 9.dp, vertical = 0.dp),
        ) {
            Icon(Icons.Rounded.Delete, contentDescription = "Remove vocabulary hint", modifier = Modifier.size(15.dp))
        }
    }
}

@Composable
private fun DiagnosticRow(
    label: String,
    value: String,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(AirNotePalette.SurfaceRaised.copy(alpha = 0.52f), RoundedCornerShape(10.dp))
            .border(1.dp, AirNotePalette.Border, RoundedCornerShape(10.dp))
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(label, color = AirNotePalette.Muted, fontSize = 12.sp, fontWeight = FontWeight.Bold, modifier = Modifier.width(104.dp))
        Text(
            value,
            color = AirNotePalette.ForegroundFixed,
            fontSize = 12.sp,
            lineHeight = 16.sp,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
    }
}

@Composable
private fun HistoryRow(
    item: RuntimeHistoryItem,
    onCopy: () -> Unit,
    onLearn: () -> Unit,
    onDelete: () -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(AirNotePalette.SurfaceRaised.copy(alpha = 0.52f), RoundedCornerShape(10.dp))
            .border(1.dp, AirNotePalette.Border, RoundedCornerShape(10.dp))
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            text = item.displayText,
            color = AirNotePalette.ForegroundFixed,
            fontSize = 14.sp,
            lineHeight = 20.sp,
            fontWeight = FontWeight.Medium,
        )
        if (item.transcript.isNotBlank() && item.transcript != item.displayText) {
            Text(
                text = item.transcript,
                color = AirNotePalette.Muted,
                fontSize = 12.sp,
                lineHeight = 17.sp,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            StatusPill(Icons.Rounded.CheckCircle, item.platform.ifBlank { "mobile" }, AirNotePalette.Success)
            Text(item.createdAt.ifBlank { "Server" }, color = AirNotePalette.Muted, fontSize = 11.sp, modifier = Modifier.weight(1f))
            OutlinedButton(
                onClick = onCopy,
                modifier = Modifier.height(32.dp),
                shape = RoundedCornerShape(8.dp),
                colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.ForegroundFixed),
                border = BorderStroke(1.dp, AirNotePalette.BorderStrong),
                contentPadding = PaddingValues(horizontal = 9.dp, vertical = 0.dp),
            ) {
                Icon(Icons.Rounded.ContentCopy, contentDescription = "Copy", modifier = Modifier.size(15.dp))
            }
            OutlinedButton(
                onClick = onLearn,
                modifier = Modifier.height(32.dp),
                shape = RoundedCornerShape(8.dp),
                colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.Accent),
                border = BorderStroke(1.dp, AirNotePalette.Accent.copy(alpha = 0.30f)),
                contentPadding = PaddingValues(horizontal = 9.dp, vertical = 0.dp),
            ) {
                Icon(Icons.Rounded.CheckCircle, contentDescription = "Review learning", modifier = Modifier.size(15.dp))
            }
            OutlinedButton(
                onClick = onDelete,
                modifier = Modifier.height(32.dp),
                shape = RoundedCornerShape(8.dp),
                colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.Danger),
                border = BorderStroke(1.dp, AirNotePalette.Danger.copy(alpha = 0.35f)),
                contentPadding = PaddingValues(horizontal = 9.dp, vertical = 0.dp),
            ) {
                Icon(Icons.Rounded.Delete, contentDescription = "Delete", modifier = Modifier.size(15.dp))
            }
        }
    }
}

@Composable
private fun LearningReviewPanel(
    item: RuntimeHistoryItem,
    text: String,
    message: String,
    candidates: List<RuntimeLearningCandidate>,
    working: Boolean,
    onTextChange: (String) -> Unit,
    onAnalyze: () -> Unit,
    onConfirm: () -> Unit,
    onCancel: () -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(AirNotePalette.SurfaceRaised.copy(alpha = 0.52f), RoundedCornerShape(10.dp))
            .border(1.dp, AirNotePalette.BorderStrong, RoundedCornerShape(10.dp))
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    text = "Learning review",
                    color = AirNotePalette.ForegroundFixed,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    text = item.createdAt.ifBlank { item.learningRecordingId() },
                    color = AirNotePalette.Muted,
                    fontSize = 12.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            StatusPill(Icons.Rounded.Bolt, if (working) "Working" else "Review", AirNotePalette.Accent)
        }
        OutlinedTextField(
            value = text,
            onValueChange = onTextChange,
            modifier = Modifier.fillMaxWidth(),
            minLines = 3,
            label = { Text("Kept text") },
        )
        if (item.transcript.isNotBlank() && item.transcript != item.displayText) {
            Text(
                text = item.transcript,
                color = AirNotePalette.Muted,
                fontSize = 12.sp,
                lineHeight = 17.sp,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Text(
            text = message,
            color = if (message.startsWith("Could not") || message.startsWith("Kept")) AirNotePalette.Danger else AirNotePalette.Muted,
            fontSize = 12.sp,
            lineHeight = 17.sp,
        )
        candidates.forEach { candidate ->
            SetupRow(
                icon = Icons.Rounded.CheckCircle,
                title = candidate.corrected.ifBlank { "Candidate" },
                subtitle = "${candidate.original} -> ${candidate.termType}",
                status = if (candidate.learnable) "Ready" else "Blocked",
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
            OutlinedButton(
                onClick = onCancel,
                enabled = !working,
                modifier = Modifier
                    .weight(1f)
                    .height(40.dp),
                shape = RoundedCornerShape(10.dp),
                colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.ForegroundFixed),
                border = BorderStroke(1.dp, AirNotePalette.BorderStrong),
            ) {
                Text("Close", fontWeight = FontWeight.SemiBold)
            }
            OutlinedButton(
                onClick = onAnalyze,
                enabled = !working,
                modifier = Modifier
                    .weight(1f)
                    .height(40.dp),
                shape = RoundedCornerShape(10.dp),
                colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.Accent),
                border = BorderStroke(1.dp, AirNotePalette.Accent.copy(alpha = 0.30f)),
            ) {
                Text(if (working) "Analyzing" else "Analyze", fontWeight = FontWeight.SemiBold)
            }
            Button(
                onClick = onConfirm,
                enabled = !working && candidates.any { it.learnable },
                modifier = Modifier
                    .weight(1f)
                    .height(40.dp),
                shape = RoundedCornerShape(10.dp),
                colors = ButtonDefaults.buttonColors(
                    containerColor = AirNotePalette.PrimaryButtonFill,
                    contentColor = AirNotePalette.PrimaryButtonContent,
                ),
            ) {
                Text("Learn", fontWeight = FontWeight.SemiBold)
            }
        }
    }
}

@Composable
private fun AppHeader(
    label: String,
    trailing: @Composable (() -> Unit)? = null,
) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        LogoTile(44.dp)
        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text("AirNote", color = AirNotePalette.ForegroundFixed, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
            Text("Voice Polish Studio", color = AirNotePalette.Muted, fontSize = 13.sp, maxLines = 1)
        }
        Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(6.dp)) {
            StatusPill(Icons.Rounded.Bolt, label, AirNotePalette.Accent)
            trailing?.invoke()
        }
    }
}

@Composable
private fun AppearancePreferenceControl(
    selected: AndroidAppearanceMode,
    onSelected: (AndroidAppearanceMode) -> Unit,
) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
        AndroidAppearanceMode.entries.forEach { mode ->
            val active = mode == selected
            OutlinedButton(
                onClick = { onSelected(mode) },
                modifier = Modifier
                    .weight(1f)
                    .height(38.dp),
                shape = RoundedCornerShape(9.dp),
                colors = ButtonDefaults.outlinedButtonColors(
                    contentColor = if (active) AirNotePalette.PrimaryButtonContent else AirNotePalette.ForegroundFixed,
                    containerColor = if (active) AirNotePalette.PrimaryButtonFill else Color.Transparent,
                ),
                border = BorderStroke(1.dp, if (active) AirNotePalette.PrimaryButtonFill else AirNotePalette.BorderStrong),
                contentPadding = PaddingValues(horizontal = 4.dp, vertical = 0.dp),
            ) {
                Icon(appearanceIcon(mode), contentDescription = null, modifier = Modifier.size(15.dp))
                Spacer(Modifier.width(5.dp))
                Text(mode.label, fontSize = 12.sp, fontWeight = FontWeight.Bold, maxLines = 1)
            }
        }
    }
}

private fun appearanceIcon(mode: AndroidAppearanceMode): ImageVector =
    when (mode) {
        AndroidAppearanceMode.System -> Icons.Rounded.Settings
        AndroidAppearanceMode.Light -> Icons.Rounded.LightMode
        AndroidAppearanceMode.Dark -> Icons.Rounded.DarkMode
    }

@Composable
private fun ProgressRail(step: AndroidSetupStep) {
    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.fillMaxWidth()) {
        AndroidSetupStep.ordered.forEach { item ->
            Box(
                modifier = Modifier
                    .height(4.dp)
                    .weight(1f)
                    .clip(RoundedCornerShape(50))
                    .background(if (item.ordinal <= step.ordinal) AirNotePalette.ForegroundFixed else AirNotePalette.SurfaceHover),
            )
        }
    }
}

@Composable
private fun AirNoteCard(
    padding: Dp,
    content: @Composable () -> Unit,
) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(12.dp),
        color = AirNotePalette.Surface.copy(alpha = 0.92f),
        border = BorderStroke(1.dp, AirNotePalette.Border),
        tonalElevation = 0.dp,
        shadowElevation = 0.dp,
    ) {
        Box(Modifier.padding(padding)) {
            content()
        }
    }
}

@Composable
private fun SetupRow(
    icon: ImageVector,
    title: String,
    subtitle: String,
    status: String,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(AirNotePalette.SurfaceRaised.copy(alpha = 0.52f), RoundedCornerShape(10.dp))
            .border(1.dp, AirNotePalette.Border, RoundedCornerShape(10.dp))
            .padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(
            modifier = Modifier
                .size(34.dp)
                .background(AirNotePalette.ForegroundFixed.copy(alpha = 0.045f), RoundedCornerShape(8.dp))
                .border(1.dp, AirNotePalette.Border, RoundedCornerShape(8.dp)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, contentDescription = null, tint = AirNotePalette.Accent, modifier = Modifier.size(17.dp))
        }
        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, color = AirNotePalette.ForegroundFixed, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            Text(subtitle, color = AirNotePalette.Muted, fontSize = 13.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        Text(
            text = status,
            color = AirNotePalette.Accent,
            fontSize = 12.sp,
            fontWeight = FontWeight.Bold,
            modifier = Modifier
                .background(AirNotePalette.Accent.copy(alpha = 0.12f), RoundedCornerShape(7.dp))
                .padding(horizontal = 8.dp, vertical = 5.dp),
        )
    }
}

@Composable
private fun ToggleRow(
    label: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(AirNotePalette.SurfaceRaised.copy(alpha = 0.52f), RoundedCornerShape(10.dp))
            .border(1.dp, AirNotePalette.Border, RoundedCornerShape(10.dp))
            .padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, color = AirNotePalette.ForegroundFixed, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
        Switch(
            checked = checked,
            onCheckedChange = onCheckedChange,
            colors = SwitchDefaults.colors(
                checkedThumbColor = AirNotePalette.ForegroundFixed,
                checkedTrackColor = AirNotePalette.Accent.copy(alpha = 0.56f),
                uncheckedThumbColor = AirNotePalette.Muted,
                uncheckedTrackColor = AirNotePalette.SurfaceHover,
            ),
        )
    }
}

@Composable
private fun StatusPill(
    icon: ImageVector,
    text: String,
    color: Color,
) {
    Row(
        modifier = Modifier
            .background(color.copy(alpha = 0.14f), RoundedCornerShape(7.dp))
            .border(1.dp, color.copy(alpha = 0.20f), RoundedCornerShape(7.dp))
            .padding(horizontal = 9.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(Modifier.size(6.dp).background(color, CircleShape))
        Icon(icon, contentDescription = null, tint = color, modifier = Modifier.size(13.dp))
        Text(text, color = color, fontSize = 12.sp, fontWeight = FontWeight.Bold)
    }
}

@Composable
private fun LogoTile(tileSize: Dp) {
    val markColor = AirNotePalette.ForegroundFixed
    Box(
        modifier = Modifier
            .size(tileSize)
            .background(
                Brush.linearGradient(listOf(AirNotePalette.SurfaceRaised, AirNotePalette.Surface)),
                RoundedCornerShape(tileSize * 0.24f),
            )
            .border(1.dp, AirNotePalette.BorderStrong, RoundedCornerShape(tileSize * 0.24f)),
        contentAlignment = Alignment.Center,
    ) {
        Canvas(modifier = Modifier.size(tileSize * 0.46f)) {
            val heights = listOf(0.38f, 0.78f, 0.55f, 0.92f)
            val barWidth = this.size.width * 0.15f
            val gap = this.size.width * 0.11f
            val total = heights.size * barWidth + (heights.size - 1) * gap
            var x = (this.size.width - total) / 2f
            heights.forEach { fraction ->
                val h = this.size.height * fraction
                drawRoundRect(
                    color = markColor,
                    topLeft = Offset(x, (this.size.height - h) / 2f),
                    size = Size(barWidth, h),
                    cornerRadius = CornerRadius(barWidth / 2f, barWidth / 2f),
                )
                x += barWidth + gap
            }
        }
    }
}

@Composable
private fun SectionLabel(text: String) {
    Text(
        text = text.uppercase(),
        color = AirNotePalette.Muted,
        fontSize = 12.sp,
        fontWeight = FontWeight.Bold,
    )
}

@Composable
private fun Waveform(
    level: Float,
    active: Boolean,
    modifier: Modifier = Modifier,
) {
    val waveColor = AirNotePalette.Accent.copy(alpha = if (active) 0.95f else 0.48f)
    Canvas(
        modifier = modifier
            .fillMaxWidth()
            .height(48.dp),
    ) {
        val bars = 7
        val gap = 6.dp.toPx()
        val width = 6.dp.toPx()
        val total = bars * width + (bars - 1) * gap
        var x = (size.width - total) / 2f
        repeat(bars) { index ->
            val base = 12 + ((index * 7) % 20)
            val h = if (active) base + level * 26f else base.toFloat()
            drawRoundRect(
                color = waveColor,
                topLeft = Offset(x, (size.height - h) / 2f),
                size = Size(width, h),
                cornerRadius = CornerRadius(width / 2f, width / 2f),
            )
            x += width + gap
        }
    }
}

@Composable
private fun SegmentedPreviewControl(
    state: AndroidPreviewState,
    onStateChange: (AndroidPreviewState) -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(44.dp)
            .background(AirNotePalette.SurfaceHover, RoundedCornerShape(22.dp))
            .padding(3.dp),
        horizontalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        AndroidPreviewState.entries.forEach { item ->
            val selected = item == state
            val color by animateColorAsState(
                targetValue = if (selected) AirNotePalette.ForegroundFixed.copy(alpha = 0.16f) else Color.Transparent,
                label = "segment-color",
            )
            Box(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxSize()
                    .background(color, RoundedCornerShape(19.dp))
                    .clickable { onStateChange(item) },
                contentAlignment = Alignment.Center,
            ) {
                Text(item.label, color = AirNotePalette.ForegroundFixed, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            }
        }
    }
}

@Composable
private fun AndroidBubbleKeyboardPreview(state: AndroidPreviewState) {
    val tint = when (state) {
        AndroidPreviewState.Listening -> AirNotePalette.Danger
        AndroidPreviewState.Insert -> AirNotePalette.Success
        else -> AirNotePalette.Accent
    }

    Box(
        modifier = Modifier
            .fillMaxWidth()
            .background(AirNotePalette.KeyboardWell, RoundedCornerShape(16.dp))
            .border(1.dp, AirNotePalette.BorderStrong, RoundedCornerShape(16.dp))
            .padding(10.dp),
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            MockAppSurface(state = state, tint = tint)
            KeyboardMock()
        }

        FloatingBubble(
            state = state,
            tint = tint,
            modifier = Modifier
                .align(Alignment.BottomEnd)
                .offset(x = (-18).dp, y = (-132).dp),
        )
    }
}

@Composable
private fun MockAppSurface(
    state: AndroidPreviewState,
    tint: Color,
) {
    Surface(
        shape = RoundedCornerShape(12.dp),
        color = AirNotePalette.Surface,
        border = BorderStroke(1.dp, AirNotePalette.BorderStrong),
    ) {
        Column(
            modifier = Modifier.padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("Messages", color = AirNotePalette.ForegroundFixed, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                    Text("Rahul", color = AirNotePalette.Muted, fontSize = 12.sp)
                }
                StatusPill(Icons.Rounded.Bolt, "Work", AirNotePalette.Accent)
            }
            Box(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(78.dp)
                    .background(AirNotePalette.SurfaceRaised, RoundedCornerShape(10.dp))
                    .border(1.dp, AirNotePalette.Border, RoundedCornerShape(10.dp))
                    .padding(10.dp),
            ) {
                Text(
                    text = when (state) {
                        AndroidPreviewState.Ready, AndroidPreviewState.Listening -> "Type a message..."
                        AndroidPreviewState.Insert -> "Kal ka update concise bana ke Rahul ko bhej do."
                        AndroidPreviewState.CopyOnly -> "Secure field detected"
                    },
                    color = if (state == AndroidPreviewState.Ready || state == AndroidPreviewState.Listening) AirNotePalette.Muted else AirNotePalette.ForegroundFixed,
                    fontSize = 14.sp,
                    lineHeight = 19.sp,
                )
                Canvas(modifier = Modifier.matchParentSize()) {
                    if (state == AndroidPreviewState.Listening) {
                        drawLine(
                            color = tint.copy(alpha = 0.45f),
                            start = Offset(10.dp.toPx(), size.height - 12.dp.toPx()),
                            end = Offset(size.width - 10.dp.toPx(), size.height - 12.dp.toPx()),
                            strokeWidth = 1.dp.toPx(),
                            pathEffect = PathEffect.dashPathEffect(floatArrayOf(8f, 8f)),
                            cap = StrokeCap.Round,
                        )
                    }
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Icon(if (state == AndroidPreviewState.Listening) Icons.Rounded.Stop else Icons.Rounded.PlayArrow, contentDescription = null, tint = tint, modifier = Modifier.size(18.dp))
                Text(state.subtitle, color = AirNotePalette.Muted, fontSize = 12.sp, maxLines = 2)
            }
        }
    }
}

@Composable
private fun FloatingBubble(
    state: AndroidPreviewState,
    tint: Color,
    modifier: Modifier,
) {
    Surface(
        modifier = modifier,
        shape = RoundedCornerShape(24.dp),
        color = if (state == AndroidPreviewState.Listening) tint else AirNotePalette.SurfaceRaised,
        border = BorderStroke(1.dp, if (state == AndroidPreviewState.Listening) tint.copy(alpha = 0.65f) else AirNotePalette.BorderStrong),
    ) {
        Row(
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 9.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(
                imageVector = when (state) {
                    AndroidPreviewState.Listening -> Icons.Rounded.Stop
                    AndroidPreviewState.Insert -> Icons.Rounded.CheckCircle
                    AndroidPreviewState.CopyOnly -> Icons.Rounded.ContentCopy
                    else -> Icons.Rounded.Mic
                },
                contentDescription = null,
                tint = if (state == AndroidPreviewState.Listening) Color.White else tint,
                modifier = Modifier.size(18.dp),
            )
            Text(
                text = when (state) {
                    AndroidPreviewState.Listening -> "Stop"
                    AndroidPreviewState.Insert -> "Insert"
                    AndroidPreviewState.CopyOnly -> "Copy"
                    else -> "AirNote"
                },
                color = if (state == AndroidPreviewState.Listening) Color.White else AirNotePalette.ForegroundFixed,
                fontSize = 13.sp,
                fontWeight = FontWeight.Bold,
            )
        }
    }
}

@Composable
private fun KeyboardMock() {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(AirNotePalette.Surface, RoundedCornerShape(12.dp))
            .border(1.dp, AirNotePalette.BorderStrong, RoundedCornerShape(12.dp))
            .padding(7.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        listOf("QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM").forEach { row ->
            Row(horizontalArrangement = Arrangement.spacedBy(5.dp), modifier = Modifier.fillMaxWidth()) {
                row.forEach { key ->
                    KeyButton(key.toString(), Modifier.weight(1f))
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.fillMaxWidth()) {
            KeyButton("lang", Modifier.width(56.dp), Icons.Rounded.Language)
            KeyButton("space", Modifier.weight(1f))
            KeyButton("del", Modifier.width(58.dp), Icons.Rounded.Delete)
        }
    }
}

@Composable
private fun KeyButton(
    label: String,
    modifier: Modifier,
    icon: ImageVector? = null,
) {
    Box(
        modifier = modifier
            .height(34.dp)
            .background(AirNotePalette.SurfaceRaised, RoundedCornerShape(8.dp))
            .border(1.dp, AirNotePalette.Border, RoundedCornerShape(8.dp)),
        contentAlignment = Alignment.Center,
    ) {
        if (icon != null) {
            Icon(icon, contentDescription = null, tint = AirNotePalette.ForegroundFixed, modifier = Modifier.size(15.dp))
        } else {
            Text(label, color = AirNotePalette.ForegroundFixed, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

@Composable
private fun MiniStat(
    title: String,
    value: String,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier
            .background(AirNotePalette.SurfaceRaised.copy(alpha = 0.52f), RoundedCornerShape(10.dp))
            .border(1.dp, AirNotePalette.Border, RoundedCornerShape(10.dp))
            .padding(10.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(title, color = AirNotePalette.Muted, fontSize = 11.sp, fontWeight = FontWeight.Bold)
        Text(value, color = AirNotePalette.ForegroundFixed, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
    }
}

@Composable
private fun ToolRow(
    icon: ImageVector,
    title: String,
    subtitle: String,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(AirNotePalette.Surface.copy(alpha = 0.92f), RoundedCornerShape(12.dp))
            .border(1.dp, AirNotePalette.Border, RoundedCornerShape(12.dp))
            .padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(
            modifier = Modifier
                .size(34.dp)
                .background(AirNotePalette.ForegroundFixed.copy(alpha = 0.045f), RoundedCornerShape(8.dp))
                .border(1.dp, AirNotePalette.Border, RoundedCornerShape(8.dp)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, contentDescription = null, tint = AirNotePalette.Accent, modifier = Modifier.size(17.dp))
        }
        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, color = AirNotePalette.ForegroundFixed, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            Text(subtitle, color = AirNotePalette.Muted, fontSize = 13.sp)
        }
        Icon(Icons.AutoMirrored.Rounded.ArrowForward, contentDescription = null, tint = AirNotePalette.Muted, modifier = Modifier.size(16.dp))
    }
}

@Preview(widthDp = 390, heightDp = 844)
@Composable
private fun SetupPreview() {
    AirNoteTheme {
        SetupFlowScreen(
            accountEmail = "anugra@airnote.preview",
            runtimeStatus = "Preview",
            onAuthenticate = { _, _, _ -> Result.success(MockGatewayClient().authenticate("anugra@airnote.preview", "preview-password", false)) },
            onFinish = {},
        )
    }
}

@Preview(widthDp = 390, heightDp = 844)
@Composable
private fun BubblePreview() {
    AirNoteTheme {
        AirNoteBackground {
            Column(
                Modifier
                    .fillMaxSize()
                    .statusBarsPadding()
                    .padding(16.dp),
                verticalArrangement = Arrangement.Center,
            ) {
                AndroidBubbleKeyboardPreview(AndroidPreviewState.Ready)
            }
        }
    }
}
