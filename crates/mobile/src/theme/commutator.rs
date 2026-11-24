//! Commutator Light / Dark as the phone has always painted them: the one copy of these values
//! (they used to live in `Palette.swift`, `Theme.kt` and `terminal/color.rs`). Kept exact rather
//! than derived so the default look doesn't move.

use super::{AppColors, MaterialColors, PhonePalette, TerminalColors};
use crate::layout::display::ColorRole;

/// Opaque 0xRRGGBB → 0xAARRGGBB.
const fn o(rgb: u32) -> u32 {
    0xFF00_0000 | rgb
}

/// 0xRRGGBB at `alpha` → 0xAARRGGBB (alpha rounded to 8 bits).
fn a(rgb: u32, alpha: f32) -> u32 {
    ((alpha * 255.0).round() as u32) << 24 | rgb
}

const WHITE: u32 = 0xFFFFFF;
const BLACK: u32 = 0x000000;

/// One colour per light/dark pair, picked for `dark`.
struct Pick(bool);

impl Pick {
    fn o(&self, light: u32, dark: u32) -> u32 {
        o(if self.0 { dark } else { light })
    }
    fn a(&self, light: u32, dark: u32, alpha: f32) -> u32 {
        a(if self.0 { dark } else { light }, alpha)
    }
    /// Different alphas per appearance.
    fn a2(&self, light: (u32, f32), dark: (u32, f32)) -> u32 {
        let (rgb, alpha) = if self.0 { dark } else { light };
        a(rgb, alpha)
    }
}

pub(super) fn light() -> PhonePalette {
    palette(false)
}

pub(super) fn dark() -> PhonePalette {
    palette(true)
}

fn palette(dark: bool) -> PhonePalette {
    let p = Pick(dark);
    let app = AppColors {
        background: p.o(0xF3F3F5, 0x060606),
        row_active: p.a2((WHITE, 0.9), (WHITE, 0.06)),
        elevated: p.o(0xFFFFFF, 0x111113),
        text: p.o(0x27272C, 0xE8E8EA),
        secondary: p.o(0x62626A, 0xA9A9AE),
        tertiary: p.o(0x97979F, 0x6B6B72),
        hairline: p.o(0xE2E2E6, 0x1E1E22),
        accent: p.o(0x18181B, 0xECECEC),
        toggle_on: p.o(0x18181B, 0x8E8E93),
        on_accent: p.o(0xFFFFFF, 0x18181B),
        control_fill: p.a(0x27272C, 0xE8E8EA, 0.075),
        accent_soft: p.a(0x18181B, 0xECECEC, 0.12),
        danger: p.o(0xDC2626, 0xF87171),
        success: p.o(0x15803D, 0x34D399),
        warning: p.o(0xA16207, 0xFACC15),
        user_bubble: p.o(0xFFFFFF, 0x19191C),
        code_background: p.o(0xFAFAFB, 0x0B0B0D),
        code_border: p.o(0xE4E4E8, 0x1F1F23),
        chip: p.o(0xE7E7EB, 0x1C1C20),
        subline: p.a(0x62626A, 0xA9A9AE, 0.5),
    };
    let roles = ColorRole::ALL
        .iter()
        .map(|role| role_color(*role, &app, &p))
        .collect();
    PhonePalette {
        dark,
        app,
        roles,
        terminal: terminal(&p),
        material: material(&p),
    }
}

fn role_color(role: ColorRole, app: &AppColors, p: &Pick) -> u32 {
    use ColorRole::*;
    match role {
        Text => app.text,
        TextSecondary => app.secondary,
        TextTertiary => app.tertiary,
        Link | Accent => app.accent,
        Danger => app.danger,
        Success => app.success,
        Warning => app.warning,
        InlineCodeText => p.o(0x3F3F46, 0xDCDCE0),
        InlineCodeBackground => p.o(0xE9E9ED, 0x1A1A1E),
        CodeText => p.o(0x303035, 0xE8E8EA),
        CodeBackground => app.code_background,
        CodeBorder => app.code_border,
        QuoteBar => p.a(0x18181B, 0xECECEC, 0.45),
        Rule => app.hairline,
        TableBorder => p.o(0xE2E2E6, 0x232327),
        TableHeaderBackground => p.o(0xF3F3F5, 0x121215),
        UserBubble => app.user_bubble,
        ChipBackground => app.chip,
        SyntaxKeyword => p.o(0x5B43E8, 0x8B7CF6),
        SyntaxString => p.o(0x15803D, 0x34D399),
        SyntaxComment => p.o(0x6B7280, 0x92929A),
        SyntaxNumber | SyntaxConstant => p.o(0xA16207, 0xFACC15),
        SyntaxFunction => p.o(0x2563EB, 0x60A5FA),
        SyntaxType => p.o(0x7E22CE, 0xC084FC),
        SyntaxVariable => p.o(0x303035, 0xE8E8EA),
        SyntaxProperty | SyntaxEscape => p.o(0x0E7490, 0x22D3EE),
        SyntaxOperator | SyntaxPunctuation => p.o(0x52525B, 0xA1A1AA),
        SyntaxTag => p.o(0xBE185D, 0xF472B6),
        SyntaxAttribute => p.o(0xB91C1C, 0xF87171),
        TextFaint => p.o(0x797981, 0x85858A),
        TextSoft => p.a(0x303035, 0xE8E8EA, 0.85),
        ToolRail => p.a2((BLACK, 0.162), (WHITE, 0.12)),
        ToolBadge => p.a2((BLACK, 0.06), (WHITE, 0.06)),
        ToolWell => p.a2((WHITE, 0.16), (BLACK, 0.16)),
        AgentCard => p.a2((BLACK, 0.03), (WHITE, 0.03)),
        AgentCardBorder => p.a2((BLACK, 0.0945), (WHITE, 0.07)),
        AgentTile => p.a2((BLACK, 0.08), (WHITE, 0.08)),
        DiffAddWash => p.a(0x15803D, 0x34D399, 0.055),
        DiffDelWash => p.a(0xDC2626, 0xF87171, 0.055),
        DiffAddBar => p.a(0x15803D, 0x34D399, 0.55),
        DiffDelBar => p.a(0xDC2626, 0xF87171, 0.55),
        DiffHunk => p.a2((0x18181B, 0.07), (0xECECEC, 0.08)),
        DangerWash => p.a2((0xDC2626, 0.08), (0xF87171, 0.12)),
        DangerSoft => p.o(0xB91C1C, 0xFCA5A5),
    }
}

