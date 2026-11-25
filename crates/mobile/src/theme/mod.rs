//! Desktop themes (`crates/theme`) resolved for the phone: the one mapping from a theme's roles
//! to everything the apps paint with. Colours are 0xAARRGGBB.

mod commutator;

use std::sync::{Arc, RwLock};

use commutator_theme::{Appearance, Color, ThemeVariant, builtin_registry};

use crate::layout::display::ColorRole;

#[cfg(test)]
pub(crate) const COLOR_ROLE_COUNT: usize = ColorRole::ALL.len();

/// Everything the apps paint with, for one theme variant.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PhonePalette {
    pub dark: bool,
    pub app: AppColors,
    /// One colour per transcript `ColorRole`, at the role's index in `ColorRole::ALL`.
    pub roles: Vec<u32>,
    pub terminal: TerminalColors,
    /// Android's Material `ColorScheme` slots.
    pub material: MaterialColors,
}

/// The app chrome's roles (iOS `Palette`).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AppColors {
    pub background: u32,
    /// The open session's row in the iPad sidebar.
    pub row_active: u32,
    pub elevated: u32,
    pub text: u32,
    pub secondary: u32,
    pub tertiary: u32,
    pub hairline: u32,
    pub accent: u32,
    /// A switch's on track.
    pub toggle_on: u32,
    pub on_accent: u32,
    pub control_fill: u32,
    pub accent_soft: u32,
    pub danger: u32,
    pub success: u32,
    pub warning: u32,
    pub user_bubble: u32,
    pub code_background: u32,
    pub code_border: u32,
    pub chip: u32,
    /// Sidebar sublines (project label, branch).
    pub subline: u32,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TerminalColors {
    pub background: u32,
    pub foreground: u32,
    pub selection: u32,
    /// ANSI 0–15.
    pub ansi: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MaterialColors {
    pub primary: u32,
    pub on_primary: u32,
    pub primary_container: u32,
    pub on_primary_container: u32,
    pub inverse_primary: u32,
    pub secondary: u32,
    pub on_secondary: u32,
    pub secondary_container: u32,
    pub on_secondary_container: u32,
    pub tertiary: u32,
    pub on_tertiary: u32,
    pub tertiary_container: u32,
    pub on_tertiary_container: u32,
    pub background: u32,
    pub on_background: u32,
    pub surface: u32,
    pub on_surface: u32,
    pub surface_variant: u32,
    pub on_surface_variant: u32,
    pub surface_tint: u32,
    pub surface_bright: u32,
    pub surface_dim: u32,
    pub surface_container_lowest: u32,
    pub surface_container_low: u32,
    pub surface_container: u32,
    pub surface_container_high: u32,
    pub surface_container_highest: u32,
    pub outline: u32,
    pub outline_variant: u32,
    pub error: u32,
    pub on_error: u32,
    pub error_container: u32,
    pub on_error_container: u32,
    pub inverse_surface: u32,
    pub inverse_on_surface: u32,
    pub scrim: u32,
}

/// The variant `id` names if it exists and has the requested appearance; otherwise Commutator's.
pub(crate) fn variant_or_default(id: &str, dark: bool) -> &'static ThemeVariant {
    let wanted = if dark {
        Appearance::Dark
    } else {
        Appearance::Light
    };
    let fallback = if dark {
        "commutator-dark"
    } else {
        "commutator-light"
    };
    let variants = || {
        builtin_registry()
            .families
            .iter()
            .flat_map(|f| f.variants.iter())
    };
    variants()
        .find(|v| v.id == id && v.appearance == wanted)
        .or_else(|| variants().find(|v| v.id == fallback))
        .expect("the Commutator themes are built in")
}

/// The variants the core paints its own visuals and terminal cells with, as the app last set
/// them (`set_theme_selection`). Global in the app; per thread under test so tests that switch
/// themes don't leak into others running alongside.
struct Current {
    light: Arc<PhonePalette>,
    dark: Arc<PhonePalette>,
}

impl Current {
    fn new(light_id: &str, dark_id: &str) -> Self {
        Self {
            light: Arc::new(phone_palette(variant_or_default(light_id, false))),
            dark: Arc::new(phone_palette(variant_or_default(dark_id, true))),
        }
    }
}

