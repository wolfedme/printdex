//! Settings: libraries, appearance, about.

use iced::widget::{
    Space, button, column, container, row, scrollable, text, text_input, toggler, tooltip,
};
use iced::{Alignment, Element, Length};
use printdex_core::{Library, ScanPhase};

use super::widgets::{self, icon_button, segmented};
use crate::app::{App, Message, SettingsSection, rename_id};
use crate::format::{group, size};
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::org::Org;
use crate::settings::{CACHE_LIMITS_MB, Settings, ThemeMode, Translucency};
use crate::theme::{self, Badge, ColorTheme, Palette, Surface};

const SECTION_LIST_WIDTH: f32 = 200.0;
const CONTENT_MAX: f32 = 760.0;
const LOCATION_WIDTH: f32 = 200.0;
const STATUS_WIDTH: f32 = 140.0;
const MODELS_WIDTH: f32 = 70.0;

pub fn theme_name(theme: ColorTheme) -> String {
    match theme {
        ColorTheme::GraphiteSignal => t("theme-graphite"),
        ColorTheme::VioletFrost => t("theme-frost"),
        ColorTheme::SmokedQuartz => t("theme-quartz"),
        ColorTheme::SeaGlass => t("theme-seaglass"),
        ColorTheme::AmberResin => t("theme-resin"),
        ColorTheme::RosePetg => t("theme-rose"),
        ColorTheme::Glacier => t("theme-glacier"),
    }
}

pub fn view(app: &App, p: Palette) -> Element<'_, Message> {
    let mut list = column![
        container(
            text(t("settings-title"))
                .size(theme::SIZE_TITLE)
                .font(theme::FONT_BOLD)
                .color(p.n.ink)
        )
        .padding([theme::S4, theme::S4]),
    ]
    .spacing(theme::S1);
    for (section, icon, label) in [
        (
            SettingsSection::Libraries,
            Icon::Library,
            t("settings-libraries"),
        ),
        (
            SettingsSection::Appearance,
            Icon::Settings,
            t("settings-appearance"),
        ),
        (
            SettingsSection::Storage,
            Icon::Layers,
            t("settings-storage"),
        ),
        (SettingsSection::About, Icon::FileText, t("settings-about")),
    ] {
        let active = app.settings_section == section;
        list = list.push(
            button(
                row![
                    icons::icon(
                        icon,
                        theme::ICON,
                        if active { p.t.accent_ink } else { p.n.ink_2 }
                    ),
                    text(label).size(theme::SIZE_BODY).font(if active {
                        theme::FONT_STRONG
                    } else {
                        theme::FONT_BODY
                    }),
                ]
                .spacing(7)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .height(theme::RAIL_ITEM_HEIGHT + 4.0)
            .padding([6, 8])
            .style(move |_, status| p.rail_item(active, status))
            .on_press(Message::ShowSection(section)),
        );
    }

    let content = match app.settings_section {
        SettingsSection::Libraries => libraries(app, p),
        SettingsSection::Appearance => appearance(app, p),
        SettingsSection::Storage => storage(app, p),
        SettingsSection::About => about(app, p),
    };

    row![
        container(list)
            .width(SECTION_LIST_WIDTH)
            .height(Length::Fill)
            .padding([theme::S6, theme::S4])
            .style(move |_| p.bar()),
        container(Space::new().width(1).height(Length::Fill)).style(move |_| p.hairline()),
        container(
            scrollable(
                container(content)
                    .max_width(CONTENT_MAX + 2.0 * theme::S9)
                    .padding([theme::S8, theme::S9])
            )
            .height(Length::Fill)
            .style(move |_, status| p.scrollbar(status))
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| p.ground()),
    ]
    .into()
}

fn heading<'a>(p: Palette, title: String, body: String) -> Element<'a, Message> {
    column![
        text(title)
            .size(theme::SIZE_TITLE)
            .font(theme::FONT_BOLD)
            .color(p.n.ink),
        text(body).size(theme::SIZE_BODY).color(p.n.ink_2),
    ]
    .spacing(theme::S2)
    .into()
}

fn col_label<'a>(p: Palette, label: String) -> iced::widget::Text<'a> {
    text(label.to_uppercase())
        .size(theme::SIZE_GROUP)
        .font(theme::FONT_STRONG)
        .color(p.n.ink_3)
}

