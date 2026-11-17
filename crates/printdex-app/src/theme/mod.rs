//! Design tokens and widget styles for "Frosted print" (see `DESIGN.md`).
//!
//! Widgets read every colour, size, radius and font from here. Colours are
//! the sRGB hex values from DESIGN.md. This module ships the **solid** token
//! set (opaque window); translucency with OS blur comes later and adds the
//! blur set beside it.
//!
//! A colour theme changes only the glass tint and the accent set
//! ([`ThemeTokens`]). Neutral ink and rules depend only on light/dark, and
//! status colours never change with the colour theme.

use iced::overlay::menu;
use iced::widget::{button, container, pick_list, scrollable, text_input};
use iced::{Background, Border, Color, Font, Shadow, font};

mod presets;

// ---------- spacing, radii, sizes ----------

pub const S1: f32 = 2.0;
pub const S2: f32 = 4.0;
pub const S3: f32 = 6.0;
pub const S4: f32 = 8.0;
pub const S5: f32 = 12.0;
pub const S6: f32 = 16.0;
pub const S7: f32 = 20.0;
#[allow(dead_code)] // DESIGN.md token; first used by the model view (part B).
pub const S8: f32 = 24.0;
pub const S9: f32 = 32.0;

pub const RADIUS_PANE: f32 = 14.0;
pub const RADIUS_WELL: f32 = 9.0;
pub const RADIUS_CONTROL: f32 = 10.0;
pub const RADIUS_CHIP: f32 = 999.0;

pub const SIDEBAR_WIDTH: f32 = 232.0;
pub const STATUSBAR_HEIGHT: f32 = 30.0;
pub const CONTROL_HEIGHT: f32 = 32.0;
pub const RAIL_ITEM_HEIGHT: f32 = 28.0;
pub const ICON: f32 = 16.0;
#[allow(dead_code)] // DESIGN.md token; first used by the model view (part B).
pub const ICON_DENSE: f32 = 14.0;
pub const ICON_CHIP: f32 = 12.0;

// ---------- type ----------

#[allow(dead_code)] // DESIGN.md token; first used by the model view (part B).
pub const SIZE_DISPLAY: f32 = 24.0;
pub const SIZE_TITLE: f32 = 19.0;
pub const SIZE_SECTION: f32 = 15.0;
pub const SIZE_BODY: f32 = 13.0;
pub const SIZE_CAPTION: f32 = 12.0;
pub const SIZE_GROUP: f32 = 11.0;
pub const SIZE_DATA: f32 = 11.0;

const FIGTREE: &str = "Figtree";
const MONO: &str = "JetBrains Mono";

pub const FONT_BODY: Font = Font::with_name(FIGTREE);
pub const FONT_MEDIUM: Font = Font {
    weight: font::Weight::Medium,
    ..FONT_BODY
};
pub const FONT_STRONG: Font = Font {
    weight: font::Weight::Semibold,
    ..FONT_BODY
};
pub const FONT_BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..FONT_BODY
};
pub const FONT_DATA: Font = Font::with_name(MONO);

/// Font files bundled with the app (SIL OFL 1.1, Latin subset).
pub const FONT_FILES: [&[u8]; 6] = [
    include_bytes!("../../assets/fonts/Figtree-400.ttf"),
    include_bytes!("../../assets/fonts/Figtree-500.ttf"),
    include_bytes!("../../assets/fonts/Figtree-600.ttf"),
    include_bytes!("../../assets/fonts/Figtree-700.ttf"),
    include_bytes!("../../assets/fonts/JetBrainsMono-400.ttf"),
    include_bytes!("../../assets/fonts/JetBrainsMono-500.ttf"),
];

// ---------- colours ----------

/// Parses `#rrggbb` or `#rrggbbaa`. Panics on malformed input, which only
/// ever comes from the constant tables below (covered by tests).
pub const fn hex(s: &str) -> Color {
    let b = s.as_bytes();
    assert!(
        b[0] == b'#' && (b.len() == 7 || b.len() == 9),
        "bad hex colour"
    );
    let r = byte(b[1], b[2]);
    let g = byte(b[3], b[4]);
    let bl = byte(b[5], b[6]);
    let a = if b.len() == 9 { byte(b[7], b[8]) } else { 255 };
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: bl as f32 / 255.0,
        a: a as f32 / 255.0,
    }
}

