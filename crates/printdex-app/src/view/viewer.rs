//! The 3D preview widget: an iced shader that draws the part with
//! [`printdex_render::GpuViewer`] and turns drags, the wheel and
//! double-clicks into camera messages.
//!
//! When iced runs on its software renderer the shader draws nothing; a
//! small probe notices that renderer and the app shows a CPU picture of
//! the same view instead (re-rendered when a drag ends).

use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Tree, Widget};
use iced::advanced::{Clipboard, Shell, renderer};
use iced::widget::shader::{self, Viewport};
use iced::{Element, Event, Length, Point, Rectangle, Size, keyboard, mouse, wgpu};
use printdex_mesh::Mesh;
use printdex_render::{GpuViewer, Material, Placement, Sphere, View};

use crate::viewer::{Loaded, Msg};

/// Two clicks within this time reset the camera.
const DOUBLE_CLICK: Duration = Duration::from_millis(350);
/// Pixels per wheel line for touchpads that report pixels.
const PIXELS_PER_LINE: f32 = 40.0;

/// What the shader draws: the loaded mesh from the current camera.
#[derive(Clone)]
pub struct Scene {
    pub id: u64,
    pub mesh: Arc<Mesh>,
    pub sphere: Sphere,
    pub view: View,
}

impl std::fmt::Debug for Scene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scene")
            .field("id", &self.id)
            .field("triangles", &self.mesh.triangles())
            .field("view", &self.view)
            .finish()
    }
}

impl Scene {
    pub fn new(loaded: &Loaded, view: View) -> Scene {
        Scene {
            id: loaded.id,
            mesh: loaded.mesh.clone(),
            sphere: loaded.sphere,
            view,
        }
    }
}

/// The shader program. `scene` is `None` while nothing is loaded; the
/// widget still takes input so the size is known early.
#[derive(Debug)]
pub struct Program {
    pub scene: Option<Scene>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragKind {
    Orbit,
    Pan,
}

#[derive(Debug, Default)]
pub struct State {
    drag: Option<(DragKind, Point)>,
    last_click: Option<Instant>,
    shift: bool,
    size: Option<Size>,
}

impl shader::Program<Msg> for Program {
    type State = State;
    type Primitive = Primitive;

    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<Msg>> {
        if state.size != Some(bounds.size()) {
            state.size = Some(bounds.size());
            return Some(shader::Action::publish(Msg::Resized(bounds.size())));
        }
        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                state.shift = m.shift();
                None
            }
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let at = cursor.position_in(bounds)?;
                let kind = match button {
                    mouse::Button::Left if state.shift => DragKind::Pan,
                    mouse::Button::Left => DragKind::Orbit,
                    mouse::Button::Right | mouse::Button::Middle => DragKind::Pan,
                    _ => return None,
                };
                if *button == mouse::Button::Left {
                    let now = Instant::now();
                    if state
                        .last_click
                        .is_some_and(|t| now.duration_since(t) < DOUBLE_CLICK)
                    {
                        state.last_click = None;
                        state.drag = None;
                        return Some(shader::Action::publish(Msg::Reset).and_capture());
                    }
                    state.last_click = Some(now);
                }
                state.drag = Some((kind, at));
                Some(shader::Action::capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let (kind, last) = state.drag?;
                let at = cursor.position_from(bounds.position())?;
                let (dx, dy) = (at.x - last.x, at.y - last.y);
                state.drag = Some((kind, at));
                let msg = match kind {
                    DragKind::Orbit => Msg::Orbit(dx, dy),
                    DragKind::Pan => Msg::Pan(dx, dy),
                };
                Some(shader::Action::publish(msg).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(_)) => {
                state.drag.take()?;
                Some(shader::Action::publish(Msg::Released).and_capture())
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                cursor.position_in(bounds)?;
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / PIXELS_PER_LINE,
                };
                Some(shader::Action::publish(Msg::Zoom(lines)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(&self, _state: &State, _cursor: mouse::Cursor, _bounds: Rectangle) -> Primitive {
        Primitive {
            scene: self.scene.clone(),
        }
    }

    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.drag.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}

#[derive(Debug)]
pub struct Primitive {
    scene: Option<Scene>,
}

/// The viewer's GPU state, kept by iced between frames.
#[derive(Debug)]
pub struct Pipeline {
    viewer: GpuViewer,
    /// Drawn this frame; the mesh is freed after a frame without it.
    used: bool,
    ready: bool,
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Pipeline {
        Pipeline {
            viewer: GpuViewer::new(device, format),
            used: false,
            ready: false,
        }
    }

    fn trim(&mut self) {
        if !self.used {
            self.viewer.clear_mesh();
        }
        self.used = false;
    }
}

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        pipeline.used = true;
        let Some(scene) = &self.scene else {
            pipeline.ready = false;
            return;
        };
        let s = viewport.scale_factor();
        let place = Placement {
            origin: [bounds.x * s, bounds.y * s],
            width: (bounds.width * s).round().max(1.0) as u32,
            height: (bounds.height * s).round().max(1.0) as u32,
        };
        pipeline.viewer.set_mesh(device, scene.id, &scene.mesh);
        pipeline.viewer.prepare(
            device,
            queue,
            scene.sphere,
            &scene.view,
            Material::Unknown,
            place,
        );
        pipeline.ready = true;
    }

    fn render(
        &self,
        pipeline: &Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        if pipeline.ready && self.scene.is_some() {
            pipeline.viewer.render(
                encoder,
                target,
                [
                    clip_bounds.x,
                    clip_bounds.y,
                    clip_bounds.width,
                    clip_bounds.height,
                ],
            );
        }
    }
}

/// The interactive viewer filling its parent.
pub fn shader_view<'a>(scene: Option<Scene>) -> Element<'a, Msg> {
    iced::widget::shader(Program { scene })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// An invisible widget that reports when iced draws with its software
/// renderer, where shader widgets draw nothing.
pub struct SoftwareProbe<Message> {
    on_software: Message,
}

pub fn software_probe<'a, Message: Clone + 'a>(on_software: Message) -> Element<'a, Message> {
    Element::new(SoftwareProbe { on_software })
}

impl<Message: Clone, Theme> Widget<Message, Theme, iced::Renderer> for SoftwareProbe<Message> {
    fn size(&self) -> Size<Length> {
        // Not zero: rows and columns drop children of zero size.
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        _limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(Size::ZERO)
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }

    fn update(
        &mut self,
        _tree: &mut Tree,
        _event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        if matches!(renderer, iced::Renderer::Secondary(_)) {
            shell.publish(self.on_software.clone());
        }
    }
}