fn libraries(app: &App, p: Palette) -> Element<'_, Message> {
    let mut table = column![
        container(
            row![
                col_label(p, t("settings-col-library")).width(Length::Fill),
                col_label(p, t("settings-col-location")).width(LOCATION_WIDTH),
                col_label(p, t("settings-col-status")).width(STATUS_WIDTH),
                col_label(p, t("settings-col-models"))
                    .width(MODELS_WIDTH)
                    .align_x(Alignment::End),
                Space::new().width(ACTIONS_WIDTH),
            ]
            .spacing(theme::S5)
            .align_y(Alignment::Center)
        )
        .padding([theme::S4, theme::S5]),
    ];
    for lib in &app.libraries {
        table = table.push(widgets::hairline(p));
        table = table.push(library_row(app, p, lib));
        if app.confirm_remove == Some(lib.id) {
            table = table.push(confirm_row(p, lib));
        }
    }

    column![
        heading(p, t("settings-libraries"), t("settings-libraries-body")),
        container(table)
            .clip(true)
            .style(move |_| p.pane(false, false)),
        icon_button(
            p,
            Icon::Plus,
            t("sidebar-add-library"),
            Palette::button_secondary,
            Some(Message::PickLibrary)
        ),
        sidecars(app, p),
    ]
    .spacing(theme::S6)
    .into()
}

/// Per library: write `.printdex.toml` sidecars next to models.
fn sidecars(app: &App, p: Palette) -> Element<'_, Message> {
    let mut col = column![
        column![
            text(t("settings-sidecars"))
                .size(theme::SIZE_SECTION)
                .font(theme::FONT_BOLD)
                .color(p.n.ink),
            text(t("settings-sidecars-body"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_2),
        ]
        .spacing(theme::S2),
        widgets::hairline(p),
    ]
    .spacing(theme::S5);
    for lib in &app.libraries {
        let id = lib.id;
        let control = toggler(lib.sidecar_enabled)
            .size(20)
            .style(move |_, status| p.toggler(status))
            .on_toggle(move |on| Message::Org(Org::SetSidecars(id, on)));
        col = col.push(setting(
            p,
            lib.display_name.clone(),
            if lib.sidecar_enabled {
                t("settings-sidecars-on")
            } else {
                t("settings-sidecars-off")
            },
            control.into(),
        ));
    }
    col.into()
}

const ACTIONS_WIDTH: f32 = 150.0;

