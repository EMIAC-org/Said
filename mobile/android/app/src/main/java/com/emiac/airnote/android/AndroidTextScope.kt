package com.emiac.airnote.android

enum class AndroidRewriteScope {
    Selection,
    CursorSentence,
}

data class AndroidRewriteTarget(
    val text: String,
    val scope: AndroidRewriteScope,
    val start: Int,
    val end: Int,
    val fullText: String,
) {
    val canReplace: Boolean
        get() = start >= 0 && end > start && end <= fullText.length && fullText.substring(start, end) == text
}

fun resolveAndroidRewriteTarget(
    fullText: CharSequence?,
    selectionStart: Int,
    selectionEnd: Int,
): AndroidRewriteTarget? {
    val full = fullText?.toString().orEmpty()
    if (full.isBlank()) return null

    selectedRange(full, selectionStart, selectionEnd)?.let { (start, end) ->
        return AndroidRewriteTarget(
            text = full.substring(start, end),
            scope = AndroidRewriteScope.Selection,
            start = start,
            end = end,
            fullText = full,
        )
    }

    val cursor = selectionEnd.takeIf { it in 0..full.length }
        ?: selectionStart.takeIf { it in 0..full.length }
        ?: full.length

    lastSentenceRangeBeforeCursor(full, cursor)?.let { (start, end) ->
        return AndroidRewriteTarget(
            text = full.substring(start, end),
            scope = AndroidRewriteScope.CursorSentence,
            start = start,
            end = end,
            fullText = full,
        )
    }

    return null
}

fun replaceAndroidRewriteTarget(target: AndroidRewriteTarget, replacement: String): String? {
    val clean = replacement.trim()
    if (clean.isBlank() || !target.canReplace) return null
    return target.fullText.substring(0, target.start) + clean + target.fullText.substring(target.end)
}

fun buildAndroidRewriteScreenContext(
    target: AndroidRewriteTarget,
    hint: CharSequence?,
    className: CharSequence?,
): String {
    val before = target.fullText.substring(0, target.start).takeLast(180)
    val after = target.fullText.substring(target.end).take(120)
    return listOf(
        "platform=android",
        "scope=${target.scope.name.lowercase()}",
        "hint=${hint?.toString().orEmpty().take(80)}",
        "class=${className?.toString().orEmpty().take(80)}",
        "before=$before",
        "after=$after",
    ).joinToString("\n")
}

private fun selectedRange(full: String, selectionStart: Int, selectionEnd: Int): Pair<Int, Int>? {
    if (selectionStart !in 0..full.length || selectionEnd !in 0..full.length) return null
    if (selectionStart == selectionEnd) return null
    val rawStart = minOf(selectionStart, selectionEnd)
    val rawEnd = maxOf(selectionStart, selectionEnd)
    val selected = full.substring(rawStart, rawEnd)
    val leading = selected.indexOfFirst { !it.isWhitespace() }
    val trailing = selected.indexOfLast { !it.isWhitespace() }
    if (leading < 0 || trailing < leading) return null
    return rawStart + leading to rawStart + trailing + 1
}

private fun lastSentenceRangeBeforeCursor(full: String, cursor: Int): Pair<Int, Int>? {
    val safeCursor = cursor.coerceIn(0, full.length)
    val before = full.substring(0, safeCursor)
    val end = before.indexOfLast { !it.isWhitespace() } + 1
    if (end <= 0) return null

    val sentenceStartBoundary = before
        .substring(0, end)
        .indexOfLast { it == '.' || it == '?' || it == '!' || it == '\n' || it == '\r' }
    val rawStart = sentenceStartBoundary + 1
    val start = full.substring(rawStart, end).indexOfFirst { !it.isWhitespace() }
        .takeIf { it >= 0 }
        ?.let { rawStart + it }
        ?: return null

    if (end <= start) return null
    val candidate = full.substring(start, end)
    return candidate.takeIf { it.any(Char::isLetterOrDigit) }?.let { start to end }
}
