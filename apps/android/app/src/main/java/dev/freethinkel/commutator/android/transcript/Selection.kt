package dev.freethinkel.commutator.android.transcript

import android.graphics.RectF
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import dev.freethinkel.commutator.android.transcript.TranscriptState.TextPoint
import uniffi.commutator_core.RowDisplay
import java.text.BreakIterator
import kotlin.math.hypot
import kotlin.math.max

// The transcript is painted, so selection is ours: a long press selects the
// word under the finger and keeps extending while it moves; handles adjust
// either end, across rows; a tap clears. Positions are (row key, UTF-16
// offset into the row's text); geometry is in content dp.

internal class Ordered(val lo: TextPoint, val loIndex: UInt, val hi: TextPoint, val hiIndex: UInt)

private fun TranscriptState.index(p: TextPoint) = frame?.indexOf(p.key)

/** a before b in reading order (null when either row is gone). */
internal fun TranscriptState.precedes(a: TextPoint, b: TextPoint): Boolean? {
    val ia = index(a) ?: return null
    val ib = index(b) ?: return null
    return ia < ib || (ia == ib && a.offset < b.offset)
}

internal fun TranscriptState.ordered(): Ordered? {
    val a = selAnchor ?: return null
    val f = selFocus ?: return null
    val ia = index(a) ?: return null
    val iff = index(f) ?: return null
    return if (ia < iff || (ia == iff && a.offset <= f.offset)) Ordered(a, ia, f, iff) else Ordered(f, iff, a, ia)
}

fun TranscriptState.clearSelection() {
    selAnchor = null
    selFocus = null
    pressWord = null
}

/** The selected part `[from, to)` of row `index`'s text, or null. */
internal fun TranscriptState.selectionIn(index: UInt): Pair<Int, Int>? {
    val o = ordered() ?: return null
    if (index < o.loIndex || index > o.hiIndex) return null
    return (if (index == o.loIndex) o.lo.offset else 0) to (if (index == o.hiIndex) o.hi.offset else Int.MAX_VALUE)
}

/** Where scroller `s`'s content sits in its row (dp). */
private fun TranscriptState.scrollerShift(key: ULong, m: RowModel, s: UInt?, d: Float): Pair<Float, Float> {
    s ?: return 0f to 0f
    val sc = m.display.scrollers.getOrNull(s.toInt()) ?: return 0f to 0f
    return (sc.x - (scrolls[key to s.toInt()]?.value ?: 0) / d) to sc.y
}

/** The text position nearest content point (`x`, `y`) dp, within `limit` dp. */
internal fun TranscriptState.hitText(x: Float, y: Float, d: Float, limit: Float = Float.MAX_VALUE): TextPoint? {
    val f = frame ?: return null
    var best: TextPoint? = null
    var bestDistance = Float.MAX_VALUE
    for (p in f.rowsIn(y - 300f, y + 300f)) {
        if (max(0f, max(p.y - y, y - (p.y + p.height))) >= bestDistance) continue
        val m = model(f, p.index, p.key, p.version) ?: continue
        var rowBest: Triple<Int, Float, Float>? = null // offset, dy, dx
        for (i in m.textOrder) {
            val r = m.display.runs[i]
            val (sx, sy) = scrollerShift(p.key, m, r.scroller, d)
            val lx = x - sx
            val ly = y - p.y - sy
            val box = m.runBox(i, fonts, d)
            val dy = max(0f, max(box.top - ly, ly - box.bottom))
            val dx = max(0f, max(box.left - lx, lx - box.right))
            val b = rowBest
            if (b != null && (b.second < dy || (b.second == dy && b.third <= dx))) continue
            rowBest = Triple(m.runOffset(i, lx, fonts, d), dy, dx)
        }
        val b = rowBest ?: continue
        val distance = hypot(b.second, b.third)
        if (distance < bestDistance) {
            bestDistance = distance
            best = TextPoint(p.key, b.first)
        }
    }
    return if (bestDistance <= limit) best else null
}

/** Caret at `pt` in content dp: on the run it starts (or, for the end, ends). */
internal fun TranscriptState.caret(pt: TextPoint, end: Boolean, d: Float): RectF? {
    val f = frame ?: return null
    val p = f.indexOf(pt.key)?.let { f.placement(it) } ?: return null
    val m = model(f, p.index, p.key, p.version) ?: return null
    val runs = m.display.runs
    val order = m.textOrder
    val i = (if (end) order.lastOrNull { runs[it].start.toInt() < pt.offset } ?: order.firstOrNull()
    else order.firstOrNull { runs[it].start.toInt() + runs[it].len.toInt() > pt.offset } ?: order.lastOrNull()) ?: return null
    val box = m.runBox(i, fonts, d)
    val (sx, sy) = scrollerShift(p.key, m, runs[i].scroller, d)
    val x = m.runX(i, pt.offset, fonts, d) + sx
    return RectF(x, p.y + sy + box.top, x, p.y + sy + box.bottom)
}

