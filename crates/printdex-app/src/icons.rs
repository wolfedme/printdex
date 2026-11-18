//! Bundled Lucide icons (ISC) and the placeholder preview, as SVG handles.

use iced::widget::svg::{self, Svg};
use iced::widget::{pin, stack};
use iced::{Color, ContentFit, Element, Length};

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
    Chevron => "chevron-right.svg",
    Settings => "settings.svg",
    Offline => "wifi-off.svg",
    Plus => "plus.svg",
    Close => "x.svg",
    List => "list.svg",
    Grid => "layout-grid.svg",
    Check => "check.svg",
    Remove => "trash-2.svg",
    Rename => "pencil.svg",
    Tag => "tag.svg",
    Collection => "bookmark.svg",
    Success => "circle-check.svg",
    LogPrint => "clipboard-pen.svg",
    Tip => "lightbulb.svg",
    AddPhoto => "image-plus.svg",
    Sparkle => "sparkle.svg",
    NeverPrinted => "circle-dashed.svg",
    Failed => "circle-x.svg",
    Recent => "history.svg",
    Link => "link.svg",
    Notes => "notebook-pen.svg",
    Info => "info.svg",
    Move => "folder-input.svg",
    Undo => "undo-2.svg",
    App => "app-window.svg",
    ChevronDown => "chevron-down.svg",
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

/// Size of the placeholder's view box.
const PLACEHOLDER_W: f32 = 160.0;
const PLACEHOLDER_H: f32 = 120.0;

/// The placeholder model fitted into a `width` × `height` box, lifted
/// `lift` px off its contact shadow, which is scaled by `shadow_scale`
/// (the hover motion in DESIGN.md).
pub fn placeholder_lifted<'a, M: 'a>(
    width: f32,
    height: f32,
    lift: f32,
    shadow_scale: f32,
) -> Element<'a, M> {
    on_bed(width, height, lift, shadow_scale, |w, h| {
        Svg::new(svg::Handle::from_memory(
            include_bytes!("../assets/placeholder-model.svg").as_slice(),
        ))
        .width(w)
        .height(h)
        .into()
    })
}

/// A 4:3 model picture fitted into a `width` × `height` box over its
/// contact shadow, lifted `lift` px, the shadow scaled by `shadow_scale`.
/// Rendered thumbnails and the placeholder share this geometry: the
/// model's lowest point sits 80% down, where the shadow is.
pub fn on_bed<'a, M: 'a>(
    width: f32,
    height: f32,
    lift: f32,
    shadow_scale: f32,
    model: impl FnOnce(f32, f32) -> Element<'a, M>,
) -> Element<'a, M> {
    let fit = (width / PLACEHOLDER_W).min(height / PLACEHOLDER_H).max(0.0);
    let (w, h) = (PLACEHOLDER_W * fit, PLACEHOLDER_H * fit);
    let (x, y) = ((width - w) / 2.0, (height - h) / 2.0);
    let sw = w * shadow_scale;
    let shadow = Svg::new(svg::Handle::from_memory(
        include_bytes!("../assets/placeholder-shadow.svg").as_slice(),
    ))
    .content_fit(ContentFit::Fill)
    .width(sw)
    .height(h);
    stack![
        pin(shadow).x(x + (w - sw) / 2.0).y(y),
        pin(model(w, h)).x(x).y(y - lift),
    ]
    .width(width)
    .height(height)
    .into()
}
