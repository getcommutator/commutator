package dev.freethinkel.commutator.android.design

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialExpressiveTheme
import androidx.compose.material3.MotionScheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import uniffi.commutator_core.MaterialColors
import uniffi.commutator_core.PhonePalette
import uniffi.commutator_core.themePalette

/** A theme's Material slots (resolved in the core from the desktop's themes). */
private fun MaterialColors.toScheme(dark: Boolean): ColorScheme {
    fun c(argb: UInt) = Color(argb.toInt())
    return (if (dark) darkColorScheme() else lightColorScheme()).copy(
        primary = c(primary),
        onPrimary = c(onPrimary),
        primaryContainer = c(primaryContainer),
        onPrimaryContainer = c(onPrimaryContainer),
        inversePrimary = c(inversePrimary),
        secondary = c(secondary),
        onSecondary = c(onSecondary),
        secondaryContainer = c(secondaryContainer),
        onSecondaryContainer = c(onSecondaryContainer),
        tertiary = c(tertiary),
        onTertiary = c(onTertiary),
        tertiaryContainer = c(tertiaryContainer),
        onTertiaryContainer = c(onTertiaryContainer),
        background = c(background),
        onBackground = c(onBackground),
        surface = c(surface),
        onSurface = c(onSurface),
        surfaceVariant = c(surfaceVariant),
        onSurfaceVariant = c(onSurfaceVariant),
        surfaceTint = c(surfaceTint),
        surfaceBright = c(surfaceBright),
        surfaceDim = c(surfaceDim),
        surfaceContainerLowest = c(surfaceContainerLowest),
        surfaceContainerLow = c(surfaceContainerLow),
        surfaceContainer = c(surfaceContainer),
        surfaceContainerHigh = c(surfaceContainerHigh),
        surfaceContainerHighest = c(surfaceContainerHighest),
        outline = c(outline),
        outlineVariant = c(outlineVariant),
        error = c(error),
        onError = c(onError),
        errorContainer = c(errorContainer),
        onErrorContainer = c(onErrorContainer),
        inverseSurface = c(inverseSurface),
        inverseOnSurface = c(inverseOnSurface),
        scrim = c(scrim),
    )
}

/** "Wallpaper colors" is on: Material slots (and a few transcript roles) come from the system. */
val LocalDynamicColor = staticCompositionLocalOf { false }

/** The selected theme's palette for the current appearance (transcript roles, terminal). */
val LocalPhonePalette = staticCompositionLocalOf<PhonePalette> { error("CommutatorTheme provides it") }

/** Geist, from the same asset bytes the transcript measures. */
val Geist: FontFamily by lazy {
    FontFamily(
    Font("Geist.ttf", LocalAssets.manager, FontWeight.Normal),
    Font("Geist-Medium.ttf", LocalAssets.manager, FontWeight.Medium),
    Font("Geist-SemiBold.ttf", LocalAssets.manager, FontWeight.SemiBold),
    Font("Geist-Bold.ttf", LocalAssets.manager, FontWeight.Bold),
    Font("Geist-Italic.ttf", LocalAssets.manager, FontWeight.Normal, FontStyle.Italic),
    )
}

val IoskeleyMono: FontFamily by lazy {
    FontFamily(
    Font("IoskeleyMono-Regular.ttf", LocalAssets.manager, FontWeight.Normal),
    Font("IoskeleyMono-Medium.ttf", LocalAssets.manager, FontWeight.Medium),
    Font("IoskeleyMono-SemiBold.ttf", LocalAssets.manager, FontWeight.SemiBold),
    )
}

/** App-wide AssetManager for font families declared at top level. */
object LocalAssets {
    lateinit var manager: android.content.res.AssetManager
}

private fun TextStyle.geist() = copy(fontFamily = Geist)

