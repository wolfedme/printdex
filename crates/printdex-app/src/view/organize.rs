//! The model view's organizing parts: tags, collections and source in the
//! side column, notes in the main column.

use iced::widget::{
    Space, button, column, container, markdown, row, text, text_editor, text_input,
};
use iced::{Alignment, Element, Length, Padding, keyboard};
use printdex_core::{ModelData, ModelId};

use super::statusbar::hint;
use super::widgets;
use crate::app::Message;
use crate::i18n::t;
use crate::icons::{self, Icon};
use crate::org::{NewFor, Org, OrgState, collection_input_id, source_input_id, tag_input_id};
use crate::theme::{self, Palette};

/// The notes editor's widget id.
pub fn notes_editor_id() -> iced::widget::Id {
    iced::widget::Id::new("notes-editor")
}

const NOTES_EDITOR_HEIGHT: f32 = 180.0;

fn org(o: Org) -> Message {
    Message::Org(o)
}

/// Tags, collections and source: the "Organize" pane of the side column.
pub fn side<'a>(org_state: &'a OrgState, p: Palette, id: ModelId) -> Element<'a, Message> {
    static EMPTY: std::sync::LazyLock<ModelData> = std::sync::LazyLock::new(ModelData::default);
    let data = org_state.data_for(id).unwrap_or(&EMPTY);
    // Nothing to show and nothing being edited: one row of add buttons
    // instead of three headed sections of empty space.
    let untouched = data.tags.is_empty()
        && data.collections.is_empty()
        && data.source.is_none()
        && org_state.tag_input.is_none()
        && !org_state.collection_menu
        && org_state.source_edit.is_none();
    if untouched {
        return row![
            add_button(p, Icon::Tag, t("tag-add"), org(Org::StartTag)),
            add_button(
                p,
                Icon::Collection,
                t("collection-add-model"),
                org(Org::ToggleCollectionMenu)
            ),
            add_button(p, Icon::Link, t("source-add"), org(Org::StartSource)),
        ]
        .spacing(theme::S3)
        .wrap()
        .vertical_spacing(theme::S3)
        .into();
    }
    column![
        tags(org_state, p, data),
        collections(org_state, p, id, data),
        source(org_state, p, data),
    ]
    .spacing(theme::S6)
    .into()
}

/// A chip with a remove button.
fn chip<'a>(
    p: Palette,
    icon: Option<Icon>,
    name: String,
    remove: Message,
    remove_label: String,
) -> Element<'a, Message> {
    let mut r = row![].spacing(theme::S2).align_y(Alignment::Center);
    if let Some(i) = icon {
        r = r.push(icons::icon(i, theme::ICON_CHIP, p.n.ink_3));
    }
    r = r.push(
        text(name)
            .size(theme::SIZE_CAPTION)
            .font(theme::FONT_MEDIUM)
            .color(p.n.ink),
    );
    r = r.push(iced::widget::tooltip(
        button(icons::icon(Icon::Close, theme::ICON_CHIP, p.n.ink_3))
            .padding(2)
            .style(move |_, status| p.button_ghost(status))
            .on_press(remove),
        container(text(remove_label).size(theme::SIZE_CAPTION))
            .padding([3.0, theme::S3])
            .style(move |_| p.tooltip()),
        iced::widget::tooltip::Position::Top,
    ));
    container(r)
        .padding(Padding {
            top: 2.0,
            bottom: 2.0,
            left: theme::S4,
            right: theme::S1,
        })
        .style(move |_| p.chip())
        .into()
}

/// A small secondary button with a leading icon.
fn add_button<'a>(p: Palette, icon: Icon, s: String, msg: Message) -> Element<'a, Message> {
    button(
        row![
            icons::icon(icon, theme::ICON_CHIP, p.n.ink_2),
            text(s).size(theme::SIZE_CAPTION),
        ]
        .spacing(theme::S2)
        .align_y(Alignment::Center),
    )
    .padding([3.0, theme::S4])
    .style(move |_, status| p.button_secondary(status))
    .on_press(msg)
    .into()
}

fn field<'a>(
    p: Palette,
    id: iced::widget::Id,
    placeholder: String,
    value: &str,
    on_input: impl Fn(String) -> Message + 'a,
    on_submit: Message,
) -> iced::widget::TextInput<'a, Message> {
    text_input(&placeholder, value)
        .id(id)
        .on_input(on_input)
        .on_submit(on_submit)
        .size(theme::SIZE_BODY)
        .padding([5.0, theme::S4])
        .style(move |_, status| p.search_field(status))
}

