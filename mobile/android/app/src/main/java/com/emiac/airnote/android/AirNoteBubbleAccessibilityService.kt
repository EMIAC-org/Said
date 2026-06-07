package com.emiac.airnote.android

import android.accessibilityservice.AccessibilityService
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.widget.LinearLayout
import android.widget.TextView
import kotlin.math.roundToInt

class AirNoteBubbleAccessibilityService : AccessibilityService() {
    private var windowManager: WindowManager? = null
    private var bubbleView: View? = null
    private var layoutParams: WindowManager.LayoutParams? = null

    override fun onServiceConnected() {
        super.onServiceConnected()
        windowManager = getSystemService(WINDOW_SERVICE) as WindowManager
        showBubble()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        val isTextFocus = event?.eventType == AccessibilityEvent.TYPE_VIEW_FOCUSED ||
            event?.eventType == AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED
        bubbleView?.visibility = if (isTextFocus) View.VISIBLE else View.VISIBLE
    }

    override fun onInterrupt() = Unit

    override fun onDestroy() {
        hideBubble()
        super.onDestroy()
    }

    private fun showBubble() {
        if (bubbleView != null) return

        val bubble = LinearLayout(this).apply {
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

        val mark = TextView(this).apply {
            text = "|||"
            setTextColor(Color.rgb(158, 179, 250))
            textSize = 16f
            typeface = Typeface.DEFAULT_BOLD
        }
        val label = TextView(this).apply {
            text = "AirNote"
            setTextColor(Color.rgb(237, 237, 245))
            textSize = 13f
            typeface = Typeface.DEFAULT_BOLD
            setPadding(10, 0, 0, 0)
        }
        bubble.addView(mark)
        bubble.addView(label)

        val params = WindowManager.LayoutParams(
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

        attachDragHandler(bubble, params)
        windowManager?.addView(bubble, params)
        bubbleView = bubble
        layoutParams = params
    }

    private fun hideBubble() {
        val view = bubbleView ?: return
        windowManager?.removeView(view)
        bubbleView = null
        layoutParams = null
    }

    private fun attachDragHandler(view: View, params: WindowManager.LayoutParams) {
        var startX = 0
        var startY = 0
        var downRawX = 0f
        var downRawY = 0f
        view.isClickable = true

        view.setOnTouchListener { _, event ->
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    startX = params.x
                    startY = params.y
                    downRawX = event.rawX
                    downRawY = event.rawY
                    true
                }
                MotionEvent.ACTION_MOVE -> {
                    params.x = startX + (event.rawX - downRawX).roundToInt()
                    params.y = startY + (event.rawY - downRawY).roundToInt()
                    windowManager?.updateViewLayout(view, params)
                    true
                }
                MotionEvent.ACTION_UP -> {
                    view.performClick()
                    true
                }
                else -> true
            }
        }
    }
}