internal fun TranscriptState.selectedText(): String {
    val o = ordered() ?: return ""
    val f = frame ?: return ""
    val parts = ArrayList<String>()
    for (i in o.loIndex..o.hiIndex) {
        val display = f.placement(i)?.let { model(f, i, it.key, it.version)?.display } ?: continue
        val text = display.selectedText(if (i == o.loIndex) o.lo.offset else 0, if (i == o.hiIndex) o.hi.offset else Int.MAX_VALUE)
        if (text.isNotEmpty()) parts.add(text)
    }
    return parts.joinToString("\n\n")
}

/** Long press: select the word under content point (`x`, `y`). */
internal fun TranscriptState.beginPress(x: Float, y: Float, d: Float): Boolean {
    val hit = hitText(x, y, d, limit = 24f) ?: return false
    val f = frame ?: return false
    val p = f.indexOf(hit.key)?.let { f.placement(it) } ?: return false
    val display = model(f, p.index, p.key, p.version)?.display ?: return false
    val (lo, hi) = display.wordRange(hit.offset)
    val start = TextPoint(hit.key, lo)
    val end = TextPoint(hit.key, hi)
    pressWord = start to end
    selAnchor = start
    selFocus = end
    return true
}

/** The press moved: grow back from the word's end, or forward from its start. */
internal fun TranscriptState.extendPress(x: Float, y: Float, d: Float) {
    val (start, end) = pressWord ?: return
    val hit = hitText(x, y, d) ?: return
    if (precedes(hit, start) == true) {
        selAnchor = end
        selFocus = hit
    } else {
        selAnchor = start
        selFocus = if (precedes(hit, end) == true) end else hit
    }
}

/** Selection tint for layer `layer` of a row (dp × `d` = px). */
internal fun DrawScope.drawSelection(m: RowModel, layer: Int, range: Pair<Int, Int>, d: Float, fonts: StyleFonts, color: Color) {
    for (i in m.textOrder) {
        val r = m.display.runs[i]
        if ((r.scroller?.toInt()?.plus(1) ?: 0) != layer) continue
        val lo = max(range.first, r.start.toInt())
        val hi = minOf(range.second, r.start.toInt() + r.len.toInt())
        if (lo >= hi) continue
        val box = m.runBox(i, fonts, d)
        val x0 = m.runX(i, lo, fonts, d)
        val x1 = m.runX(i, hi, fonts, d)
        drawRect(color, androidx.compose.ui.geometry.Offset(x0 * d, box.top * d), androidx.compose.ui.geometry.Size((x1 - x0) * d, box.height() * d))
    }
}

/**
 * The painted text in `[from, to)`. Paragraphs sit back to back in `text`,
 * so a line change with no whitespace between becomes a newline.
 */
fun RowDisplay.selectedText(from: Int, to: Int): String {
    val end = minOf(to, text.length)
    if (from >= end) return ""
    val out = StringBuilder()
    var prevEnd = -1
    var prevBaseline = 0f
    var prevScroller: UInt? = null
    for (r in runs.sortedBy { it.start }) {
        if (r.len == 0u) continue
        var s = max(from, r.start.toInt())
        val e = minOf(end, r.start.toInt() + r.len.toInt())
        if (prevEnd >= 0) {
            s = max(s, prevEnd)
            if (s >= e) continue
            if (s > prevEnd) {
                out.append(text, prevEnd, s)
            } else if ((r.baseline != prevBaseline || r.scroller != prevScroller) && out.isNotEmpty() && !out.last().isWhitespace()) {
                out.append('\n')
            }
        } else if (s >= e) {
            continue
        }
        out.append(text, s, e)
        prevEnd = e
        prevBaseline = r.baseline
        prevScroller = r.scroller
    }
    return out.toString()
}

/**
 * The word around `offset` (a single character between words), `[start, end)`,
 * within its run: paragraphs sit back to back in `text`, so their edge words
 * would otherwise merge.
 */
fun RowDisplay.wordRange(offset: Int): Pair<Int, Int> {
    if (text.isEmpty()) return 0 to 0
    val o = offset.coerceIn(0, text.length - 1)
    val run = runs.firstOrNull { o >= it.start.toInt() && o < it.start.toInt() + it.len.toInt() }
    val lo = run?.start?.toInt() ?: 0
    val hi = run?.let { minOf(text.length, lo + it.len.toInt()) } ?: text.length
    val words = BreakIterator.getWordInstance().apply { setText(this@wordRange.text.substring(lo, hi)) }
    val end = words.following(o - lo)
    val start = words.previous()
    if (start != BreakIterator.DONE && end != BreakIterator.DONE && text.substring(lo + start, lo + end).any { it.isLetterOrDigit() }) {
        return lo + start to lo + end
    }
    return o to minOf(text.length, o + if (Character.isHighSurrogate(text[o])) 2 else 1)
}
