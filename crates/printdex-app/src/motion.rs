//! Motion (DESIGN.md "Motion"): durations, easing and the small amount of
//! animation state the views read. Everything here is pure: callers pass
//! the current time, so tests can step through an animation.
//!
//! Animations start in `update()` on events and only change colours,
//! alpha and offsets. While one runs, the app subscribes to window frames;
//! when none runs, it does not.

use std::time::{Duration, Instant};

use std::collections::HashMap;

use printdex_core::{ModelId, ThumbTarget};

use crate::filters::Chip;
use crate::theme::Palette;

/// Press feedback.
pub const INSTANT: Duration = Duration::from_millis(90);
/// Hover, selection ring, state colours.
pub const QUICK: Duration = Duration::from_millis(160);
/// Panels and sliding indicators (the log form).
pub const MOVE: Duration = Duration::from_millis(260);
/// The print-in.
pub const FOCAL: Duration = Duration::from_millis(720);
/// Filter chips scaling in.
pub const CHIP: Duration = Duration::from_millis(180);
/// Colour theme and light/dark changes.
pub const THEME: Duration = Duration::from_millis(240);

/// Print-in: layers, delay between items, longest delay, the fade that
/// replaces it under Reduce motion, and the "Rendering preview" label's
/// fade-out.
pub const PRINT_LAYERS: u32 = 16;
pub const PRINT_STAGGER: Duration = Duration::from_millis(30);
pub const PRINT_STAGGER_CAP: Duration = Duration::from_millis(330);
pub const PRINT_FADE: Duration = Duration::from_millis(150);
pub const LABEL_FADE: Duration = Duration::from_millis(120);
/// Where rendered thumbnails stand: the bed line, as a fraction of the
/// image height from the top (printdex-render's thumbnail margins).
pub const BED_LINE: f32 = 0.8;

/// How a thumbnail shows right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reveal {
    Shown,
    /// Printing in: rows above `clip` (a fraction of the image height
    /// from the top) are hidden; `label` is the fading "Rendering
    /// preview" note's opacity.
    Build {
        clip: f32,
        label: f32,
    },
    /// Reduce motion: fading in.
    Fade(f32),
}

/// Hover lift of the placeholder model, in px.
pub const LIFT: f32 = 4.0;
/// Contact shadow scale at full lift.
pub const SHADOW_LIFTED: f32 = 0.82;
/// Press sink, in px.
pub const SINK: f32 = 1.0;
/// Chip scale when it appears.
pub const CHIP_FROM: f32 = 0.92;

/// `cubic-bezier(x1, y1, x2, y2)` at time `t` in `0..=1`.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let bez = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    // Find s with x(s) = t by bisection: monotonic for valid curves.
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if bez(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y1, y2, (lo + hi) / 2.0)
}

/// DESIGN.md `ease-out`: cubic-bezier(0.16, 1, 0.3, 1).
pub fn ease_out(t: f32) -> f32 {
    cubic_bezier(0.16, 1.0, 0.3, 1.0, t)
}

/// Eased progress of an animation that started at `start` and lasts
/// `duration`, at `now`.
pub fn progress(start: Instant, duration: Duration, now: Instant) -> f32 {
    let elapsed = now.saturating_duration_since(start).as_secs_f32();
    let total = duration.as_secs_f32();
    if total <= 0.0 {
        return 1.0;
    }
    ease_out(elapsed / total)
}

fn running(start: Instant, duration: Duration, now: Instant) -> bool {
    now.saturating_duration_since(start) < duration
}

