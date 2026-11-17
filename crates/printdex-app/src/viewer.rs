//! The model view's 3D preview: which part is loaded, the camera, and the
//! CPU fallback picture when iced draws without a GPU.
//!
//! Loading parses and decimates on the engine's workers; this only keeps
//! the result. Opening another part or leaving the model view aborts a
//! load that is still running and drops its mesh.

use std::sync::Arc;

use iced::widget::image;
use iced::{Size, Task};
use printdex_core::{Detail, Engine, PartId, Preview, ThumbFailure};
use printdex_mesh::{Bounds, Mesh};
use printdex_render::{Material, RenderOptions, Sphere, View};

/// Largest CPU fallback picture, per side, in pixels.
const MAX_CPU_SIDE: f32 = 2048.0;

/// A loaded mesh, shared with the GPU primitive.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// Changes with every load, so the GPU uploads each mesh once.
    pub id: u64,
    pub mesh: Arc<Mesh>,
    pub sphere: Sphere,
    /// Bounds of the shown (possibly simplified) mesh.
    pub bounds: Bounds,
    pub dims: [f32; 3],
    pub triangles: u64,
    pub decimated: bool,
    pub detail: Detail,
}

#[derive(Clone, Debug, Default)]
pub enum Load {
    #[default]
    Idle,
    Loading(Detail),
    Ready(Loaded),
    Failed(ThumbFailure),
}

#[derive(Clone, Debug)]
pub enum Msg {
    Loaded(u64, Result<Preview, ThumbFailure>),
    /// Drag to orbit, in pixels.
    Orbit(f32, f32),
    /// Drag to pan, in pixels.
    Pan(f32, f32),
    /// Wheel, in lines (positive zooms in).
    Zoom(f32),
    /// Double-click or the reset button.
    Reset,
    /// The pointer let go after a drag.
    Released,
    /// The viewer's size on screen, in logical pixels.
    Resized(Size),
    /// iced draws with its software renderer: show CPU pictures.
    Software,
    /// A CPU picture finished.
    CpuRendered(u64, image::Handle),
    /// Load every triangle.
    FullDetail,
}

#[derive(Debug, Default)]
pub struct Viewer {
    pub part: Option<PartId>,
    pub load: Load,
    pub view: View,
    pub size: Size,
    /// The camera was moved since the model loaded (or the last reset).
    pub touched: bool,
    /// iced runs on its software renderer.
    pub software: bool,
    /// The latest CPU picture of the current view (software renderer).
    pub picture: Option<image::Handle>,
    /// Request counter: results of older requests are ignored.
    request: u64,
    abort: Option<iced::task::Handle>,
}

impl Viewer {
    /// Shows part `part`: starts loading it unless it is already shown.
    pub fn open(&mut self, engine: Option<&Engine>, part: PartId) -> Task<Msg> {
        if self.part == Some(part) && !matches!(self.load, Load::Idle) {
            return Task::none();
        }
        self.close();
        self.part = Some(part);
        self.view = View::default();
        self.touched = false;
        self.start(engine, Detail::Preview)
    }

    /// Stops any load and forgets the mesh.
    pub fn close(&mut self) {
        if let Some(h) = self.abort.take() {
            h.abort();
        }
        self.request += 1;
        self.part = None;
        self.load = Load::Idle;
        self.picture = None;
    }

    fn start(&mut self, engine: Option<&Engine>, detail: Detail) -> Task<Msg> {
        let (Some(engine), Some(part)) = (engine.cloned(), self.part) else {
            return Task::none();
        };
        if let Some(h) = self.abort.take() {
            h.abort();
        }
        self.request += 1;
        let req = self.request;
        self.load = Load::Loading(detail);
        let (task, handle) = Task::perform(
            async move { engine.thumbnails().load_preview(part, detail).await },
            move |r| Msg::Loaded(req, r),
        )
        .abortable();
        self.abort = Some(handle);
        task
    }

