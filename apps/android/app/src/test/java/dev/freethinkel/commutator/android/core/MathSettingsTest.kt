package dev.freethinkel.commutator.android.core

import android.content.SharedPreferences
import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.commutator_core.MathFace

class MathSettingsTest {
    @Test
    fun nothing_stored_reads_as_rendered_fira_at_100() {
        assertEquals(MathSettings.State(render = true, face = MathFace.FIRA, size = 100), MathSettings(MemoryPrefs()).state.value)
    }

    @Test
    fun what_is_set_reads_back_from_a_new_store() {
        val prefs = MemoryPrefs()
        MathSettings(prefs).apply {
            setRender(false)
            setFace(MathFace.COMPUTER_MODERN)
            setSize(135)
        }
        assertEquals("computerModern", prefs.values["mathFont"])
        assertEquals(MathSettings.State(render = false, face = MathFace.COMPUTER_MODERN, size = 135), MathSettings(prefs).state.value)
    }

    @Test
    fun an_unknown_face_reads_as_fira() {
        val prefs = MemoryPrefs().apply { values["mathFont"] = "comicSans" }
        assertEquals(MathFace.FIRA, MathSettings(prefs).state.value.face)
    }

    /** Just the SharedPreferences calls the store makes. */
    private class MemoryPrefs : SharedPreferences {
        val values = HashMap<String, Any?>()

        override fun getAll(): Map<String, *> = values
        override fun getString(key: String, defValue: String?) = values[key] as? String ?: defValue
        override fun getStringSet(key: String, defValues: Set<String>?) = defValues
        override fun getInt(key: String, defValue: Int) = values[key] as? Int ?: defValue
        override fun getLong(key: String, defValue: Long) = values[key] as? Long ?: defValue
        override fun getFloat(key: String, defValue: Float) = values[key] as? Float ?: defValue
        override fun getBoolean(key: String, defValue: Boolean) = values[key] as? Boolean ?: defValue
        override fun contains(key: String) = key in values
        override fun registerOnSharedPreferenceChangeListener(l: SharedPreferences.OnSharedPreferenceChangeListener?) = Unit
        override fun unregisterOnSharedPreferenceChangeListener(l: SharedPreferences.OnSharedPreferenceChangeListener?) = Unit

        override fun edit(): SharedPreferences.Editor = object : SharedPreferences.Editor {
            override fun putString(key: String, value: String?) = apply { values[key] = value }
            override fun putStringSet(key: String, values: Set<String>?) = this
            override fun putInt(key: String, value: Int) = apply { values[key] = value }
            override fun putLong(key: String, value: Long) = apply { values[key] = value }
            override fun putFloat(key: String, value: Float) = apply { values[key] = value }
            override fun putBoolean(key: String, value: Boolean) = apply { values[key] = value }
            override fun remove(key: String) = apply { values.remove(key) }
            override fun clear() = apply { values.clear() }
            override fun commit() = true
            override fun apply() = Unit
        }
    }
}