#[cfg(not(test))]
static CURRENT: std::sync::LazyLock<RwLock<Current>> =
    std::sync::LazyLock::new(|| RwLock::new(Current::new("commutator-light", "commutator-dark")));

#[cfg(test)]
thread_local! {
    static CURRENT: RwLock<Current> = RwLock::new(Current::new("commutator-light", "commutator-dark"));
}

fn with_current<T>(f: impl FnOnce(&RwLock<Current>) -> T) -> T {
    #[cfg(not(test))]
    return f(&CURRENT);
    #[cfg(test)]
    return CURRENT.with(f);
}

pub(crate) fn set_selection(light_id: &str, dark_id: &str) {
    let next = Current::new(light_id, dark_id);
    with_current(|current| *current.write().unwrap_or_else(|e| e.into_inner()) = next);
}

/// The selected variant's palette for `dark`.
pub(crate) fn current_palette(dark: bool) -> Arc<PhonePalette> {
    with_current(|current| {
        let current = current.read().unwrap_or_else(|e| e.into_inner());
        if dark { current.dark.clone() } else { current.light.clone() }
    })
}

pub(crate) fn phone_palette(variant: &ThemeVariant) -> PhonePalette {
    match variant.id.as_str() {
        "commutator-light" => commutator::light(),
        "commutator-dark" => commutator::dark(),
        _ => derive(variant),
    }
}

