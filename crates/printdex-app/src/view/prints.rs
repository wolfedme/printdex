//! Recipe card, print log and the "Log a print" form (DESIGN.md "Recipe
//! card and print log").

use iced::widget::{
    Space, button, column, container, image, mouse_area, row, text, text_editor, text_input,
};
use iced::{Alignment, Color, Element, Length, Padding, keyboard};
use printdex_core::{
    Adhesion, Attempt, AttemptResult, Material, ModelData, PrintSettings, Supports,
};

use super::widgets::{self, date, ghost_icon, segmented};
use crate::app::{App, Message};
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::log_form::{Field, FormMode, LogForm, REQUIRABLE, format_duration};
use crate::prints::{Input, Pr, SHOWN_ATTEMPTS, has_error, note_input_id, printer_input_id};
use crate::theme::{self, Palette};

const PHOTO: f32 = 56.0;
const NOTES_HEIGHT: f32 = 84.0;

fn pr(m: Pr) -> Message {
    Message::Prints(m)
}

/// Icon, word and colour for an outcome.
pub fn outcome(p: Palette, r: AttemptResult) -> (Icon, String, Color) {
    match r {
        AttemptResult::Success => (Icon::Success, t("outcome-success"), p.status.ok),
        AttemptResult::Partial => (Icon::Alert, t("outcome-partial"), p.status.partial),
        AttemptResult::Failed => (Icon::Failed, t("outcome-failed"), p.status.fail),
    }
}

/// "0.20 mm" for a layer height.
pub fn layer_text(v: f32) -> String {
    format!("{v:.2} mm")
}

fn supports_text(s: Supports) -> String {
    match s {
        Supports::None => t("supports-none"),
        Supports::Normal => t("supports-normal"),
        Supports::Tree => t("supports-tree"),
    }
}

fn adhesion_text(a: Adhesion) -> String {
    match a {
        Adhesion::None => t("adhesion-none"),
        Adhesion::Skirt => t("adhesion-skirt"),
        Adhesion::Brim => t("adhesion-brim"),
        Adhesion::Raft => t("adhesion-raft"),
    }
}

/// A small data chip: words in Figtree, numbers in mono.
pub fn chip<'a>(p: Palette, s: String, mono: bool) -> Element<'a, Message> {
    container(
        text(s)
            .size(theme::SIZE_DATA)
            .font(if mono {
                theme::FONT_DATA
            } else {
                theme::FONT_MEDIUM
            })
            .color(p.n.ink_2)
            .wrapping(text::Wrapping::None),
    )
    .padding([1.0, 6.0])
    .style(move |_| p.chip())
    .into()
}

/// Chips for the settings of an attempt.
fn setting_chips<'a>(p: Palette, s: &PrintSettings, duration: Option<u32>) -> Element<'a, Message> {
    let mut r = row![].spacing(theme::S2);
    if let Some(m) = &s.material {
        r = r.push(chip(p, m.label().to_owned(), false));
    }
    if let Some(l) = s.layer_mm {
        r = r.push(chip(p, layer_text(l), true));
    }
    match (s.bed_temp, s.nozzle_temp) {
        (Some(b), Some(h)) => r = r.push(chip(p, format!("{b} / {h} °C"), true)),
        (Some(b), None) => r = r.push(chip(p, format!("{b} °C"), true)),
        (None, Some(h)) => r = r.push(chip(p, format!("{h} °C"), true)),
        (None, None) => {}
    }
    if let Some(sup) = s.supports {
        r = r.push(chip(p, supports_text(sup), false));
    }
    if let Some(d) = duration {
        r = r.push(chip(p, format_duration(d), true));
    }
    r.wrap().vertical_spacing(theme::S2).into()
}

/// The recipe values shown as a grid: (label, value, value is a number).
fn recipe_rows(s: &PrintSettings) -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    let mut push = |label: &str, v: Option<String>, mono: bool| {
        if let Some(v) = v {
            out.push((t(label), v, mono));
        }
    };
    push(
        "form-material",
        s.material.as_ref().map(|m| m.label().to_owned()),
        false,
    );
    push("form-nozzle", s.nozzle_mm.map(|v| format!("{v} mm")), true);
    push("form-layer", s.layer_mm.map(layer_text), true);
    push("form-bed", s.bed_temp.map(|v| format!("{v} °C")), true);
    push(
        "form-hotend",
        s.nozzle_temp.map(|v| format!("{v} °C")),
        true,
    );
    push("form-infill", s.infill_pct.map(|v| format!("{v} %")), true);
    push("form-walls", s.walls.map(|v| v.to_string()), true);
    push("form-supports", s.supports.map(supports_text), false);
    push("form-adhesion", s.adhesion.map(adhesion_text), false);
    push("form-filament", s.filament.clone(), false);
    push("form-printer", s.printer.clone(), false);
    push("recipe-orientation", s.orientation.clone(), false);
    out
}

