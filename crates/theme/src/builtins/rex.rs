//! Rex's built-in themes (`Rex.app`, SuperUI design tokens), generated from its
//! `themes.json`: UI colours from its CSS variables, syntax from its terminal palette.

use super::{Seeds, source, variant};
use crate::{Appearance, SurfaceTreatment, ThemeFamily, ThemeVariant};

pub(super) fn families() -> Vec<ThemeFamily> {
    vec![
        super::family("abyssal-trench", "Abyssal Trench", vec![abyssal_trench()]),
        super::family("alpine-milk", "Alpine Milk", vec![alpine_milk()]),
        super::family("basalt-shore", "Basalt Shore", vec![basalt_shore()]),
        super::family("buttercream-diner", "Buttercream Diner", vec![buttercream_diner()]),
        super::family("cathode-amber", "Cathode Amber", vec![cathode_amber()]),
        super::family("charcoal-atelier", "Charcoal Atelier", vec![charcoal_atelier()]),
        super::family("cinder-peak", "Cinder Peak", vec![cinder_peak()]),
        super::family("coastal-linen", "Coastal Linen", vec![coastal_linen()]),
        super::family("cobalt-foundry", "Cobalt Foundry", vec![cobalt_foundry()]),
        super::family("ember-hollow", "Ember Hollow", vec![ember_hollow()]),
        super::family("glacier-mint", "Glacier Mint", vec![glacier_mint()]),
        super::family("heliotrope-haze", "Heliotrope Haze", vec![heliotrope_haze()]),
        super::family("ink-meridian", "Ink Meridian", vec![ink_meridian()]),
        super::family("ivory-gallery", "Ivory Gallery", vec![ivory_gallery()]),
        super::family("juniper-smoke", "Juniper Smoke", vec![juniper_smoke()]),
        super::family("lavender-postcard", "Lavender Postcard", vec![lavender_postcard()]),
        super::family("merino", "Merino", vec![merino_light(), merino_dark()]),
        super::family("midnight-tundra", "Midnight Tundra", vec![midnight_tundra()]),
        super::family("misty-forest", "Misty Forest", vec![misty_forest()]),
        super::family("moss-cathedral", "Moss Cathedral", vec![moss_cathedral()]),
        super::family("neon-arcade", "Neon Arcade", vec![neon_arcade()]),
        super::family("newsprint-fog", "Newsprint Fog", vec![newsprint_fog()]),
        super::family("nocturne-rose", "Nocturne Rose", vec![nocturne_rose()]),
        super::family("orchard-breeze", "Orchard Breeze", vec![orchard_breeze()]),
        super::family("oxblood-library", "Oxblood Library", vec![oxblood_library()]),
        super::family("peach-veranda", "Peach Veranda", vec![peach_veranda()]),
        super::family("petrol-lagoon", "Petrol Lagoon", vec![petrol_lagoon()]),
        super::family("phosphor-archive", "Phosphor Archive", vec![phosphor_archive()]),
        super::family("polar-aurora", "Polar Aurora", vec![polar_aurora()]),
        super::family("porcelain-morning", "Porcelain Morning", vec![porcelain_morning()]),
        super::family("roasted-umber", "Roasted Umber", vec![roasted_umber()]),
        super::family("saffron-nightfall", "Saffron Nightfall", vec![saffron_nightfall()]),
        super::family("silver-point", "Silver Point", vec![silver_point_light(), silver_point_dark()]),
        super::family("sonoran-dusk", "Sonoran Dusk", vec![sonoran_dusk()]),
        super::family("static-noir", "Static Noir", vec![static_noir()]),
        super::family("sunlit-parchment", "Sunlit Parchment", vec![sunlit_parchment()]),
        super::family("terracotta-noon", "Terracotta Noon", vec![terracotta_noon()]),
        super::family("velvet-dusk", "Velvet Dusk", vec![velvet_dusk()]),
        super::family("whitby-bay", "Whitby Bay", vec![whitby_bay()]),
    ]
}