/// Animation state read by the views.
#[derive(Clone, Debug)]
pub struct Motion {
    /// Reduce motion: no lift, sink or scale; colour fades stay.
    pub reduce: bool,
    /// The time views draw for (the last frame, or the last event).
    pub now: Instant,
    hover_in: Option<(ModelId, Instant)>,
    hover_out: Option<(ModelId, Instant)>,
    /// Selection change: previous, new, start.
    select: Option<(Option<ModelId>, Option<ModelId>, Instant)>,
    press: Option<(ModelId, Instant)>,
    chip: Option<(Chip, Instant)>,
    /// Part tile selection change: previous, new, start.
    part: Option<(usize, usize, Instant)>,
    /// The palette before a theme change, and when it started.
    palette: Option<(Palette, Instant)>,
    /// Running print-ins: start and the model's top edge (a fraction of
    /// the image height).
    print_ins: HashMap<ThumbTarget, (Instant, f32)>,
    /// Start of the latest print-in, for the stagger.
    last_print_in: Option<Instant>,
    /// The side-column panel (log form) opened.
    panel: Option<Instant>,
}

/// How far the log form slides in from, in px.
pub const PANEL_SLIDE: f32 = 24.0;

impl Motion {
    pub fn new(now: Instant) -> Motion {
        Motion {
            reduce: false,
            now,
            hover_in: None,
            hover_out: None,
            select: None,
            press: None,
            chip: None,
            part: None,
            palette: None,
            print_ins: HashMap::new(),
            last_print_in: None,
            panel: None,
        }
    }

    /// A thumbnail was just rendered: it prints in once, starting
    /// [`PRINT_STAGGER`] after the previous one (at most
    /// [`PRINT_STAGGER_CAP`] from now).
    pub fn print_in(&mut self, target: ThumbTarget, top: f32, now: Instant) {
        self.now = now;
        let start = match self.last_print_in {
            Some(prev) if prev + PRINT_STAGGER > now => {
                (prev + PRINT_STAGGER).min(now + PRINT_STAGGER_CAP)
            }
            _ => now,
        };
        self.last_print_in = Some(start);
        let total = self.print_duration();
        self.print_ins
            .retain(|_, (s, _)| running(*s, total + PRINT_STAGGER_CAP, now));
        self.print_ins
            .insert(target, (start, top.clamp(0.0, BED_LINE)));
    }

    fn print_duration(&self) -> Duration {
        if self.reduce { PRINT_FADE } else { FOCAL }
    }

    /// How `target`'s thumbnail shows now.
    pub fn reveal(&self, target: ThumbTarget) -> Reveal {
        let Some(&(start, top)) = self.print_ins.get(&target) else {
            return Reveal::Shown;
        };
        let elapsed = self.now.saturating_duration_since(start);
        let before = self.now < start;
        if self.reduce {
            if !before && elapsed >= PRINT_FADE {
                return Reveal::Shown;
            }
            let t = if before {
                0.0
            } else {
                elapsed.as_secs_f32() / PRINT_FADE.as_secs_f32()
            };
            return Reveal::Fade(ease_out(t));
        }
        if !before && elapsed >= FOCAL {
            return Reveal::Shown;
        }
        // Even steps in time, so every layer shows.
        let layers = if before {
            0
        } else {
            let t = elapsed.as_secs_f32() / FOCAL.as_secs_f32();
            ((t * PRINT_LAYERS as f32).floor() as u32 + 1).min(PRINT_LAYERS)
        };
        let label = if before {
            1.0
        } else {
            1.0 - progress(start, LABEL_FADE, self.now)
        };
        Reveal::Build {
            clip: BED_LINE - (BED_LINE - top) * layers as f32 / PRINT_LAYERS as f32,
            label,
        }
    }

    pub fn hover_changed(&mut self, old: Option<ModelId>, new: Option<ModelId>, now: Instant) {
        self.now = now;
        if old == new {
            return;
        }
        if let Some(id) = old {
            // Fall back from wherever the lift had got to.
            let lifted = self.hover_amount(id);
            let back = now.checked_sub(QUICK.mul_f32(1.0 - lifted)).unwrap_or(now);
            self.hover_out = Some((id, back));
        }
        self.hover_in = new.map(|id| (id, now));
    }

    pub fn selection_changed(&mut self, old: Option<ModelId>, new: Option<ModelId>, now: Instant) {
        self.now = now;
        if old != new {
            self.select = Some((old, new, now));
        }
    }

