//! A button with a menu that drops down over the content below it, so
//! opening it never pushes the layout around. The menu's right edge lines
//! up with the button's; it opens upwards when there is no room below.

use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, mouse};

/// Space between the button and the menu.
const GAP: f32 = 4.0;

pub struct Dropdown<'a, Message, Theme, Renderer> {
    anchor: Element<'a, Message, Theme, Renderer>,
    menu: Option<Element<'a, Message, Theme, Renderer>>,
    width: f32,
    on_dismiss: Option<Message>,
}

/// `anchor` with `menu` below it while `menu` is `Some`; the menu is laid
/// out `width` wide.
pub fn dropdown<'a, Message, Theme, Renderer>(
    anchor: impl Into<Element<'a, Message, Theme, Renderer>>,
    menu: Option<Element<'a, Message, Theme, Renderer>>,
    width: f32,
) -> Dropdown<'a, Message, Theme, Renderer> {
    Dropdown {
        anchor: anchor.into(),
        menu,
        width,
        on_dismiss: None,
    }
}

impl<Message, Theme, Renderer> Dropdown<'_, Message, Theme, Renderer> {
    /// Sent on a click outside the menu and the anchor.
    pub fn on_dismiss(mut self, msg: Message) -> Self {
        self.on_dismiss = Some(msg);
        self
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Dropdown<'_, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: renderer::Renderer,
{
    fn children(&self) -> Vec<Tree> {
        let mut c = vec![Tree::new(&self.anchor)];
        if let Some(m) = &self.menu {
            c.push(Tree::new(m));
        }
        c
    }

    fn diff(&self, tree: &mut Tree) {
        match &self.menu {
            Some(m) => tree.diff_children(&[self.anchor.as_widget(), m.as_widget()]),
            None => tree.diff_children(&[self.anchor.as_widget()]),
        }
    }

    fn size(&self) -> Size<Length> {
        self.anchor.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.anchor.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.anchor
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.anchor.as_widget_mut().update(
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
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.anchor.as_widget().mouse_interaction(
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
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.anchor.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.anchor
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        _renderer: &Renderer,
        _viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let menu = self.menu.as_mut()?;
        let tree = tree.children.get_mut(1)?;
        let b = layout.bounds();
        Some(overlay::Element::new(Box::new(Menu {
            menu,
            tree,
            anchor: Point::new(b.x + b.width, b.y + b.height + GAP) + translation,
            button: b + translation,
            width: self.width,
            on_dismiss: self.on_dismiss.clone(),
        })))
    }
}

struct Menu<'a, 'b, Message, Theme, Renderer> {
    menu: &'b mut Element<'a, Message, Theme, Renderer>,
    tree: &'b mut Tree,
    /// Top-right corner of the menu.
    anchor: Point,
    /// Where the anchor button is, so its own click isn't a dismissal.
    button: Rectangle,
    width: f32,
    on_dismiss: Option<Message>,
}

impl<Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for Menu<'_, '_, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: renderer::Renderer,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let limits = layout::Limits::new(Size::ZERO, Size::new(self.width, bounds.height));
        let node = self
            .menu
            .as_widget_mut()
            .layout(self.tree, renderer, &limits.width(self.width));
        let Size {
            width: w,
            height: h,
        } = node.size();
        let x = (self.anchor.x - w).clamp(0.0, (bounds.width - w).max(0.0));
        // No room below (a button near the bottom of the window): open
        // upwards instead.
        let y = if self.anchor.y + h > bounds.height && self.button.y - GAP - h >= 0.0 {
            self.button.y - GAP - h
        } else {
            self.anchor.y
        };
        node.move_to(Point::new(x, y))
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.menu.as_widget().draw(
            self.tree,
            renderer,
            theme,
            style,
            layout,
            cursor,
            &layout.bounds(),
        );
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.menu
            .as_widget_mut()
            .operate(self.tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let bounds = layout.bounds();
        if let Event::Mouse(mouse::Event::ButtonPressed(_)) = event
            && let Some(msg) = &self.on_dismiss
            && !cursor.is_over(bounds)
            && !cursor.is_over(self.button)
        {
            shell.publish(msg.clone());
        }
        self.menu.as_widget_mut().update(
            self.tree, event, layout, cursor, renderer, clipboard, shell, &bounds,
        );
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.menu.as_widget().mouse_interaction(
            self.tree,
            layout,
            cursor,
            &layout.bounds(),
            renderer,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<Dropdown<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: renderer::Renderer + 'a,
{
    fn from(d: Dropdown<'a, Message, Theme, Renderer>) -> Self {
        Element::new(d)
    }
}
