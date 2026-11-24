//! Keyboard focus for controls iced can't focus on its own.
//!
//! iced 0.14 only lets text inputs take focus, so Tab skipped every
//! button, segment, chip and switch. [`Focus`] wraps any control and joins
//! it to iced's focus chain: Tab and Shift+Tab (`operation::focus_next` /
//! `focus_previous`) move onto it, it draws the app's focus ring (2px
//! accent outside a 1px gap, DESIGN.md "Focus"), and Enter or Space press
//! it by sending the control a click at its centre, so the control's own
//! message and behaviour stay the single source of truth.
//!
//! Focus only arrives from the keyboard: a mouse click never shows the
//! ring, and clicking anywhere or pressing Escape drops it.
//!
//! [`button`] is a drop-in for `iced::widget::button` that wraps itself,
//! so views keep the builder calls they had.

use std::cell::Cell;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::Widget;
use iced::advanced::widget::operation::{Focusable, Operation};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::keyboard::{self, key};
use iced::widget::button as iced_button;
use iced::{Border, Color, Element, Event, Length, Padding, Rectangle, Size, Vector, mouse};

use crate::app::Message;
use crate::theme;

/// Gap between a control's edge and its ring, and the ring's width.
const GAP: f32 = 1.0;
const RING: f32 = 2.0;

thread_local! {
    /// The current accent, set once per frame by the root view so the
    /// ring follows the colour theme without every caller passing it.
    static ACCENT: Cell<Color> = const { Cell::new(Color::from_rgb(0.98, 0.87, 0.15)) };
}

/// Sets the ring colour for the widgets built in this frame.
pub fn set_accent(color: Color) {
    ACCENT.with(|c| c.set(color));
}

/// Wraps `content` so Tab can reach it. `radius` is the control's corner
/// radius; the ring adds its own offset.
pub fn wrap<'a>(content: impl Into<Element<'a, Message>>, radius: f32) -> Element<'a, Message> {
    Element::new(Focus {
        content: content.into(),
        radius,
        color: ACCENT.with(Cell::get),
    })
}

struct Focus<'a> {
    content: Element<'a, Message>,
    radius: f32,
    color: Color,
}

#[derive(Default)]
struct State {
    focused: bool,
}

impl Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }

    fn focus(&mut self) {
        self.focused = true;
    }

    fn unfocus(&mut self) {
        self.focused = false;
    }
}

impl Widget<Message, iced::Theme, iced::Renderer> for Focus<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<State>();
        operation.focusable(None, layout.bounds(), state);
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let focused = tree.state.downcast_ref::<State>().focused;
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(_)) if focused => {
                tree.state.downcast_mut::<State>().focused = false;
                shell.request_redraw();
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) if focused => {
                match key.as_ref() {
                    keyboard::Key::Named(key::Named::Escape) => {
                        tree.state.downcast_mut::<State>().focused = false;
                        shell.request_redraw();
                    }
                    keyboard::Key::Named(key::Named::Enter | key::Named::Space)
                        if !modifiers.command() && !modifiers.alt() =>
                    {
                        self.press(tree, layout, renderer, clipboard, shell, viewport);
                        shell.capture_event();
                        return;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
        if tree.state.downcast_ref::<State>().focused {
            let out = GAP + RING;
            let b = layout.bounds();
            let ring = Rectangle {
                x: b.x - out,
                y: b.y - out,
                width: b.width + 2.0 * out,
                height: b.height + 2.0 * out,
            };
            <iced::Renderer as renderer::Renderer>::fill_quad(
                renderer,
                Quad {
                    bounds: ring,
                    border: Border {
                        color: self.color,
                        width: RING,
                        radius: (self.radius + out).into(),
                    },
                    ..Quad::default()
                },
                Color::TRANSPARENT,
            );
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl Focus<'_> {
    /// Presses the control as a click at its centre would.
    fn press(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let at = mouse::Cursor::Available(layout.bounds().center());
        // Each synthetic event gets a fresh shell: a control that
        // captures the press would otherwise see the release as already
        // handled and never fire.
        for event in [
            mouse::Event::ButtonPressed(mouse::Button::Left),
            mouse::Event::ButtonReleased(mouse::Button::Left),
        ] {
            let mut messages = Vec::new();
            let mut inner = Shell::new(&mut messages);
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                &Event::Mouse(event),
                layout,
                at,
                renderer,
                clipboard,
                &mut inner,
                viewport,
            );
            shell.merge(inner, |m| m);
        }
    }
}

/// `iced::widget::button` that Tab can reach. Same builder calls; the
/// ring follows the corner radius set with [`Button::radius`] (the
/// control radius by default).
pub fn button<'a>(content: impl Into<Element<'a, Message>>) -> Button<'a> {
    Button {
        inner: iced_button(content),
        radius: theme::RADIUS_CONTROL,
        enabled: false,
    }
}