fn abyssal_trench() -> ThemeVariant {
    variant(Seeds {
        id: "abyssal-trench",
        family_id: "abyssal-trench",
        name: "Abyssal Trench",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#06101a",
        shell: "#0d1720",
        raised: "#16202a",
        card: "#0a141d",
        text: "#b8cdd9",
        muted: "#768894",
        faint: "#5b6b77",
        accent: "#4fe3c1",
        danger: "#e26a6a",
        warning: "#d4b25e",
        success: "#52c79a",
        terminal_background: "#0a141d",
        ansi: [
            "#0e1c29", "#e26a6a", "#52c79a", "#d4b25e", "#4a90c9", "#9179c9", "#3fbdc9", "#9db4c2",
            "#47606f", "#f18787", "#74dcb2", "#e6ca7f", "#6dabdd", "#ab95dc", "#64d5df", "#dcebf3",
        ],
        syntax: [
            "#5b6b77", "#9179c9", "#52c79a", "#d4b25e", "#3fbdc9", "#4a90c9", "#e26a6a", "#b8cdd9",
            "#768894", "#e26a6a", "#d4b25e", "#e26a6a",
        ],
        source: source(
            "abyssal-trench",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn alpine_milk() -> ThemeVariant {
    variant(Seeds {
        id: "alpine-milk",
        family_id: "alpine-milk",
        name: "Alpine Milk",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#fafafa",
        shell: "#f0f0f0",
        raised: "#fbfcfe",
        card: "#fafbfd",
        text: "#333940",
        muted: "#696e74",
        faint: "#878b90",
        accent: "#2d6fb1",
        danger: "#bb453d",
        warning: "#8f6600",
        success: "#367a42",
        terminal_background: "#fafbfd",
        ansi: [
            "#333940", "#c94f46", "#4a9256", "#b28422", "#3d7cc9", "#8a5fb0", "#2e8fa0", "#d7dade",
            "#757d86", "#e0655b", "#58ab66", "#c9982e", "#5292dd", "#a075c6", "#3ba6b8", "#ffffff",
        ],
        syntax: [
            "#878b90", "#8a5fb0", "#4a9256", "#b28422", "#2e8fa0", "#3d7cc9", "#c94f46", "#333940",
            "#696e74", "#c94f46", "#b28422", "#bb453d",
        ],
        source: source(
            "alpine-milk",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn basalt_shore() -> ThemeVariant {
    variant(Seeds {
        id: "basalt-shore",
        family_id: "basalt-shore",
        name: "Basalt Shore",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#14181a",
        shell: "#1b1f21",
        raised: "#25282a",
        card: "#181c1e",
        text: "#c8d3d3",
        muted: "#879090",
        faint: "#6a7273",
        accent: "#7fd8c4",
        danger: "#dd7a72",
        warning: "#d2b071",
        success: "#7cbfa0",
        terminal_background: "#181c1e",
        ansi: [
            "#1f2527", "#dd7a72", "#7cbfa0", "#d2b071", "#729fc0", "#a68fc0", "#66c2b5", "#a9b7b7",
            "#576466", "#ea948d", "#97d5b7", "#e3c78e", "#8fb8d4", "#bcaad4", "#86d9cc", "#e5eded",
        ],
        syntax: [
            "#6a7273", "#a68fc0", "#7cbfa0", "#d2b071", "#66c2b5", "#729fc0", "#dd7a72", "#c8d3d3",
            "#879090", "#dd7a72", "#d2b071", "#dd7a72",
        ],
        source: source(
            "basalt-shore",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn buttercream_diner() -> ThemeVariant {
    variant(Seeds {
        id: "buttercream-diner",
        family_id: "buttercream-diner",
        name: "Buttercream Diner",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#fbf3dd",
        shell: "#f0e9d6",
        raised: "#fdf8eb",
        card: "#fcf6e6",
        text: "#4d3f33",
        muted: "#756759",
        faint: "#918474",
        accent: "#b04827",
        danger: "#bd3b28",
        warning: "#8b6200",
        success: "#4a742a",
        terminal_background: "#fcf6e6",
        ansi: [
            "#4d3f33", "#c8432f", "#5c8a3c", "#ac7c14", "#3f74a8", "#a2538c", "#38857a", "#ecdfc2",
            "#8a7a68", "#e0553f", "#6fa24a", "#c79320", "#5089bf", "#b968a2", "#479e91", "#fffcf2",
        ],
        syntax: [
            "#918474", "#a2538c", "#5c8a3c", "#ac7c14", "#38857a", "#3f74a8", "#c8432f", "#4d3f33",
            "#756759", "#c8432f", "#ac7c14", "#bd3b28",
        ],
        source: source(
            "buttercream-diner",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn cathode_amber() -> ThemeVariant {
    variant(Seeds {
        id: "cathode-amber",
        family_id: "cathode-amber",
        name: "Cathode Amber",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#140f08",
        shell: "#1a160f",
        raised: "#241f18",
        card: "#18130d",
        text: "#f0c88a",
        muted: "#9d8259",
        faint: "#7c6645",
        accent: "#ffb642",
        danger: "#e8763f",
        warning: "#ffb642",
        success: "#b8a83f",
        terminal_background: "#18130d",
        ansi: [
            "#241a0e", "#e8763f", "#b8a83f", "#ffb642", "#c98d4f", "#cf8a62", "#d4a852", "#cfa972",
            "#7a6238", "#f2925f", "#d0c05e", "#ffcc70", "#dca86e", "#e2a67f", "#e7c077", "#ffe8c2",
        ],
        syntax: [
            "#7c6645", "#cf8a62", "#b8a83f", "#ffb642", "#d4a852", "#c98d4f", "#e8763f", "#f0c88a",
            "#9d8259", "#e8763f", "#ffb642", "#e8763f",
        ],
        source: source(
            "cathode-amber",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn charcoal_atelier() -> ThemeVariant {
    variant(Seeds {
        id: "charcoal-atelier",
        family_id: "charcoal-atelier",
        name: "Charcoal Atelier",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#161616",
        shell: "#1d1d1d",
        raised: "#262626",
        card: "#1a1a1a",
        text: "#d6d2cc",
        muted: "#8f8c88",
        faint: "#716f6c",
        accent: "#e66543",
        danger: "#db6d50",
        warning: "#cfae7a",
        success: "#9aab84",
        terminal_background: "#1a1a1a",
        ansi: [
            "#222222", "#d96a4d", "#9aab84", "#cfae7a", "#8aa0b4", "#b193a8", "#8fafa8", "#b5b1ab",
            "#5f5c58", "#e8886e", "#b3c39d", "#e0c493", "#a4b9cc", "#c8adc0", "#a8c7c0", "#efece7",
        ],
        syntax: [
            "#716f6c", "#b193a8", "#9aab84", "#cfae7a", "#8fafa8", "#8aa0b4", "#d96a4d", "#d6d2cc",
            "#8f8c88", "#d96a4d", "#cfae7a", "#db6d50",
        ],
        source: source(
            "charcoal-atelier",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn cinder_peak() -> ThemeVariant {
    variant(Seeds {
        id: "cinder-peak",
        family_id: "cinder-peak",
        name: "Cinder Peak",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#151011",
        shell: "#1c1718",
        raised: "#252021",
        card: "#191415",
        text: "#ddccc6",
        muted: "#928582",
        faint: "#736866",
        accent: "#ff6b4a",
        danger: "#f25c4a",
        warning: "#f0a04f",
        success: "#97ad72",
        terminal_background: "#191415",
        ansi: [
            "#231a1b", "#f25c4a", "#97ad72", "#f0a04f", "#9188c4", "#d47a94", "#8fb2a8", "#c0b0ab",
            "#6d5a58", "#ff7f68", "#b1c58d", "#fcba70", "#aba3d6", "#e596ac", "#a9cabf", "#f2e7e3",
        ],
        syntax: [
            "#736866", "#d47a94", "#97ad72", "#f0a04f", "#8fb2a8", "#9188c4", "#f25c4a", "#ddccc6",
            "#928582", "#f25c4a", "#f0a04f", "#f25c4a",
        ],
        source: source(
            "cinder-peak",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn coastal_linen() -> ThemeVariant {
    variant(Seeds {
        id: "coastal-linen",
        family_id: "coastal-linen",
        name: "Coastal Linen",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#faf6ee",
        shell: "#efece5",
        raised: "#fcfaf5",
        card: "#fbf8f2",
        text: "#3d4652",
        muted: "#656c74",
        faint: "#84898e",
        accent: "#12758a",
        danger: "#b34a3d",
        warning: "#8b6400",
        success: "#417638",
        terminal_background: "#fbf8f2",
        ansi: [
            "#3d4652", "#c25547", "#57904d", "#ac8125", "#3a74ad", "#91589a", "#2f8fa6", "#e3dccb",
            "#7a828c", "#d96a5a", "#68a95d", "#c39631", "#4c89c4", "#a86db1", "#3ba6bf", "#fefcf7",
        ],
        syntax: [
            "#84898e", "#91589a", "#57904d", "#ac8125", "#2f8fa6", "#3a74ad", "#c25547", "#3d4652",
            "#656c74", "#c25547", "#ac8125", "#b34a3d",
        ],
        source: source(
            "coastal-linen",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn cobalt_foundry() -> ThemeVariant {
    variant(Seeds {
        id: "cobalt-foundry",
        family_id: "cobalt-foundry",
        name: "Cobalt Foundry",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#10151c",
        shell: "#171c22",
        raised: "#20252c",
        card: "#141920",
        text: "#c2cedb",
        muted: "#828c97",
        faint: "#666f79",
        accent: "#4fa8ff",
        danger: "#e57373",
        warning: "#d4a95f",
        success: "#81b366",
        terminal_background: "#141920",
        ansi: [
            "#1a2129", "#e57373", "#81b366", "#d4a95f", "#4f95e0", "#9d85cf", "#56b3c4", "#a2b0be",
            "#56626f", "#ef8f8f", "#9cc983", "#e5c07e", "#74aeec", "#b5a0e0", "#78cbda", "#e2eaf2",
        ],
        syntax: [
            "#666f79", "#9d85cf", "#81b366", "#d4a95f", "#56b3c4", "#4f95e0", "#e57373", "#c2cedb",
            "#828c97", "#e57373", "#d4a95f", "#e57373",
        ],
        source: source(
            "cobalt-foundry",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn ember_hollow() -> ThemeVariant {
    variant(Seeds {
        id: "ember-hollow",
        family_id: "ember-hollow",
        name: "Ember Hollow",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#171210",
        shell: "#1e1917",
        raised: "#272220",
        card: "#1b1614",
        text: "#e0cfc4",
        muted: "#94877f",
        faint: "#756a64",
        accent: "#f0955c",
        danger: "#e06c4f",
        warning: "#e8a75a",
        success: "#a3b06a",
        terminal_background: "#1b1614",
        ansi: [
            "#241b17", "#e06c4f", "#a3b06a", "#e8a75a", "#8e9fb3", "#c98a8e", "#8fb0a2", "#c3b3a8",
            "#6b5a50", "#f08767", "#bcc985", "#f5bf77", "#a8b9cc", "#dda4a7", "#a9c9ba", "#f3e8e0",
        ],
        syntax: [
            "#756a64", "#c98a8e", "#a3b06a", "#e8a75a", "#8fb0a2", "#8e9fb3", "#e06c4f", "#e0cfc4",
            "#94877f", "#e06c4f", "#e8a75a", "#e06c4f",
        ],
        source: source(
            "ember-hollow",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn glacier_mint() -> ThemeVariant {
    variant(Seeds {
        id: "glacier-mint",
        family_id: "glacier-mint",
        name: "Glacier Mint",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#f0f9f6",
        shell: "#e7efec",
        raised: "#f6fbfa",
        card: "#f4fbf8",
        text: "#2f4640",
        muted: "#5c706a",
        faint: "#7b8d87",
        accent: "#007961",
        danger: "#b5483e",
        warning: "#866700",
        success: "#28784c",
        terminal_background: "#f4fbf8",
        ansi: [
            "#2f4640", "#c05045", "#3d9060", "#a48224", "#3f7ab8", "#8858ab", "#26948e", "#d2e8e0",
            "#6c837c", "#d76458", "#4aa971", "#bb9830", "#5190cc", "#9e6ec1", "#32ada6", "#fbfffd",
        ],
        syntax: [
            "#7b8d87", "#8858ab", "#3d9060", "#a48224", "#26948e", "#3f7ab8", "#c05045", "#2f4640",
            "#5c706a", "#c05045", "#a48224", "#b5483e",
        ],
        source: source(
            "glacier-mint",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn heliotrope_haze() -> ThemeVariant {
    variant(Seeds {
        id: "heliotrope-haze",
        family_id: "heliotrope-haze",
        name: "Heliotrope Haze",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#14101f",
        shell: "#1b1725",
        raised: "#24202f",
        card: "#181422",
        text: "#dcd2ee",
        muted: "#8d859d",
        faint: "#6f687f",
        accent: "#ff7edb",
        danger: "#f66e93",
        warning: "#fdca6a",
        success: "#72e0aa",
        terminal_background: "#181422",
        ansi: [
            "#211a33", "#f66e93", "#72e0aa", "#fdca6a", "#7a9bf5", "#c980f2", "#5fd0e5", "#b6abd0",
            "#6a5f85", "#fa8cab", "#92edc0", "#fed98c", "#97b2f8", "#da9ff6", "#82e0ef", "#f0eaf9",
        ],
        syntax: [
            "#6f687f", "#c980f2", "#72e0aa", "#fdca6a", "#5fd0e5", "#7a9bf5", "#f66e93", "#dcd2ee",
            "#8d859d", "#f66e93", "#fdca6a", "#f66e93",
        ],
        source: source(
            "heliotrope-haze",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn ink_meridian() -> ThemeVariant {
    variant(Seeds {
        id: "ink-meridian",
        family_id: "ink-meridian",
        name: "Ink Meridian",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#101623",
        shell: "#171d29",
        raised: "#202734",
        card: "#151a26",
        text: "#ccd6ea",
        muted: "#848d9f",
        faint: "#677081",
        accent: "#83a8f2",
        danger: "#de6e7c",
        warning: "#d9b06e",
        success: "#7cbf8e",
        terminal_background: "#151a26",
        ansi: [
            "#1a2233", "#de6e7c", "#7cbf8e", "#d9b06e", "#6f96e8", "#a687e0", "#67b7cf", "#aab8d0",
            "#58657f", "#ec8b97", "#98d4a7", "#e9c78c", "#8db0f2", "#bda3ec", "#87cee2", "#e6ecf7",
        ],
        syntax: [
            "#677081", "#a687e0", "#7cbf8e", "#d9b06e", "#67b7cf", "#6f96e8", "#de6e7c", "#ccd6ea",
            "#848d9f", "#de6e7c", "#d9b06e", "#de6e7c",
        ],
        source: source(
            "ink-meridian",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn ivory_gallery() -> ThemeVariant {
    variant(Seeds {
        id: "ivory-gallery",
        family_id: "ivory-gallery",
        name: "Ivory Gallery",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#ffffff",
        shell: "#f5f5f5",
        raised: "#fbfcfe",
        card: "#fafbfd",
        text: "#1f2328",
        muted: "#6e7175",
        faint: "#8c8e91",
        accent: "#1a6fd4",
        danger: "#c13a24",
        warning: "#966700",
        success: "#2f7d3c",
        terminal_background: "#fafbfd",
        ansi: [
            "#1f2328", "#c13a24", "#2f7d3c", "#9a6a00", "#1a6fd4", "#7d4bb5", "#1b7c8c", "#d0d7de",
            "#656d76", "#dd4a30", "#3a9449", "#b37e0a", "#388ae8", "#9560cc", "#2694a6", "#ffffff",
        ],
        syntax: [
            "#8c8e91", "#7d4bb5", "#2f7d3c", "#9a6a00", "#1b7c8c", "#1a6fd4", "#c13a24", "#1f2328",
            "#6e7175", "#c13a24", "#9a6a00", "#c13a24",
        ],
        source: source(
            "ivory-gallery",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn juniper_smoke() -> ThemeVariant {
    variant(Seeds {
        id: "juniper-smoke",
        family_id: "juniper-smoke",
        name: "Juniper Smoke",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#171b18",
        shell: "#1e221f",
        raised: "#282c29",
        card: "#1b1f1c",
        text: "#ccd5cd",
        muted: "#8b938c",
        faint: "#6e756f",
        accent: "#a9c7ad",
        danger: "#d59090",
        warning: "#d3bb8b",
        success: "#99bd9b",
        terminal_background: "#1b1f1c",
        ansi: [
            "#232823", "#d59090", "#99bd9b", "#d3bb8b", "#8ea8c0", "#b39cbb", "#92bab2", "#b4bfb5",
            "#5d6a5f", "#e2a8a8", "#b1d1b2", "#e3cea4", "#a7bed3", "#c8b3cf", "#abd0c8", "#e8ede8",
        ],
        syntax: [
            "#6e756f", "#b39cbb", "#99bd9b", "#d3bb8b", "#92bab2", "#8ea8c0", "#d59090", "#ccd5cd",
            "#8b938c", "#d59090", "#d3bb8b", "#d59090",
        ],
        source: source(
            "juniper-smoke",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn lavender_postcard() -> ThemeVariant {
    variant(Seeds {
        id: "lavender-postcard",
        family_id: "lavender-postcard",
        name: "Lavender Postcard",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#f7f4fb",
        shell: "#edeaf0",
        raised: "#faf8fd",
        card: "#f9f7fc",
        text: "#40374f",
        muted: "#6f677c",
        faint: "#8b8597",
        accent: "#7758bb",
        danger: "#b3454d",
        warning: "#886307",
        success: "#367742",
        terminal_background: "#f9f7fc",
        ansi: [
            "#40374f", "#c04e56", "#4a8f56", "#a67e28", "#4a6fc0", "#8557b8", "#3e8496", "#ded7ec",
            "#7d7590", "#d86470", "#58a866", "#bf9334", "#5d84d6", "#9b6ecd", "#4c9cb0", "#fefdff",
        ],
        syntax: [
            "#8b8597", "#8557b8", "#4a8f56", "#a67e28", "#3e8496", "#4a6fc0", "#c04e56", "#40374f",
            "#6f677c", "#c04e56", "#a67e28", "#b3454d",
        ],
        source: source(
            "lavender-postcard",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn merino_light() -> ThemeVariant {
    variant(Seeds {
        id: "merino-light",
        family_id: "merino",
        name: "Merino Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#f6f7f9",
        shell: "#eceef2",
        raised: "#fafbfc",
        card: "#f8f9fb",
        text: "#1f2328",
        muted: "#696d71",
        faint: "#878a8e",
        accent: "#0667d8",
        danger: "#cf222e",
        warning: "#9d5c00",
        success: "#116329",
        terminal_background: "#ffffff",
        ansi: [
            "#24292f", "#cf222e", "#116329", "#9d5c00", "#0969da", "#8250df", "#1b7c83", "#6e7781",
            "#57606a", "#a40e26", "#1a7f37", "#c47a00", "#218bff", "#a475f9", "#3192aa", "#8c959f",
        ],
        syntax: [
            "#878a8e", "#8250df", "#116329", "#9d5c00", "#1b7c83", "#0969da", "#cf222e", "#1f2328",
            "#696d71", "#cf222e", "#9d5c00", "#cf222e",
        ],
        source: source(
            "merino-light",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn merino_dark() -> ThemeVariant {
    variant(Seeds {
        id: "merino-dark",
        family_id: "merino",
        name: "Merino Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#0c0e11",
        shell: "#131519",
        raised: "#1e2023",
        card: "#0f1319",
        text: "#e6edf3",
        muted: "#82878c",
        faint: "#666a6f",
        accent: "#3485fa",
        danger: "#ff7b72",
        warning: "#d29922",
        success: "#3fb950",
        terminal_background: "#0d1117",
        ansi: [
            "#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4",
            "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#ffffff",
        ],
        syntax: [
            "#666a6f", "#bc8cff", "#3fb950", "#d29922", "#39c5cf", "#58a6ff", "#ff7b72", "#e6edf3",
            "#82878c", "#ff7b72", "#d29922", "#ff7b72",
        ],
        source: source(
            "merino-dark",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn midnight_tundra() -> ThemeVariant {
    variant(Seeds {
        id: "midnight-tundra",
        family_id: "midnight-tundra",
        name: "Midnight Tundra",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#0e141b",
        shell: "#151b21",
        raised: "#1e242b",
        card: "#12181f",
        text: "#c9d4de",
        muted: "#818b94",
        faint: "#656e77",
        accent: "#8fb8d8",
        danger: "#d9707c",
        warning: "#d8b174",
        success: "#7fb88a",
        terminal_background: "#12181f",
        ansi: [
            "#17202a", "#d9707c", "#7fb88a", "#d8b174", "#6da3cf", "#a58cc7", "#6fb5b8", "#aebccb",
            "#4f6272", "#e88a95", "#99cfa3", "#e8c88f", "#8abde4", "#bda6dd", "#8cced0", "#e6eef5",
        ],
        syntax: [
            "#656e77", "#a58cc7", "#7fb88a", "#d8b174", "#6fb5b8", "#6da3cf", "#d9707c", "#c9d4de",
            "#818b94", "#d9707c", "#d8b174", "#d9707c",
        ],
        source: source(
            "midnight-tundra",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn misty_forest() -> ThemeVariant {
    variant(Seeds {
        id: "misty-forest",
        family_id: "misty-forest",
        name: "Misty Forest",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#131a16",
        shell: "#1a211d",
        raised: "#242b27",
        card: "#181e1a",
        text: "#c4d2c5",
        muted: "#869288",
        faint: "#69746c",
        accent: "#9dc2a0",
        danger: "#cf7a70",
        warning: "#c9ad72",
        success: "#8fbf8f",
        terminal_background: "#181e1a",
        ansi: [
            "#1c2620", "#cf7a70", "#8fbf8f", "#c9ad72", "#77a3b5", "#a793bb", "#7cb5a3", "#aebbaf",
            "#55665a", "#e09489", "#a9d6a6", "#ddc48d", "#93bccb", "#bfadd1", "#97cdbb", "#e2ece2",
        ],
        syntax: [
            "#69746c", "#a793bb", "#8fbf8f", "#c9ad72", "#7cb5a3", "#77a3b5", "#cf7a70", "#c4d2c5",
            "#869288", "#cf7a70", "#c9ad72", "#cf7a70",
        ],
        source: source(
            "misty-forest",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn moss_cathedral() -> ThemeVariant {
    variant(Seeds {
        id: "moss-cathedral",
        family_id: "moss-cathedral",
        name: "Moss Cathedral",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#0f1710",
        shell: "#161e17",
        raised: "#1f2720",
        card: "#141b14",
        text: "#c6d5c2",
        muted: "#828f80",
        faint: "#667164",
        accent: "#8fd48f",
        danger: "#d4756b",
        warning: "#cdb35e",
        success: "#74bd74",
        terminal_background: "#141b14",
        ansi: [
            "#1a261b", "#d4756b", "#74bd74", "#cdb35e", "#6d9cba", "#a488bd", "#6bb8a0", "#aabca6",
            "#52654f", "#e39187", "#92d492", "#e0ca7e", "#8ab6d1", "#bba4d2", "#89d0b8", "#e4eee1",
        ],
        syntax: [
            "#667164", "#a488bd", "#74bd74", "#cdb35e", "#6bb8a0", "#6d9cba", "#d4756b", "#c6d5c2",
            "#828f80", "#d4756b", "#cdb35e", "#d4756b",
        ],
        source: source(
            "moss-cathedral",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn neon_arcade() -> ThemeVariant {
    variant(Seeds {
        id: "neon-arcade",
        family_id: "neon-arcade",
        name: "Neon Arcade",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#0d0d15",
        shell: "#14141b",
        raised: "#1d1d25",
        card: "#111118",
        text: "#d8dcf0",
        muted: "#818393",
        faint: "#656775",
        accent: "#ff4fd8",
        danger: "#ff5370",
        warning: "#ffd166",
        success: "#3ddc97",
        terminal_background: "#111118",
        ansi: [
            "#1a1a28", "#ff5370", "#3ddc97", "#ffd166", "#4f9cff", "#c774ff", "#2fd6e0", "#b8bcd8",
            "#5c5e78", "#ff7a92", "#6ceeb4", "#ffe08a", "#7cb6ff", "#da9bff", "#63e6ee", "#f0f2ff",
        ],
        syntax: [
            "#656775", "#c774ff", "#3ddc97", "#ffd166", "#2fd6e0", "#4f9cff", "#ff5370", "#d8dcf0",
            "#818393", "#ff5370", "#ffd166", "#ff5370",
        ],
        source: source(
            "neon-arcade",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn newsprint_fog() -> ThemeVariant {
    variant(Seeds {
        id: "newsprint-fog",
        family_id: "newsprint-fog",
        name: "Newsprint Fog",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#f2f2f0",
        shell: "#e8e8e6",
        raised: "#f7f7f5",
        card: "#f5f5f3",
        text: "#333331",
        muted: "#696967",
        faint: "#858583",
        accent: "#b3402e",
        danger: "#b3402e",
        warning: "#816407",
        success: "#507045",
        terminal_background: "#f5f5f3",
        ansi: [
            "#333331", "#b3402e", "#5d7f52", "#987921", "#4f6d94", "#85607f", "#4d7f7c", "#d5d5d0",
            "#78786f", "#cc4f3b", "#6f9562", "#af8e2c", "#6182ab", "#9b7494", "#5e9692", "#fdfdfb",
        ],
        syntax: [
            "#858583", "#85607f", "#5d7f52", "#987921", "#4d7f7c", "#4f6d94", "#b3402e", "#333331",
            "#696967", "#b3402e", "#987921", "#b3402e",
        ],
        source: source(
            "newsprint-fog",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn nocturne_rose() -> ThemeVariant {
    variant(Seeds {
        id: "nocturne-rose",
        family_id: "nocturne-rose",
        name: "Nocturne Rose",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#191216",
        shell: "#20191d",
        raised: "#292226",
        card: "#1d161a",
        text: "#e2cdd6",
        muted: "#96868d",
        faint: "#786a70",
        accent: "#f27fa5",
        danger: "#ec6d8b",
        warning: "#dfae79",
        success: "#8cba92",
        terminal_background: "#1d161a",
        ansi: [
            "#271c22", "#ec6d8b", "#8cba92", "#dfae79", "#8f95d6", "#cd85c0", "#83b7b2", "#c3aeb8",
            "#6e5a63", "#f68ba4", "#a7d1ab", "#efc694", "#aab0e5", "#dfa2d4", "#9ecfca", "#f5e9ee",
        ],
        syntax: [
            "#786a70", "#cd85c0", "#8cba92", "#dfae79", "#83b7b2", "#8f95d6", "#ec6d8b", "#e2cdd6",
            "#96868d", "#ec6d8b", "#dfae79", "#ec6d8b",
        ],
        source: source(
            "nocturne-rose",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn orchard_breeze() -> ThemeVariant {
    variant(Seeds {
        id: "orchard-breeze",
        family_id: "orchard-breeze",
        name: "Orchard Breeze",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#f4f9ef",
        shell: "#eaefe6",
        raised: "#f8fbf5",
        card: "#f7fbf3",
        text: "#3a4a35",
        muted: "#62705d",
        faint: "#808d7b",
        accent: "#3b792e",
        danger: "#b64539",
        warning: "#866600",
        success: "#397a2a",
        terminal_background: "#f7fbf3",
        ansi: [
            "#3a4a35", "#c65043", "#4f9440", "#a8831f", "#4478b0", "#90569c", "#37897a", "#d9e5cf",
            "#75856b", "#dd6455", "#5fae4e", "#c1992b", "#558dc6", "#a76bb3", "#45a191", "#fcfef9",
        ],
        syntax: [
            "#808d7b", "#90569c", "#4f9440", "#a8831f", "#37897a", "#4478b0", "#c65043", "#3a4a35",
            "#62705d", "#c65043", "#a8831f", "#b64539",
        ],
        source: source(
            "orchard-breeze",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn oxblood_library() -> ThemeVariant {
    variant(Seeds {
        id: "oxblood-library",
        family_id: "oxblood-library",
        name: "Oxblood Library",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#191012",
        shell: "#201719",
        raised: "#292022",
        card: "#1d1416",
        text: "#dfcbc2",
        muted: "#968581",
        faint: "#776966",
        accent: "#da655a",
        danger: "#d9675e",
        warning: "#d8a55c",
        success: "#94ad72",
        terminal_background: "#1d1416",
        ansi: [
            "#281a1d", "#cf5a52", "#94ad72", "#d8a55c", "#8f97b8", "#c07e93", "#8fb0a0", "#c0aca4",
            "#705a58", "#e07a70", "#adc48c", "#e8be7c", "#a9b1cc", "#d49aab", "#a8c8b8", "#f1e4dd",
        ],
        syntax: [
            "#776966", "#c07e93", "#94ad72", "#d8a55c", "#8fb0a0", "#8f97b8", "#cf5a52", "#dfcbc2",
            "#968581", "#cf5a52", "#d8a55c", "#d9675e",
        ],
        source: source(
            "oxblood-library",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn peach_veranda() -> ThemeVariant {
    variant(Seeds {
        id: "peach-veranda",
        family_id: "peach-veranda",
        name: "Peach Veranda",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#fdf3ec",
        shell: "#f2e9e3",
        raised: "#fef8f4",
        card: "#fef6f1",
        text: "#4d3a33",
        muted: "#796760",
        faint: "#94847d",
        accent: "#a95028",
        danger: "#b5442c",
        warning: "#8b6200",
        success: "#457630",
        terminal_background: "#fef6f1",
        ansi: [
            "#4d3a33", "#c44f36", "#578c42", "#ab7c1c", "#4472ac", "#9c5589", "#3a8878", "#f0ddd0",
            "#8a746a", "#dc6146", "#67a350", "#c39228", "#5687c2", "#b26a9f", "#48a08e", "#fffaf6",
        ],
        syntax: [
            "#94847d", "#9c5589", "#578c42", "#ab7c1c", "#3a8878", "#4472ac", "#c44f36", "#4d3a33",
            "#796760", "#c44f36", "#ab7c1c", "#b5442c",
        ],
        source: source(
            "peach-veranda",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn petrol_lagoon() -> ThemeVariant {
    variant(Seeds {
        id: "petrol-lagoon",
        family_id: "petrol-lagoon",
        name: "Petrol Lagoon",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#0c1618",
        shell: "#141d1f",
        raised: "#1d2628",
        card: "#111a1c",
        text: "#c2d8d8",
        muted: "#7c8e8f",
        faint: "#617172",
        accent: "#35d0c0",
        danger: "#e0736e",
        warning: "#d1b366",
        success: "#6cc48e",
        terminal_background: "#111a1c",
        ansi: [
            "#152325", "#e0736e", "#6cc48e", "#d1b366", "#5b9fc4", "#9d8ccc", "#3fc4b7", "#a0b8b8",
            "#4d6467", "#ec8f88", "#8adaa8", "#e2c985", "#7db8d8", "#b5a7dd", "#66d9cd", "#dfeeee",
        ],
        syntax: [
            "#617172", "#9d8ccc", "#6cc48e", "#d1b366", "#3fc4b7", "#5b9fc4", "#e0736e", "#c2d8d8",
            "#7c8e8f", "#e0736e", "#d1b366", "#e0736e",
        ],
        source: source(
            "petrol-lagoon",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn phosphor_archive() -> ThemeVariant {
    variant(Seeds {
        id: "phosphor-archive",
        family_id: "phosphor-archive",
        name: "Phosphor Archive",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#0a120c",
        shell: "#111913",
        raised: "#1a221c",
        card: "#0f1610",
        text: "#a8e5a8",
        muted: "#689069",
        faint: "#527253",
        accent: "#58f158",
        danger: "#cf9a5f",
        warning: "#a5dc5f",
        success: "#4fd24f",
        terminal_background: "#0f1610",
        ansi: [
            "#142314", "#cf9a5f", "#4fd24f", "#a5dc5f", "#55b884", "#8fce74", "#5fdcb0", "#8fc48f",
            "#3f6b3f", "#e5b578", "#74e874", "#bfeb82", "#77d1a2", "#aade92", "#82edc6", "#d2f5d2",
        ],
        syntax: [
            "#527253", "#8fce74", "#4fd24f", "#a5dc5f", "#5fdcb0", "#55b884", "#cf9a5f", "#a8e5a8",
            "#689069", "#cf9a5f", "#a5dc5f", "#cf9a5f",
        ],
        source: source(
            "phosphor-archive",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn polar_aurora() -> ThemeVariant {
    variant(Seeds {
        id: "polar-aurora",
        family_id: "polar-aurora",
        name: "Polar Aurora",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#0b1016",
        shell: "#12171c",
        raised: "#1b2026",
        card: "#0f1419",
        text: "#cfdae2",
        muted: "#7e878f",
        faint: "#626a72",
        accent: "#6ef2b8",
        danger: "#ee7189",
        warning: "#e5c568",
        success: "#55dfa2",
        terminal_background: "#0f1419",
        ansi: [
            "#151d26", "#ee7189", "#55dfa2", "#e5c568", "#5fa5e8", "#a888ef", "#47d3d6", "#a9bac6",
            "#526270", "#f78da1", "#79eeb9", "#f1d787", "#82bcf2", "#bfa3f6", "#6ce5e8", "#e8f1f7",
        ],
        syntax: [
            "#626a72", "#a888ef", "#55dfa2", "#e5c568", "#47d3d6", "#5fa5e8", "#ee7189", "#cfdae2",
            "#7e878f", "#ee7189", "#e5c568", "#ee7189",
        ],
        source: source(
            "polar-aurora",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn porcelain_morning() -> ThemeVariant {
    variant(Seeds {
        id: "porcelain-morning",
        family_id: "porcelain-morning",
        name: "Porcelain Morning",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#f7f9fa",
        shell: "#edeff0",
        raised: "#fafcfc",
        card: "#f8fbfc",
        text: "#2e3a42",
        muted: "#656e75",
        faint: "#838b91",
        accent: "#176dbf",
        danger: "#ba4439",
        warning: "#926300",
        success: "#2f7a41",
        terminal_background: "#f8fbfc",
        ansi: [
            "#2e3a42", "#c74e42", "#3d8a4e", "#b07c22", "#2c6fbe", "#8455a8", "#22808a", "#cfd8dd",
            "#6d7d88", "#e0604f", "#4aa55f", "#cf9226", "#3d85d8", "#9c6ac2", "#2d99a5", "#ffffff",
        ],
        syntax: [
            "#838b91", "#8455a8", "#3d8a4e", "#b07c22", "#22808a", "#2c6fbe", "#c74e42", "#2e3a42",
            "#656e75", "#c74e42", "#b07c22", "#ba4439",
        ],
        source: source(
            "porcelain-morning",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn roasted_umber() -> ThemeVariant {
    variant(Seeds {
        id: "roasted-umber",
        family_id: "roasted-umber",
        name: "Roasted Umber",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#16110d",
        shell: "#1d1814",
        raised: "#26211d",
        card: "#1a1511",
        text: "#d9c8b8",
        muted: "#93867a",
        faint: "#756960",
        accent: "#c98a4b",
        danger: "#cf6f52",
        warning: "#d3a35f",
        success: "#96a86a",
        terminal_background: "#1a1511",
        ansi: [
            "#241c15", "#cf6f52", "#96a86a", "#d3a35f", "#8c95a8", "#b58a92", "#8fac93", "#bfae9d",
            "#6d5e50", "#df8a6e", "#afbf84", "#e4bb7d", "#a6afc0", "#c9a4ab", "#a8c3ab", "#efe3d5",
        ],
        syntax: [
            "#756960", "#b58a92", "#96a86a", "#d3a35f", "#8fac93", "#8c95a8", "#cf6f52", "#d9c8b8",
            "#93867a", "#cf6f52", "#d3a35f", "#cf6f52",
        ],
        source: source(
            "roasted-umber",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn saffron_nightfall() -> ThemeVariant {
    variant(Seeds {
        id: "saffron-nightfall",
        family_id: "saffron-nightfall",
        name: "Saffron Nightfall",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#131320",
        shell: "#1a1a26",
        raised: "#232330",
        card: "#171723",
        text: "#d9d4c8",
        muted: "#8d8a89",
        faint: "#6f6d70",
        accent: "#ffc247",
        danger: "#e0705f",
        warning: "#ffc247",
        success: "#9cb56b",
        terminal_background: "#171723",
        ansi: [
            "#1e1e30", "#e0705f", "#9cb56b", "#ffc247", "#7d8fd0", "#bb87c0", "#76b7ad", "#b7b2a5",
            "#625f72", "#ee8b7a", "#b6cc87", "#ffd36e", "#98a9e0", "#d0a3d3", "#93cfc5", "#f1ecdf",
        ],
        syntax: [
            "#6f6d70", "#bb87c0", "#9cb56b", "#ffc247", "#76b7ad", "#7d8fd0", "#e0705f", "#d9d4c8",
            "#8d8a89", "#e0705f", "#ffc247", "#e0705f",
        ],
        source: source(
            "saffron-nightfall",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn silver_point_light() -> ThemeVariant {
    variant(Seeds {
        id: "silver-point-light",
        family_id: "silver-point",
        name: "Silver Point Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#f4f4f4",
        shell: "#eaeaea",
        raised: "#f7f9fa",
        card: "#f5f7f9",
        text: "#1a1a1a",
        muted: "#6a6a6a",
        faint: "#878787",
        accent: "#696969",
        danger: "#5c5c5c",
        warning: "#5c5c5c",
        success: "#6a6a6a",
        terminal_background: "#f5f7f9",
        ansi: [
            "#1a1a1a", "#5c5c5c", "#8e8e8e", "#5c5c5c", "#8e8e8e", "#5c5c5c", "#8e8e8e", "#8e8e8e",
            "#5c5c5c", "#1a1a1a", "#5c5c5c", "#1a1a1a", "#5c5c5c", "#1a1a1a", "#5c5c5c", "#f4f4f4",
        ],
        syntax: [
            "#878787", "#5c5c5c", "#8e8e8e", "#5c5c5c", "#8e8e8e", "#8e8e8e", "#5c5c5c", "#1a1a1a",
            "#6a6a6a", "#5c5c5c", "#5c5c5c", "#5c5c5c",
        ],
        source: source(
            "silver-point-light",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn silver_point_dark() -> ThemeVariant {
    variant(Seeds {
        id: "silver-point-dark",
        family_id: "silver-point",
        name: "Silver Point Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#131313",
        shell: "#1a1a1a",
        raised: "#232323",
        card: "#171717",
        text: "#b4b4b4",
        muted: "#898989",
        faint: "#6c6c6c",
        accent: "#898989",
        danger: "#b4b4b4",
        warning: "#b4b4b4",
        success: "#898989",
        terminal_background: "#171717",
        ansi: [
            "#1a1a1a", "#b4b4b4", "#707070", "#b4b4b4", "#707070", "#b4b4b4", "#707070", "#b4b4b4",
            "#707070", "#f4f4f4", "#b4b4b4", "#f4f4f4", "#b4b4b4", "#f4f4f4", "#b4b4b4", "#f4f4f4",
        ],
        syntax: [
            "#6c6c6c", "#b4b4b4", "#707070", "#b4b4b4", "#707070", "#707070", "#b4b4b4", "#b4b4b4",
            "#898989", "#b4b4b4", "#b4b4b4", "#b4b4b4",
        ],
        source: source(
            "silver-point-dark",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn sonoran_dusk() -> ThemeVariant {
    variant(Seeds {
        id: "sonoran-dusk",
        family_id: "sonoran-dusk",
        name: "Sonoran Dusk",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#171412",
        shell: "#1e1b19",
        raised: "#272422",
        card: "#1b1816",
        text: "#ddd0bf",
        muted: "#938a7e",
        faint: "#746d64",
        accent: "#e8a860",
        danger: "#d96e56",
        warning: "#e0a45c",
        success: "#a0b06a",
        terminal_background: "#1b1816",
        ansi: [
            "#26201c", "#d96e56", "#a0b06a", "#e0a45c", "#8598b0", "#bd8aa2", "#92b39c", "#c2b4a3",
            "#6e6355", "#e88a72", "#b8c786", "#efbc7c", "#a0b2c8", "#d1a5b8", "#accab4", "#f2e9dc",
        ],
        syntax: [
            "#746d64", "#bd8aa2", "#a0b06a", "#e0a45c", "#92b39c", "#8598b0", "#d96e56", "#ddd0bf",
            "#938a7e", "#d96e56", "#e0a45c", "#d96e56",
        ],
        source: source(
            "sonoran-dusk",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn static_noir() -> ThemeVariant {
    variant(Seeds {
        id: "static-noir",
        family_id: "static-noir",
        name: "Static Noir",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#0a0a0a",
        shell: "#101010",
        raised: "#191919",
        card: "#0e0e0e",
        text: "#ededed",
        muted: "#818181",
        faint: "#656565",
        accent: "#ff3b30",
        danger: "#ff4d42",
        warning: "#e6c15c",
        success: "#9fd08a",
        terminal_background: "#0e0e0e",
        ansi: [
            "#1a1a1a", "#ff4d42", "#9fd08a", "#e6c15c", "#7fb2e8", "#d093d0", "#74cfd1", "#c9c9c9",
            "#666666", "#ff6e64", "#b7e2a3", "#f2d47c", "#9dc7f2", "#e0aede", "#93e1e2", "#ffffff",
        ],
        syntax: [
            "#656565", "#d093d0", "#9fd08a", "#e6c15c", "#74cfd1", "#7fb2e8", "#ff4d42", "#ededed",
            "#818181", "#ff4d42", "#e6c15c", "#ff4d42",
        ],
        source: source(
            "static-noir",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn sunlit_parchment() -> ThemeVariant {
    variant(Seeds {
        id: "sunlit-parchment",
        family_id: "sunlit-parchment",
        name: "Sunlit Parchment",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#f8f2e3",
        shell: "#ede8db",
        raised: "#fbf7ee",
        card: "#faf5ea",
        text: "#4a3f2e",
        muted: "#726857",
        faint: "#8e8474",
        accent: "#975a0f",
        danger: "#b6432e",
        warning: "#846200",
        success: "#4c722a",
        terminal_background: "#faf5ea",
        ansi: [
            "#4a3f2e", "#b8442f", "#587f35", "#9a7410", "#3a6ea5", "#96568e", "#337a70", "#e2d7bd",
            "#85765e", "#d4553c", "#6b9642", "#b58a1a", "#4a82bd", "#ad6ba4", "#429388", "#fdfaf1",
        ],
        syntax: [
            "#8e8474", "#96568e", "#587f35", "#9a7410", "#337a70", "#3a6ea5", "#b8442f", "#4a3f2e",
            "#726857", "#b8442f", "#9a7410", "#b6432e",
        ],
        source: source(
            "sunlit-parchment",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn terracotta_noon() -> ThemeVariant {
    variant(Seeds {
        id: "terracotta-noon",
        family_id: "terracotta-noon",
        name: "Terracotta Noon",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#f9efe6",
        shell: "#eee5de",
        raised: "#fbf5f0",
        card: "#fbf3ec",
        text: "#4a382c",
        muted: "#766559",
        faint: "#918276",
        accent: "#aa4a26",
        danger: "#b24226",
        warning: "#896000",
        success: "#497125",
        terminal_background: "#fbf3ec",
        ansi: [
            "#4a382c", "#bd4a2d", "#5f8a3a", "#a97a16", "#47709f", "#9d5680", "#3e8570", "#ead9c8",
            "#857262", "#d55c3c", "#72a248", "#c18f22", "#5985b5", "#b36b95", "#4d9d86", "#fefaf5",
        ],
        syntax: [
            "#918276", "#9d5680", "#5f8a3a", "#a97a16", "#3e8570", "#47709f", "#bd4a2d", "#4a382c",
            "#766559", "#bd4a2d", "#a97a16", "#b24226",
        ],
        source: source(
            "terracotta-noon",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn velvet_dusk() -> ThemeVariant {
    variant(Seeds {
        id: "velvet-dusk",
        family_id: "velvet-dusk",
        name: "Velvet Dusk",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#171122",
        shell: "#1e1828",
        raised: "#272232",
        card: "#1b1525",
        text: "#d6cbe6",
        muted: "#90869f",
        faint: "#726a81",
        accent: "#c9a1f0",
        danger: "#e07396",
        warning: "#dcae76",
        success: "#83bb8d",
        terminal_background: "#1b1525",
        ansi: [
            "#241a35", "#e07396", "#83bb8d", "#dcae76", "#7f95dc", "#b98ae0", "#7fb8c4", "#b5aacb",
            "#635677", "#ef92ae", "#9ed3a7", "#edc793", "#9cafe9", "#cfa8f0", "#9cd0da", "#ede6f7",
        ],
        syntax: [
            "#726a81", "#b98ae0", "#83bb8d", "#dcae76", "#7fb8c4", "#7f95dc", "#e07396", "#d6cbe6",
            "#90869f", "#e07396", "#dcae76", "#e07396",
        ],
        source: source(
            "velvet-dusk",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}

fn whitby_bay() -> ThemeVariant {
    variant(Seeds {
        id: "whitby-bay",
        family_id: "whitby-bay",
        name: "Whitby Bay",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#272a38",
        shell: "#2f323f",
        raised: "#393c4a",
        card: "#2c2f3c",
        text: "#f7f7ee",
        muted: "#a4a6a7",
        faint: "#84868b",
        accent: "#b696f7",
        danger: "#ff8177",
        warning: "#f5f28a",
        success: "#4df08a",
        terminal_background: "#2c2f3c",
        ansi: [
            "#20222e", "#ff5d57", "#4df08a", "#f5f28a", "#b696f7", "#ff7ed1", "#90ecf6", "#f2f2ea",
            "#66759f", "#ff7a72", "#72fa9e", "#fbfcae", "#d0b0fb", "#fc97d8", "#aefbf8", "#fdfdfa",
        ],
        syntax: [
            "#84868b", "#ff7ed1", "#4df08a", "#f5f28a", "#90ecf6", "#b696f7", "#ff5d57", "#f7f7ee",
            "#a4a6a7", "#ff5d57", "#f5f28a", "#ff8177",
        ],
        source: source(
            "whitby-bay",
            "rex",
            "/Applications/Rex.app (SuperUI_SuperDesignTokens themes.json)",
            "Rex.app 2026-11-28",
            "Proprietary (Rex)",
        ),
    })
}
