package dev.freethinkel.commutator.android.design

import androidx.compose.material3.ColorScheme
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import uniffi.commutator_core.ColorRole
import uniffi.commutator_core.PhonePalette

/**
 * Transcript paint roles → ARGB for one appearance. Paint-only: nothing here
 * affects layout. Neutrals are Commutator's (the iOS palette); accent roles follow
 * the Material scheme so wallpaper colors flow into links and quotes.
 */
/**
 * Transcript role colours: the selected theme's role table. Text and the user bubble always come
 * from the Material scheme (built from the same theme; Android's bubble is its container tone);
 * with "Wallpaper colors" on (`dynamic`) the accent roles do too, so the wallpaper's palette
 * reaches the transcript.
 */
class TranscriptPalette(dark: Boolean, scheme: ColorScheme, theme: PhonePalette, dynamic: Boolean) {
    private val colors = IntArray(ColorRole.entries.size)

    val background: Int = scheme.background.toArgb()

    operator fun get(role: ColorRole): Int = colors[role.ordinal]

    init {
        val accent = scheme.primary
        for (role in ColorRole.entries) {
            colors[role.ordinal] = when (role) {
                ColorRole.TEXT -> scheme.onSurface.toArgb()
                ColorRole.USER_BUBBLE -> scheme.surfaceContainerHigh.toArgb()
                ColorRole.LINK, ColorRole.ACCENT -> if (dynamic) accent.toArgb() else theme.roles[role.ordinal].toInt()
                ColorRole.QUOTE_BAR -> if (dynamic) accent.copy(alpha = 0.45f).toArgb() else theme.roles[role.ordinal].toInt()
                ColorRole.DIFF_HUNK -> if (dynamic) accent.copy(alpha = if (dark) 0.08f else 0.07f).toArgb() else theme.roles[role.ordinal].toInt()
                else -> theme.roles[role.ordinal].toInt()
            }
        }
    }
}

/** Project tones — the desktop's monogram palette, by `project_color_index`. */
object ProjectColors {
    private val light = longArrayOf(0x475569, 0x2563EB, 0x7C3AED, 0xBE123C, 0xA16207, 0x047857, 0x0F766E, 0xC2410C)
    private val dark = longArrayOf(0x94A3B8, 0x93C5FD, 0xC4B5FD, 0xFDA4AF, 0xFCD34D, 0x6EE7B7, 0x5EEAD4, 0xFDBA74)

    fun color(index: Int, isDark: Boolean): Color {
        val table = if (isDark) dark else light
        return Color(table[Math.floorMod(index, table.size)] or 0xFF000000)
    }
}
