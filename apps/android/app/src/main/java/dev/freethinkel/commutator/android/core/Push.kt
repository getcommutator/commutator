package dev.freethinkel.commutator.android.core

import android.Manifest
import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import dev.freethinkel.commutator.android.CommutatorApplication
import dev.freethinkel.commutator.android.MainActivity
import dev.freethinkel.commutator.android.R
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import uniffi.commutator_core.serverRemovePushTarget
import uniffi.commutator_core.serverSetPushTarget

/**
 * Session notifications, like the desktop's and the iOS app's: a run finished, a session is
 * waiting on you, a run failed. The machine's engine decides and the server sends them over FCM
 * (`apps/commutator-server/src/push.rs`); this keeps the choices, hands the server this phone's
 * token, shows them while the app is in the background, and opens the session on a tap.
 */
class Push(private val app: Application, private val model: AppModel) {
    enum class Kind(val key: String, val title: String) {
        Done("done", "Run finished"),
        Input("input", "Waiting on your input"),
        Failed("failed", "Run failed"),
    }

    private val prefs = app.getSharedPreferences("notifications", Context.MODE_PRIVATE)
    private val scope = MainScope()

    private val _enabled = MutableStateFlow(prefs.getBoolean("enabled", true))
    /** The master switch (on unless turned off here; the system permission is separate). */
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    private val _kinds = MutableStateFlow(Kind.entries.associateWith { prefs.getBoolean(it.key, true) })
    val kinds: StateFlow<Map<Kind, Boolean>> = _kinds.asStateFlow()

    /** Set when the system should be asked (the root holds the permission launcher). */
    val askPermission = MutableStateFlow(false)

    /** A tapped notification's chat, until the shell opens it. */
    val tappedChat = MutableStateFlow<String?>(null)

    fun setEnabled(on: Boolean) {
        prefs.edit().putBoolean("enabled", on).apply()
        _enabled.value = on
        if (on && !allowed()) askPermission.value = true
        sync()
    }

    fun set(kind: Kind, on: Boolean) {
        prefs.edit().putBoolean(kind.key, on).apply()
        _kinds.value = _kinds.value + (kind to on)
        sync()
    }

    /** Android allows this app to post them (13+ asks; before that, the per-app switch). */
    fun allowed(): Boolean =
        (Build.VERSION.SDK_INT < 33 ||
            ContextCompat.checkSelfPermission(app, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) &&
            NotificationManagerCompat.from(app).areNotificationsEnabled()

    /** The first time a session is started from this phone: the moment the ask makes sense. */
    fun askAfterFirstSession() {
        if (model.isDemo || !_enabled.value || prefs.getBoolean("asked", false)) return
        prefs.edit().putBoolean("asked", true).apply()
        if (!allowed()) askPermission.value = true
    }

    /** Tell the server what this phone wants now (or that it wants nothing). */
    fun sync(token: String? = null) {
        val account = model.account.value ?: return
        if (model.isDemo) return
        if (!_enabled.value) {
            scope.launch { runCatching { serverRemovePushTarget(account.serverUrl, account.token) } }
            return
        }
        if (token != null) return register(account, token)
        FirebaseMessaging.getInstance().token.addOnSuccessListener { register(account, it) }
    }

    private fun register(account: CredentialStore.Account, token: String) {
        val kinds = _kinds.value
        scope.launch {
            runCatching {
                serverSetPushTarget(
                    account.serverUrl, account.token, "fcm", token, "production",
                    kinds[Kind.Done] != false, kinds[Kind.Input] != false, kinds[Kind.Failed] != false,
                )
            }.onFailure { android.util.Log.w("Push", "push target sync failed", it) }
        }
    }

    /** Signing out removes this phone on the server, its push target with it. */
    fun signingOut() {
        runCatching { FirebaseMessaging.getInstance().deleteToken() }
    }

    fun show(data: Map<String, String>) {
        // In the app: no banner (the list and the session already show it).
        if (model.inForegroundNow || !allowed()) return
        val chatId = data["chatId"] ?: return
        val manager = app.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(CHANNEL, "Sessions", NotificationManager.IMPORTANCE_HIGH))
        val open = PendingIntent.getActivity(
            app, chatId.hashCode(),
            Intent(app, MainActivity::class.java).putExtra(EXTRA_CHAT, chatId).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(data["title"])
            .setContentText(data["body"])
            .setAutoCancel(true)
            .setContentIntent(open)
            .build()
        // One per chat: a newer one replaces it.
        manager.notify(chatId.hashCode(), notification)
    }

    companion object {
        const val CHANNEL = "sessions"
        const val EXTRA_CHAT = "chatId"
    }
}

/** FCM's end: new tokens go to the server, messages become notifications. */
class PushService : FirebaseMessagingService() {
    private val push get() = (application as CommutatorApplication).model.push

    override fun onNewToken(token: String) = push.sync(token)

    override fun onMessageReceived(message: RemoteMessage) = push.show(message.data)
}
