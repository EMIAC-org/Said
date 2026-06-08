package com.emiac.airnote.android

import android.util.Base64
import java.io.BufferedReader
import java.io.OutputStreamWriter
import java.net.HttpURLConnection
import java.net.URL
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

data class GatewayAccount(
    val id: String,
    val email: String,
    val licenseTier: String,
)

data class GatewayAuthResponse(
    val token: String,
    val account: GatewayAccount,
)

data class RuntimeStatus(
    val credentialEncryptionConfigured: Boolean,
    val activeCredentialCount: Int,
    val runtimeSessionCount: Int,
    val learningEventCount: Int,
    val personalReplacementCount: Int,
    val personalVocabCount: Int,
    val personalAliasCount: Int,
    val activeEditPolicyCount: Int,
    val serverMemoryReady: Boolean,
) {
    val readinessLabel: String
        get() = when {
            activeCredentialCount > 0 && serverMemoryReady -> "server_memory_ready"
            activeCredentialCount > 0 -> "server_ready"
            credentialEncryptionConfigured -> "needs_credentials"
            else -> "needs_runtime_key"
        }
}

data class RuntimeSettings(
    val selectedModel: String,
    val outputLanguage: String,
    val tonePreset: String,
    val learningEnabled: Boolean,
    val serverRuntimeEnabled: Boolean,
    val serverAudioRuntimeEnabled: Boolean,
    val version: Int,
)

data class RuntimeVoiceResult(
    val runId: String,
    val transcript: String,
    val output: String,
    val totalLatencyMs: Int,
)

data class RuntimeHistoryItem(
    val id: String,
    val runId: String?,
    val clientRunId: String?,
    val transcript: String,
    val finalText: String,
    val source: String,
    val platform: String,
    val createdAt: String,
) {
    val displayText: String
        get() = finalText.ifBlank { transcript }
}

data class RuntimeLearningCandidate(
    val original: String,
    val corrected: String,
    val termType: String,
    val learnable: Boolean,
    val tag: String,
)

data class RuntimeLearningAnalysis(
    val candidates: List<RuntimeLearningCandidate>,
    val changed: Boolean,
    val source: String,
)

data class RuntimeLearningConfirmResult(
    val learnedCount: Int,
    val blockedCount: Int,
    val learnedTerms: List<String>,
    val status: String,
)

interface GatewayClient {
    suspend fun authenticate(email: String, password: String, signup: Boolean): GatewayAuthResponse
    suspend fun restoreSession(token: String): GatewayAuthResponse
    suspend fun runtimeStatus(): RuntimeStatus
    suspend fun runtimeSettings(): RuntimeSettings
    suspend fun listHistory(limit: Int = 20): List<RuntimeHistoryItem>
    suspend fun deleteHistory(id: String)
    suspend fun analyzeEdit(
        recordingId: String,
        transcript: String,
        aiOutput: String,
        userKept: String,
    ): RuntimeLearningAnalysis
    suspend fun confirmLearning(
        recordingId: String,
        items: List<RuntimeLearningCandidate>,
    ): RuntimeLearningConfirmResult
    suspend fun polishWav(
        wavBytes: ByteArray,
        clientRunId: String,
        deviceId: String,
        outputLanguage: String = "hinglish",
        selectedModel: String = "fast",
    ): RuntimeVoiceResult
}

class MockGatewayClient : GatewayClient {
    override suspend fun authenticate(email: String, password: String, signup: Boolean): GatewayAuthResponse =
        GatewayAuthResponse(
            token = "mock-android-token",
            account = GatewayAccount(
                id = "mock-account",
                email = email.ifBlank { "anugra@airnote.preview" },
                licenseTier = "free",
            ),
        )

    override suspend fun restoreSession(token: String): GatewayAuthResponse =
        GatewayAuthResponse(
            token = token,
            account = GatewayAccount(
                id = "mock-account",
                email = "anugra@airnote.preview",
                licenseTier = "free",
            ),
        )