fn library_row<'a>(app: &'a App, p: Palette, lib: &'a Library) -> Element<'a, Message> {
    let name: Element<'a, Message> = match &app.renaming {
        Some((id, current)) if *id == lib.id => text_input("", current)
            .id(rename_id())
            .on_input(Message::RenameInput)
            .on_submit(Message::CommitRename)
            .size(theme::SIZE_BODY)
            .padding([5.0, theme::S4])
            .style(move |_, status| p.search_field(status))
            .into(),
        _ => text(lib.display_name.clone())
            .size(theme::SIZE_BODY)
            .font(theme::FONT_STRONG)
            .wrapping(text::Wrapping::None)
            .into(),
    };

    let offline = app.offline.get(&lib.id);
    let (badge, label) = match app.scans.get(&lib.id) {
        _ if offline.is_some() => (
            Badge::Offline,
            t_args(
                "settings-status-offline",
                &[("time", Arg::str(offline.cloned().unwrap_or_default()))],
            ),
        ),
        Some(scan) if scan.phase == ScanPhase::Walking || scan.total == 0 => {
            (Badge::Busy, t("settings-status-listing"))
        }
        Some(scan) => (
            Badge::Busy,
            t_args(
                "settings-status-scanning",
                &[
                    ("done", Arg::str(group(scan.done))),
                    ("total", Arg::str(group(scan.total))),
                ],
            ),
        ),
        None => (Badge::Ok, t("settings-status-ok")),
    };
    let badge_icon = match badge {
        Badge::Ok => Icon::Check,
        Badge::Busy => Icon::Refresh,
        Badge::Offline => Icon::Offline,
    };
    let fg = match badge {
        Badge::Ok => p.status.ok,
        Badge::Busy => p.t.accent_ink,
        Badge::Offline => p.status.offline,
    };
    let status = container(
        row![
            icons::icon(badge_icon, theme::ICON_CHIP, fg),
            text(label)
                .size(theme::SIZE_CAPTION)
                .font(theme::FONT_MEDIUM),
        ]
        .spacing(theme::S2)
        .align_y(Alignment::Center),
    )
    .padding([2.0, theme::S3])
    .style(move |_| p.badge(badge));

    let renaming = matches!(&app.renaming, Some((id, _)) if *id == lib.id);
    let actions: Element<'a, Message> = if renaming {
        row![
            small_button(p, t("settings-save"), Message::CommitRename),
            small_button(p, t("settings-cancel"), Message::CancelRename),
        ]
        .spacing(theme::S2)
        .into()
    } else {
        let scanning = app.scans.contains_key(&lib.id);
        row![
            icon_action(
                p,
                Icon::Refresh,
                if offline.is_some() {
                    t("settings-retry")
                } else {
                    t("settings-rescan")
                },
                (!scanning).then_some(Message::RescanLibrary(lib.id))
            ),
            icon_only(
                p,
                Icon::Rename,
                t("settings-rename"),
                Some(Message::StartRename(lib.id))
            ),
            icon_only(
                p,
                Icon::Remove,
                t("settings-remove"),
                (!scanning).then_some(Message::AskRemove(lib.id))
            ),
        ]
        .spacing(theme::S1)
        .into()
    };

    container(
        row![
            row![
                icons::icon(
                    if offline.is_some() {
                        Icon::Offline
                    } else {
                        Icon::Folder
                    },
                    theme::ICON,
                    p.n.ink_2
                ),
                container(name).width(Length::Fill),
            ]
            .spacing(theme::S4)
            .align_y(Alignment::Center)
            .width(Length::Fill),
            text(lib.path.display().to_string())
                .size(theme::SIZE_DATA)
                .font(theme::FONT_DATA)
                .color(p.n.ink_2)
                .wrapping(text::Wrapping::None)
                .width(LOCATION_WIDTH),
            container(status).width(STATUS_WIDTH),
            text(group(app.counts.get(&lib.id).copied().unwrap_or(0)))
                .size(theme::SIZE_BODY)
                .font(theme::FONT_MEDIUM)
                .width(MODELS_WIDTH)
                .align_x(Alignment::End),
            container(actions)
                .width(ACTIONS_WIDTH)
                .align_x(Alignment::End),
        ]
        .spacing(theme::S5)
        .align_y(Alignment::Center),
    )
    .padding([theme::S4, theme::S5])
    .height(48)
    .align_y(Alignment::Center)
    .clip(true)
    .into()
}

fn icon_action<'a>(
    p: Palette,
    icon: Icon,
    label: String,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let fg = if on_press.is_some() {
        p.n.ink_2
    } else {
        p.n.ink_3
    };
    button(
        row![
            icons::icon(icon, theme::ICON_DENSE, fg),
            text(label).size(theme::SIZE_CAPTION),
        ]
        .spacing(theme::S2)
        .align_y(Alignment::Center),
    )
    .padding([4.0, theme::S3])
    .style(move |_, status| p.button_ghost(status))
    .on_press_maybe(on_press)
    .into()
}

/// An icon button whose label shows as a tooltip.
fn icon_only<'a>(
    p: Palette,
    icon: Icon,
    label: String,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let fg = if on_press.is_some() {
        p.n.ink_2
    } else {
        p.n.ink_3
    };
    tooltip(
        button(icons::icon(icon, theme::ICON_DENSE, fg))
            .padding([5.0, theme::S3])
            .style(move |_, status| p.button_ghost(status))
            .on_press_maybe(on_press),
        container(text(label).size(theme::SIZE_CAPTION))
            .padding([3.0, theme::S3])
            .style(move |_| p.tooltip()),
        tooltip::Position::Top,
    )
    .into()
}

fn small_button<'a>(p: Palette, label: String, msg: Message) -> Element<'a, Message> {
    button(text(label).size(theme::SIZE_CAPTION))
        .padding([4.0, theme::S4])
        .style(move |_, status| p.button_secondary(status))
        .on_press(msg)
        .into()
}