fn section_head<'a>(
    p: Palette,
    icon: Icon,
    title: String,
    caption: Option<String>,
) -> iced::widget::Row<'a, Message> {
    let mut r = row![
        icons::icon(icon, theme::ICON, p.n.ink_2),
        text(title)
            .size(theme::SIZE_SECTION)
            .font(theme::FONT_BOLD)
            .color(p.n.ink),
    ]
    .spacing(theme::S3)
    .align_y(Alignment::Center);
    if let Some(c) = caption {
        r = r.push(text(c).size(theme::SIZE_CAPTION).color(p.n.ink_3));
    }
    r.push(Space::new().width(Length::Fill))
}

fn pane<'a>(p: Palette, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .padding(theme::S6)
        .width(Length::Fill)
        .style(move |_| p.pane(false, false))
        .into()
}

/// The recipe card at the top of the side column.
pub fn recipe_card<'a>(p: Palette, data: &'a ModelData) -> Element<'a, Message> {
    let Some(r) = &data.recipe else {
        return pane(
            p,
            column![
                section_head(p, Icon::Notes, t("recipe-title"), None),
                text(t("recipe-empty"))
                    .size(theme::SIZE_CAPTION)
                    .color(p.n.ink_2),
                row![widgets::icon_button(
                    p,
                    Icon::Rename,
                    t("recipe-write"),
                    Palette::button_secondary,
                    Some(pr(Pr::OpenRecipe)),
                )],
            ]
            .spacing(theme::S4),
        );
    };
    let from = r
        .from_attempt
        .and_then(|a| data.attempts.iter().find(|x| x.id == a));
    let caption = from.map(|a| {
        t_args(
            "recipe-from",
            &[
                ("n", Arg::str(a.number.to_string())),
                ("date", Arg::str(date(a.printed_at))),
            ],
        )
    });
    let head = section_head(p, Icon::Notes, t("recipe-title"), caption).push(ghost_icon(
        p,
        Icon::Rename,
        t("recipe-edit"),
        pr(Pr::OpenRecipe),
    ));

    let rows = recipe_rows(&r.settings);
    let mut grid = column![].spacing(theme::S4);
    for chunk in rows.chunks(3) {
        let mut line = row![].spacing(theme::S4);
        for (label, value, mono) in chunk {
            line = line.push(
                column![
                    text(label.clone()).size(theme::SIZE_DATA).color(p.n.ink_3),
                    text(value.clone())
                        .size(theme::SIZE_BODY)
                        .font(if *mono {
                            theme::FONT_DATA
                        } else {
                            theme::FONT_MEDIUM
                        })
                        .color(p.n.ink),
                ]
                .spacing(1)
                .width(Length::FillPortion(1)),
            );
        }
        for _ in chunk.len()..3 {
            line = line.push(Space::new().width(Length::FillPortion(1)));
        }
        grid = grid.push(line);
    }
    let mut col = column![head].spacing(theme::S5);
    if !rows.is_empty() {
        col = col.push(grid);
    }
    if !r.required.is_empty() {
        let names: Vec<String> = REQUIRABLE
            .iter()
            .filter(|(k, _)| r.required.contains(*k))
            .map(|(_, label)| t(label))
            .collect();
        col = col.push(
            text(t_args(
                "recipe-required",
                &[("fields", Arg::str(names.join(", ")))],
            ))
            .size(theme::SIZE_CAPTION)
            .color(p.n.ink_2),
        );
    }
    for note in &r.notes {
        let (icon, body) = match &note.material {
            Some(m) => (Icon::Tip, format!("{}: {}", m.label(), note.text)),
            None => (Icon::Info, note.text.clone()),
        };
        col = col.push(
            row![
                icons::icon(icon, theme::ICON_DENSE, p.n.ink_2),
                text(body)
                    .size(theme::SIZE_CAPTION)
                    .color(p.n.ink)
                    .width(Length::Fill),
            ]
            .spacing(theme::S3),
        );
    }
    for part in &r.parts {
        let vals: Vec<String> = recipe_rows(&part.settings)
            .into_iter()
            .map(|(l, v, _)| format!("{l} {v}"))
            .collect();
        col = col.push(
            button(
                row![
                    text(part.part.clone())
                        .size(theme::SIZE_CAPTION)
                        .font(theme::FONT_DATA)
                        .color(p.n.ink),
                    text(vals.join(", "))
                        .size(theme::SIZE_CAPTION)
                        .color(p.n.ink_2)
                        .width(Length::Fill),
                ]
                .spacing(theme::S3),
            )
            .width(Length::Fill)
            .padding([2.0, theme::S2])
            .style(move |_, status| p.button_ghost(status))
            .on_press(pr(Pr::OpenRecipePart(part.part.clone()))),
        );
    }
    pane(p, col)
}

