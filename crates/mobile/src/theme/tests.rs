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

#[test]
fn families_split_by_appearance() {
    let families = crate::client_ffi::themes::theme_families();
    let ids: Vec<&str> = families.iter().map(|f| f.id.as_str()).collect();
    let registry: Vec<&str> = builtin_registry().families.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, registry);
    let dracula = families.iter().find(|f| f.id == "dracula").unwrap();
    assert!(dracula.variants.iter().all(|v| v.dark));
    let commutator = families.iter().find(|f| f.id == "commutator").unwrap();
    assert_eq!(commutator.variants.iter().filter(|v| v.dark).count(), 1);
    assert_eq!(commutator.variants.iter().filter(|v| !v.dark).count(), 1);
    assert!(families.iter().flat_map(|f| &f.variants).all(|v| v.swatches.len() == 4));
}

#[test]
fn selection_drives_core_palette() {
    // The selection is per thread under test, so this doesn't leak into other tests.
    crate::client_ffi::themes::set_theme_selection("github-light".into(), "dracula".into());
    let dracula = phone_palette(variant_or_default("dracula", true));
    assert_eq!(current_palette(true).app.background, dracula.app.background);
    let github = phone_palette(variant_or_default("github-light", false));
    assert_eq!(current_palette(false).app.background, github.app.background);
    assert_eq!(crate::visual::palette::Palette::new(true).text, crate::visual::palette::rgba(dracula.app.text));
}

#[test]
fn terminal_ansi_follows_selection() {
    crate::client_ffi::themes::set_theme_selection("commutator-light".into(), "dracula".into());
    let dracula = phone_palette(variant_or_default("dracula", true));
    let red = crate::terminal::color::rgb(commutator_term::emulator::CellColor::Indexed(1), true);
    assert_eq!(red, Some(dracula.terminal.ansi[1] & 0xFF_FFFF));
}

#[test]
fn ffi_palette_falls_back() {
    assert_eq!(
        crate::client_ffi::themes::theme_palette("gone".into(), true),
        phone_palette(variant_or_default("commutator-dark", true))
    );
}

#[test]
fn ffi_color_roles_are_the_role_table_order() {
    assert_eq!(crate::client_ffi::themes::color_roles(), ColorRole::ALL.to_vec());
}

#[test]
fn faint_text_stays_readable() {
    // TextFaint paints whole blocks (tool output, diff context): at least 3.5:1 on the background
    // (Commutator Light has 3.9).
    for v in all_variants() {
        let p = phone_palette(v);
        let ratio = color(p.roles[ColorRole::TextFaint as usize]).contrast(color(p.app.background));
        assert!(ratio >= 3.5, "{}: faint text is {ratio:.2}", v.id);
    }
}
