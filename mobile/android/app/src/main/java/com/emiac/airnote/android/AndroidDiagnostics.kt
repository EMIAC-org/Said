package com.emiac.airnote.android

import android.content.ComponentName
import android.content.Context
import android.content.SharedPreferences
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.provider.Settings

data class AndroidAudioRouteSnapshot(
    val label: String,
    val hasHeadset: Boolean,
    val inputDeviceCount: Int,
)

data class AndroidDiagnosticsSnapshot(
    val serverUrl: String,
    val authState: String,
    val micPermission: String,
    val accessibilityEnabled: Boolean,
    val audioRoute: String,
    val lastRequestId: String,
    val lastLatencyMs: Int,
    val lastInsertionResult: String,
    val lastFailure: String,
) {
    val redactedSummary: String
        get() = listOf(
            "server=$serverUrl",
            "auth=$authState",
            "mic=$micPermission",
            "accessibility=$accessibilityEnabled",
            "audio=$audioRoute",
            "request=${lastRequestId.ifBlank { "none" }}",
            "latency_ms=$lastLatencyMs",
            "insert=$lastInsertionResult",
            "failure=${lastFailure.ifBlank { "none" }}",
        ).joinToString("\n")
}

class AndroidDiagnosticsStore(context: Context) {
    private val appContext = context.applicationContext
    private val prefs: SharedPreferences =
        appContext.getSharedPreferences("airnote_android_diagnostics", Context.MODE_PRIVATE)

    fun snapshot(
        serverUrl: String,
        authState: String,
        micPermission: String,
    ): AndroidDiagnosticsSnapshot =
        AndroidDiagnosticsSnapshot(
            serverUrl = normalizeGatewayUrl(serverUrl),
            authState = authState,
            micPermission = micPermission,
            accessibilityEnabled = isAirNoteAccessibilityEnabled(appContext),
            audioRoute = AndroidAudioRoute.current(appContext).label,
            lastRequestId = prefs.getString(KEY_LAST_REQUEST_ID, "").orEmpty(),
            lastLatencyMs = prefs.getInt(KEY_LAST_LATENCY_MS, 0),
            lastInsertionResult = prefs.getString(KEY_LAST_INSERTION_RESULT, "none").orEmpty(),
            lastFailure = prefs.getString(KEY_LAST_FAILURE, "").orEmpty(),
        )

    fun recordRequestStarted(requestId: String) {
        prefs.edit()
            .putString(KEY_LAST_REQUEST_ID, requestId)
            .putInt(KEY_LAST_LATENCY_MS, 0)
            .putString(KEY_LAST_FAILURE, "")
            .apply()
    }

    fun recordVoiceSuccess(requestId: String, latencyMs: Int) {
        prefs.edit()
            .putString(KEY_LAST_REQUEST_ID, requestId)
            .putInt(KEY_LAST_LATENCY_MS, latencyMs.coerceAtLeast(0))
            .putString(KEY_LAST_FAILURE, "")
            .apply()
    }

    fun recordFailure(message: String) {
        prefs.edit()
            .putString(KEY_LAST_FAILURE, message.take(180))
            .apply()
    }

    fun recordInsertionResult(result: String) {
        prefs.edit()
            .putString(KEY_LAST_INSERTION_RESULT, result.take(80))
            .apply()
    }

    private companion object {
        const val KEY_LAST_REQUEST_ID = "last_request_id"
        const val KEY_LAST_LATENCY_MS = "last_latency_ms"
        const val KEY_LAST_INSERTION_RESULT = "last_insertion_result"
        const val KEY_LAST_FAILURE = "last_failure"
    }
}

object AndroidAudioRoute {
    fun current(context: Context): AndroidAudioRouteSnapshot {
        val audioManager = context.getSystemService(AudioManager::class.java)
        val inputs = audioManager
            ?.getDevices(AudioManager.GET_DEVICES_INPUTS)
            .orEmpty()
        val preferred = inputs.firstOrNull { it.isBluetoothInput() }
            ?: inputs.firstOrNull { it.isWiredInput() }
            ?: inputs.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_MIC }
            ?: inputs.firstOrNull()
        val label = when {
            preferred == null -> "Unknown mic"
            preferred.isBluetoothInput() -> "Bluetooth headset"
            preferred.isWiredInput() -> "Wired headset"
            preferred.type == AudioDeviceInfo.TYPE_BUILTIN_MIC -> "Phone mic"
            else -> preferred.productName?.toString()?.takeIf { it.isNotBlank() } ?: "External mic"
        }
        return AndroidAudioRouteSnapshot(
            label = label,
            hasHeadset = inputs.any { it.isBluetoothInput() || it.isWiredInput() },
            inputDeviceCount = inputs.size,
        )
    }

    private fun AudioDeviceInfo.isBluetoothInput(): Boolean =
        type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO ||
            type == AudioDeviceInfo.TYPE_BLE_HEADSET ||
            type == AudioDeviceInfo.TYPE_BLE_BROADCAST

    private fun AudioDeviceInfo.isWiredInput(): Boolean =
        type == AudioDeviceInfo.TYPE_WIRED_HEADSET ||
            type == AudioDeviceInfo.TYPE_USB_HEADSET ||
            type == AudioDeviceInfo.TYPE_USB_DEVICE
}

fun isAirNoteAccessibilityEnabled(context: Context): Boolean {
    val expected = ComponentName(context, AirNoteBubbleAccessibilityService::class.java)
    val expectedPackage = expected.packageName
    val expectedClass = expected.className
    val enabled = Settings.Secure.getString(
        context.contentResolver,
        Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
    ).orEmpty()
    return isAirNoteAccessibilityServiceListed(enabled, expectedPackage, expectedClass)
}

internal fun isAirNoteAccessibilityServiceListed(
    enabledServices: String,
    expectedPackage: String,
    expectedClass: String,
): Boolean =
    enabledServices.split(':').any { raw ->
        val component = parseAccessibilityComponent(raw.trim()) ?: return@any false
        component.first.equals(expectedPackage, ignoreCase = true) &&
            component.second.equals(expectedClass, ignoreCase = true)
    }

private fun parseAccessibilityComponent(raw: String): Pair<String, String>? {
    val separatorIndex = raw.indexOf('/')
    if (separatorIndex <= 0 || separatorIndex == raw.lastIndex) return null
    val packageName = raw.substring(0, separatorIndex)
    val className = raw.substring(separatorIndex + 1).let { name ->
        when {
            name.startsWith('.') -> packageName + name
            '.' in name -> name
            else -> "$packageName.$name"
        }
    }
    return packageName to className
}