/// The print log under the organize pane.
pub fn print_log<'a>(app: &'a App, p: Palette, data: &'a ModelData) -> Element<'a, Message> {
    let n = data.attempts.len() as u64;
    // Effort worth acknowledging: the print time logged, when any was.
    let logged: u32 = data.attempts.iter().filter_map(|a| a.duration_min).sum();
    let caption = (n > 0).then(|| {
        let count = t_args(
            "log-count",
            &[("count", Arg::count(n)), ("n", Arg::str(n.to_string()))],
        );
        if logged == 0 {
            return count;
        }
        t_args(
            "log-count-time",
            &[
                ("attempts", Arg::str(count)),
                ("time", Arg::str(crate::log_form::format_duration(logged))),
            ],
        )
    });
    let head = section_head(p, Icon::LogPrint, t("log-title"), caption).push(
        button(
            row![
                icons::icon(Icon::LogPrint, theme::ICON_CHIP, p.n.ink_2),
                text(t("log-a-print")).size(theme::SIZE_CAPTION),
            ]
            .spacing(theme::S2)
            .align_y(Alignment::Center),
        )
        .padding([3.0, theme::S4])
        .style(move |_, status| p.button_secondary(status))
        .on_press(pr(Pr::OpenLog)),
    );
    let mut col = column![head].spacing(theme::S4);
    if data.attempts.is_empty() {
        col = col.push(
            text(t("log-empty"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_2),
        );
        return pane(p, col);
    }
    let shown = if app.prints.show_all {
        data.attempts.len()
    } else {
        SHOWN_ATTEMPTS.min(data.attempts.len())
    };
    for (i, a) in data.attempts[..shown].iter().enumerate() {
        if i > 0 {
            col = col.push(widgets::hairline(p));
        }
        col = col.push(entry(app, p, data, a));
    }
    let hidden = data.attempts.len() - shown;
    if hidden > 0 || app.prints.show_all && data.attempts.len() > SHOWN_ATTEMPTS {
        let (label, all) = if hidden > 0 {
            (
                t_args(
                    "log-show-earlier",
                    &[
                        ("count", Arg::count(hidden as u64)),
                        ("n", Arg::str(hidden.to_string())),
                    ],
                ),
                true,
            )
        } else {
            (t("log-show-fewer"), false)
        };
        col = col.push(
            button(text(label).size(theme::SIZE_CAPTION))
                .padding([3.0, theme::S4])
                .style(move |_, status| p.button_ghost(status))
                .on_press(pr(Pr::ShowAll(all))),
        );
    }
    pane(p, col)
}

fn small<'a>(p: Palette, label: String, msg: Message) -> Element<'a, Message> {
    button(text(label).size(theme::SIZE_CAPTION))
        .padding([3.0, theme::S4])
        .style(move |_, status| p.button_ghost(status))
        .on_press(msg)
        .into()
}

fn entry<'a>(
    app: &'a App,
    p: Palette,
    data: &'a ModelData,
    a: &'a Attempt,
) -> Element<'a, Message> {
    let (icon, word, color) = outcome(p, a.result);
    let mut meta = vec![date(a.printed_at)];
    if let Some(printer) = &a.settings.printer {
        meta.push(printer.clone());
    }
    let head = row![
        icons::icon(icon, theme::ICON_DENSE, color),
        text(word)
            .size(theme::SIZE_BODY)
            .font(theme::FONT_STRONG)
            .color(color),
        text(format!("· {}", meta.join(" · ")))
            .size(theme::SIZE_CAPTION)
            .color(p.n.ink_2)
            .wrapping(text::Wrapping::None)
            .width(Length::Fill),
        text(t_args(
            "log-attempt",
            &[("n", Arg::str(a.number.to_string()))]
        ))
        .size(theme::SIZE_CAPTION)
        .color(p.n.ink_3),
        // Edit and delete sit on the heading line instead of a row of
        // their own under every attempt.
        row![
            ghost_icon(p, Icon::Rename, t("log-edit"), pr(Pr::OpenEdit(a.id))),
            ghost_icon(p, Icon::Remove, t("log-delete"), pr(Pr::AskDelete(a.id))),
        ]
        .spacing(theme::S1),
    ]
    .spacing(theme::S3)
    .align_y(Alignment::Center);
    let mut col = column![head].spacing(theme::S3);
    if !a.settings.is_empty() || a.duration_min.is_some() {
        col = col.push(setting_chips(p, &a.settings, a.duration_min));
    }
    if !a.notes.trim().is_empty() {
        col = col.push(
            text(a.notes.clone())
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink),
        );
    }
    let is_recipe = data
        .recipe
        .as_ref()
        .is_some_and(|r| r.from_attempt == Some(a.id));
    if is_recipe {
        col = col.push(
            text(t("log-recipe-from-this"))
                .size(theme::SIZE_CAPTION)
                .color(p.n.ink_3),
        );
    }
    if !a.photos.is_empty() {
        let mut photos = row![].spacing(theme::S3);
        for ph in &a.photos {
            photos = photos.push(
                mouse_area(
                    container(
                        image(image::Handle::from_path(&ph.path))
                            .width(PHOTO)
                            .height(PHOTO)
                            .content_fit(iced::ContentFit::Cover),
                    )
                    .clip(true)
                    .style(move |_| p.well()),
                )
                .interaction(iced::mouse::Interaction::Pointer)
                .on_press(pr(Pr::OpenPhoto(ph.path.clone()))),
            );
        }
        col = col.push(photos.wrap().vertical_spacing(theme::S3));
    }

    // Inline prompts, then actions.
    match (&app.prints.note_for, app.prints.confirm_delete) {
        (Some((id, note)), _) if *id == a.id => {
            col = col.push(
                column![
                    text_input(&t("log-note-placeholder"), note)
                        .id(note_input_id())
                        .on_input(|s| pr(Pr::NoteInput(s)))
                        .on_submit(pr(Pr::CommitNote))
                        .size(theme::SIZE_BODY)
                        .padding([5.0, theme::S4])
                        .style(move |_, status| p.search_field(status)),
                    row![
                        text(t_args(
                            "log-note-hint",
                            &[(
                                "material",
                                Arg::str(
                                    a.settings
                                        .material
                                        .as_ref()
                                        .map(|m| m.label().to_owned())
                                        .unwrap_or_else(|| "none".to_owned())
                                )
                            )]
                        ))
                        .size(theme::SIZE_DATA)
                        .color(p.n.ink_3)
                        .width(Length::Fill),
                        small(p, t("action-cancel"), Message::Escape),
                        button(text(t("log-add-note")).size(theme::SIZE_CAPTION))
                            .padding([3.0, theme::S4])
                            .style(move |_, status| p.button_primary(status))
                            .on_press(pr(Pr::CommitNote)),
                    ]
                    .spacing(theme::S3)
                    .align_y(Alignment::Center),
                ]
                .spacing(theme::S3),
            );
        }
        (_, Some(id)) if id == a.id => {
            col = col.push(
                row![
                    text(t("log-delete-confirm"))
                        .size(theme::SIZE_CAPTION)
                        .color(p.n.ink_2)
                        .width(Length::Fill),
                    small(p, t("action-cancel"), Message::Escape),
                    button(text(t("log-delete")).size(theme::SIZE_CAPTION))
                        .padding([3.0, theme::S4])
                        .style(move |_, status| p.button_danger(status))
                        .on_press(pr(Pr::ConfirmDelete(a.id))),
                ]
                .spacing(theme::S3)
                .align_y(Alignment::Center),
            );
        }
        _ => {
            // One text action at most: "Save as recipe" only on the newest
            // success (older ones are rarely what you want to keep), "Add as
            // note" on failures that say what went wrong.
            let newest_success = data
                .attempts
                .iter()
                .filter(|x| x.result == AttemptResult::Success)
                .max_by_key(|x| (x.printed_at, x.number))
                .map(|x| x.id);
            let action = match a.result {
                AttemptResult::Success if !is_recipe && newest_success == Some(a.id) => {
                    Some(small(p, t("log-save-recipe"), pr(Pr::SaveAsRecipe(a.id))))
                }
                AttemptResult::Failed | AttemptResult::Partial if !a.notes.trim().is_empty() => {
                    Some(small(p, t("log-add-as-note"), pr(Pr::StartNote(a.id))))
                }
                _ => None,
            };
            if let Some(action) = action {
                col = col.push(row![action]);
            }
        }
    }
    let fresh = app.prints.fresh == Some(a.id);
    container(col)
        .padding(if fresh { theme::S3 } else { 0.0 })
        .style(move |_| {
            if fresh {
                p.pane(true, false)
            } else {
                container::Style::default()
            }
        })
        .into()
}

