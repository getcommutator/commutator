package dev.freethinkel.vardoger.android.core

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import org.json.JSONObject

/**
 * This phone's sign-in to the account's Vardoger server and the machine it
 * drives, in app-private storage. Tokens are sealed with an
 * Android Keystore key (the iOS Keychain's counterpart): the key never
 * leaves the keystore, so a copied preferences file can't be replayed.
 */
class CredentialStore(context: Context) {
    private val prefs = context.getSharedPreferences("credentials", Context.MODE_PRIVATE)

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .build(),
        )
        return generator.generateKey()
    }

    private fun seal(plain: String): String {
        val cipher = Cipher.getInstance(TRANSFORMATION).apply { init(Cipher.ENCRYPT_MODE, key()) }
        val sealed = cipher.iv + cipher.doFinal(plain.toByteArray())
        return Base64.encodeToString(sealed, Base64.NO_WRAP)
    }

    private fun open(sealed: String): String? = runCatching {
        val bytes = Base64.decode(sealed, Base64.NO_WRAP)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes, 0, 12))
        String(cipher.doFinal(bytes, 12, bytes.size - 12))
    }.getOrNull()

    private fun storedJson(): String? {
        prefs.getString("sealed", null)?.let { return open(it) }
        // Builds before the keystore stored plain JSON: seal it on first read.
        val legacy = prefs.getString("credentials", null) ?: return null
        prefs.edit().putString("sealed", seal(legacy)).remove("credentials").apply()
        return legacy
    }

    /** This phone's sign-in to the account's Vardoger server. */
    data class Account(val serverUrl: String, val token: String, val deviceId: String, val email: String, val name: String?)

    /** The machine whose engine this phone drives. */
    data class Host(val id: String, val name: String)

    fun account(): Account? {
        val json = storedJson()?.let { runCatching { JSONObject(it) }.getOrNull() } ?: return null
        if (json.optString("kind") != "server") return null
        return Account(
            json.getString("serverUrl"),
            json.getString("token"),
            json.getString("deviceId"),
            json.getString("email"),
            json.optString("name").ifEmpty { null },
        )
    }

    fun store(account: Account) {
        val json = JSONObject()
            .put("kind", "server")
            .put("serverUrl", account.serverUrl)
            .put("token", account.token)
            .put("deviceId", account.deviceId)
            .put("email", account.email)
            .put("name", account.name ?: "")
        prefs.edit().putString("sealed", seal(json.toString())).remove("credentials").apply()
    }

    var host: Host?
        get() = prefs.getString("hostId", null)?.let { Host(it, prefs.getString("hostName", null) ?: it) }
        set(value) {
            prefs.edit().putString("hostId", value?.id).putString("hostName", value?.name).apply()
        }

    /** The last server signed in to (the sign-in page's field). Kept across sign-outs. */
    var lastServerUrl: String?
        get() = prefs.getString("lastServerUrl", null)
        set(value) = prefs.edit().putString("lastServerUrl", value).apply()

    fun clear() {
        val server = lastServerUrl
        prefs.edit().clear().apply()
        lastServerUrl = server
    }

    private companion object {
        const val KEY_ALIAS = "vardoger-credentials"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
    }
}
