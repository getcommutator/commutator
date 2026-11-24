use super::*;
use crate::layout::display::ColorRole;
use commutator_theme::{Appearance, Color, ThemeVariant, builtin_registry};

fn all_variants() -> impl Iterator<Item = &'static ThemeVariant> {
    builtin_registry()
        .families
        .iter()
        .flat_map(|f| f.variants.iter())
}

fn color(argb: u32) -> Color {
    Color::rgba(
        (argb >> 16) as u8,
        (argb >> 8) as u8,
        argb as u8,
        (argb >> 24) as u8,
    )
}

#[test]
fn commutator_matches_today() {
    let dark = phone_palette(variant_or_default("commutator-dark", true));
    let light = phone_palette(variant_or_default("commutator-light", false));
    assert_eq!(dark, commutator::dark());
    assert_eq!(light, commutator::light());
    // Spot checks against today's literals (Palette.swift, Theme.kt, terminal/color.rs).
    assert_eq!(dark.app.background, 0xFF060606);
    assert_eq!(light.app.background, 0xFFF3F3F5);
    assert_eq!(dark.app.text, 0xFFE8E8EA);
    assert_eq!(light.app.text, 0xFF27272C);
    assert_eq!(dark.terminal.background, 0xFF090909);
    assert_eq!(light.terminal.background, 0xFFFAFAFB);
    assert_eq!(dark.terminal.ansi[1], 0xFFF87171);
    assert_eq!(light.terminal.ansi[1], 0xFFDC2626);
    assert_eq!(dark.roles[ColorRole::SyntaxKeyword as usize], 0xFF8B7CF6);
    assert_eq!(light.material.surface_container_high, 0xFFE8E8EC);
    assert_eq!(dark.material.tertiary, 0xFF5EEAD4);
}

#[test]
fn every_builtin_maps() {
    for v in all_variants() {
        let p = phone_palette(v);
        assert_eq!(p.roles.len(), COLOR_ROLE_COUNT, "{}", v.id);
        assert_eq!(p.terminal.ansi.len(), 16, "{}", v.id);
        assert_eq!(p.dark, v.appearance == Appearance::Dark, "{}", v.id);
    }
}

#[test]
fn unknown_id_falls_back() {
    assert_eq!(
        variant_or_default("no-such-theme", true).id,
        "commutator-dark"
    );
    assert_eq!(
        variant_or_default("no-such-theme", false).id,
        "commutator-light"
    );
    // a variant of the wrong appearance is not used for the other one
    assert_eq!(
        variant_or_default("commutator-dark", false).id,
        "commutator-light"
    );
    assert_eq!(variant_or_default("dracula", true).id, "dracula");
}

#[test]
fn derived_roles_keep_text_readable() {
    for v in all_variants() {
        let p = phone_palette(v);
        let text = color(p.app.text);
        for (name, surface) in [
            ("user_bubble", p.app.user_bubble),
            ("code_background", p.app.code_background),
        ] {
            let ratio = text.contrast(color(surface));
            assert!(ratio >= 4.5, "{}: text on {name} is {ratio:.2}", v.id);
        }
    }
}

#[test]
fn role_table_order_matches_the_enum() {
    for (i, role) in ColorRole::ALL.iter().enumerate() {
        assert_eq!(*role as usize, i, "{role:?}");
    }
}
