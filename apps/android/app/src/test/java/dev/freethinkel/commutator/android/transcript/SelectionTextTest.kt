package dev.freethinkel.commutator.android.transcript

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.commutator_core.ColorRole
import uniffi.commutator_core.Decoration
import uniffi.commutator_core.RowDisplay
import uniffi.commutator_core.TextRun

class SelectionTextTest {
    private fun run(start: Int, len: Int, baseline: Float) =
        TextRun(start.toUInt(), len.toUInt(), 0f, baseline, 10f, 0u, ColorRole.TEXT, Decoration.NONE, null, null)

    // Two paragraphs back to back; the first wraps after "hello " (space kept).
    private val row = RowDisplay(
        1u, 1u, 100f, 60f, "hello worldSecond",
        listOf(run(0, 6, 10f), run(6, 5, 30f), run(11, 6, 50f)),
        emptyList(), emptyList(), emptyList(), emptyList(), emptyList(), emptyList(), "",
    )

    @Test
    fun a_soft_wrap_joins_and_a_paragraph_breaks() {
        assertEquals("hello world\nSecond", row.selectedText(0, Int.MAX_VALUE))
        assertEquals("lo wor", row.selectedText(3, 9))
    }

    @Test
    fun a_press_selects_the_word_not_the_next_paragraph() {
        assertEquals(6 to 11, row.wordRange(8))
        assertEquals(11 to 17, row.wordRange(12))
    }
}