/// A labelled field.
fn field<'a>(
    p: Palette,
    label: String,
    input: Input,
    value: &'a str,
    hint: &str,
    unit: Option<&'static str>,
    error: bool,
) -> Element<'a, Message> {
    let mut inp = text_input(hint, value)
        .on_input(move |s| pr(Pr::Input(input, s)))
        .on_submit(pr(Pr::Save))
        .size(theme::SIZE_BODY)
        .padding([5.0, theme::S4])
        .style(move |_, status| {
            let mut s = p.search_field(status);
            if error {
                s.border.color = p.status.fail;
                s.border.width = 1.5;
            }
            s
        });
    if input == Input::Printer {
        inp = inp.id(printer_input_id());
    }
    let mut r = row![inp].spacing(theme::S2).align_y(Alignment::Center);
    if let Some(u) = unit {
        r = r.push(
            text(u)
                .size(theme::SIZE_CAPTION)
                .font(theme::FONT_DATA)
                .color(p.n.ink_3),
        );
    }
    column![text(label).size(theme::SIZE_DATA).color(p.n.ink_2), r]
        .spacing(theme::S1)
        .width(Length::Fill)
        .into()
}

fn suggestion_row<'a>(
    p: Palette,
    items: Vec<String>,
    on: impl Fn(String) -> Message,
) -> Element<'a, Message> {
    let mut r = row![].spacing(theme::S2);
    for s in items {
        r = r.push(
            button(text(s.clone()).size(theme::SIZE_DATA))
                .padding([2.0, theme::S3])
                .style(move |_, status| p.button_ghost(status))
                .on_press(on(s)),
        );
    }
    r.wrap().vertical_spacing(theme::S1).into()
}