fn confirm_row<'a>(p: Palette, lib: &Library) -> Element<'a, Message> {
    container(
        row![
            icons::icon(Icon::Alert, theme::ICON, p.status.fail),
            text(t_args(
                "settings-remove-confirm",
                &[("name", Arg::str(lib.display_name.clone()))]
            ))
            .size(theme::SIZE_BODY)
            .color(p.n.ink)
            .width(Length::Fill),
            button(text(t("settings-cancel")).size(theme::SIZE_CAPTION))
                .padding([5.0, theme::S4])
                .style(move |_, status| p.button_secondary(status))
                .on_press(Message::CancelRemove),
            button(
                text(t("settings-remove-yes"))
                    .size(theme::SIZE_CAPTION)
                    .font(theme::FONT_STRONG)
            )
            .padding([5.0, theme::S4])
            .style(move |_, status| p.button_danger(status))
            .on_press(Message::ConfirmRemove(lib.id)),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center),
    )
    .padding([theme::S4, theme::S5])
    .style(move |_| p.error_line())
    .into()
}

/// One labelled setting: title and explanation left, control right.
fn setting<'a>(
    p: Palette,
    title: String,
    body: String,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    column![
        row![
            column![
                text(title)
                    .size(theme::SIZE_BODY)
                    .font(theme::FONT_STRONG)
                    .color(p.n.ink),
                text(body).size(theme::SIZE_CAPTION).color(p.n.ink_2),
            ]
            .spacing(theme::S1)
            .width(Length::Fill),
            control,
        ]
        .spacing(theme::S6)
        .align_y(Alignment::Center),
        Space::new().height(theme::S5),
        widgets::hairline(p),
    ]
    .into()
}

fn appearance(app: &App, p: Palette) -> Element<'_, Message> {
    let s = app.settings;

    // Colour theme chips with a two-tone swatch (tint and accent).
    let mut chips = row![].spacing(theme::S3);
    for ct in ColorTheme::ALL {
        let tokens = ct.tokens(app.mode(), Surface::Solid);
        let active = s.color_theme == ct;
        let swatch = row![
            container(Space::new().width(7).height(14)).style(move |_| p.swatch(tokens.tray)),
            container(Space::new().width(7).height(14)).style(move |_| p.swatch(tokens.accent)),
        ];
        chips = chips.push(
            button(
                row![
                    swatch,
                    text(theme_name(ct))
                        .size(theme::SIZE_CAPTION)
                        .font(if active {
                            theme::FONT_STRONG
                        } else {
                            theme::FONT_BODY
                        }),
                ]
                .spacing(theme::S3)
                .align_y(Alignment::Center),
            )
            .padding([5.0, theme::S4])
            .style(move |_, status| {
                if active {
                    p.segment(true, status)
                } else {
                    p.button_secondary(status)
                }
            })
            .on_press(Message::SettingsChanged(Settings {
                color_theme: ct,
                ..s
            })),
        );
    }

    let mode_label = |m: ThemeMode| match m {
        ThemeMode::System => t("settings-mode-system"),
        ThemeMode::Light => t("settings-mode-light"),
        ThemeMode::Dark => t("settings-mode-dark"),
    };
    let modes = segmented(
        p,
        &ThemeMode::ALL.map(|m| (m, None, Some(mode_label(m)))),
        s.mode,
        move |mode| Message::SettingsChanged(Settings { mode, ..s }),
    );

    let blur = theme::blur_supported();
    let translucency_note = if !blur {
        t("settings-translucency-unsupported")
    } else if app.options.glass_window != (s.translucency == Translucency::Auto) {
        t("settings-translucency-restart")
    } else {
        t("settings-translucency-body")
    };
    let translucency = segmented(
        p,
        &[
            (
                Translucency::Auto,
                None,
                Some(t("settings-translucency-auto")),
            ),
            (
                Translucency::Off,
                None,
                Some(t("settings-translucency-off")),
            ),
        ],
        s.translucency,
        move |translucency| Message::SettingsChanged(Settings { translucency, ..s }),
    );

    let motion = toggler(s.reduce_motion)
        .size(20)
        .style(move |_, status| p.toggler(status))
        .on_toggle(move |reduce_motion| Message::SettingsChanged(Settings { reduce_motion, ..s }));

    column![
        text(t("settings-appearance"))
            .size(theme::SIZE_TITLE)
            .font(theme::FONT_BOLD)
            .color(p.n.ink),
        column![
            text(t("settings-color-theme"))
                .size(theme::SIZE_BODY)
                .font(theme::FONT_STRONG)
                .color(p.n.ink),
            text(t("settings-color-theme-body"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_2),
            Space::new().height(theme::S3),
            chips.wrap().vertical_spacing(theme::S3),
            Space::new().height(theme::S4),
            widgets::hairline(p),
        ]
        .spacing(theme::S1),
        setting(p, t("settings-mode"), t("settings-mode-body"), modes),
        setting(
            p,
            t("settings-translucency"),
            translucency_note,
            translucency
        ),
        setting(
            p,
            t("settings-reduce-motion"),
            t("settings-reduce-motion-body"),
            motion.into()
        ),
    ]
    .spacing(theme::S6)
    .into()
}