    override suspend fun runtimeStatus(): RuntimeStatus =
        RuntimeStatus(
            credentialEncryptionConfigured = true,
            activeCredentialCount = 2,
            runtimeSessionCount = 0,
            learningEventCount = 0,
            personalReplacementCount = 0,
            personalVocabCount = 0,
            personalAliasCount = 0,
            activeEditPolicyCount = 0,
            serverMemoryReady = false,
        )

    override suspend fun runtimeSettings(): RuntimeSettings =
        RuntimeSettings(
            selectedModel = "fast",
            outputLanguage = "hinglish",
            tonePreset = "work",
            learningEnabled = true,
            serverRuntimeEnabled = true,
            serverAudioRuntimeEnabled = true,
            version = 1,
        )

    override suspend fun polishWav(
        wavBytes: ByteArray,
        clientRunId: String,
        deviceId: String,
        outputLanguage: String,
        selectedModel: String,
    ): RuntimeVoiceResult =
        RuntimeVoiceResult(
            runId = clientRunId,
            transcript = "kal ka update concise banake rahul ko bhej do",
            output = "Kal ka update concise bana ke Rahul ko bhej do.",
            totalLatencyMs = 420,
        )

    override suspend fun listHistory(limit: Int): List<RuntimeHistoryItem> =
        listOf(
            RuntimeHistoryItem(
                id = "mock-history-1",
                runId = "mock-run-1",
                clientRunId = "mock-client-run-1",
                transcript = "kal ka update concise banake rahul ko bhej do",
                finalText = "Kal ka update concise bana ke Rahul ko bhej do.",
                source = "server_wav",
                platform = "android",
                createdAt = "now",
            ),
        ).take(limit.coerceIn(1, 200))

    override suspend fun deleteHistory(id: String) = Unit

    override suspend fun analyzeEdit(
        recordingId: String,
        transcript: String,
        aiOutput: String,
        userKept: String,
    ): RuntimeLearningAnalysis {
        val candidate = RuntimeLearningCandidate(
            original = aiOutput.ifBlank { transcript }.take(24),
            corrected = userKept.take(32),
            termType = "proper_noun",
            learnable = userKept.isNotBlank() && userKept != aiOutput,
            tag = "mock_mobile_edit",
        )
        return RuntimeLearningAnalysis(
            candidates = listOf(candidate).filter { it.learnable },
            changed = userKept != aiOutput,
            source = "mock_mobile_learning",
        )
    }

    override suspend fun confirmLearning(
        recordingId: String,
        items: List<RuntimeLearningCandidate>,
    ): RuntimeLearningConfirmResult =
        RuntimeLearningConfirmResult(
            learnedCount = items.count { it.learnable },
            blockedCount = 0,
            learnedTerms = items.map { it.corrected },
            status = "accepted",
        )
}