pub struct Button<'a> {
    inner: iced_button::Button<'a, Message>,
    radius: f32,
    enabled: bool,
}

impl<'a> Button<'a> {
    pub fn on_press(self, message: Message) -> Self {
        Self {
            inner: self.inner.on_press(message),
            enabled: true,
            ..self
        }
    }

    pub fn on_press_maybe(self, message: Option<Message>) -> Self {
        Self {
            enabled: message.is_some(),
            inner: self.inner.on_press_maybe(message),
            ..self
        }
    }

    pub fn style(
        self,
        style: impl Fn(&iced::Theme, iced_button::Status) -> iced_button::Style + 'a,
    ) -> Self {
        Self {
            inner: self.inner.style(style),
            ..self
        }
    }

    pub fn padding(self, padding: impl Into<Padding>) -> Self {
        Self {
            inner: self.inner.padding(padding),
            ..self
        }
    }

    pub fn width(self, width: impl Into<Length>) -> Self {
        Self {
            inner: self.inner.width(width),
            ..self
        }
    }

    pub fn height(self, height: impl Into<Length>) -> Self {
        Self {
            inner: self.inner.height(height),
            ..self
        }
    }

    pub fn clip(self, clip: bool) -> Self {
        Self {
            inner: self.inner.clip(clip),
            ..self
        }
    }

    /// Corner radius for the focus ring.
    pub fn radius(self, radius: f32) -> Self {
        Self { radius, ..self }
    }
}

impl<'a> From<Button<'a>> for Element<'a, Message> {
    fn from(b: Button<'a>) -> Self {
        // A disabled button is skipped by Tab, as on every desktop.
        if b.enabled {
            wrap(b.inner, b.radius)
        } else {
            b.inner.into()
        }
    }
}

/// `iced::widget::toggler` that Tab can reach; Space or Enter flips it.
pub fn toggler<'a>(on: bool) -> Toggler<'a> {
    Toggler {
        inner: iced::widget::toggler(on),
        enabled: false,
    }
}

pub struct Toggler<'a> {
    inner: iced::widget::Toggler<'a, Message>,
    enabled: bool,
}

impl<'a> Toggler<'a> {
    pub fn on_toggle(self, f: impl Fn(bool) -> Message + 'a) -> Self {
        Self {
            inner: self.inner.on_toggle(f),
            enabled: true,
        }
    }

    pub fn on_toggle_maybe(self, f: Option<impl Fn(bool) -> Message + 'a>) -> Self {
        Self {
            enabled: f.is_some(),
            inner: self.inner.on_toggle_maybe(f),
        }
    }

    pub fn size(self, size: impl Into<iced::Pixels>) -> Self {
        Self {
            inner: self.inner.size(size),
            ..self
        }
    }

    pub fn label(self, label: impl iced::widget::text::IntoFragment<'a>) -> Self {
        Self {
            inner: self.inner.label(label),
            ..self
        }
    }

    pub fn text_size(self, size: impl Into<iced::Pixels>) -> Self {
        Self {
            inner: self.inner.text_size(size),
            ..self
        }
    }

    pub fn spacing(self, spacing: impl Into<iced::Pixels>) -> Self {
        Self {
            inner: self.inner.spacing(spacing),
            ..self
        }
    }

    pub fn style(
        self,
        style: impl Fn(&iced::Theme, iced::widget::toggler::Status) -> iced::widget::toggler::Style + 'a,
    ) -> Self {
        Self {
            inner: self.inner.style(style),
            ..self
        }
    }
}

impl<'a> From<Toggler<'a>> for Element<'a, Message> {
    fn from(t: Toggler<'a>) -> Self {
        if t.enabled {
            wrap(t.inner, theme::RADIUS_CHIP)
        } else {
            t.inner.into()
        }
    }
}