/// "250 MB", "1 GB": round limits without decimals.
fn limit_label(mb: u32) -> String {
    if mb >= 1000 && mb.is_multiple_of(1000) {
        format!("{} GB", mb / 1000)
    } else {
        format!("{mb} MB")
    }
}

fn storage(app: &App, p: Palette) -> Element<'_, Message> {
    let s = app.settings;
    let limits = segmented(
        p,
        &CACHE_LIMITS_MB.map(|mb| (mb, None, Some(limit_label(mb)))),
        s.cache_mb,
        move |cache_mb| Message::SettingsChanged(Settings { cache_mb, ..s }),
    );
    let usage = match app.cache_usage {
        Some(bytes) => t_args(
            "settings-cache-usage",
            &[
                ("used", Arg::str(size(bytes))),
                ("limit", Arg::str(limit_label(s.cache_mb))),
            ],
        ),
        None => t("settings-cache-measuring"),
    };
    let folder = app
        .engine
        .as_ref()
        .and_then(|e| {
            e.thumbnails()
                .cache_dir()
                .parent()
                .map(|d| d.display().to_string())
        })
        .unwrap_or_default();
    let clear = button(text(t("settings-cache-clear")).size(theme::SIZE_BODY))
        .height(theme::CONTROL_HEIGHT)
        .padding([8.0, theme::S5])
        .style(move |_, status| p.button_secondary(status))
        .on_press_maybe(
            (!app.clearing_cache && app.cache_usage.is_some_and(|b| b > 0))
                .then_some(Message::ClearCache),
        );
    column![
        text(t("settings-storage"))
            .size(theme::SIZE_TITLE)
            .font(theme::FONT_BOLD)
            .color(p.n.ink),
        setting(
            p,
            t("settings-cache-limit"),
            t("settings-cache-limit-body"),
            limits
        ),
        setting(p, t("settings-cache"), usage, clear.into()),
        text(folder)
            .size(theme::SIZE_DATA)
            .font(theme::FONT_DATA)
            .color(p.n.ink_3),
    ]
    .spacing(theme::S6)
    .into()
}

fn about(app: &App, p: Palette) -> Element<'_, Message> {
    let line = |label: String, value: String, mono: bool| -> Element<'_, Message> {
        row![
            text(label)
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_3)
                .width(140),
            text(value)
                .size(if mono {
                    theme::SIZE_DATA
                } else {
                    theme::SIZE_BODY
                })
                .font(if mono {
                    theme::FONT_DATA
                } else {
                    theme::FONT_MEDIUM
                })
                .color(p.n.ink),
        ]
        .spacing(theme::S4)
        .align_y(Alignment::Center)
        .into()
    };
    let config = app
        .options
        .config_dir
        .as_ref()
        .map(|d| d.join(crate::settings::FILE_NAME).display().to_string())
        .unwrap_or_default();
    column![
        text(t("settings-about"))
            .size(theme::SIZE_TITLE)
            .font(theme::FONT_BOLD)
            .color(p.n.ink),
        container(
            column![
                line(
                    t("settings-version"),
                    env!("CARGO_PKG_VERSION").to_owned(),
                    true
                ),
                line(
                    t("settings-data-folder"),
                    app.options.data_dir.display().to_string(),
                    true
                ),
                line(t("settings-config-folder"), config, true),
                line(t("settings-licence"), t("settings-licence-value"), false),
            ]
            .spacing(theme::S5)
        )
        .padding(theme::S6)
        .width(Length::Fill)
        .style(move |_| p.pane(false, false)),
    ]
    .spacing(theme::S6)
    .into()
}
