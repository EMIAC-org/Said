package com.emiac.airnote.android

import android.content.Context
import android.content.SharedPreferences

enum class AndroidOutputLanguage(
    val wireValue: String,
    val label: String,
    val detail: String,
) {
    Hinglish("hinglish", "Hinglish", "Roman Hinglish"),
    English("english", "English", "English output"),
    Hindi("hindi", "Hindi", "Hindi output");

    companion object {
        fun fromWire(value: String?): AndroidOutputLanguage =
            entries.firstOrNull { it.wireValue == value } ?: Hinglish
    }
}

enum class AndroidPolishModel(
    val wireValue: String,
    val label: String,
    val detail: String,
) {
    Fast("fast", "Fast", "Lower latency"),
    Smart("smart", "Smart", "More careful");

    companion object {
        fun fromWire(value: String?): AndroidPolishModel =
            entries.firstOrNull { it.wireValue == value } ?: Fast
    }
}

enum class AndroidGatewayPreset(
    val label: String,
    val url: String,
) {
    Dev("Dev", "https://airnote-dev.103.180.163.41.sslip.io"),
    Production("Prod", "https://airnote.emiactech.com");

    companion object {
        fun fromUrl(url: String): AndroidGatewayPreset? =
            entries.firstOrNull { normalizeGatewayUrl(it.url) == normalizeGatewayUrl(url) }
    }
}

data class AndroidPolishPreferences(
    val gatewayBaseUrl: String,
    val outputLanguage: AndroidOutputLanguage,
    val selectedModel: AndroidPolishModel,
    val learningEnabled: Boolean,
    val safeVocabTerms: List<String>,
) {
    val gatewayPresetLabel: String
        get() = AndroidGatewayPreset.fromUrl(gatewayBaseUrl)?.label ?: "Custom"
}

class AndroidSettingsStore(context: Context) {
    private val prefs: SharedPreferences =
        context.getSharedPreferences("airnote_android_settings", Context.MODE_PRIVATE)

    fun isSetupComplete(): Boolean =
        prefs.getBoolean(KEY_SETUP_COMPLETE, false)

    fun setSetupComplete(complete: Boolean) {
        prefs.edit().putBoolean(KEY_SETUP_COMPLETE, complete).apply()
    }

    fun readPolishPreferences(): AndroidPolishPreferences =
        AndroidPolishPreferences(
            gatewayBaseUrl = normalizeGatewayUrl(
                prefs.getString(KEY_GATEWAY_BASE_URL, null)
                    ?: BuildConfig.GATEWAY_BASE_URL,
            ),
            outputLanguage = AndroidOutputLanguage.fromWire(prefs.getString(KEY_OUTPUT_LANGUAGE, null)),
            selectedModel = AndroidPolishModel.fromWire(prefs.getString(KEY_SELECTED_MODEL, null)),
            learningEnabled = prefs.getBoolean(KEY_LEARNING_ENABLED, true),
            safeVocabTerms = readSafeVocabTerms(),
        )

    fun setGatewayBaseUrl(url: String) {
        prefs.edit().putString(KEY_GATEWAY_BASE_URL, normalizeGatewayUrl(url)).apply()
    }

    fun setOutputLanguage(language: AndroidOutputLanguage) {
        prefs.edit().putString(KEY_OUTPUT_LANGUAGE, language.wireValue).apply()
    }

    fun setSelectedModel(model: AndroidPolishModel) {
        prefs.edit().putString(KEY_SELECTED_MODEL, model.wireValue).apply()
    }

    fun setLearningEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(KEY_LEARNING_ENABLED, enabled).apply()
    }

    fun applyRuntimeSettings(settings: RuntimeSettings) {
        prefs.edit()
            .putString(KEY_OUTPUT_LANGUAGE, AndroidOutputLanguage.fromWire(settings.outputLanguage).wireValue)
            .putString(KEY_SELECTED_MODEL, AndroidPolishModel.fromWire(settings.selectedModel).wireValue)
            .putBoolean(KEY_LEARNING_ENABLED, settings.learningEnabled)
            .apply()
    }

    fun addSafeVocabTerm(rawTerm: String): Boolean {
        val term = sanitizeVocabTerm(rawTerm) ?: return false
        val current = readSafeVocabTerms().toMutableList()
        if (current.any { it.equals(term, ignoreCase = true) }) return false
        current.add(term)
        writeSafeVocabTerms(current)
        return true
    }

    fun removeSafeVocabTerm(term: String) {
        val filtered = readSafeVocabTerms()
            .filterNot { it.equals(term.trim(), ignoreCase = true) }
        writeSafeVocabTerms(filtered)
    }

    fun cycleOutputLanguage(): AndroidOutputLanguage {
        val current = readPolishPreferences().outputLanguage
        val next = AndroidOutputLanguage.entries[(current.ordinal + 1) % AndroidOutputLanguage.entries.size]
        setOutputLanguage(next)
        return next
    }

    fun cycleSelectedModel(): AndroidPolishModel {
        val current = readPolishPreferences().selectedModel
        val next = AndroidPolishModel.entries[(current.ordinal + 1) % AndroidPolishModel.entries.size]
        setSelectedModel(next)
        return next
    }

    private fun readSafeVocabTerms(): List<String> =
        prefs.getString(KEY_SAFE_VOCAB_TERMS, "")
            .orEmpty()
            .lineSequence()
            .mapNotNull(::sanitizeVocabTerm)
            .distinctBy { it.lowercase() }
            .take(MAX_SAFE_VOCAB_TERMS)
            .toList()

    private fun writeSafeVocabTerms(terms: List<String>) {
        val clean = terms
            .mapNotNull(::sanitizeVocabTerm)
            .distinctBy { it.lowercase() }
            .take(MAX_SAFE_VOCAB_TERMS)
            .joinToString("\n")
        prefs.edit().putString(KEY_SAFE_VOCAB_TERMS, clean).apply()
    }

    private companion object {
        const val KEY_SETUP_COMPLETE = "setup_complete"
        const val KEY_GATEWAY_BASE_URL = "gateway_base_url"
        const val KEY_OUTPUT_LANGUAGE = "output_language"
        const val KEY_SELECTED_MODEL = "selected_model"
        const val KEY_LEARNING_ENABLED = "learning_enabled"
        const val KEY_SAFE_VOCAB_TERMS = "safe_vocab_terms"
        const val MAX_SAFE_VOCAB_TERMS = 50
    }
}

fun normalizeGatewayUrl(rawUrl: String): String {
    val trimmed = rawUrl.trim().trimEnd('/')
    return when {
        trimmed.isBlank() -> BuildConfig.GATEWAY_BASE_URL.trimEnd('/')
        trimmed.startsWith("http://") || trimmed.startsWith("https://") -> trimmed
        else -> "https://$trimmed"
    }
}

fun sanitizeVocabTerm(rawTerm: String?): String? {
    val term = rawTerm
        ?.replace(Regex("\\s+"), " ")
        ?.trim()
        .orEmpty()
    if (term.length !in 2..80) return null
    if (term.any { it.isISOControl() }) return null
    return term
}