fn terminal(p: &Pick) -> TerminalColors {
    const ANSI_DARK: [u32; 16] = [
        0x242424, 0xf87171, 0x4ade80, 0xfacc15, 0x60a5fa, 0xc084fc, 0x22d3ee, 0xd4d4d8, 0x52525b,
        0xfca5a5, 0x86efac, 0xfde047, 0x93c5fd, 0xd8b4fe, 0x67e8f9, 0xfafafa,
    ];
    const ANSI_LIGHT: [u32; 16] = [
        0x1f1f1f, 0xdc2626, 0x16a34a, 0xb45309, 0x2563eb, 0x9333ea, 0x0e7490, 0x3f3f46, 0x71717a,
        0xb91c1c, 0x15803d, 0x92400e, 0x1d4ed8, 0x7e22ce, 0x155e75, 0x18181b,
    ];
    let ansi = if p.0 { ANSI_DARK } else { ANSI_LIGHT };
    TerminalColors {
        background: p.o(0xFAFAFB, 0x090909),
        foreground: p.o(0x27272C, 0xE8E8EA),
        // The phone never painted a selection of its own: the accent, softly.
        selection: p.a(0x18181B, 0xECECEC, 0.25),
        ansi: ansi.iter().map(|&rgb| o(rgb)).collect(),
    }
}

fn material(p: &Pick) -> MaterialColors {
    MaterialColors {
        primary: p.o(0x18181B, 0xECECEC),
        on_primary: p.o(0xFFFFFF, 0x18181B),
        primary_container: p.o(0xE4E4E7, 0x2C2C30),
        on_primary_container: p.o(0x18181B, 0xECECEC),
        inverse_primary: p.o(0xECECEC, 0x18181B),
        secondary: p.o(0x5F5F66, 0xC6C6CC),
        on_secondary: p.o(0xFFFFFF, 0x2E2E33),
        secondary_container: p.o(0xE4E4E7, 0x26262A),
        on_secondary_container: p.o(0x1C1C20, 0xE4E4E7),
        tertiary: p.o(0x0F766E, 0x5EEAD4),
        on_tertiary: p.o(0xFFFFFF, 0x003731),
        tertiary_container: p.o(0xCCF2EC, 0x0C3A35),
        on_tertiary_container: p.o(0x00201D, 0xCCF2EC),
        background: p.o(0xF3F3F5, 0x060606),
        on_background: p.o(0x27272C, 0xE8E8EA),
        surface: p.o(0xF3F3F5, 0x060606),
        on_surface: p.o(0x27272C, 0xE8E8EA),
        surface_variant: p.o(0xE7E7EB, 0x1C1C20),
        on_surface_variant: p.o(0x62626A, 0xA9A9AE),
        surface_tint: p.o(0x18181B, 0xECECEC),
        surface_bright: p.o(0xFFFFFF, 0x26262A),
        surface_dim: p.o(0xE2E2E6, 0x060606),
        surface_container_lowest: p.o(0xFFFFFF, 0x000000),
        surface_container_low: p.o(0xFAFAFB, 0x0E0E10),
        surface_container: p.o(0xEEEEF1, 0x131316),
        surface_container_high: p.o(0xE8E8EC, 0x1A1A1E),
        surface_container_highest: p.o(0xE2E2E7, 0x222227),
        outline: p.o(0x97979F, 0x6B6B72),
        outline_variant: p.o(0xE2E2E6, 0x1E1E22),
        error: p.o(0xDC2626, 0xF87171),
        on_error: p.o(0xFFFFFF, 0x450A0A),
        error_container: p.o(0xFDE2E2, 0x3B1212),
        on_error_container: p.o(0x5F1111, 0xFDE2E2),
        inverse_surface: p.o(0x27272C, 0xE8E8EA),
        inverse_on_surface: p.o(0xF3F3F5, 0x111113),
        scrim: o(0x000000),
    }
}