fn tags<'a>(o: &'a OrgState, p: Palette, data: &'a ModelData) -> Element<'a, Message> {
    let mut chips = row![].spacing(theme::S3).align_y(Alignment::Center);
    for tag in &data.tags {
        chips = chips.push(chip(
            p,
            None,
            tag.clone(),
            org(Org::RemoveTag(tag.clone())),
            t("tag-remove"),
        ));
    }
    let mut col = column![widgets::section(p, t("model-tags"), None)].spacing(theme::S4);
    match &o.tag_input {
        Some(typed) => {
            col = col.push(chips.wrap().vertical_spacing(theme::S3));
            col = col.push(field(
                p,
                tag_input_id(),
                t("tag-placeholder"),
                typed,
                |s| org(Org::TagInput(s)),
                org(Org::SubmitTag),
            ));
            let suggestions = o.suggestions(typed, &data.tags);
            if !suggestions.is_empty() {
                let mut r = row![].spacing(theme::S3);
                for s in suggestions {
                    r = r.push(
                        button(text(s.clone()).size(theme::SIZE_CAPTION))
                            .padding([3.0, theme::S4])
                            .style(move |_, status| p.button_ghost(status))
                            .on_press(org(Org::AddTag(s))),
                    );
                }
                col = col.push(r.wrap().vertical_spacing(theme::S2));
            }
            col = col.push(text(t("tag-hint")).size(theme::SIZE_DATA).color(p.n.ink_3));
        }
        None => {
            chips = chips.push(add_button(p, Icon::Plus, t("tag-add"), org(Org::StartTag)));
            col = col.push(chips.wrap().vertical_spacing(theme::S3));
        }
    }
    col.into()
}

