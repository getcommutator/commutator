package dev.freethinkel.commutator.android.core

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ApplicationInfo
import android.content.pm.PackageInstaller
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL

/**
 * Updates without a store: the latest GitHub release of getcommutator/commutator with an asset named
 * `commutator-<version>-android-<versionCode>.apk` (the release workflow's android job uploads one). The system asks
 * the user before installing, and installs over this app only when the APK is signed with the same key.
 */
object AppUpdate {
    class Release(val code: Long, val name: String, val apk: String)

    var available by mutableStateOf<Release?>(null)
        private set
    /** 0…1 while downloading, null otherwise. */
    var progress by mutableStateOf<Float?>(null)
        private set
    var error by mutableStateOf<String?>(null)
        private set

    private val asset = Regex("""commutator-.+-android-(\d+)\.apk""")
    private var checked = false

    /** Once per process. Debug builds are signed with a per-machine key and can't take a release APK, so they skip it. */
    suspend fun check(context: Context) {
        if (checked || context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) return
        checked = true
        // ponytail: stable channel only (releases/latest), unauthenticated (60 requests an hour per IP)
        val release = runCatching { JSONObject(String(get("https://api.github.com/repos/getcommutator/commutator/releases/latest"))) }
            .getOrNull() ?: return
        val assets = release.getJSONArray("assets")
        val current = context.packageManager.getPackageInfo(context.packageName, 0).longVersionCode
        available = (0 until assets.length()).map { assets.getJSONObject(it) }
            .mapNotNull { a ->
                asset.matchEntire(a.getString("name"))?.let {
                    Release(it.groupValues[1].toLong(), release.optString("name").ifEmpty { release.getString("tag_name") }, a.getString("browser_download_url"))
                }
            }
            .maxByOrNull { it.code }
            ?.takeIf { it.code > current }
    }

    suspend fun install(context: Context) {
        val release = available ?: return
        if (progress != null) return
        error = null
        progress = 0f
        try {
            val apk = get(release.apk) { progress = it }
            progress = 1f
            withContext(Dispatchers.IO) {
                val installer = context.packageManager.packageInstaller
                val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
                    .apply { setAppPackageName(context.packageName) }
                val id = installer.createSession(params)
                installer.openSession(id).use { s ->
                    s.openWrite("app.apk", 0, apk.size.toLong()).use { it.write(apk); s.fsync(it) }
                    listen(context)
                    val intent = Intent(ACTION).setPackage(context.packageName)
                    s.commit(PendingIntent.getBroadcast(context, id, intent, PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT).intentSender)
                }
            }
        } catch (e: Exception) {
            error = e.message ?: "Download failed"
        } finally {
            progress = null
        }
    }

    /** The body of a GET, following GitHub's redirect to its asset storage. */
    private suspend fun get(url: String, onProgress: (Float) -> Unit = {}): ByteArray = withContext(Dispatchers.IO) {
        val c = URL(url).openConnection() as HttpURLConnection
        try {
            c.setRequestProperty("Accept", if (url.contains("api.github.com")) "application/vnd.github+json" else "application/octet-stream")
            if (c.responseCode !in 200..299) error("HTTP ${c.responseCode}")
            val total = c.contentLengthLong
            val out = java.io.ByteArrayOutputStream(total.coerceAtLeast(0).toInt())
            c.inputStream.use { input ->
                val buf = ByteArray(64 * 1024)
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    out.write(buf, 0, n)
                    if (total > 0) withContext(Dispatchers.Main) { onProgress(out.size().toFloat() / total) }
                }
            }
            out.toByteArray()
        } finally {
            c.disconnect()
        }
    }

    private const val ACTION = "dev.freethinkel.commutator.android.UPDATE"
    private var listening = false

    /** The installer's answers: the "Update this app?" dialog to show, or why it failed. */
    private fun listen(context: Context) {
        if (listening) return
        listening = true
        ContextCompat.registerReceiver(context.applicationContext, object : BroadcastReceiver() {
            override fun onReceive(c: Context, i: Intent) {
                when (i.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
                    PackageInstaller.STATUS_PENDING_USER_ACTION ->
                        @Suppress("DEPRECATION")
                        (i.getParcelableExtra(Intent.EXTRA_INTENT) as? Intent)
                            ?.let { c.startActivity(it.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
                    PackageInstaller.STATUS_SUCCESS, PackageInstaller.STATUS_FAILURE_ABORTED -> {}
                    else -> error = i.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE) ?: "Install failed"
                }
            }
        }, IntentFilter(ACTION), ContextCompat.RECEIVER_NOT_EXPORTED)
    }
}
