//! Small pieces shared by several views.

use iced::widget::{Space, button, container, row, text};
use iced::{Alignment, Element, Length};

use crate::app::Message;
use crate::format::civil_date;
use crate::i18n::{Arg, t, t_args};
use crate::icons::{self, Icon};
use crate::theme::{self, Palette};

/// "3 Oct 2026" for unix milliseconds.
pub fn date(unix_ms: i64) -> String {
    let (year, month, day) = civil_date(unix_ms);
    let month = match month {
        1 => t("month-1"),
        2 => t("month-2"),
        3 => t("month-3"),
        4 => t("month-4"),
        5 => t("month-5"),
        6 => t("month-6"),
        7 => t("month-7"),
        8 => t("month-8"),
        9 => t("month-9"),
        10 => t("month-10"),
        11 => t("month-11"),
        _ => t("month-12"),
    };
    t_args(
        "date-short",
        &[
            ("day", Arg::str(day.to_string())),
            ("month", Arg::str(month)),
            ("year", Arg::str(year.to_string())),
        ],
    )
}

/// Button with an icon and a label, in one of the button styles.
pub fn icon_button<'a>(
    p: Palette,
    icon: Icon,
    label: String,
    style: fn(&Palette, button::Status) -> button::Style,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let fg = if on_press.is_some() {
        p.n.ink_2
    } else {
        p.n.ink_3
    };
    button(
        row![
            icons::icon(icon, theme::ICON, fg),
            text(label).size(theme::SIZE_BODY),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .height(theme::CONTROL_HEIGHT)
    .padding([8.0, theme::S5])
    .style(move |_, status| style(&p, status))
    .on_press_maybe(on_press)
    .into()
}

/// Section heading with an optional caption after it.
pub fn section<'a>(p: Palette, title: String, caption: Option<String>) -> Element<'a, Message> {
    let mut r = row![
        text(title)
            .size(theme::SIZE_SECTION)
            .font(theme::FONT_BOLD)
            .color(p.n.ink)
    ]
    .spacing(theme::S4)
    .align_y(Alignment::Center);
    if let Some(c) = caption {
        r = r.push(text(c).size(theme::SIZE_CAPTION).color(p.n.ink_3));
    }
    r.into()
}

/// Horizontal hairline.
pub fn hairline<'a>(p: Palette) -> Element<'a, Message> {
    container(Space::new().width(Length::Fill).height(1))
        .style(move |_| p.hairline())
        .into()
}

/// A segmented control: one button per option, the active one washed.
pub fn segmented<'a, T: Copy + PartialEq + 'a>(
    p: Palette,
    options: &[(T, Option<Icon>, Option<String>)],
    active: T,
    on_pick: impl Fn(T) -> Message + 'a,
) -> Element<'a, Message> {
    let mut r = row![].spacing(theme::S1);
    for (value, icon, label) in options.iter().cloned() {
        let is_active = value == active;
        let color = if is_active { p.t.accent_ink } else { p.n.ink_2 };
        let mut content = row![].spacing(theme::S3).align_y(Alignment::Center);
        if let Some(i) = icon {
            content = content.push(icons::icon(i, theme::ICON, color));
        }
        if let Some(l) = label {
            content = content.push(text(l).size(theme::SIZE_BODY));
        }
        r = r.push(
            button(content)
                .height(theme::CONTROL_HEIGHT - 6.0)
                .padding([4.0, theme::S4])
                .style(move |_, status| p.segment(is_active, status))
                .on_press(on_pick(value)),
        );
    }
    container(r)
        .padding(2)
        .height(theme::CONTROL_HEIGHT)
        .style(move |_| p.segmented())
        .into()
}
