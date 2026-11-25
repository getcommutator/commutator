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
import uniffi.commutator_core.serverRemoveActivityTarget
import uniffi.commutator_core.serverRemovePushTarget
import uniffi.commutator_core.serverSetActivityTarget
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

    private val _activity = MutableStateFlow(prefs.getBoolean("activity", true))
    /** The agents' live update (an ongoing notification; Android 16 promotes it to a Live Update). */
    val activity: StateFlow<Boolean> = _activity.asStateFlow()

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

    fun setActivity(on: Boolean) {
        prefs.edit().putBoolean("activity", on).apply()
        _activity.value = on
        if (!on) endActivity(unregister = true)
    }

    /**
     * Work was just sent from here: from now on the server keeps the agents' card on this phone
     * (as in T3 Code), until nothing has moved for a while. Registering sends what every machine is
     * doing at once.
     */
    fun armActivity() {
        val account = model.account.value ?: return
        if (model.isDemo || !_activity.value || !allowed() || activityShown) return
        FirebaseMessaging.getInstance().token.addOnSuccessListener { token ->
            scope.launch {
                runCatching { serverSetActivityTarget(account.serverUrl, account.token, "fcm", token, "") }
                    .onFailure { android.util.Log.w("Push", "activity target sync failed", it) }
            }
        }
    }

    /** Whether the card is up now (it then stays registered on the server). */
    private var activityShown = false

    private fun endActivity(unregister: Boolean) {
        activityShown = false
        NotificationManagerCompat.from(app).cancel(ACTIVITY_ID)
        val account = model.account.value ?: return
        if (unregister) scope.launch { runCatching { serverRemoveActivityTarget(account.serverUrl, account.token) } }
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
        activityShown = false
        NotificationManagerCompat.from(app).cancel(ACTIVITY_ID)
        runCatching { FirebaseMessaging.getInstance().deleteToken() }
    }

    fun show(data: Map<String, String>) {
        when (data["type"]) {
            "activity" -> return showActivity(data["state"] ?: return)
            // The server ended it and forgot this phone's card.
            "activity_end" -> return endActivity(unregister = false)
        }
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

    /** The server's card state (`push/activity.rs`, `State`) as one quiet ongoing notification. */
    private fun showActivity(json: String) {
        if (!_activity.value || !allowed()) return
        val state = runCatching { org.json.JSONObject(json) }.getOrNull() ?: return
        val active = state.optInt("active")
        val attention = state.optInt("attention")
        val rows = state.optJSONArray("rows") ?: org.json.JSONArray()
        val lines = (0 until rows.length()).map { rows.getJSONObject(it) }
        val failed = lines.any { it.optString("phase") == "failed" }
        val title = when {
            active == 0 -> if (failed) "Finished, with a failure" else "All done"
            attention > 0 -> "$active agent${if (active == 1) "" else "s"} active · $attention need${if (attention == 1) "s" else ""} you"
            else -> "$active agent${if (active == 1) "" else "s"} active"
        }
        val chip = when {
            attention > 0 -> "Input"
            failed -> "Failed"
            active > 0 -> "Active"
            else -> "Done"
        }
        val label = mapOf("working" to "Working", "input" to "Needs you", "failed" to "Failed", "done" to "Done")
        val focus = lines.firstOrNull()?.optString("chatId")
        val manager = app.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(ACTIVITY_CHANNEL, "Agents", NotificationManager.IMPORTANCE_LOW))
        val open = PendingIntent.getActivity(
            app, ACTIVITY_ID,
            Intent(app, MainActivity::class.java).apply { focus?.let { putExtra(EXTRA_CHAT, it) } }.addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val dismiss = PendingIntent.getBroadcast(
            app, ACTIVITY_ID, Intent(app, ActivityDismissReceiver::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val style = NotificationCompat.InboxStyle()
        lines.forEach { style.addLine("${label[it.optString("phase")] ?: "Done"} · ${it.optString("title")}") }
        val notification = NotificationCompat.Builder(app, ACTIVITY_CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(lines.firstOrNull()?.let { "${label[it.optString("phase")]} · ${it.optString("title")}" })
            .setStyle(style)
            .setOngoing(active > 0)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setContentIntent(open)
            .addAction(0, "Open", open)
            .addAction(0, "Dismiss", dismiss)
            // Nothing heard for this long: the server would have ended it.
            .setTimeoutAfter(2 * 60 * 60 * 1000L)
            // Android 16: a Live Update, with the chip in the status bar.
            .setRequestPromotedOngoing(true)
            .setShortCriticalText(chip)
            .build()
        manager.notify(ACTIVITY_ID, notification)
        activityShown = true
    }

    /** The card's Dismiss: gone here, and the server stops sending it. */
    fun dismissActivity() = endActivity(unregister = true)

    companion object {
        const val ACTIVITY_CHANNEL = "agents"
        const val ACTIVITY_ID = 1
        const val CHANNEL = "sessions"
        const val EXTRA_CHAT = "chatId"
    }
}

/** The live update's Dismiss button. */
class ActivityDismissReceiver : android.content.BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        (context.applicationContext as CommutatorApplication).model.push.dismissActivity()
    }
}

/** FCM's end: new tokens go to the server, messages become notifications. */
class PushService : FirebaseMessagingService() {
    private val push get() = (application as CommutatorApplication).model.push

    override fun onNewToken(token: String) = push.sync(token)

    override fun onMessageReceived(message: RemoteMessage) = push.show(message.data)
}