fn collections<'a>(
    o: &'a OrgState,
    p: Palette,
    id: ModelId,
    data: &'a ModelData,
) -> Element<'a, Message> {
    let mut chips = row![].spacing(theme::S3).align_y(Alignment::Center);
    for c in &data.collections {
        chips = chips.push(chip(
            p,
            Some(Icon::Collection),
            c.name.clone(),
            org(Org::RemoveFromCollection(c.id)),
            t("collection-remove-model"),
        ));
    }
    chips = chips.push(add_button(
        p,
        Icon::Plus,
        t("collection-add-model"),
        org(Org::ToggleCollectionMenu),
    ));
    let mut col = column![
        widgets::section(p, t("model-collections"), None),
        chips.wrap().vertical_spacing(theme::S3),
    ]
    .spacing(theme::S4);

    if o.collection_menu {
        let mut menu = column![].spacing(theme::S1);
        for c in o
            .lists
            .collections
            .iter()
            .filter(|c| !data.collections.iter().any(|m| m.id == c.id))
        {
            menu = menu.push(
                button(
                    row![
                        icons::icon(Icon::Collection, theme::ICON_DENSE, p.n.ink_3),
                        text(c.name.clone())
                            .size(theme::SIZE_BODY)
                            .width(Length::Fill),
                    ]
                    .spacing(theme::S3)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([4.0, theme::S4])
                .style(move |_, status| p.button_ghost(status))
                .on_press(org(Org::AddToCollection(c.id))),
            );
        }
        match &o.new_collection {
            Some((NewFor::Model(m), name)) if *m == id => {
                menu = menu.push(field(
                    p,
                    collection_input_id(),
                    t("sidebar-new-collection"),
                    name,
                    |s| org(Org::NewCollectionInput(s)),
                    org(Org::CommitNewCollection),
                ));
            }
            _ => {
                menu = menu.push(
                    button(
                        row![
                            icons::icon(Icon::Plus, theme::ICON_DENSE, p.n.ink_3),
                            text(t("collection-new")).size(theme::SIZE_BODY),
                        ]
                        .spacing(theme::S3)
                        .align_y(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding([4.0, theme::S4])
                    .style(move |_, status| p.button_ghost(status))
                    .on_press(org(Org::StartNewCollection(NewFor::Model(id)))),
                );
            }
        }
        col = col.push(
            container(menu)
                .padding(theme::S2)
                .width(Length::Fill)
                .style(move |_| p.well()),
        );
    }
    col.into()
}

fn source<'a>(o: &'a OrgState, p: Palette, data: &'a ModelData) -> Element<'a, Message> {
    let body: Element<'a, Message> = match (&o.source_edit, &data.source) {
        (Some(url), _) => row![
            field(
                p,
                source_input_id(),
                t("source-placeholder"),
                url,
                |s| org(Org::SourceInput(s)),
                org(Org::SaveSource),
            )
            .width(Length::Fill),
            button(text(t("action-save")).size(theme::SIZE_CAPTION))
                .padding([4.0, theme::S4])
                .style(move |_, status| p.button_primary(status))
                .on_press(org(Org::SaveSource)),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center)
        .into(),
        (None, Some(url)) => row![
            button(
                row![
                    icons::icon(Icon::Link, theme::ICON_DENSE, p.t.accent_ink),
                    text(short_url(url))
                        .size(theme::SIZE_BODY)
                        .color(p.t.accent_ink)
                        .wrapping(text::Wrapping::None),
                ]
                .spacing(theme::S3)
                .align_y(Alignment::Center),
            )
            .padding([2.0, 0.0])
            .style(move |_, _| p.bare())
            .on_press(org(Org::OpenLink(url.clone()))),
            Space::new().width(Length::Fill),
            widgets::ghost_icon(p, Icon::Rename, t("source-edit"), org(Org::StartSource)),
        ]
        .spacing(theme::S3)
        .align_y(Alignment::Center)
        .into(),
        (None, None) => row![add_button(
            p,
            Icon::Link,
            t("source-add"),
            org(Org::StartSource)
        )]
        .into(),
    };
    column![widgets::section(p, t("model-source"), None), body]
        .spacing(theme::S4)
        .into()
}

/// "printables.com/model/1234" for display.
pub fn short_url(url: &str) -> String {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let rest = rest.trim_end_matches('/');
    const MAX: usize = 42;
    if rest.chars().count() > MAX {
        let cut: String = rest.chars().take(MAX - 1).collect();
        format!("{cut}…")
    } else {
        rest.to_owned()
    }
}

/// The Notes section of the main column.
/// The notes section; `device` is "on Laptop" when another computer
/// wrote them last.
pub fn notes<'a>(
    o: &'a OrgState,
    p: Palette,
    id: ModelId,
    device: Option<String>,
) -> Element<'a, Message> {
    let has_notes = o.data_for(id).is_some_and(|d| !d.notes.trim().is_empty());
    let mut head = row![widgets::section(
        p,
        t("model-notes"),
        device.filter(|_| has_notes)
    )]
    .spacing(theme::S4)
    .align_y(Alignment::Center);
    head = head.push(Space::new().width(Length::Fill));

    let body: Element<'a, Message> = match &o.notes_edit {
        Some(content) => {
            let editor = text_editor(content)
                .id(notes_editor_id())
                .placeholder(t("notes-placeholder"))
                .on_action(|a| org(Org::NotesEdit(a)))
                .size(theme::SIZE_BODY)
                .font(theme::FONT_BODY)
                .padding(theme::S5)
                .height(NOTES_EDITOR_HEIGHT)
                // Esc reaches the app's own handler, which cancels.
                .key_binding(|kp| {
                    let named = |n| kp.key == keyboard::Key::Named(n);
                    if named(keyboard::key::Named::Enter) && kp.modifiers.command() {
                        Some(text_editor::Binding::Custom(org(Org::SaveNotes)))
                    } else {
                        text_editor::Binding::from_key_press(kp)
                    }
                })
                .style(move |_, status| editor_style(p, status));
            let actions = row![
                hint(p, "Ctrl Enter", t("notes-key-save")),
                hint(p, "Esc", t("notes-key-cancel")),
                Space::new().width(Length::Fill),
                button(text(t("action-cancel")).size(theme::SIZE_CAPTION))
                    .padding([4.0, theme::S4])
                    .style(move |_, status| p.button_secondary(status))
                    .on_press(org(Org::Cancel)),
                button(text(t("action-save")).size(theme::SIZE_CAPTION))
                    .padding([4.0, theme::S4])
                    .style(move |_, status| p.button_primary(status))
                    .on_press(org(Org::SaveNotes)),
            ]
            .spacing(theme::S4)
            .align_y(Alignment::Center);
            column![editor, actions].spacing(theme::S4).into()
        }
        None if has_notes => {
            head = head.push(widgets::ghost_icon(
                p,
                Icon::Rename,
                t("notes-edit"),
                org(Org::StartNotes),
            ));
            let style = markdown::Style {
                font: theme::FONT_BODY,
                inline_code_highlight: markdown::Highlight {
                    background: p.n.rule_soft.into(),
                    border: iced::border::rounded(4),
                },
                inline_code_padding: Padding::from([0, 2]),
                inline_code_color: p.n.ink,
                inline_code_font: theme::FONT_DATA,
                code_block_font: theme::FONT_DATA,
                link_color: p.t.accent_ink,
            };
            let settings = markdown::Settings::with_text_size(theme::SIZE_BODY, style);
            markdown::view(&o.notes_md, settings).map(|url| org(Org::OpenLink(url)))
        }
        None => button(
            row![
                icons::icon(Icon::Notes, theme::ICON, p.n.ink_3),
                text(t("notes-empty"))
                    .size(theme::SIZE_BODY)
                    .color(p.n.ink_3),
            ]
            .spacing(theme::S4)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding([theme::S5, theme::S5])
        .style(move |_, status| p.button_ghost(status))
        .on_press(org(Org::StartNotes))
        .into(),
    };
    container(column![head, body].spacing(theme::S4))
        .padding(theme::S6)
        .width(Length::Fill)
        .style(move |_| p.pane(false, false))
        .into()
}

/// Text editor style matching the text fields.
pub fn editor_style(p: Palette, status: text_editor::Status) -> text_editor::Style {
    use iced::widget::text_input::Status as In;
    use text_editor::Status as Ed;
    let s = p.search_field(match status {
        Ed::Active => In::Active,
        Ed::Hovered => In::Hovered,
        Ed::Focused { is_hovered } => In::Focused { is_hovered },
        Ed::Disabled => In::Disabled,
    });
    text_editor::Style {
        background: s.background,
        border: s.border,
        placeholder: s.placeholder,
        value: s.value,
        selection: s.selection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_url_drops_scheme_and_www() {
        assert_eq!(
            short_url("https://www.printables.com/model/1234/"),
            "printables.com/model/1234"
        );
        assert_eq!(short_url("http://example.org"), "example.org");
        let long = format!("https://example.org/{}", "a".repeat(80));
        assert_eq!(short_url(&long).chars().count(), 42);
    }
}
