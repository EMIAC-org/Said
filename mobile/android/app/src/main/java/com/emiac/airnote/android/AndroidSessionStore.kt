package com.emiac.airnote.android

import android.content.Context
import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import org.json.JSONObject

data class GatewaySession(
    val token: String,
    val account: GatewayAccount,
)

interface GatewaySessionStore {
    fun read(): GatewaySession?
    fun write(session: GatewaySession)
    fun clear()
}

class AndroidSecureSessionStore(context: Context) : GatewaySessionStore {
    private val prefs: SharedPreferences =
        context.getSharedPreferences("airnote_mobile_session", Context.MODE_PRIVATE)

    override fun read(): GatewaySession? {
        val envelope = prefs.getString(KEY_SESSION, null) ?: return null
        return runCatching {
            val encrypted = JSONObject(envelope)
            val iv = Base64.decode(encrypted.getString("iv"), Base64.NO_WRAP)
            val ciphertext = Base64.decode(encrypted.getString("ciphertext"), Base64.NO_WRAP)
            val cipher = Cipher.getInstance(TRANSFORMATION).apply {
                init(Cipher.DECRYPT_MODE, secretKey(), GCMParameterSpec(128, iv))
            }
            val json = JSONObject(String(cipher.doFinal(ciphertext), Charsets.UTF_8))
            GatewaySession(
                token = json.getString("token"),
                account = GatewayAccount(
                    id = json.getJSONObject("account").getString("id"),
                    email = json.getJSONObject("account").getString("email"),
                    licenseTier = json.getJSONObject("account").optString("license_tier", "free"),
                ),
            )
        }.getOrNull()
    }

    override fun write(session: GatewaySession) {
        val json = JSONObject()
            .put("token", session.token)
            .put(
                "account",
                JSONObject()
                    .put("id", session.account.id)
                    .put("email", session.account.email)
                    .put("license_tier", session.account.licenseTier),
            )
        val cipher = Cipher.getInstance(TRANSFORMATION).apply {
            init(Cipher.ENCRYPT_MODE, secretKey())
        }
        val ciphertext = cipher.doFinal(json.toString().toByteArray(Charsets.UTF_8))
        val envelope = JSONObject()
            .put("iv", Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
            .put("ciphertext", Base64.encodeToString(ciphertext, Base64.NO_WRAP))
        prefs.edit().putString(KEY_SESSION, envelope.toString()).apply()
    }

    override fun clear() {
        prefs.edit().remove(KEY_SESSION).apply()
    }

    private fun secretKey(): SecretKey {
        val keyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        (keyStore.getEntry(KEY_ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }

        val keyGenerator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        val spec = KeyGenParameterSpec.Builder(
            KEY_ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setUserAuthenticationRequired(false)
            .build()
        keyGenerator.init(spec)
        return keyGenerator.generateKey()
    }

    private companion object {
        const val ANDROID_KEYSTORE = "AndroidKeyStore"
        const val KEY_ALIAS = "airnote_mobile_session_v1"
        const val KEY_SESSION = "gateway_session"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
    }
}
