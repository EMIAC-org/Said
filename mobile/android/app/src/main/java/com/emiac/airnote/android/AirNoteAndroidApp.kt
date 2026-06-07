package com.emiac.airnote.android

import android.app.Activity
import android.content.Intent
import android.provider.Settings
import android.view.View
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
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

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

@Composable
fun AirNoteAndroidApp() {
    var setupComplete by rememberSaveable { mutableStateOf(false) }
    var lightMode by rememberSaveable { mutableStateOf(false) }
    val recent = remember { mutableStateListOf<String>() }

    AirNoteTheme(lightMode = lightMode) {
        if (setupComplete) {
            HomeScreen(
                recent = recent,
                lightMode = lightMode,
                onLightModeChange = { lightMode = it },
                onReplaySetup = {
                    recent.clear()
                    setupComplete = false
                },
            )
        } else {
            SetupFlowScreen(
                lightMode = lightMode,
                onLightModeChange = { lightMode = it },
                onFinish = {
                    recent.add("Kal ka update concise bana ke Rahul ko bhej do.")
                    setupComplete = true
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
    lightMode: Boolean,
    onLightModeChange: (Boolean) -> Unit,
    onFinish: () -> Unit,
) {
    var step by rememberSaveable { mutableStateOf(AndroidSetupStep.Welcome) }
    var privacyAccepted by rememberSaveable { mutableStateOf(false) }
    var micChecked by rememberSaveable { mutableStateOf(false) }
    var bubbleEnabled by rememberSaveable { mutableStateOf(false) }
    var previewState by rememberSaveable { mutableStateOf(AndroidPreviewState.Ready) }

    val context = LocalContext.current
    val canContinue = when (step) {
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
                lightMode = lightMode,
                onLightModeChange = onLightModeChange,
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
                        AndroidSetupStep.Account -> AccountStep()
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
                        AndroidSetupStep.Account,
                        AndroidSetupStep.Privacy,
                        AndroidSetupStep.Bubble,
                        -> step.next()?.let { step = it }
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
        SetupRow(Icons.Rounded.Person, "Account", "Preview profile and mobile runtime.", "Ready")
        SetupRow(Icons.Rounded.Mic, "Microphone", "Recording surface and route check.", "Ready")
        SetupRow(Icons.Rounded.Keyboard, "Floating bubble", "Dictate above your existing keyboard.", "Ready")
    }
}

@Composable
private fun AccountStep() {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        SetupRow(Icons.Rounded.AccountCircle, "Preview account", "anugra@airnote.preview", "Signed")
        SetupRow(Icons.Rounded.Bolt, "Mobile Gateway", "Standalone Android runtime, independent from desktop.", "Preview")
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

@Composable
private fun HomeScreen(
    recent: List<String>,
    lightMode: Boolean,
    onLightModeChange: (Boolean) -> Unit,
    onReplaySetup: () -> Unit,
) {
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
                label = "Preview",
                lightMode = lightMode,
                onLightModeChange = onLightModeChange,
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
                            Text("Ready to dictate", color = AirNotePalette.ForegroundFixed, fontSize = 28.sp, fontWeight = FontWeight.SemiBold)
                            Text("Bubble, mic, and Gateway are ready", color = AirNotePalette.Muted, fontSize = 15.sp)
                        }
                        StatusPill(Icons.Rounded.CheckCircle, "Ready", AirNotePalette.Success)
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        MiniStat("Runtime", "Preview", Modifier.weight(1f))
                        MiniStat("Style", "Work", Modifier.weight(1f))
                        MiniStat("Lang", "Hinglish", Modifier.weight(1f))
                    }
                    Button(
                        onClick = {},
                        modifier = Modifier
                            .fillMaxWidth()
                            .height(44.dp),
                        shape = RoundedCornerShape(10.dp),
                        colors = ButtonDefaults.buttonColors(
                            containerColor = AirNotePalette.PrimaryButtonFill,
                            contentColor = AirNotePalette.PrimaryButtonContent,
                        ),
                    ) {
                        Icon(Icons.Rounded.Mic, contentDescription = null)
                        Spacer(Modifier.width(8.dp))
                        Text("Open voice session", fontWeight = FontWeight.SemiBold)
                    }
                }
            }

            AirNoteCard(padding = 14.dp) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        SectionLabel("Setup")
                        Spacer(Modifier.weight(1f))
                        Text("4/4", color = AirNotePalette.Accent, fontSize = 12.sp, fontWeight = FontWeight.Bold)
                    }
                    SetupRow(Icons.Rounded.Person, "Account", "Mobile account ready.", "Done")
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

            if (recent.isNotEmpty()) {
                SectionLabel("Recent")
                recent.take(2).forEach { text ->
                    AirNoteCard(padding = 14.dp) {
                        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(text, color = AirNotePalette.ForegroundFixed, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                StatusPill(Icons.Rounded.CheckCircle, "Inserted", AirNotePalette.Success)
                                Spacer(Modifier.weight(1f))
                                Text("Now", color = AirNotePalette.Muted, fontSize = 12.sp)
                            }
                        }
                    }
                }
            }

            SectionLabel("Tools")
            ToolRow(Icons.Rounded.Tune, "Language and style", "Auto, Hinglish, Work")
            ToolRow(Icons.Rounded.History, "History", "Copy, retry, and recover")
            ToolRow(Icons.Rounded.Language, "Vocabulary", "Names, aliases, and terms")
            ToolRow(Icons.Rounded.Settings, "Settings", "Privacy, Gateway, diagnostics")
        }
    }
}

@Composable
private fun AppHeader(
    label: String,
    lightMode: Boolean,
    onLightModeChange: (Boolean) -> Unit,
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
            AppearanceToggle(lightMode = lightMode, onLightModeChange = onLightModeChange)
            trailing?.invoke()
        }
    }
}

@Composable
private fun AppearanceToggle(
    lightMode: Boolean,
    onLightModeChange: (Boolean) -> Unit,
) {
    OutlinedButton(
        onClick = { onLightModeChange(!lightMode) },
        modifier = Modifier.height(32.dp),
        shape = RoundedCornerShape(7.dp),
        colors = ButtonDefaults.outlinedButtonColors(contentColor = AirNotePalette.Accent),
        border = BorderStroke(1.dp, AirNotePalette.Accent.copy(alpha = 0.20f)),
        contentPadding = PaddingValues(horizontal = 9.dp, vertical = 0.dp),
    ) {
        Icon(
            if (lightMode) Icons.Rounded.DarkMode else Icons.Rounded.LightMode,
            contentDescription = null,
            modifier = Modifier.size(14.dp),
        )
        Spacer(Modifier.width(6.dp))
        Text(if (lightMode) "Dark" else "Light", fontSize = 12.sp, fontWeight = FontWeight.Bold)
    }
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
        SetupFlowScreen(lightMode = false, onLightModeChange = {}, onFinish = {})
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