const fn byte(hi: u8, lo: u8) -> u8 {
    nibble(hi) * 16 + nibble(lo)
}

const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("bad hex digit"),
    }
}

/// Light or dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Light,
    Dark,
}

/// The user's colour theme (Settings > Appearance > Colour theme). The
/// tokens come from `presets.rs`, generated from `.impeccable/design.json`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorTheme {
    #[default]
    GraphiteSignal,
    VioletFrost,
    SmokedQuartz,
    SeaGlass,
    AmberResin,
    RosePetg,
    Glacier,
}

impl ColorTheme {
    /// Every preset, in the order Settings shows them.
    pub const ALL: [ColorTheme; 7] = [
        ColorTheme::GraphiteSignal,
        ColorTheme::VioletFrost,
        ColorTheme::SmokedQuartz,
        ColorTheme::SeaGlass,
        ColorTheme::AmberResin,
        ColorTheme::RosePetg,
        ColorTheme::Glacier,
    ];

    /// Stable name used in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            ColorTheme::GraphiteSignal => "graphite",
            ColorTheme::VioletFrost => "frost",
            ColorTheme::SmokedQuartz => "quartz",
            ColorTheme::SeaGlass => "seaglass",
            ColorTheme::AmberResin => "resin",
            ColorTheme::RosePetg => "rose",
            ColorTheme::Glacier => "glacier",
        }
    }

    pub fn from_key(key: &str) -> Option<ColorTheme> {
        ColorTheme::ALL.into_iter().find(|t| t.key() == key)
    }
}

/// Translucent glass over an OS blur, or opaque.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surface {
    Glass,
    #[default]
    Solid,
}

/// Whether the OS can blur what is behind a transparent window. iced 0.14
/// (winit) blurs on macOS and on KDE Plasma; elsewhere glass would show
/// the desktop unblurred, so the app uses the solid set instead.
pub fn blur_supported() -> bool {
    blur_supported_on(
        std::env::consts::OS,
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
    )
}

fn blur_supported_on(os: &str, desktop: Option<&str>) -> bool {
    match os {
        "macos" => true,
        "linux" => desktop.is_some_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("kde"))),
        _ => false,
    }
}

/// One colour theme in one mode: the glass set and its solid twin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeSet {
    pub glass: ThemeTokens,
    pub solid: ThemeTokens,
}

/// Tokens a colour theme may change: glass tint and the accent set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeTokens {
    pub ground: Color,
    pub surface: Color,
    pub tray: Color,
    pub rail: Color,
    pub rail_hi: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub accent_ink: Color,
    pub primary: Color,
    pub primary_hi: Color,
    pub on_primary: Color,
}

/// Neutral tokens that depend only on light/dark.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Neutral {
    pub edge: Color,
    pub rail_rule: Color,
    pub ink: Color,
    pub ink_2: Color,
    pub ink_3: Color,
    pub rule: Color,
    pub rule_soft: Color,
    pub on_rail: Color,
    pub on_rail_2: Color,
}

/// Status colours: fixed in every colour theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status {
    pub ok: Color,
    pub ok_soft: Color,
    pub fail: Color,
    pub fail_soft: Color,
    pub partial: Color,
    pub partial_soft: Color,
    pub offline: Color,
}

const NEUTRAL_LIGHT: Neutral = Neutral {
    edge: hex("#d9dfe5"),
    rail_rule: hex("#ffffff80"),
    ink: hex("#181d27"),
    ink_2: hex("#4a505b"),
    ink_3: hex("#636975"),
    rule: hex("#1b21301f"),
    rule_soft: hex("#1b21300f"),
    on_rail: hex("#12161f"),
    on_rail_2: hex("#3d424d"),
};

const NEUTRAL_DARK: Neutral = Neutral {
    edge: hex("#2d3038"),
    rail_rule: hex("#ffffff14"),
    ink: hex("#eceef4"),
    ink_2: hex("#babec6"),
    ink_3: hex("#9b9ea6"),
    rule: hex("#ffffff1a"),
    rule_soft: hex("#ffffff0f"),
    on_rail: hex("#eceef4"),
    on_rail_2: hex("#b4b7bf"),
};

