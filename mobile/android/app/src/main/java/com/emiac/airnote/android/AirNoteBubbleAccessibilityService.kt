package com.emiac.airnote.android

import android.accessibilityservice.AccessibilityService
import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.os.Bundle
import android.provider.Settings
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.LinearLayout
import android.widget.TextView
import java.util.UUID
import kotlin.math.roundToInt
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

class AirNoteBubbleAccessibilityService : AccessibilityService() {
    private var windowManager: WindowManager? = null
    private var bubbleView: LinearLayout? = null
    private var layoutParams: WindowManager.LayoutParams? = null
    private val serviceScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val recorder = AndroidVoiceRecorder()
    private lateinit var sessionStore: AndroidSecureSessionStore
    private lateinit var settingsStore: AndroidSettingsStore
    private lateinit var diagnosticsStore: AndroidDiagnosticsStore
    private var phase = BubblePhase.Idle
    private var lastResult: RuntimeVoiceResult? = null

    override fun onServiceConnected() {
        super.onServiceConnected()
        windowManager = getSystemService(WINDOW_SERVICE) as WindowManager
        sessionStore = AndroidSecureSessionStore(applicationContext)
        settingsStore = AndroidSettingsStore(applicationContext)
        diagnosticsStore = AndroidDiagnosticsStore(applicationContext)
        showBubble()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        val isTextFocus = event?.eventType == AccessibilityEvent.TYPE_VIEW_FOCUSED ||
            event?.eventType == AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED
        bubbleView?.visibility = if (isTextFocus) View.VISIBLE else View.VISIBLE
        if (phase == BubblePhase.Complete) {
            renderBubble()
        }
    }

    override fun onInterrupt() = Unit

    override fun onDestroy() {
        hideBubble()
        recorder.cancel()
        serviceScope.cancel()
        super.onDestroy()
    }

    private fun showBubble() {
        if (bubbleView != null) {
            renderBubble()
            return
        }

        val params = layoutParams ?: WindowManager.LayoutParams(
            WindowManager.LayoutParams.WRAP_CONTENT,
            WindowManager.LayoutParams.WRAP_CONTENT,
            WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
            android.graphics.PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            x = 40
            y = 420
        }

        val bubble = LinearLayout(this)
        windowManager?.addView(bubble, params)
        bubbleView = bubble
        layoutParams = params
        renderBubble()
    }

    private fun renderBubble() {
        val bubble = bubbleView ?: return
        val params = layoutParams ?: return
        bubble.removeAllViews()

        bubble.apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER
            setPadding(18, 12, 18, 12)
            background = GradientDrawable().apply {
                cornerRadius = 48f
                setColor(phase.backgroundColor)
                setStroke(1, Color.argb(42, 255, 255, 255))
            }
            elevation = 18f
        }

        val mark = chip("|||", phase.accentColor)
        attachDragHandler(mark, params)
        bubble.addView(mark)
        val prefs = settingsStore.readPolishPreferences()