fn argb(c: Color) -> u32 {
    (c.a as u32) << 24 | (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32
}

/// Any other theme: its roles mapped directly, the phone-only ones derived from them with the
/// same alphas Commutator uses over its own text and background.
fn derive(v: &ThemeVariant) -> PhonePalette {
    let dark = v.appearance == Appearance::Dark;
    let c = &v.colors;
    let (bg, text) = (c.background, c.text);
    let accent = v.accent.primary.ensure_contrast(bg, 3.0);
    let tint = |amount: f32| bg.mix(text, amount);
    let pick = |light: f32, dark_: f32| if dark { dark_ } else { light };

    let app = AppColors {
        background: argb(bg),
        row_active: argb(if dark {
            text.with_alpha(0.06)
        } else {
            c.raised.with_alpha(0.9)
        }),
        elevated: argb(c.raised),
        text: argb(text),
        secondary: argb(c.text_muted),
        tertiary: argb(c.text_faint),
        hairline: argb(c.border),
        // The theme's accent, not `solid`: most themes' solid is near-black (or white) ink.
        accent: argb(accent),
        toggle_on: argb(accent),
        // White reads better than black on mid tones even at a lower ratio (iOS buttons do this).
        on_accent: argb(if Color::WHITE.contrast(accent) >= 3.0 { Color::WHITE } else { accent.best_on_color() }),
        control_fill: argb(text.with_alpha(0.075)),
        accent_soft: argb(accent.with_alpha(0.12)),
        danger: argb(c.danger),
        success: argb(c.success),
        warning: argb(c.warning),
        user_bubble: argb(tint(0.07)),
        code_background: argb(tint(0.03)),
        code_border: argb(tint(0.12)),
        chip: argb(tint(0.08)),
        subline: argb(c.text_muted.with_alpha(0.5)),
    };
    let syntax = |key: &str| argb(v.syntax.get(key).copied().unwrap_or(text));
    let roles = ColorRole::ALL
        .iter()
        .map(|role| {
            use ColorRole::*;
            match role {
                Text | CodeText => app.text,
                // The desktop's inline code (`crates/ui` theme): accent text on an accent wash.
                InlineCodeText => argb(accent),
                InlineCodeBackground => argb(accent.with_alpha(pick(0.10, 0.12))),
                TextSecondary => app.secondary,
                TextTertiary => app.tertiary,
                // Whole blocks (tool output, diff context) paint in it: keep them readable.
                TextFaint => argb(c.text_faint.ensure_contrast(bg, 3.5)),
                Link | Accent => argb(accent),
                Danger => app.danger,
                Success => app.success,
                Warning => app.warning,
                ChipBackground => app.chip,
                CodeBackground => app.code_background,
                CodeBorder => app.code_border,
                QuoteBar => argb(accent.with_alpha(0.45)),
                Rule => app.hairline,
                TableBorder => argb(c.border),
                TableHeaderBackground => argb(tint(0.04)),
                UserBubble => app.user_bubble,
                SyntaxKeyword => syntax("keyword"),
                SyntaxString => syntax("string"),
                SyntaxComment => syntax("comment"),
                SyntaxNumber => syntax("number"),
                SyntaxFunction => syntax("function"),
                SyntaxType => syntax("type"),
                SyntaxConstant => syntax("constant"),
                SyntaxVariable => syntax("variable"),
                SyntaxProperty => syntax("property"),
                SyntaxOperator => syntax("operator"),
                SyntaxPunctuation => syntax("punctuation"),
                SyntaxTag => syntax("tag"),
                SyntaxAttribute => syntax("attribute"),
                SyntaxEscape => syntax("escape"),
                TextSoft => argb(text.with_alpha(0.85)),
                ToolRail => argb(text.with_alpha(pick(0.162, 0.12))),
                ToolBadge => argb(text.with_alpha(0.06)),
                ToolWell => argb(bg.with_alpha(0.16)),
                AgentCard => argb(text.with_alpha(0.03)),
                AgentCardBorder => argb(text.with_alpha(pick(0.0945, 0.07))),
                AgentTile => argb(text.with_alpha(0.08)),
                DiffAddWash => argb(c.success.with_alpha(0.055)),
                DiffDelWash => argb(c.danger.with_alpha(0.055)),
                DiffAddBar => argb(c.success.with_alpha(0.55)),
                DiffDelBar => argb(c.danger.with_alpha(0.55)),
                DiffHunk => argb(accent.with_alpha(pick(0.07, 0.08))),
                DangerWash => argb(c.danger.with_alpha(pick(0.08, 0.12))),
                DangerSoft => argb(c.danger.mix(text, 0.3)),
            }
        })
        .collect();
    let terminal = TerminalColors {
        background: argb(v.terminal.background),
        foreground: argb(v.terminal.foreground),
        selection: argb(v.terminal.selection),
        ansi: v.terminal.ansi.iter().map(|&c| argb(c)).collect(),
    };
    let material = MaterialColors {
        primary: argb(c.solid),
        on_primary: argb(c.on_solid),
        primary_container: argb(bg.mix(c.solid, 0.18)),
        on_primary_container: argb(text),
        inverse_primary: argb(c.on_solid),
        secondary: argb(c.text_muted),
        on_secondary: argb(bg),
        secondary_container: argb(tint(0.1)),
        on_secondary_container: argb(text),
        tertiary: argb(accent),
        on_tertiary: argb(accent.best_on_color()),
        tertiary_container: argb(bg.mix(accent, 0.2)),
        on_tertiary_container: argb(text),
        background: argb(bg),
        on_background: argb(text),
        surface: argb(bg),
        on_surface: argb(text),
        surface_variant: app.chip,
        on_surface_variant: argb(c.text_muted),
        surface_tint: argb(c.solid),
        surface_bright: argb(if dark { tint(0.12) } else { c.raised }),
        surface_dim: argb(if dark { bg } else { tint(0.08) }),
        surface_container_lowest: argb(if dark {
            bg.mix(Color::BLACK, 0.5)
        } else {
            c.raised
        }),
        surface_container_low: argb(tint(0.02)),
        surface_container: argb(tint(0.04)),
        surface_container_high: app.user_bubble,
        surface_container_highest: argb(tint(0.1)),
        outline: argb(c.text_faint),
        outline_variant: argb(c.border),
        error: argb(c.danger),
        on_error: argb(c.danger.best_on_color()),
        error_container: argb(bg.mix(c.danger, 0.18)),
        on_error_container: argb(text),
        inverse_surface: argb(text),
        inverse_on_surface: argb(bg),
        scrim: 0xFF00_0000,
    };
    PhonePalette {
        dark,
        app,
        roles,
        terminal,
        material,
    }
}

#[cfg(test)]
mod tests;