/// Recipe editor: whether the fields are for the whole model or one part.
fn part_picker<'a>(p: Palette, f: &'a LogForm) -> Element<'a, Message> {
    let chip = |label: String, scope: Option<String>, marked: bool| {
        let on = f.scope == scope;
        let mut content = row![].spacing(theme::S2).align_y(Alignment::Center);
        if marked {
            // A part with settings of its own.
            content = content.push(icons::icon(Icon::Rename, theme::ICON_CHIP, p.t.accent_ink));
        }
        let mono = scope.is_some();
        content = content.push(text(label).size(theme::SIZE_CAPTION).font(if mono {
            theme::FONT_DATA
        } else {
            theme::FONT_MEDIUM
        }));
        button(content)
            .padding([3.0, theme::S4])
            .style(move |_, status| {
                if on {
                    p.segment(true, status)
                } else {
                    p.button_secondary(status)
                }
            })
            .on_press(pr(Pr::Scope(scope)))
    };
    let mut chips = row![chip(t("recipe-whole-model"), None, false)].spacing(theme::S2);
    for part in &f.parts {
        chips = chips.push(chip(
            part.clone(),
            Some(part.clone()),
            f.part_has_settings(part),
        ));
    }
    let caption = match &f.scope {
        None => t("recipe-parts-caption"),
        Some(part) => t_args("recipe-part-caption", &[("part", Arg::str(part.clone()))]),
    };
    column![
        text(t("recipe-applies-to"))
            .size(theme::SIZE_DATA)
            .color(p.n.ink_2),
        chips.wrap().vertical_spacing(theme::S2),
        text(caption).size(theme::SIZE_CAPTION).color(p.n.ink_3),
    ]
    .spacing(theme::S2)
    .into()
}

