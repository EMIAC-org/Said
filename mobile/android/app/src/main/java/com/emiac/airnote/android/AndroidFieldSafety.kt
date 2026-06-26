package com.emiac.airnote.android

import android.text.InputType

object AndroidFieldSafety {
    fun isSensitiveField(
        inputType: Int,
        isPassword: Boolean,
        text: CharSequence?,
        hint: CharSequence?,
        className: CharSequence?,
    ): Boolean {
        if (isPassword) return true
        val classAndVariation = inputType and (InputType.TYPE_MASK_CLASS or InputType.TYPE_MASK_VARIATION)
        if (classAndVariation in PASSWORD_INPUT_TYPES) return true

        val context = listOfNotNull(text, hint, className)
            .joinToString(" ")
            .lowercase()
        return SENSITIVE_HINTS.any { it in context }
    }

    private val PASSWORD_INPUT_TYPES = setOf(
        InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD,
        InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD,
        InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_WEB_PASSWORD,
        InputType.TYPE_CLASS_NUMBER or InputType.TYPE_NUMBER_VARIATION_PASSWORD,
    )

    private val SENSITIVE_HINTS = listOf(
        "password",
        "passcode",
        "pin",
        "otp",
        "one time",
        "one-time",
        "verification code",
        "2fa",
        "cvv",
        "card number",
        "upi pin",
        "bank password",
        "security code",
    )
}