        when (phase) {
            BubblePhase.Idle -> {
                bubble.addView(actionChip("AirNote", primary = true) { startDictation() })
                bubble.addView(actionChip(prefs.outputLanguage.label) {
                    settingsStore.cycleOutputLanguage()
                    renderBubble()
                })
                bubble.addView(actionChip(prefs.selectedModel.label) {
                    settingsStore.cycleSelectedModel()
                    renderBubble()
                })
            }
            BubblePhase.Recording -> {
                bubble.addView(actionChip("Stop", primary = true) { finishDictation() })
                bubble.addView(actionChip("Cancel") { cancelDictation() })
            }
            BubblePhase.Uploading -> {
                bubble.addView(chip("Polishing", Color.rgb(237, 237, 245)))
                bubble.addView(chip(prefs.outputLanguage.label, Color.rgb(158, 179, 250)))
            }
            BubblePhase.Complete -> {
                val primary = if (canAttemptInsert()) "Insert" else "Copy"
                bubble.addView(actionChip(primary, primary = true) {
                    if (canAttemptInsert()) insertResult() else copyResult("Copied")
                })
                bubble.addView(actionChip("Copy") { copyResult("Copied") })
                bubble.addView(actionChip("Retry") { startDictation() })
                bubble.addView(actionChip("Saved") { acknowledgeSaved() })
            }
            BubblePhase.Error -> {
                bubble.addView(actionChip("Retry", primary = true) { startDictation() })
                bubble.addView(actionChip("Open app") { openMainApp() })
            }
        }
    }

    private fun hideBubble() {
        val view = bubbleView ?: return
        windowManager?.removeView(view)
        bubbleView = null
        layoutParams = null
    }

    private fun startDictation() {
        if (!BuildConfig.USE_MOCK_GATEWAY && sessionStore.read()?.token.isNullOrBlank()) {
            phase = BubblePhase.Error
            lastResult = null
            renderBubbleWithNotice("Sign in")
            openMainApp()
            return
        }
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            phase = BubblePhase.Error
            lastResult = null
            renderBubbleWithNotice("Grant mic")
            openMainApp()
            return
        }
        lastResult = null
        val started = recorder.start(applicationContext, serviceScope) {
            // The bubble stays intentionally calm while recording; the dashboard shows detailed levels.
        }
        phase = if (started) BubblePhase.Recording else BubblePhase.Error
        renderBubble()
    }

    private fun finishDictation() {
        if (!recorder.isRecording) return
        phase = BubblePhase.Uploading
        renderBubble()
        serviceScope.launch {
            val prefs = settingsStore.readPolishPreferences()
            val clientRunId = "android-bubble-${UUID.randomUUID()}"
            diagnosticsStore.recordRequestStarted(clientRunId)
            val requestGateway = if (BuildConfig.USE_MOCK_GATEWAY) {
                MockGatewayClient()
            } else {
                HttpGatewayClient(prefs.gatewayBaseUrl) {
                    sessionStore.read()?.token
                }
            }
            val result = runCatching {
                val wav = recorder.stop()
                require(wav.size > WAV_HEADER_SIZE) { "No audio captured" }
                requestGateway.polishWav(
                    wavBytes = wav,
                    clientRunId = clientRunId,
                    deviceId = androidDeviceId(),
                    outputLanguage = prefs.outputLanguage.wireValue,
                    selectedModel = prefs.selectedModel.wireValue,
                    safeVocabTerms = prefs.safeVocabTerms,
                )
            }
            result.fold(
                onSuccess = { response ->
                    lastResult = response
                    diagnosticsStore.recordVoiceSuccess(response.runId.ifBlank { clientRunId }, response.totalLatencyMs)
                    phase = BubblePhase.Complete
                    renderBubble()
                },
                onFailure = { error ->
                    lastResult = null
                    phase = BubblePhase.Error
                    diagnosticsStore.recordFailure(error.message ?: "bubble_voice_failed")
                    renderBubbleWithNotice("Failed")
                },
            )
        }
    }

    private fun cancelDictation() {
        recorder.cancel()
        phase = BubblePhase.Idle
        lastResult = null
        renderBubble()
    }

    private fun insertResult() {
        val text = lastResult?.output?.takeIf { it.isNotBlank() } ?: return
        val node = focusedInputNode()
        if (node == null || isSecureField(node)) {
            copyToClipboard(text)
            val notice = if (node == null) "Copied" else "Secure copy"
            diagnosticsStore.recordInsertionResult(notice)
            renderBubbleWithNotice(notice)
            scheduleReset()
            return
        }

        copyToClipboard(text)
        val pasted = node.performAction(AccessibilityNodeInfo.ACTION_PASTE)
        val setEmptyField = if (!pasted && node.text.isNullOrEmpty() && node.isEditable) {
            val args = Bundle().apply {
                putCharSequence(
                    AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,
                    text,
                )
            }
            node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
        } else {
            false
        }

        if (pasted || setEmptyField) {
            diagnosticsStore.recordInsertionResult("inserted")
            renderBubbleWithNotice("Inserted")
        } else {
            diagnosticsStore.recordInsertionResult("copied_fallback")
            renderBubbleWithNotice("Copied")
        }
        scheduleReset()
    }

    private fun copyResult(notice: String) {
        val text = lastResult?.output?.takeIf { it.isNotBlank() } ?: return
        copyToClipboard(text)
        diagnosticsStore.recordInsertionResult(notice)
        renderBubbleWithNotice(notice)
        scheduleReset()
    }

    private fun acknowledgeSaved() {
        renderBubbleWithNotice("Saved")
        scheduleReset()
    }

    private fun renderBubbleWithNotice(notice: String) {
        val bubble = bubbleView ?: return
        val params = layoutParams ?: return
        bubble.removeAllViews()
        bubble.apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER
            setPadding(18, 12, 18, 12)
            background = GradientDrawable().apply {
                cornerRadius = 48f
                setColor(Color.rgb(22, 22, 28))
                setStroke(1, Color.argb(42, 255, 255, 255))
            }
            elevation = 18f
        }
        val mark = chip("|||", Color.rgb(158, 179, 250))
        attachDragHandler(mark, params)
        bubble.addView(mark)
        bubble.addView(chip(notice, Color.rgb(237, 237, 245)))
    }

    private fun scheduleReset() {
        serviceScope.launch {
            delay(1_300)
            if (phase != BubblePhase.Recording && phase != BubblePhase.Uploading) {
                phase = BubblePhase.Idle
                lastResult = null
                renderBubble()
            }
        }
    }

    private fun canAttemptInsert(): Boolean {
        val node = focusedInputNode() ?: return false
        return node.isEditable && !isSecureField(node)
    }

    private fun focusedInputNode(): AccessibilityNodeInfo? {
        val root = rootInActiveWindow ?: return null
        root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)?.let { return it }
        return findFocusedEditable(root)
    }

    private fun findFocusedEditable(node: AccessibilityNodeInfo): AccessibilityNodeInfo? {
        if (node.isFocused && node.isEditable) return node
        for (index in 0 until node.childCount) {
            val child = node.getChild(index) ?: continue
            findFocusedEditable(child)?.let { return it }
        }
        return null
    }

    private fun isSecureField(node: AccessibilityNodeInfo): Boolean {
        return AndroidFieldSafety.isSensitiveField(
            inputType = node.inputType,
            isPassword = node.isPassword,
            text = node.text,
            hint = node.hintText,
            className = node.className,
        )
    }

    private fun copyToClipboard(text: String) {
        val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        clipboard.setPrimaryClip(ClipData.newPlainText("AirNote", text))
    }

    private fun openMainApp() {
        val intent = Intent(this, MainActivity::class.java).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        }
        runCatching { startActivity(intent) }
    }

    private fun androidDeviceId(): String =
        Settings.Secure.getString(contentResolver, Settings.Secure.ANDROID_ID)
            ?.takeIf { it.isNotBlank() }
            ?: "android-${UUID.randomUUID()}"

    private fun chip(text: String, color: Int): TextView =
        TextView(this).apply {
            this.text = text
            setTextColor(color)
            textSize = 13f
            typeface = Typeface.DEFAULT_BOLD
            setPadding(10, 0, 0, 0)
        }

    private fun actionChip(
        text: String,
        primary: Boolean = false,
        action: () -> Unit,
    ): TextView =
        chip(
            text = text,
            color = if (primary) Color.rgb(237, 237, 245) else Color.rgb(158, 179, 250),
        ).apply {
            minHeight = 36
            gravity = Gravity.CENTER
            isClickable = true
            isFocusable = true
            setPadding(12, 0, 12, 0)
            background = GradientDrawable().apply {
                cornerRadius = 32f
                setColor(if (primary) Color.argb(46, 255, 255, 255) else Color.TRANSPARENT)
            }
            setOnClickListener { action() }
        }

    private fun attachDragHandler(view: View, params: WindowManager.LayoutParams) {
        var startX = 0
        var startY = 0
        var downRawX = 0f
        var downRawY = 0f
        var dragged = false
        view.isClickable = true

        view.setOnTouchListener { _, event ->
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    startX = params.x
                    startY = params.y
                    downRawX = event.rawX
                    downRawY = event.rawY
                    dragged = false
                    true
                }
                MotionEvent.ACTION_MOVE -> {
                    val deltaX = event.rawX - downRawX
                    val deltaY = event.rawY - downRawY
                    dragged = dragged || kotlin.math.abs(deltaX) > DRAG_SLOP || kotlin.math.abs(deltaY) > DRAG_SLOP
                    params.x = startX + (event.rawX - downRawX).roundToInt()
                    params.y = startY + (event.rawY - downRawY).roundToInt()
                    windowManager?.updateViewLayout(view, params)
                    true
                }
                MotionEvent.ACTION_UP -> {
                    if (!dragged) {
                        handleHandleTap()
                    }
                    true
                }
                else -> true
            }
        }
    }

    private fun handleHandleTap() {
        when (phase) {
            BubblePhase.Idle,
            BubblePhase.Error,
            -> startDictation()
            BubblePhase.Recording -> finishDictation()
            BubblePhase.Uploading -> Unit
            BubblePhase.Complete -> if (canAttemptInsert()) insertResult() else copyResult("Copied")
        }
    }

    private enum class BubblePhase(
        val backgroundColor: Int,
        val accentColor: Int,
    ) {
        Idle(Color.rgb(22, 22, 28), Color.rgb(158, 179, 250)),
        Recording(Color.rgb(240, 77, 92), Color.WHITE),
        Uploading(Color.rgb(22, 22, 28), Color.rgb(158, 179, 250)),
        Complete(Color.rgb(22, 22, 28), Color.rgb(135, 209, 155)),
        Error(Color.rgb(22, 22, 28), Color.rgb(240, 77, 92));
    }

    private companion object {
        const val DRAG_SLOP = 8f
        const val WAV_HEADER_SIZE = 44
    }
}