    pub fn pressed(&mut self, id: ModelId, now: Instant) {
        self.now = now;
        self.press = Some((id, now));
    }

    /// The log form slides in from the right.
    pub fn panel_opened(&mut self, now: Instant) {
        self.now = now;
        self.panel = Some(now);
    }

    /// The log form's offset from its place (px) and opacity.
    pub fn panel(&self) -> (f32, f32) {
        match self.panel {
            Some(start) if !self.reduce && running(start, MOVE, self.now) => {
                let p = progress(start, MOVE, self.now);
                (PANEL_SLIDE * (1.0 - p), p)
            }
            _ => (0.0, 1.0),
        }
    }

    pub fn chip_added(&mut self, chip: Chip, now: Instant) {
        self.now = now;
        self.chip = Some((chip, now));
    }

    pub fn part_changed(&mut self, old: usize, new: usize, now: Instant) {
        self.now = now;
        if old != new {
            self.part = Some((old, new, now));
        }
    }

    pub fn palette_changed(&mut self, old: Palette, now: Instant) {
        // A change during a change starts from what is on screen.
        let from = self.palette(old);
        self.now = now;
        self.palette = Some((from, now));
    }

    /// Hover amount of `id`, eased, `0..=1`.
    pub fn hover_amount(&self, id: ModelId) -> f32 {
        match (self.hover_in, self.hover_out) {
            (Some((h, start)), _) if h == id => progress(start, QUICK, self.now),
            (_, Some((h, start))) if h == id => 1.0 - progress(start, QUICK, self.now),
            _ => 0.0,
        }
    }

    /// How far the placeholder model of `id` is lifted, in px.
    pub fn lift(&self, id: ModelId) -> f32 {
        if self.reduce {
            0.0
        } else {
            LIFT * self.hover_amount(id)
        }
    }

    /// Scale of the contact shadow of `id`.
    pub fn shadow_scale(&self, id: ModelId) -> f32 {
        if self.reduce {
            1.0
        } else {
            1.0 - (1.0 - SHADOW_LIFTED) * self.hover_amount(id)
        }
    }

    /// How selected `id` looks, `0..=1`; `selected` is the current selection.
    pub fn selection(&self, id: ModelId, selected: Option<ModelId>) -> f32 {
        if let Some((old, new, start)) = self.select
            && running(start, QUICK, self.now)
        {
            let p = progress(start, QUICK, self.now);
            if new == Some(id) {
                return p;
            }
            if old == Some(id) {
                return 1.0 - p;
            }
        }
        if selected == Some(id) { 1.0 } else { 0.0 }
    }

    /// Press sink of `id`, in px: down at once, back up over [`INSTANT`].
    pub fn sink(&self, id: ModelId) -> f32 {
        match self.press {
            Some((p, start)) if p == id && !self.reduce => {
                SINK * (1.0 - progress(start, INSTANT, self.now))
            }
            _ => 0.0,
        }
    }

    /// Scale of a filter chip.
    pub fn chip_scale(&self, chip: Chip) -> f32 {
        match self.chip {
            Some((c, start)) if c == chip && !self.reduce => {
                CHIP_FROM + (1.0 - CHIP_FROM) * progress(start, CHIP, self.now)
            }
            _ => 1.0,
        }
    }

    /// How selected part tile `index` looks, `0..=1`.
    pub fn part(&self, index: usize, current: usize) -> f32 {
        if let Some((old, new, start)) = self.part
            && running(start, QUICK, self.now)
        {
            let p = progress(start, QUICK, self.now);
            if new == index {
                return p;
            }
            if old == index {
                return 1.0 - p;
            }
        }
        if index == current { 1.0 } else { 0.0 }
    }

    /// The palette to draw: `current`, or a cross-fade into it.
    pub fn palette(&self, current: Palette) -> Palette {
        match self.palette {
            Some((from, start)) if running(start, THEME, self.now) => {
                Palette::blend(&from, &current, progress(start, THEME, self.now))
            }
            _ => current,
        }
    }

