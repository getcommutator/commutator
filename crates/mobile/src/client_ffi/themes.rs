//! Themes for the apps' Appearance page: the desktop's built-in set (`crates/theme`), each
//! variant resolved to the phone's palette by `crate::theme`.

use commutator_theme::{Appearance, builtin_registry};

use crate::layout::display::ColorRole;
use crate::theme::{self, PhonePalette};

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ThemeFamilyInfo {
    pub id: String,
    pub name: String,
    pub variants: Vec<ThemeVariantInfo>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ThemeVariantInfo {
    pub id: String,
    pub name: String,
    pub dark: bool,
    /// For the picker: background, text, accent (0xAARRGGBB).
    pub swatches: Vec<u32>,
}

/// Every built-in family, in the desktop's order.
#[uniffi::export]
pub fn theme_families() -> Vec<ThemeFamilyInfo> {
    builtin_registry()
        .families
        .iter()
        .map(|family| ThemeFamilyInfo {
            id: family.id.clone(),
            name: family.name.clone(),
            variants: family
                .variants
                .iter()
                .map(|variant| {
                    let palette = theme::phone_palette(variant);
                    ThemeVariantInfo {
                        id: variant.id.clone(),
                        name: variant.name.clone(),
                        dark: variant.appearance == Appearance::Dark,
                        swatches: vec![
                            palette.app.background,
                            palette.app.text,
                            palette.app.accent,
                        ],
                    }
                })
                .collect(),
        })
        .collect()
}

/// The palette for `variant_id`; an unknown id, or one of the other appearance, gives
/// Commutator Light / Dark.
#[uniffi::export]
pub fn theme_palette(variant_id: String, dark: bool) -> PhonePalette {
    theme::phone_palette(theme::variant_or_default(&variant_id, dark))
}

/// Which variants the core's own drawing (visuals, terminal cells) uses from now on.
#[uniffi::export]
pub fn set_theme_selection(light_id: String, dark_id: String) {
    theme::set_selection(&light_id, &dark_id);
}

/// The transcript roles in the order of `PhonePalette::roles` (Swift has no enum ordinal to
/// index it with; Kotlin's `ordinal` already matches).
#[uniffi::export]
pub fn color_roles() -> Vec<ColorRole> {
    ColorRole::ALL.to_vec()
}