const STATUS_LIGHT: Status = Status {
    ok: hex("#1a813f"),
    ok_soft: hex("#d9f3dd"),
    fail: hex("#c72e28"),
    fail_soft: hex("#ffe6e2"),
    partial: hex("#8e5e00"),
    partial_soft: hex("#feedc9"),
    offline: hex("#5c6b7a"),
};

const STATUS_DARK: Status = Status {
    ok: hex("#62c37a"),
    ok_soft: hex("#1a3520"),
    fail: hex("#fe6270"),
    fail_soft: hex("#572125"),
    partial: hex("#ebbd57"),
    partial_soft: hex("#46350d"),
    offline: hex("#8b9aab"),
};

impl ColorTheme {
    /// The theme-dependent tokens for `mode` and `surface`.
    pub fn tokens(self, mode: Mode, surface: Surface) -> ThemeTokens {
        let (light, dark) = presets::preset(self);
        let set = match mode {
            Mode::Light => light,
            Mode::Dark => dark,
        };
        match surface {
            Surface::Glass => set.glass,
            Surface::Solid => set.solid,
        }
    }
}

/// Every token for one colour theme in one mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub mode: Mode,
    pub surface: Surface,
    pub t: ThemeTokens,
    pub n: Neutral,
    pub status: Status,
}

impl Palette {
    pub fn new(theme: ColorTheme, mode: Mode, surface: Surface) -> Palette {
        let (mut n, status) = match mode {
            Mode::Light => (NEUTRAL_LIGHT, STATUS_LIGHT),
            Mode::Dark => (NEUTRAL_DARK, STATUS_DARK),
        };
        if surface == Surface::Glass {
            // The bright 1px edge of glass panes (DESIGN.md "Elevation").
            n.edge = match mode {
                Mode::Light => hex("#ffffffe6"),
                Mode::Dark => hex("#ffffff1a"),
            };
        }
        Palette {
            mode,
            surface,
            t: theme.tokens(mode, surface),
            n,
            status,
        }
    }

    /// The iced theme, so built-in widget defaults also fit the palette.
    pub fn iced_theme(&self) -> iced::Theme {
        iced::Theme::custom(
            "Printdex",
            iced::theme::Palette {
                background: self.t.ground,
                text: self.n.ink,
                primary: self.t.primary,
                success: self.status.ok,
                warning: self.status.partial,
                danger: self.status.fail,
            },
        )
    }

    // ---------- container styles ----------

    pub fn ground(&self) -> container::Style {
        container::Style {
            background: Some(self.t.ground.into()),
            text_color: Some(self.n.ink),
            ..container::Style::default()
        }
    }

