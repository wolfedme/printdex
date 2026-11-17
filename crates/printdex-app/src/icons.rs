//! Bundled Lucide icons (ISC) and the placeholder preview, as SVG handles.

use iced::widget::svg::{self, Svg};
use iced::{Color, Length};

macro_rules! icon_files {
    ($($name:ident => $file:literal),* $(,)?) => {
        /// An icon from `assets/icons`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Icon { $($name),* }

        impl Icon {
            fn bytes(self) -> &'static [u8] {
                match self {
                    $(Icon::$name => include_bytes!(concat!("../assets/icons/", $file))),*
                }
            }
        }
    };
}

icon_files! {
    Search => "search.svg",
    FolderPlus => "folder-plus.svg",
    Library => "library.svg",
    Folder => "folder.svg",
    Refresh => "refresh-cw.svg",
    Layers => "layers-2.svg",
    Alert => "triangle-alert.svg",
    ExternalLink => "external-link.svg",
    FolderOpen => "folder-open.svg",
    File => "file.svg",
    FileText => "file-text.svg",
    Image => "image.svg",
    Part => "box.svg",
    Chevron => "chevron-right.svg",
    Settings => "settings.svg",
    Offline => "wifi-off.svg",
    Plus => "plus.svg",
    Close => "x.svg",
    List => "list.svg",
    Grid => "layout-grid.svg",
    Check => "check.svg",
    Sort => "arrow-up-down.svg",
    Remove => "trash-2.svg",
    Rename => "pencil.svg",
}

/// `icon` at `size` px, tinted `color`.
pub fn icon<'a>(icon: Icon, size: f32, color: Color) -> Svg<'a> {
    Svg::new(svg::Handle::from_memory(icon.bytes()))
        .width(size)
        .height(size)
        .style(move |_theme, _status| svg::Style { color: Some(color) })
}

/// The neutral-ramp placeholder model shown until previews exist (milestone 2).
pub fn placeholder<'a>() -> Svg<'a> {
    Svg::new(svg::Handle::from_memory(
        include_bytes!("../assets/placeholder.svg").as_slice(),
    ))
    .width(Length::Fill)
    .height(Length::Fill)
}