    /// Whether anything is still moving at `now`; the app asks for frames
    /// only while this holds.
    pub fn animating(&self, now: Instant) -> bool {
        let quick = [
            self.hover_in.map(|(_, s)| s),
            self.hover_out.map(|(_, s)| s),
            self.select.map(|(_, _, s)| s),
            self.part.map(|(_, _, s)| s),
        ];
        quick.into_iter().flatten().any(|s| running(s, QUICK, now))
            || self.press.is_some_and(|(_, s)| running(s, INSTANT, now))
            || self.chip.is_some_and(|(_, s)| running(s, CHIP, now))
            || self.panel.is_some_and(|s| running(s, MOVE, now))
            || self.palette.is_some_and(|(_, s)| running(s, THEME, now))
            || self
                .last_print_in
                .is_some_and(|s| now < s || running(s, self.print_duration(), now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorTheme, Mode, Surface};
    use printdex_core::PrintableFormat;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn durations_match_design() {
        assert_eq!(INSTANT, ms(90));
        assert_eq!(QUICK, ms(160));
        assert_eq!(MOVE, ms(260));
        assert_eq!(FOCAL, ms(720));
        assert_eq!(CHIP, ms(180));
        assert_eq!(THEME, ms(240));
    }

    #[test]
    fn ease_out_curve_endpoints() {
        assert!(ease_out(0.0).abs() < 1e-4);
        assert!((ease_out(1.0) - 1.0).abs() < 1e-4);
        // Front-loaded: most of the way there by the halfway point.
        assert!(ease_out(0.5) > 0.85, "{}", ease_out(0.5));
        let mut last = 0.0;
        for i in 1..=20 {
            let v = ease_out(i as f32 / 20.0);
            assert!(v >= last - 1e-4, "monotonic");
            last = v;
        }
        // A linear bezier is the identity.
        assert!((cubic_bezier(0.0, 0.0, 1.0, 1.0, 0.3) - 0.3).abs() < 1e-3);
    }

    #[test]
    fn hover_lift_settles() {
        let t0 = Instant::now();
        let id = ModelId::new();
        let mut m = Motion::new(t0);
        m.hover_changed(None, Some(id), t0);
        assert!(m.lift(id) < 0.01);
        m.now = t0 + ms(80);
        let mid = m.lift(id);
        assert!(mid > 0.0 && mid < LIFT);
        m.now = t0 + QUICK;
        assert!((m.lift(id) - LIFT).abs() < 1e-3);
        assert!((m.shadow_scale(id) - SHADOW_LIFTED).abs() < 1e-3);
        // Leaving drops it back to the bed.
        m.hover_changed(Some(id), None, t0 + ms(400));
        m.now = t0 + ms(400) + QUICK;
        assert!(m.lift(id).abs() < 1e-3);
        assert!((m.shadow_scale(id) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn reduce_motion_disables_movement_keeps_fades() {
        let t0 = Instant::now();
        let (a, b) = (ModelId::new(), ModelId::new());
        let mut m = Motion::new(t0);
        m.reduce = true;
        m.hover_changed(None, Some(a), t0);
        m.pressed(a, t0);
        let chip = Chip::Format(PrintableFormat::Stl);
        m.chip_added(chip, t0);
        m.selection_changed(None, Some(b), t0);
        m.now = t0 + ms(40);
        assert_eq!(m.lift(a), 0.0);
        assert_eq!(m.shadow_scale(a), 1.0);
        assert_eq!(m.sink(a), 0.0);
        assert_eq!(m.chip_scale(chip), 1.0);
        // The selection colour still fades.
        let s = m.selection(b, Some(b));
        assert!(s > 0.0 && s < 1.0, "{s}");
    }

    #[test]
    fn selection_cross_fades() {
        let t0 = Instant::now();
        let (a, b) = (ModelId::new(), ModelId::new());
        let mut m = Motion::new(t0);
        assert_eq!(m.selection(a, Some(a)), 1.0);
        m.selection_changed(Some(a), Some(b), t0);
        m.now = t0 + ms(40);
        let (fa, fb) = (m.selection(a, Some(b)), m.selection(b, Some(b)));
        assert!(fa > 0.0 && fa < 1.0 && fb > 0.0 && fb < 1.0);
        assert!((fa + fb - 1.0).abs() < 1e-4);
        m.now = t0 + QUICK;
        assert_eq!(m.selection(a, Some(b)), 0.0);
        assert_eq!(m.selection(b, Some(b)), 1.0);
    }

    fn part(n: u128) -> ThumbTarget {
        ThumbTarget::Part(printdex_core::PartId(printdex_core::ids::Uuid::from_u128(
            n,
        )))
    }

    #[test]
    fn print_in_builds_in_sixteen_layers() {
        let t0 = Instant::now();
        let mut m = Motion::new(t0);
        assert_eq!(
            m.reveal(part(1)),
            Reveal::Shown,
            "cached thumbnails just show"
        );
        m.print_in(part(1), 0.1, t0);
        let mut clips = Vec::new();
        for i in 0..=72 {
            m.now = t0 + ms(i * 10);
            if let Reveal::Build { clip, .. } = m.reveal(part(1)) {
                clips.push(clip);
            }
        }
        clips.dedup();
        assert_eq!(clips.len(), PRINT_LAYERS as usize, "{clips:?}");
        assert!(clips.windows(2).all(|w| w[1] < w[0]), "grows upwards");
        assert!(
            (clips[clips.len() - 1] - 0.1).abs() < 1e-4,
            "reaches the top"
        );
        assert!(m.animating(t0 + ms(700)));
        m.now = t0 + FOCAL;
        assert_eq!(m.reveal(part(1)), Reveal::Shown);
        assert!(!m.animating(t0 + FOCAL));
    }

    #[test]
    fn print_in_staggers_and_caps() {
        let t0 = Instant::now();
        let mut m = Motion::new(t0);
        for i in 0..20 {
            m.print_in(part(i), 0.1, t0);
        }
        m.now = t0 + ms(15);
        // The first has started, the second waits 30 ms.
        assert!(matches!(m.reveal(part(0)), Reveal::Build { clip, .. } if clip < BED_LINE));
        assert!(matches!(m.reveal(part(1)), Reveal::Build { clip, .. } if clip == BED_LINE));
        // The twentieth waits no more than 330 ms.
        m.now = t0 + PRINT_STAGGER_CAP + ms(1);
        assert!(matches!(m.reveal(part(19)), Reveal::Build { clip, .. } if clip < BED_LINE));
    }

    #[test]
    fn reduce_motion_fades_print_in() {
        let t0 = Instant::now();
        let mut m = Motion::new(t0);
        m.reduce = true;
        m.print_in(part(1), 0.1, t0);
        m.now = t0 + ms(50);
        assert!(matches!(m.reveal(part(1)), Reveal::Fade(a) if a > 0.0 && a < 1.0));
        m.now = t0 + PRINT_FADE;
        assert_eq!(m.reveal(part(1)), Reveal::Shown);
    }

    #[test]
    fn only_animating_requests_frames() {
        let t0 = Instant::now();
        let id = ModelId::new();
        let mut m = Motion::new(t0);
        assert!(!m.animating(t0));
        m.pressed(id, t0);
        assert!(m.animating(t0 + ms(50)));
        assert!(!m.animating(t0 + INSTANT));
        let p = Palette::new(ColorTheme::default(), Mode::Light, Surface::Solid);
        m.palette_changed(p, t0);
        assert!(m.animating(t0 + ms(200)));
        assert!(!m.animating(t0 + THEME));
        let dark = Palette::new(ColorTheme::default(), Mode::Dark, Surface::Solid);
        m.now = t0 + THEME;
        assert_eq!(m.palette(dark), dark);
    }
}
