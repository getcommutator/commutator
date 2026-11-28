package dev.freethinkel.commutator.android.core

import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import uniffi.commutator_core.MathFace

/**
 * How transcripts show LaTeX math, local to this phone: rendered (in one of
 * the core's bundled math faces) or as its TeX source. The keys and values
 * are iOS's (`MathSettings` in TranscriptListView.swift).
 */
class MathSettings(private val prefs: SharedPreferences) {
    /** `render` off shows `$…$` as an inline code chip and `$$…$$` as a code block. */
    data class State(val render: Boolean = true, val face: MathFace = MathFace.FIRA, val size: Int = 100)

    private val _state = MutableStateFlow(read())
    val state: StateFlow<State> = _state.asStateFlow()

    private fun read() = State(
        render = prefs.getBoolean("mathRender", true),
        face = faceOf(prefs.getString("mathFont", null)),
        size = prefs.getInt("mathSize", 100),
    )

    fun setRender(on: Boolean) {
        prefs.edit().putBoolean("mathRender", on).apply()
        _state.value = _state.value.copy(render = on)
    }

    fun setFace(face: MathFace) {
        prefs.edit().putString("mathFont", key(face)).apply()
        _state.value = _state.value.copy(face = face)
    }

    fun setSize(percent: Int) {
        prefs.edit().putInt("mathSize", percent).apply()
        _state.value = _state.value.copy(size = percent)
    }

    companion object {
        val faces = listOf(MathFace.FIRA, MathFace.STIX_TWO, MathFace.COMPUTER_MODERN, MathFace.LIBERTINUS, MathFace.PAGELLA, MathFace.EULER)

        /** Formula size as a percent of the text around it (the desktop's options; Rust clamps to 75–200). */
        val sizes = listOf(90, 100, 110, 120, 135, 150)

        fun key(f: MathFace) = when (f) {
            MathFace.FIRA -> "fira"
            MathFace.STIX_TWO -> "stixTwo"
            MathFace.COMPUTER_MODERN -> "computerModern"
            MathFace.LIBERTINUS -> "libertinus"
            MathFace.PAGELLA -> "pagella"
            MathFace.EULER -> "euler"
        }

        /** Anything unknown reads as Fira Math. */
        fun faceOf(key: String?): MathFace = faces.firstOrNull { key(it) == key } ?: MathFace.FIRA

        fun label(f: MathFace) = when (f) {
            MathFace.FIRA -> "Fira Math"
            MathFace.STIX_TWO -> "STIX Two"
            MathFace.COMPUTER_MODERN -> "Computer Modern"
            MathFace.LIBERTINUS -> "Libertinus"
            MathFace.PAGELLA -> "Palatino"
            MathFace.EULER -> "Euler"
        }

        fun detail(f: MathFace) = when (f) {
            MathFace.FIRA -> "A sans that sits beside the interface font."
            MathFace.STIX_TWO -> "A serif with the Times-like TeX look."
            MathFace.COMPUTER_MODERN -> "The classic LaTeX look."
            MathFace.LIBERTINUS -> "A warm book serif."
            MathFace.PAGELLA -> "TeX Gyre Pagella, a Palatino serif."
            MathFace.EULER -> "Zapf's upright, hand-drawn math."
        }
    }
}
