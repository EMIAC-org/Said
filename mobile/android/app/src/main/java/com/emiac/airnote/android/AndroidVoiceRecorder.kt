package com.emiac.airnote.android

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

class AndroidVoiceRecorder(
    private val sampleRate: Int = 16_000,
) {
    private val lock = Any()
    private var record: AudioRecord? = null
    private var readJob: Job? = null
    @Volatile
    private var running = false
    private var pcm = ByteArrayOutputStream()

    val isRecording: Boolean
        get() = running

    @SuppressLint("MissingPermission")
    fun start(
        context: Context,
        scope: CoroutineScope,
        onLevel: (Float) -> Unit,
    ): Boolean {
        if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            return false
        }
        if (running) {
            return true
        }

        val minBuffer = AudioRecord.getMinBufferSize(
            sampleRate,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        val bufferSize = maxOf(minBuffer, sampleRate / 5 * 2)
        val nextRecord = AudioRecord.Builder()
            .setAudioSource(MediaRecorder.AudioSource.VOICE_RECOGNITION)
            .setAudioFormat(
                AudioFormat.Builder()
                    .setSampleRate(sampleRate)
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setChannelMask(AudioFormat.CHANNEL_IN_MONO)
                    .build(),
            )
            .setBufferSizeInBytes(bufferSize)
            .build()

        if (nextRecord.state != AudioRecord.STATE_INITIALIZED) {
            nextRecord.release()
            return false
        }

        synchronized(lock) {
            pcm = ByteArrayOutputStream()
            record = nextRecord
            running = true
        }
        try {
            nextRecord.startRecording()
        } catch (_: IllegalStateException) {
            synchronized(lock) {
                record = null
                running = false
                pcm = ByteArrayOutputStream()
            }
            nextRecord.release()
            return false
        }
        readJob = scope.launch(Dispatchers.IO) {
            val buffer = ByteArray(bufferSize)
            while (running) {
                val read = nextRecord.read(buffer, 0, buffer.size)
                if (read > 0) {
                    synchronized(lock) {
                        pcm.write(buffer, 0, read)
                    }
                    onLevel(averageLevel(buffer, read))
                }
            }
        }
        return true
    }

    suspend fun stop(): ByteArray = withContext(Dispatchers.IO) {
        val current = synchronized(lock) {
            running = false
            record
        }
        runCatching { current?.stop() }
        withTimeoutOrNull(800) { readJob?.join() }
        current?.release()
        val pcmBytes = synchronized(lock) {
            record = null
            readJob = null
            pcm.toByteArray()
        }
        encodeWav(pcmBytes)
    }

    fun cancel() {
        val current = synchronized(lock) {
            running = false
            record
        }
        readJob?.cancel()
        runCatching { current?.stop() }
        current?.release()
        synchronized(lock) {
            record = null
            readJob = null
            pcm = ByteArrayOutputStream()
        }
    }

    private fun averageLevel(buffer: ByteArray, length: Int): Float {
        if (length < 2) {
            return 0f
        }
        var sum = 0.0
        var samples = 0
        var index = 0
        while (index + 1 < length) {
            val sample = ((buffer[index + 1].toInt() shl 8) or (buffer[index].toInt() and 0xFF)).toShort()
            val value = sample.toDouble() / Short.MAX_VALUE
            sum += value * value
            samples += 1
            index += 2
        }
        return kotlin.math.sqrt(sum / samples.coerceAtLeast(1)).toFloat().coerceIn(0f, 1f)
    }

    private fun encodeWav(pcmBytes: ByteArray): ByteArray {
        val dataSize = pcmBytes.size
        val byteRate = sampleRate * 2
        val header = ByteBuffer.allocate(44).order(ByteOrder.LITTLE_ENDIAN)
        header.put("RIFF".toByteArray(Charsets.US_ASCII))
        header.putInt(36 + dataSize)
        header.put("WAVE".toByteArray(Charsets.US_ASCII))
        header.put("fmt ".toByteArray(Charsets.US_ASCII))
        header.putInt(16)
        header.putShort(1)
        header.putShort(1)
        header.putInt(sampleRate)
        header.putInt(byteRate)
        header.putShort(2)
        header.putShort(16)
        header.put("data".toByteArray(Charsets.US_ASCII))
        header.putInt(dataSize)
        return header.array() + pcmBytes
    }
}
