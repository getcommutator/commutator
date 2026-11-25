package dev.freethinkel.commutator.android

import android.content.Intent
import android.content.pm.ApplicationInfo
import android.os.Build
import android.os.Bundle
import android.view.MotionEvent
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import dev.freethinkel.commutator.android.core.LaunchOptions
import dev.freethinkel.commutator.android.ui.CommutatorRoot

/**
 * Launch extras mirror the iOS launch arguments, e.g.
 * `adb shell am start -n dev.freethinkel.commutator.android/.MainActivity --ez demo true --es route chat:chat-veil`
 * (`--es wallpaper <path>` sets the wallpaper from a file the app can read).
 * A local stack in a debuggable build (crates/client/examples/remote_stack.rs):
 * `--es account "http://10.0.2.2:<port> <token> <device id>" --es host <machine id>`.
 */
class MainActivity : ComponentActivity() {
    private val model get() = (application as CommutatorApplication).model

    override fun onCreate(savedInstanceState: Bundle?) {
        installSplashScreen()
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        val extras = intent.extras
        // Rig switches: this activity is exported, so a release build ignores them.
        val debuggable = applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0
        val rig = extras.takeIf { debuggable }
        model.boot(
            LaunchOptions(
                demo = extras?.getBoolean("demo") == true,
                fast = extras?.getBoolean("fast") == true,
                longReply = extras?.getBoolean("longreply") == true,
                big = extras?.getBoolean("big") == true,
                huge = extras?.getBoolean("huge") == true,
                noProjects = extras?.getBoolean("noprojects") == true,
                signedOut = extras?.getBoolean("signedout") == true,
                route = extras?.getString("route"),
                companion = extras?.getString("companion"),
                terminalDemo = extras?.getBoolean("terminal-demo") == true,
                wallpaper = extras?.getString("wallpaper"),
                wallpaperEffect = extras?.getString("wallpaper-effect"),
                account = rig?.getString("account"),
                host = rig?.getString("host"),
                sendOnOpen = rig?.getString("send-on-open"),
            ),
        )
        handleCallback(intent)
        handleNotification(intent)
        setContent { CommutatorRoot(model) }
    }

    // Some OEMs (Nothing OS) hold third-party windows below the panel's peak rate unless they ask for
    // a display mode. Ask while the user is touching the app and for a while after (flings, the next
    // gesture: each switch costs one long frame), then hand the choice back so an idle screen doesn't
    // stay pinned at the peak rate.
    private val fastestModeId by lazy { fastestDisplayModeId() }
    private val releaseFastestMode = Runnable { preferDisplayMode(0) }

    override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
        when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                window.decorView.removeCallbacks(releaseFastestMode)
                preferDisplayMode(fastestModeId)
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL ->
                window.decorView.postDelayed(releaseFastestMode, 10_000)
        }
        return super.dispatchTouchEvent(ev)
    }

    private fun preferDisplayMode(id: Int) {
        if (window.attributes.preferredDisplayModeId != id) {
            window.attributes = window.attributes.apply { preferredDisplayModeId = id }
        }
    }

    private fun fastestDisplayModeId(): Int {
        @Suppress("DEPRECATION")
        val display = (if (Build.VERSION.SDK_INT >= 30) display else windowManager.defaultDisplay) ?: return 0
        val current = display.mode
        return display.supportedModes
            .filter { it.physicalWidth == current.physicalWidth && it.physicalHeight == current.physicalHeight }
            .maxByOrNull { it.refreshRate }?.modeId ?: 0
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleCallback(intent)
        handleNotification(intent)
    }

    /** A tapped session notification: the shell opens that chat. */
    private fun handleNotification(intent: Intent?) {
        intent?.getStringExtra(dev.freethinkel.commutator.android.core.Push.EXTRA_CHAT)?.let { model.push.tappedChat.value = it }
    }

    private fun handleCallback(intent: Intent?) {
        val data = intent?.data ?: return
        if (data.scheme == "commutator" && data.host == "auth") model.handleCallback(data.toString())
    }
}