    pub fn rail(&self) -> container::Style {
        container::Style {
            background: Some(self.t.rail.into()),
            text_color: Some(self.n.on_rail),
            border: Border {
                color: self.n.rule,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        }
    }

    /// Frosted bar: toolbar and status bar.
    pub fn bar(&self) -> container::Style {
        container::Style {
            background: Some(self.t.surface.into()),
            text_color: Some(self.n.ink_2),
            ..container::Style::default()
        }
    }

    /// Hairline divider.
    pub fn hairline(&self) -> container::Style {
        container::Style {
            background: Some(self.n.rule.into()),
            ..container::Style::default()
        }
    }

    /// Model pane; `selected` adds the Signal Wash and the 2px accent ring.
    pub fn pane(&self, selected: bool, hovered: bool) -> container::Style {
        let (bg, border) = if selected {
            (
                self.t.accent_soft,
                Border {
                    color: self.t.accent,
                    width: 2.0,
                    radius: RADIUS_PANE.into(),
                },
            )
        } else {
            (
                if hovered {
                    mix(self.t.surface, self.n.ink, 0.03)
                } else {
                    self.t.surface
                },
                Border {
                    color: self.n.edge,
                    width: 1.0,
                    radius: RADIUS_PANE.into(),
                },
            )
        };
        container::Style {
            background: Some(bg.into()),
            text_color: Some(self.n.ink),
            border,
            shadow: Shadow::default(),
            snap: true,
        }
    }

    /// A list-mode row; `selected` gets the Signal Wash.
    pub fn list_row(&self, selected: bool, hovered: bool) -> container::Style {
        let bg = if selected {
            Some(self.t.accent_soft.into())
        } else if hovered {
            Some(self.n.rule_soft.into())
        } else {
            None
        };
        container::Style {
            background: bg,
            text_color: Some(self.n.ink),
            ..container::Style::default()
        }
    }

    /// The tray behind a segmented control.
    pub fn segmented(&self) -> container::Style {
        container::Style {
            background: Some(self.n.rule_soft.into()),
            border: Border {
                color: self.n.rule,
                width: 1.0,
                radius: RADIUS_CONTROL.into(),
            },
            ..container::Style::default()
        }
    }

    /// Preview well inside a pane.
    pub fn well(&self) -> container::Style {
        container::Style {
            background: Some(self.t.tray.into()),
            border: Border {
                radius: RADIUS_WELL.into(),
                ..Border::default()
            },
            ..container::Style::default()
        }
    }

    /// Recipe/count chip.
    pub fn chip(&self) -> container::Style {
        container::Style {
            background: Some(self.n.rule_soft.into()),
            text_color: Some(self.n.ink_2),
            border: Border {
                radius: RADIUS_CHIP.into(),
                ..Border::default()
            },
            ..container::Style::default()
        }
    }

    /// Inline error line.
    pub fn error_line(&self) -> container::Style {
        container::Style {
            background: Some(self.status.fail_soft.into()),
            text_color: Some(self.status.fail),
            border: Border {
                color: self.status.fail,
                width: 1.0,
                radius: RADIUS_CONTROL.into(),
            },
            ..container::Style::default()
        }
    }

    /// Scan badge: Signal Ink on Signal Wash.
    pub fn scan_badge(&self) -> container::Style {
        container::Style {
            background: Some(self.t.accent_soft.into()),
            text_color: Some(self.t.accent_ink),
            border: Border {
                radius: RADIUS_CONTROL.into(),
                ..Border::default()
            },
            ..container::Style::default()
        }
    }

    // ---------- buttons ----------

    pub fn button_primary(&self, status: button::Status) -> button::Style {
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => self.t.primary_hi,
            _ => self.t.primary,
        };
        button::Style {
            background: Some(bg.into()),
            text_color: self.t.on_primary,
            border: Border {
                radius: RADIUS_CONTROL.into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn button_secondary(&self, status: button::Status) -> button::Style {
        let (bg, fg, border) = match status {
            button::Status::Pressed => (self.t.accent_soft, self.t.accent_ink, self.t.accent),
            button::Status::Hovered => (
                mix(self.t.surface, self.n.ink, 0.06),
                self.n.ink,
                self.n.rule,
            ),
            button::Status::Disabled => (self.t.surface, self.n.ink_3, self.n.rule_soft),
            button::Status::Active => (self.t.surface, self.n.ink, self.n.rule),
        };
        button::Style {
            background: Some(bg.into()),
            text_color: fg,
            border: Border {
                color: border,
                width: 1.0,
                radius: RADIUS_CONTROL.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    /// One segment of a segmented control.
    pub fn segment(&self, active: bool, status: button::Status) -> button::Style {
        let (bg, fg) = if active {
            (Some(self.t.accent_soft.into()), self.t.accent_ink)
        } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            (Some(self.n.rule_soft.into()), self.n.ink)
        } else {
            (None, self.n.ink_2)
        };
        button::Style {
            background: bg,
            text_color: fg,
            border: Border {
                radius: (RADIUS_CONTROL - 3.0).into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    /// The sort pick list: a secondary control.
    pub fn pick(&self, status: pick_list::Status) -> pick_list::Style {
        let hovered = matches!(status, pick_list::Status::Hovered);
        pick_list::Style {
            text_color: self.n.ink,
            placeholder_color: self.n.ink_3,
            handle_color: self.n.ink_3,
            background: Background::Color(if hovered {
                mix(self.t.surface, self.n.ink, 0.06)
            } else {
                self.t.surface
            }),
            border: Border {
                color: if matches!(status, pick_list::Status::Opened { .. }) {
                    self.t.accent
                } else {
                    self.n.rule
                },
                width: 1.0,
                radius: RADIUS_CONTROL.into(),
            },
        }
    }

    /// The sort pick list's open menu.
    pub fn pick_menu(&self) -> menu::Style {
        menu::Style {
            // Menus float over content: always opaque.
            background: Background::Color(Color {
                a: 1.0,
                ..self.t.surface
            }),
            border: Border {
                color: self.n.edge,
                width: 1.0,
                radius: RADIUS_CONTROL.into(),
            },
            text_color: self.n.ink,
            selected_text_color: self.t.accent_ink,
            selected_background: Background::Color(self.t.accent_soft),
            shadow: Shadow::default(),
        }
    }

    /// Ghost button: text only until hovered.
    pub fn button_ghost(&self, status: button::Status) -> button::Style {
        let (bg, fg) = match status {
            button::Status::Hovered => (Some(self.n.rule_soft.into()), self.n.ink),
            button::Status::Pressed => (Some(self.n.rule.into()), self.n.ink),
            button::Status::Disabled => (None, self.n.ink_3),
            button::Status::Active => (None, self.n.ink_2),
        };
        button::Style {
            background: bg,
            text_color: fg,
            border: Border {
                radius: RADIUS_CONTROL.into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    /// Sidebar row; `active` is the current scope.
    pub fn rail_item(&self, active: bool, status: button::Status) -> button::Style {
        let bg = if active {
            Some(self.t.accent_soft.into())
        } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            Some(self.t.rail_hi.into())
        } else {
            None
        };
        button::Style {
            background: bg,
            text_color: self.n.on_rail,
            border: Border {
                radius: RADIUS_CONTROL.into(),
                ..Border::default()
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    /// Invisible button wrapping a model pane (the pane draws itself).
    pub fn bare(&self) -> button::Style {
        button::Style {
            background: None,
            text_color: self.n.ink,
            border: Border::default(),
            shadow: Shadow::default(),
            snap: true,
        }
    }

    // ---------- inputs ----------

    pub fn search_field(&self, status: text_input::Status) -> text_input::Style {
        let focused = matches!(status, text_input::Status::Focused { .. });
        text_input::Style {
            background: Background::Color(self.t.ground),
            border: Border {
                color: if focused { self.t.accent } else { self.n.rule },
                width: if focused { 2.0 } else { 1.0 },
                radius: RADIUS_CONTROL.into(),
            },
            icon: self.n.ink_3,
            placeholder: self.n.ink_3,
            value: self.n.ink,
            selection: self.t.accent_soft,
        }
    }

    pub fn scrollbar(&self, status: scrollable::Status) -> scrollable::Style {
        let active = matches!(
            status,
            scrollable::Status::Hovered { .. } | scrollable::Status::Dragged { .. }
        );
        let rail = scrollable::Rail {
            background: None,
            border: Border::default(),
            scroller: scrollable::Scroller {
                background: Background::Color(if active { self.n.ink_3 } else { self.n.rule }),
                border: Border {
                    radius: RADIUS_CHIP.into(),
                    ..Border::default()
                },
            },
        };
        scrollable::Style {
            container: container::Style::default(),
            vertical_rail: rail,
            horizontal_rail: rail,
            gap: None,
            auto_scroll: scrollable::AutoScroll {
                background: Background::Color(self.t.surface),
                border: Border::default(),
                shadow: Shadow::default(),
                icon: self.n.ink_2,
            },
        }
    }
}

/// Mixes `amount` of `b` into `a` (alpha of `a` kept).
pub fn mix(a: Color, b: Color, amount: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * amount,
        g: a.g + (b.g - a.g) * amount,
        b: a.b + (b.b - a.b) * amount,
        a: a.a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_rgb_and_rgba() {
        assert_eq!(hex("#9f8600"), Color::from_rgba8(0x9f, 0x86, 0x00, 1.0));
        let c = hex("#f7edb2cc");
        assert_eq!(c, Color::from_rgba8(0xf7, 0xed, 0xb2, 204.0 / 255.0));
        assert_eq!(hex("#FFFFFF"), Color::WHITE);
    }

    #[test]
    fn solid_set_is_opaque() {
        for mode in [Mode::Light, Mode::Dark] {
            let p = Palette::new(ColorTheme::GraphiteSignal, mode, Surface::Solid);
            for c in [
                p.t.ground,
                p.t.surface,
                p.t.tray,
                p.t.rail,
                p.t.rail_hi,
                p.n.edge,
            ] {
                assert!(
                    (c.a - 1.0).abs() < f32::EPSILON,
                    "{mode:?}: {c:?} not opaque"
                );
            }
        }
    }

    #[test]
    fn mode_switches_palette() {
        let light = Palette::new(ColorTheme::GraphiteSignal, Mode::Light, Surface::Solid);
        let dark = Palette::new(ColorTheme::GraphiteSignal, Mode::Dark, Surface::Solid);
        assert_eq!(light.n.ink, hex("#181d27"));
        assert_eq!(dark.n.ink, hex("#eceef4"));
        assert_eq!(dark.t.accent, hex("#fadf27"));
        assert_ne!(light.t.ground, dark.t.ground);
    }

    #[test]
    fn status_colours_do_not_depend_on_colour_theme() {
        for mode in [Mode::Light, Mode::Dark] {
            let p = Palette::new(ColorTheme::default(), mode, Surface::Solid);
            let expected = if mode == Mode::Light {
                STATUS_LIGHT
            } else {
                STATUS_DARK
            };
            assert_eq!(p.status, expected);
        }
    }

    #[test]
    fn blur_only_where_the_os_provides_it() {
        assert!(blur_supported_on("macos", None));
        assert!(blur_supported_on("linux", Some("KDE")));
        assert!(blur_supported_on("linux", Some("ubuntu:KDE")));
        assert!(!blur_supported_on("linux", Some("GNOME")));
        assert!(!blur_supported_on("linux", None));
        assert!(!blur_supported_on("windows", None));
    }

    #[test]
    fn seven_presets_with_unique_keys() {
        assert_eq!(ColorTheme::ALL.len(), 7);
        for t in ColorTheme::ALL {
            assert_eq!(ColorTheme::from_key(t.key()), Some(t));
        }
        assert_eq!(ColorTheme::from_key("nope"), None);
    }

    #[test]
    fn every_preset_has_opaque_solid_twins() {
        for t in ColorTheme::ALL {
            for mode in [Mode::Light, Mode::Dark] {
                let s = t.tokens(mode, Surface::Solid);
                for c in [
                    s.ground,
                    s.surface,
                    s.tray,
                    s.rail,
                    s.rail_hi,
                    s.accent_soft,
                ] {
                    assert!((c.a - 1.0).abs() < 0.01, "{t:?} {mode:?}: {c:?}");
                }
            }
        }
    }

    #[test]
    fn glass_alphas_match_design() {
        let near = |c: Color, a: f32| (c.a - a).abs() < 0.01;
        for t in ColorTheme::ALL {
            let l = t.tokens(Mode::Light, Surface::Glass);
            let d = t.tokens(Mode::Dark, Surface::Glass);
            assert!(near(l.rail, 0.82) && near(d.rail, 0.82), "{t:?} rail");
            assert!(near(l.ground, 0.86) && near(d.ground, 0.84), "{t:?} ground");
        }
    }

    #[test]
    fn presets_never_change_status_colours() {
        for t in ColorTheme::ALL {
            for mode in [Mode::Light, Mode::Dark] {
                let a = Palette::new(t, mode, Surface::Glass).status;
                let b = Palette::new(ColorTheme::default(), mode, Surface::Solid).status;
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn accent_text_contrast_holds_in_every_preset() {
        for t in ColorTheme::ALL {
            for mode in [Mode::Light, Mode::Dark] {
                let p = Palette::new(t, mode, Surface::Solid);
                let c = contrast(p.t.accent_ink, p.t.accent_soft);
                assert!(c >= 4.5, "{t:?} {mode:?}: {c}");
                assert!(
                    contrast(p.n.on_rail, p.t.rail) >= 4.5,
                    "{t:?} {mode:?} rail"
                );
            }
        }
    }

    #[test]
    fn text_contrast_on_panes_is_readable() {
        // WCAG AA for body text (4.5:1) on the pane and rail fills.
        for mode in [Mode::Light, Mode::Dark] {
            let p = Palette::new(ColorTheme::default(), mode, Surface::Solid);
            assert!(contrast(p.n.ink, p.t.surface) >= 4.5);
            assert!(contrast(p.n.ink_2, p.t.surface) >= 4.5);
            assert!(contrast(p.n.on_rail, p.t.rail) >= 4.5);
            assert!(contrast(p.t.accent_ink, p.t.accent_soft) >= 4.5);
        }
    }

    fn contrast(a: Color, b: Color) -> f32 {
        fn lum(c: Color) -> f32 {
            let ch = |v: f32| {
                if v <= 0.040_45 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b)
        }
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }
}