    pub fn update(&mut self, engine: Option<&Engine>, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Loaded(req, result) => {
                if req != self.request {
                    return Task::none();
                }
                self.abort = None;
                let detail = match self.load {
                    Load::Loading(d) => d,
                    _ => Detail::Preview,
                };
                self.load = match result {
                    Ok(p) => match (Sphere::of(&p.mesh), p.mesh.bounds()) {
                        (Some(sphere), Some(bounds)) => Load::Ready(Loaded {
                            id: req,
                            sphere,
                            bounds,
                            dims: p.dims,
                            triangles: p.triangles,
                            decimated: p.decimated,
                            mesh: Arc::new(p.mesh),
                            detail,
                        }),
                        _ => Load::Failed(ThumbFailure::Invalid),
                    },
                    Err(e) => Load::Failed(e),
                };
                if !self.touched {
                    self.view = self.home();
                }
                self.render_cpu()
            }
            Msg::Orbit(dx, dy) => {
                self.touched = true;
                self.view.orbit_px(dx, dy);
                Task::none()
            }
            Msg::Pan(dx, dy) => {
                self.touched = true;
                self.view
                    .pan_px(dx, dy, self.size.width.min(self.size.height));
                Task::none()
            }
            Msg::Zoom(lines) => {
                self.touched = true;
                self.view.zoom_lines(lines);
                self.render_cpu()
            }
            Msg::Reset => {
                self.touched = false;
                self.view = self.home();
                self.render_cpu()
            }
            Msg::Released => self.render_cpu(),
            Msg::Resized(size) => {
                if size == self.size {
                    return Task::none();
                }
                self.size = size;
                if !self.touched {
                    self.view = self.home();
                }
                self.render_cpu()
            }
            Msg::Software => {
                if self.software {
                    return Task::none();
                }
                tracing::info!("no GPU for the window; the 3D preview draws on the CPU");
                self.software = true;
                self.render_cpu()
            }
            Msg::CpuRendered(req, handle) => {
                if req == self.request {
                    self.picture = Some(handle);
                }
                Task::none()
            }
            Msg::FullDetail => match &self.load {
                Load::Ready(l) if l.decimated && l.detail == Detail::Preview => {
                    let view = self.view;
                    let task = self.start(engine, Detail::Full);
                    self.view = view;
                    task
                }
                _ => Task::none(),
            },
        }
    }

    /// The starting camera for the loaded model at the current size.
    fn home(&self) -> View {
        match &self.load {
            Load::Ready(l) => {
                printdex_render::home_view(l.sphere, l.bounds, self.size.width, self.size.height)
            }
            _ => View::default(),
        }
    }

    /// With the software renderer, draws the current view on the CPU.
    /// Newer requests win; a picture for an older view is dropped.
    fn render_cpu(&mut self) -> Task<Msg> {
        let Load::Ready(loaded) = &self.load else {
            return Task::none();
        };
        if !self.software || self.size.width < 1.0 || self.size.height < 1.0 {
            return Task::none();
        }
        let scale = (MAX_CPU_SIDE / self.size.width.max(self.size.height)).min(1.0);
        let opts = RenderOptions {
            width: (self.size.width * scale) as u32,
            height: (self.size.height * scale) as u32,
            material: Material::Unknown,
            camera: self.view.camera,
            framing: self.view.framing(),
        };
        let mesh = loaded.mesh.clone();
        let req = self.request;
        Task::perform(
            async move {
                let img =
                    printdex_core::cpu::run_cpu(move || printdex_render::render(&mesh, &opts))
                        .await;
                image::Handle::from_rgba(img.width, img.height, img.rgba)
            },
            move |handle| Msg::CpuRendered(req, handle),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview() -> Preview {
        let mesh = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 5.0]],
        };
        Preview {
            mesh,
            dims: [10.0, 10.0, 5.0],
            triangles: 1,
            decimated: false,
        }
    }

    fn part(n: u8) -> PartId {
        PartId(printdex_core::ids::Uuid::from_bytes([n; 16]))
    }

    #[test]
    fn stale_load_is_ignored() {
        let mut v = Viewer::default();
        let _ = v.open(None, part(1));
        let old = v.request;
        let _ = v.open(None, part(2));
        let _ = v.update(None, Msg::Loaded(old, Ok(preview())));
        assert!(!matches!(v.load, Load::Ready(_)), "old part's mesh dropped");
        let _ = v.update(None, Msg::Loaded(v.request, Ok(preview())));
        assert!(matches!(v.load, Load::Ready(_)));
    }

    #[test]
    fn close_forgets_the_mesh() {
        let mut v = Viewer::default();
        let _ = v.open(None, part(1));
        let req = v.request;
        v.close();
        let _ = v.update(None, Msg::Loaded(req, Ok(preview())));
        assert!(matches!(v.load, Load::Idle));
        assert_eq!(v.part, None);
    }

    #[test]
    fn opening_another_part_resets_the_camera() {
        let mut v = Viewer::default();
        let _ = v.open(None, part(1));
        let _ = v.update(None, Msg::Loaded(v.request, Ok(preview())));
        let _ = v.update(None, Msg::Zoom(3.0));
        assert!(v.view.zoom > 1.0 && v.touched);
        let _ = v.open(None, part(1));
        assert!(v.view.zoom > 1.0, "same part keeps the camera");
        let _ = v.open(None, part(2));
        assert_eq!(v.view, View::default());
        assert!(!v.touched);
    }

    #[test]
    fn cpu_pictures_only_with_the_software_renderer() {
        let mut v = Viewer::default();
        let _ = v.open(None, part(1));
        let _ = v.update(None, Msg::Resized(Size::new(400.0, 300.0)));
        let _ = v.update(None, Msg::Loaded(v.request, Ok(preview())));
        assert_eq!(v.update(None, Msg::Released).units(), 0);
        assert_eq!(v.update(None, Msg::Software).units(), 1);
        assert_eq!(v.update(None, Msg::Released).units(), 1);
        assert_eq!(
            v.update(None, Msg::Orbit(5.0, 0.0)).units(),
            0,
            "orbit waits for release"
        );
    }
}