private fun commutatorTypography(): Typography {
    val base = Typography()
    fun TextStyle.w(weight: FontWeight, tracking: Float? = null) =
        geist().copy(fontWeight = weight, letterSpacing = tracking?.sp ?: letterSpacing)
    return Typography(
        displayLarge = base.displayLarge.w(FontWeight.SemiBold, -1.5f),
        displayMedium = base.displayMedium.w(FontWeight.SemiBold, -1.2f),
        displaySmall = base.displaySmall.w(FontWeight.SemiBold, -1f),
        headlineLarge = base.headlineLarge.w(FontWeight.SemiBold, -0.6f),
        headlineMedium = base.headlineMedium.w(FontWeight.SemiBold, -0.4f),
        headlineSmall = base.headlineSmall.w(FontWeight.SemiBold, -0.3f),
        titleLarge = base.titleLarge.w(FontWeight.SemiBold, -0.2f),
        titleMedium = base.titleMedium.w(FontWeight.Medium),
        titleSmall = base.titleSmall.w(FontWeight.Medium),
        bodyLarge = base.bodyLarge.geist().copy(fontSize = 16.5.sp, lineHeight = 24.sp, letterSpacing = 0.sp),
        bodyMedium = base.bodyMedium.geist().copy(letterSpacing = 0.sp),
        bodySmall = base.bodySmall.geist().copy(letterSpacing = 0.sp),
        labelLarge = base.labelLarge.w(FontWeight.Medium, 0f),
        labelMedium = base.labelMedium.w(FontWeight.Medium, 0f),
        labelSmall = base.labelSmall.w(FontWeight.Medium, 0f),
        displayLargeEmphasized = base.displayLarge.w(FontWeight.Bold, -1.5f),
        displayMediumEmphasized = base.displayMedium.w(FontWeight.Bold, -1.2f),
        displaySmallEmphasized = base.displaySmall.w(FontWeight.Bold, -1f),
        headlineLargeEmphasized = base.headlineLarge.w(FontWeight.Bold, -0.6f),
        headlineMediumEmphasized = base.headlineMedium.w(FontWeight.Bold, -0.4f),
        headlineSmallEmphasized = base.headlineSmall.w(FontWeight.Bold, -0.3f),
        titleLargeEmphasized = base.titleLarge.w(FontWeight.Bold, -0.2f),
        titleMediumEmphasized = base.titleMedium.w(FontWeight.SemiBold),
        titleSmallEmphasized = base.titleSmall.w(FontWeight.SemiBold),
        bodyLargeEmphasized = base.bodyLarge.w(FontWeight.Medium, 0f).copy(fontSize = 16.5.sp, lineHeight = 24.sp),
        bodyMediumEmphasized = base.bodyMedium.w(FontWeight.Medium, 0f),
        bodySmallEmphasized = base.bodySmall.w(FontWeight.Medium, 0f),
        labelLargeEmphasized = base.labelLarge.w(FontWeight.SemiBold, 0f),
        labelMediumEmphasized = base.labelMedium.w(FontWeight.SemiBold, 0f),
        labelSmallEmphasized = base.labelSmall.w(FontWeight.SemiBold, 0f),
    )
}

enum class ThemeMode { System, Light, Dark }

data class Appearance(
    val mode: ThemeMode = ThemeMode.System,
    val dynamicColor: Boolean = false,
    val lightTheme: String = "commutator-light",
    val darkTheme: String = "commutator-dark",
)

/** Whether the resolved theme is dark (the transcript palette follows it). */
val LocalDarkTheme = staticCompositionLocalOf { false }

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun CommutatorTheme(appearance: Appearance = Appearance(), content: @Composable () -> Unit) {
    val dark = when (appearance.mode) {
        ThemeMode.System -> isSystemInDarkTheme()
        ThemeMode.Light -> false
        ThemeMode.Dark -> true
    }
    val context = LocalContext.current
    val themeId = if (dark) appearance.darkTheme else appearance.lightTheme
    val palette = remember(themeId, dark) { themePalette(themeId, dark) }
    // "Wallpaper colors" replaces the theme's Material slots with the system's dynamic scheme.
    val scheme: ColorScheme = when {
        appearance.dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        else -> remember(palette) { palette.material.toScheme(dark) }
    }
    val typography = remember { commutatorTypography() }
    val dynamic = appearance.dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
    CompositionLocalProvider(LocalDarkTheme provides dark, LocalPhonePalette provides palette, LocalDynamicColor provides dynamic) {
        MaterialExpressiveTheme(
            colorScheme = scheme,
            motionScheme = MotionScheme.expressive(),
            typography = typography,
            content = content,
        )
    }
}