/// The "Log a print" form (or the attempt and recipe editors), replacing
/// the side column.
pub fn form<'a>(app: &'a App, p: Palette, f: &'a LogForm) -> Element<'a, Message> {
    let (title, icon) = match f.mode {
        FormMode::Log => (t("log-a-print"), Icon::LogPrint),
        FormMode::Edit(_) => (t("log-edit-title"), Icon::LogPrint),
        FormMode::Recipe => (t("recipe-edit"), Icon::Notes),
    };
    let head = section_head(p, icon, title, None).push(ghost_icon(
        p,
        Icon::Close,
        t("action-close"),
        pr(Pr::Close),
    ));
    let mut body = column![head].spacing(theme::S5);
    let is_recipe = f.mode == FormMode::Recipe;

    if is_recipe && f.has_part_choice() {
        body = body.push(part_picker(p, f));
    }

    if !is_recipe {
        let outcomes = [
            AttemptResult::Success,
            AttemptResult::Partial,
            AttemptResult::Failed,
        ];
        let mut seg = row![].spacing(theme::S1);
        for o in outcomes {
            let (icon, word, color) = outcome(p, o);
            let on = f.outcome == o;
            seg = seg.push(
                button(
                    row![
                        icons::icon(icon, theme::ICON_DENSE, if on { color } else { p.n.ink_3 }),
                        text(word).size(theme::SIZE_BODY),
                    ]
                    .spacing(theme::S2)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([4.0, theme::S4])
                .style(move |_, status| {
                    let mut s = p.segment(on, status);
                    if on {
                        s.border.color = color;
                        s.border.width = 1.5;
                    }
                    s
                })
                .on_press(pr(Pr::Outcome(o))),
            );
        }
        body = body.push(
            column![
                text(t("form-outcome"))
                    .size(theme::SIZE_DATA)
                    .color(p.n.ink_2),
                container(seg).padding(2).style(move |_| p.segmented()),
            ]
            .spacing(theme::S1),
        );
        if f.prefilled {
            body = body.push(
                row![
                    icons::icon(Icon::Sparkle, theme::ICON_DENSE, p.t.accent_ink),
                    text(t("form-prefilled"))
                        .size(theme::SIZE_CAPTION)
                        .color(p.n.ink_2),
                ]
                .spacing(theme::S3)
                .align_y(Alignment::Center),
            );
        }
    }

    // Printer (+ date).
    let printer = field(
        p,
        t("form-printer"),
        Input::Printer,
        &f.printer,
        &f.hint(Input::Printer),
        None,
        false,
    );
    if is_recipe {
        body = body.push(printer);
    } else {
        body = body.push(
            row![
                printer,
                field(
                    p,
                    t("form-date"),
                    Input::Date,
                    &f.date,
                    &f.hint(Input::Date),
                    None,
                    has_error(f, Field::Date)
                ),
            ]
            .spacing(theme::S4),
        );
    }
    let printers = app.prints.printer_suggestions(&f.printer);
    if !printers.is_empty() && !f.printer.is_empty() || f.printer.is_empty() && !printers.is_empty()
    {
        body = body.push(suggestion_row(p, printers, |s| {
            pr(Pr::Input(Input::Printer, s))
        }));
    }

    // Material.
    body = body.push(
        row![
            field(
                p,
                t("form-material"),
                Input::Material,
                &f.material,
                &f.hint(Input::Material),
                None,
                false
            ),
            field(
                p,
                t("form-filament"),
                Input::Filament,
                &f.filament,
                &f.hint(Input::Filament),
                None,
                false
            ),
        ]
        .spacing(theme::S4),
    );
    let known: Vec<String> = Material::KNOWN
        .iter()
        .map(|m| m.label().to_owned())
        .filter(|m| !m.eq_ignore_ascii_case(f.material.trim()))
        .collect();
    body = body.push(suggestion_row(p, known, |s| {
        pr(Pr::Input(Input::Material, s))
    }));

    // Numbers.
    body = body.push(
        row![
            field(
                p,
                t("form-layer"),
                Input::Layer,
                &f.layer,
                &f.hint(Input::Layer),
                Some("mm"),
                has_error(f, Field::Layer)
            ),
            field(
                p,
                t("form-nozzle"),
                Input::Nozzle,
                &f.nozzle,
                &f.hint(Input::Nozzle),
                Some("mm"),
                has_error(f, Field::Nozzle)
            ),
            field(
                p,
                t("form-infill"),
                Input::Infill,
                &f.infill,
                &f.hint(Input::Infill),
                Some("%"),
                has_error(f, Field::Infill)
            ),
        ]
        .spacing(theme::S4),
    );
    body = body.push(
        row![
            field(
                p,
                t("form-bed"),
                Input::Bed,
                &f.bed,
                &f.hint(Input::Bed),
                Some("°C"),
                has_error(f, Field::Bed)
            ),
            field(
                p,
                t("form-hotend"),
                Input::Hotend,
                &f.hotend,
                &f.hint(Input::Hotend),
                Some("°C"),
                has_error(f, Field::Hotend)
            ),
            field(
                p,
                t("form-walls"),
                Input::Walls,
                &f.walls,
                &f.hint(Input::Walls),
                None,
                has_error(f, Field::Walls)
            ),
        ]
        .spacing(theme::S4),
    );
    let supports = f.supports;
    // On a part, "not set" means "as the whole model".
    let unset = if f.scope.is_some() {
        t("form-as-model")
    } else {
        t("form-unset")
    };
    body = body.push(
        column![
            text(t("form-supports"))
                .size(theme::SIZE_DATA)
                .color(p.n.ink_2),
            segmented(
                p,
                &[
                    (None, None, Some(unset.clone())),
                    (Some(Supports::None), None, Some(t("supports-none"))),
                    (Some(Supports::Normal), None, Some(t("supports-normal"))),
                    (Some(Supports::Tree), None, Some(t("supports-tree"))),
                ],
                supports,
                |s| pr(Pr::Supports(s)),
            ),
        ]
        .spacing(theme::S1),
    );
    body = body.push(
        column![
            text(t("form-adhesion"))
                .size(theme::SIZE_DATA)
                .color(p.n.ink_2),
            segmented(
                p,
                &[
                    (None, None, Some(unset.clone())),
                    (Some(Adhesion::Skirt), None, Some(t("adhesion-skirt"))),
                    (Some(Adhesion::Brim), None, Some(t("adhesion-brim"))),
                    (Some(Adhesion::Raft), None, Some(t("adhesion-raft"))),
                ],
                f.adhesion,
                |a| pr(Pr::Adhesion(a)),
            ),
        ]
        .spacing(theme::S1),
    );

    if is_recipe && f.scope.is_some() {
        body = body.push(row![widgets::icon_button(
            p,
            Icon::Undo,
            t("recipe-part-clear"),
            Palette::button_secondary,
            Some(pr(Pr::ClearPart)),
        )]);
    } else if is_recipe {
        // Required flags and notes.
        let mut req = row![].spacing(theme::S2);
        for (key, label) in REQUIRABLE {
            let on = f.required.contains(key);
            let mut content = row![].spacing(theme::S2).align_y(Alignment::Center);
            if on {
                content = content.push(icons::icon(Icon::Check, theme::ICON_CHIP, p.t.accent_ink));
            }
            content = content.push(text(t(label)).size(theme::SIZE_CAPTION));
            req = req.push(
                button(content)
                    .padding([3.0, theme::S4])
                    .style(move |_, status| {
                        if on {
                            p.segment(true, status)
                        } else {
                            p.button_secondary(status)
                        }
                    })
                    .on_press(pr(Pr::ToggleRequired(key))),
            );
        }
        body = body.push(
            column![
                text(t("form-required"))
                    .size(theme::SIZE_DATA)
                    .color(p.n.ink_2),
                req.wrap().vertical_spacing(theme::S2),
            ]
            .spacing(theme::S1),
        );
        let mut notes = column![
            text(t("form-recipe-notes"))
                .size(theme::SIZE_DATA)
                .color(p.n.ink_2)
        ]
        .spacing(theme::S2);
        for (i, note) in f.recipe_notes.iter().enumerate() {
            let body_text = match &note.material {
                Some(m) => format!("{}: {}", m.label(), note.text),
                None => note.text.clone(),
            };
            notes = notes.push(
                row![
                    text(body_text)
                        .size(theme::SIZE_CAPTION)
                        .width(Length::Fill),
                    ghost_icon(
                        p,
                        Icon::Close,
                        t("form-remove-note"),
                        pr(Pr::RemoveRecipeNote(i))
                    ),
                ]
                .spacing(theme::S3)
                .align_y(Alignment::Center),
            );
        }
        notes = notes.push(
            text_input(&t("form-new-note"), &f.new_note)
                .on_input(|s| pr(Pr::Input(Input::NewNote, s)))
                .on_submit(pr(Pr::Save))
                .size(theme::SIZE_BODY)
                .padding([5.0, theme::S4])
                .style(move |_, status| p.search_field(status)),
        );
        body = body.push(notes);
    } else {
        body = body.push(
            row![field(
                p,
                t("form-duration"),
                Input::Duration,
                &f.duration,
                &f.hint(Input::Duration),
                None,
                has_error(f, Field::Duration)
            )]
            .push(Space::new().width(Length::FillPortion(1))),
        );
        let label = if f.outcome == AttemptResult::Success {
            t("form-notes")
        } else {
            t("form-what-went-wrong")
        };
        body = body.push(
            column![
                text(label).size(theme::SIZE_DATA).color(p.n.ink_2),
                text_editor(&f.notes)
                    .on_action(|a| pr(Pr::Notes(a)))
                    .size(theme::SIZE_BODY)
                    .padding(theme::S4)
                    .height(NOTES_HEIGHT)
                    .key_binding(|kp| {
                        if kp.key == keyboard::Key::Named(keyboard::key::Named::Enter)
                            && kp.modifiers.command()
                        {
                            Some(text_editor::Binding::Custom(pr(Pr::Save)))
                        } else {
                            text_editor::Binding::from_key_press(kp)
                        }
                    })
                    .style(move |_, status| super::organize::editor_style(p, status)),
            ]
            .spacing(theme::S1),
        );
        // Photos: existing (edit) and to be added.
        let existing = match f.mode {
            FormMode::Edit(id) => app
                .org
                .data
                .as_ref()
                .and_then(|(_, d)| d.attempts.iter().find(|a| a.id == id))
                .map(|a| a.photos.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let mut photos = row![].spacing(theme::S3);
        for ph in existing {
            photos = photos.push(photo_tile(p, &ph.path, pr(Pr::RemovePhoto(ph.id))));
        }
        for (i, path) in f.photos.iter().enumerate() {
            photos = photos.push(photo_tile(p, path, pr(Pr::RemovePendingPhoto(i))));
        }
        body = body.push(photos.wrap().vertical_spacing(theme::S3));
        body = body.push(
            button(
                row![
                    icons::icon(Icon::AddPhoto, theme::ICON, p.n.ink_2),
                    text(t("form-photo-add"))
                        .size(theme::SIZE_CAPTION)
                        .color(p.n.ink_2),
                ]
                .spacing(theme::S3)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([theme::S5, theme::S5])
            .style(move |_, status| {
                let mut s = p.button_ghost(status);
                s.border.color = p.n.rule;
                s.border.width = 1.0;
                s
            })
            .on_press(pr(Pr::PickPhotos)),
        );
    }

    if let Some((_, msg)) = &f.error {
        body = body.push(
            row![
                icons::icon(Icon::Failed, theme::ICON_DENSE, p.status.fail),
                text(msg.clone())
                    .size(theme::SIZE_CAPTION)
                    .color(p.status.fail),
            ]
            .spacing(theme::S3)
            .align_y(Alignment::Center),
        );
    }

    let save_label = match f.mode {
        FormMode::Log => t("form-save-attempt"),
        _ => t("action-save"),
    };
    let mut save = button(
        row![
            icons::icon(Icon::Check, theme::ICON_DENSE, p.t.on_primary),
            text(save_label).size(theme::SIZE_BODY),
        ]
        .spacing(theme::S2)
        .align_y(Alignment::Center),
    )
    .padding([6.0, theme::S5])
    .style(move |_, status| p.button_primary(status));
    if !app.prints.saving {
        save = save.on_press(pr(Pr::Save));
    }
    let foot = row![
        super::statusbar::hint(p, "Ctrl Enter", t("form-key-save")),
        Space::new().width(Length::Fill),
        button(text(t("action-cancel")).size(theme::SIZE_BODY))
            .padding([6.0, theme::S5])
            .style(move |_, status| p.button_ghost(status))
            .on_press(pr(Pr::Close)),
        save,
    ]
    .spacing(theme::S4)
    .align_y(Alignment::Center);
    body = body.push(widgets::hairline(p)).push(foot);

    let (offset, alpha) = app.motion.panel();
    container(
        container(body)
            .padding(theme::S6)
            .width(Length::Fill)
            .style(move |_| {
                let mut s = p.pane(false, false);
                if let Some(iced::Background::Color(c)) = s.background {
                    s.background = Some(
                        Color {
                            a: c.a * alpha,
                            ..c
                        }
                        .into(),
                    );
                }
                s
            }),
    )
    .padding(Padding {
        left: offset,
        ..Padding::ZERO
    })
    .into()
}

fn photo_tile<'a>(p: Palette, path: &std::path::Path, remove: Message) -> Element<'a, Message> {
    iced::widget::stack![
        container(
            image(image::Handle::from_path(path))
                .width(PHOTO)
                .height(PHOTO)
                .content_fit(iced::ContentFit::Cover),
        )
        .clip(true)
        .style(move |_| p.well()),
        container(
            button(icons::icon(Icon::Close, theme::ICON_CHIP, p.n.ink))
                .padding(2)
                .style(move |_, status| p.button_secondary(status))
                .on_press(remove),
        )
        .width(PHOTO)
        .align_right(PHOTO),
    ]
    .into()
}

/// "Attempt 4 saved".
pub fn toast<'a>(p: Palette, msg: &str) -> Element<'a, Message> {
    container(
        row![
            icons::icon(Icon::Success, theme::ICON_DENSE, p.status.ok),
            text(msg.to_owned()).size(theme::SIZE_CAPTION),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center),
    )
    .padding([theme::S4, theme::S5])
    .width(Length::Fill)
    .style(move |_| container::Style {
        shadow: iced::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.18),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..p.tooltip()
    })
    .into()
}

/// Card chips, at most two: the recipe material, then "Supports" when the
/// recipe needs supports, else the layer height.
pub fn card_chips<'a>(p: Palette, hit: &printdex_core::ModelHit) -> Option<Element<'a, Message>> {
    let r = hit.recipe.as_ref()?;
    let mut chips: Vec<Element<'a, Message>> = Vec::new();
    if let Some(m) = &r.material {
        chips.push(chip(p, m.label().to_owned(), false));
    }
    match (r.supports, r.layer_mm) {
        (Some(Supports::Normal | Supports::Tree), _) => {
            chips.push(chip(p, t("card-supports"), false));
        }
        (_, Some(l)) => chips.push(chip(p, layer_text(l), true)),
        _ => {}
    }
    if chips.is_empty() {
        return None;
    }
    Some(
        iced::widget::Row::with_children(chips)
            .spacing(theme::S2)
            .into(),
    )
}

/// "Printed 3× · 16 Nov 2026", "Failed · …" or "Never printed".
pub fn status_line<'a>(
    p: Palette,
    last: Option<&printdex_core::LastPrint>,
) -> Element<'a, Message> {
    // Never printed is the common case and an absence, so it shows
    // nothing; the "Never printed" smart filter finds those models.
    let (icon, word, color, when) = match last {
        None => return Space::new().into(),
        Some(l) => {
            let (icon, word, color) = match l.result {
                AttemptResult::Success => (
                    Icon::Success,
                    t_args("card-printed", &[("n", Arg::str(l.successes.to_string()))]),
                    p.status.ok,
                ),
                r => outcome(p, r),
            };
            (icon, word, color, Some(date(l.at)))
        }
    };
    let mut r = row![
        icons::icon(icon, theme::ICON_CHIP, color),
        text(word)
            .size(theme::SIZE_DATA)
            .font(theme::FONT_MEDIUM)
            .color(color)
            .wrapping(text::Wrapping::None),
    ]
    .spacing(theme::S2)
    .align_y(Alignment::Center);
    if let Some(d) = when {
        r = r.push(
            text(format!("· {d}"))
                .size(theme::SIZE_DATA)
                .color(p.n.ink_3)
                .wrapping(text::Wrapping::None),
        );
    }
    r.into()
}
