package com.emiac.airnote.android

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.runtime.mutableStateOf

class MainActivity : ComponentActivity() {
    private val oauthToken = mutableStateOf<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        oauthToken.value = extractOAuthToken(intent)
        setContent {
            AirNoteAndroidApp(
                oauthToken = oauthToken.value,
                onOAuthTokenConsumed = { oauthToken.value = null },
            )
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        oauthToken.value = extractOAuthToken(intent)
    }

    private fun extractOAuthToken(intent: Intent?): String? =
        intent?.data
            ?.takeIf { it.scheme == "airnote" && it.host == "auth" && it.path == "/callback" }
            ?.getQueryParameter("token")
            ?.trim()
            ?.takeIf { it.isNotEmpty() }
}