class HttpGatewayClient(
    private val baseUrl: String,
    private val tokenProvider: () -> String?,
) : GatewayClient {
    override suspend fun authenticate(email: String, password: String, signup: Boolean): GatewayAuthResponse {
        val endpoint = if (signup) "/v1/auth/signup" else "/v1/auth/login"
        val body = JSONObject()
            .put("email", email.trim())
            .put("password", password)
        val json = requestJson(endpoint, method = "POST", body = body, authorized = false)
        val account = json.getJSONObject("account")
        return GatewayAuthResponse(
            token = json.getString("token"),
            account = GatewayAccount(
                id = account.getString("id"),
                email = account.getString("email"),
                licenseTier = account.optString("license_tier", "free"),
            ),
        )
    }

    override suspend fun restoreSession(token: String): GatewayAuthResponse {
        val json = requestJson("/v1/auth/me", method = "GET", authorized = true, tokenOverride = token)
        val account = json.getJSONObject("account")
        val license = json.optJSONObject("license")
        return GatewayAuthResponse(
            token = token,
            account = GatewayAccount(
                id = account.getString("id"),
                email = account.getString("email"),
                licenseTier = license?.optString("tier", "free") ?: "free",
            ),
        )
    }

    override suspend fun runtimeStatus(): RuntimeStatus {
        val json = requestJson("/v1/runtime/status", method = "GET", authorized = true)
        return RuntimeStatus(
            credentialEncryptionConfigured = json.optBoolean("credential_encryption_configured"),
            activeCredentialCount = json.optInt("active_credential_count"),
            runtimeSessionCount = json.optInt("runtime_session_count"),
            learningEventCount = json.optInt("learning_event_count"),
            personalReplacementCount = json.optInt("personal_replacement_count"),
            personalVocabCount = json.optInt("personal_vocab_count"),
            personalAliasCount = json.optInt("personal_alias_count"),
            activeEditPolicyCount = json.optInt("active_edit_policy_count"),
            serverMemoryReady = json.optBoolean("server_memory_ready"),
        )
    }

    override suspend fun runtimeSettings(): RuntimeSettings {
        val json = requestJson("/v1/runtime/settings", method = "GET", authorized = true)
        return RuntimeSettings(
            selectedModel = json.optString("selected_model", "fast"),
            outputLanguage = json.optString("output_language", "hinglish"),
            tonePreset = json.optString("tone_preset", "work"),
            learningEnabled = json.optBoolean("learning_enabled", true),
            serverRuntimeEnabled = json.optBoolean("server_runtime_enabled", true),
            serverAudioRuntimeEnabled = json.optBoolean("server_audio_runtime_enabled", true),
            version = json.optInt("version", 0),
        )
    }

    override suspend fun polishWav(
        wavBytes: ByteArray,
        clientRunId: String,
        deviceId: String,
        outputLanguage: String,
        selectedModel: String,
    ): RuntimeVoiceResult {
        val body = JSONObject()
            .put("wav_b64", Base64.encodeToString(wavBytes, Base64.NO_WRAP))
            .put("output_language", outputLanguage)
            .put("selected_model", selectedModel)
            .put("client_run_id", clientRunId)
            .put("device_id", deviceId)
            .put("platform", "android")
            .put("app_version", BuildConfig.VERSION_NAME)
        val json = requestJson("/v1/runtime/voice/wav", method = "POST", body = body, authorized = true)
        val latency = json.optJSONObject("latency_ms")
        return RuntimeVoiceResult(
            runId = json.getString("run_id"),
            transcript = json.optString("transcript"),
            output = json.optString("output"),
            totalLatencyMs = latency?.optInt("total") ?: 0,
        )
    }

    override suspend fun listHistory(limit: Int): List<RuntimeHistoryItem> {
        val clamped = limit.coerceIn(1, 200)
        val array = requestJsonArray("/v1/runtime/history?limit=$clamped", method = "GET", authorized = true)
        return List(array.length()) { index -> array.getJSONObject(index).toRuntimeHistoryItem() }
    }

    override suspend fun deleteHistory(id: String) {
        requestText("/v1/runtime/history/$id", method = "DELETE", authorized = true)
    }

    override suspend fun analyzeEdit(
        recordingId: String,
        transcript: String,
        aiOutput: String,
        userKept: String,
    ): RuntimeLearningAnalysis {
        val body = JSONObject()
            .put("recording_id", recordingId)
            .put("transcript", transcript)
            .put("ai_output", aiOutput)
            .put("user_kept", userKept)
            .put("candidates", JSONArray())
        val json = requestJson("/v1/runtime/learning/analyze-edit", method = "POST", body = body, authorized = true)
        val candidates = json.optJSONArray("candidates") ?: JSONArray()
        return RuntimeLearningAnalysis(
            candidates = List(candidates.length()) { index -> candidates.getJSONObject(index).toLearningCandidate() },
            changed = json.optBoolean("changed"),
            source = json.optString("source"),
        )
    }

    override suspend fun confirmLearning(
        recordingId: String,
        items: List<RuntimeLearningCandidate>,
    ): RuntimeLearningConfirmResult {
        val encodedItems = JSONArray()
        items.filter { it.learnable }.forEach { item ->
            encodedItems.put(
                JSONObject()
                    .put("original", item.original)
                    .put("corrected", item.corrected)
                    .put("term_type", item.termType),
            )
        }
        val body = JSONObject()
            .put("recording_id", recordingId)
            .put("items", encodedItems)
        val json = requestJson("/v1/runtime/learning/confirm-batch", method = "POST", body = body, authorized = true)
        val learnedTerms = json.optJSONArray("learned_terms") ?: JSONArray()
        val judgment = json.optJSONObject("server_judgment")
        return RuntimeLearningConfirmResult(
            learnedCount = json.optInt("learned_count"),
            blockedCount = json.optInt("blocked_count"),
            learnedTerms = List(learnedTerms.length()) { index -> learnedTerms.optString(index) },
            status = judgment?.optString("status") ?: "unknown",
        )
    }

    private suspend fun requestJson(
        path: String,
        method: String,
        body: JSONObject? = null,
        authorized: Boolean,
        tokenOverride: String? = null,
    ): JSONObject =
        JSONObject(requestText(path = path, method = method, body = body, authorized = authorized, tokenOverride = tokenOverride).ifBlank { "{}" })

    private suspend fun requestJsonArray(
        path: String,
        method: String,
        body: JSONObject? = null,
        authorized: Boolean,
        tokenOverride: String? = null,
    ): JSONArray =
        JSONArray(requestText(path = path, method = method, body = body, authorized = authorized, tokenOverride = tokenOverride).ifBlank { "[]" })

    private suspend fun requestText(
        path: String,
        method: String,
        body: JSONObject? = null,
        authorized: Boolean,
        tokenOverride: String? = null,
    ): String = withContext(Dispatchers.IO) {
        val connection = (URL(baseUrl.trimEnd('/') + path).openConnection() as HttpURLConnection).apply {
            requestMethod = method
            connectTimeout = 15_000
            readTimeout = 30_000
            setRequestProperty("Accept", "application/json")
            if (authorized) {
                val token = tokenOverride?.trim().takeUnless { it.isNullOrEmpty() } ?: tokenProvider()?.trim().orEmpty()
                require(token.isNotEmpty()) { "Missing AirNote Gateway token" }
                setRequestProperty("Authorization", "Bearer $token")
            }
            if (body != null) {
                doOutput = true
                setRequestProperty("Content-Type", "application/json")
            }
        }

        if (body != null) {
            OutputStreamWriter(connection.outputStream, Charsets.UTF_8).use { writer ->
                writer.write(body.toString())
            }
        }

        val responseCode = connection.responseCode
        val responseText = try {
            val stream = if (responseCode in 200..299) connection.inputStream else connection.errorStream
            stream?.bufferedReader(Charsets.UTF_8)?.use(BufferedReader::readText).orEmpty()
        } finally {
            connection.disconnect()
        }
        if (responseCode !in 200..299) {
            val error = runCatching { JSONObject(responseText).optString("error") }.getOrNull()
            throw IllegalStateException(error?.ifBlank { null } ?: "Gateway request failed with HTTP $responseCode")
        }
        responseText
    }

    private fun JSONObject.toRuntimeHistoryItem(): RuntimeHistoryItem =
        RuntimeHistoryItem(
            id = getString("id"),
            runId = optString("run_id").takeIf { it.isNotBlank() },
            clientRunId = optString("client_run_id").takeIf { it.isNotBlank() },
            transcript = optString("transcript"),
            finalText = optString("final_text").ifBlank { optString("polished_output") },
            source = optString("source", "server"),
            platform = optString("platform", "android"),
            createdAt = optString("created_at"),
        )

    private fun JSONObject.toLearningCandidate(): RuntimeLearningCandidate =
        RuntimeLearningCandidate(
            original = optString("original"),
            corrected = optString("corrected"),
            termType = optString("term_type", "proper_noun"),
            learnable = optBoolean("learnable", true),
            tag = optString("tag"),
        )
}
