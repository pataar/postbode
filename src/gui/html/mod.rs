//! The HTML view of a message: the page drawn from strips the render thread paints, with link hover and click.
//! `app.rs` owns `HtmlState` and talks to the render thread; this module draws and returns `UiAction`s.
#[cfg(feature = "html")]
mod net;
#[cfg(feature = "html")]
mod render;
#[cfg(feature = "html")]
#[cfg(test)]
pub(crate) mod view_tests;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use eframe::egui;

use crate::message::{HtmlBody, InlinePart};

use super::app::UiAction;
use super::body;

/// Strip height in physical pixels.
pub(crate) const STRIP: u32 = 512;

/// Layout width in points never goes below this; narrower panels scroll sideways.
pub(crate) const MIN_WIDTH: f32 = 320.0;

/// HTML larger than this shows as text.
pub(crate) const MAX_HTML: usize = 2 * 1024 * 1024;

/// A laid-out page taller than this, in points, shows as text.
pub(crate) const MAX_HEIGHT: f32 = 200_000.0;

/// Seconds the panel width must hold still before the page is laid out again.
pub(crate) const SETTLE: f64 = 0.1;

pub(crate) const TOO_LARGE: &str = "Too large to render; showing text.";
pub(crate) const FAILED: &str = "Could not render HTML; showing text.";

#[cfg(feature = "html")]
pub(crate) use render::Renderer;

/// Without the `html` feature nothing renders: messages never have an HTML body, so this is never sent to.
#[cfg(not(feature = "html"))]
pub(crate) struct Renderer;

#[cfg(not(feature = "html"))]
impl Renderer {
    pub fn start(_ctx: egui::Context) -> Renderer {
        Renderer
    }

    pub fn send(&mut self, _request: Request) {}

    pub fn replies(&self) -> Vec<Reply> {
        Vec::new()
    }
}

/// The HTML body of a raw message when this build renders HTML.
pub(crate) fn html_of(raw: &[u8]) -> Option<HtmlBody> {
    if cfg!(feature = "html") {
        crate::message::html_body(raw)
    } else {
        None
    }
}

// Without the `html` feature there is no render thread to read or build these.
#[cfg_attr(not(feature = "html"), allow(dead_code))]
pub(crate) struct Load {
    pub generation: u64,
    pub html: String,
    pub inline: Vec<InlinePart>,
    pub scale: f32,
    /// In points.
    pub width: f32,
}

// Without the `html` feature there is no render thread to read or build these.
#[cfg_attr(not(feature = "html"), allow(dead_code))]
pub(crate) enum Request {
    Load(Load),
    Paint {
        generation: u64,
        strip: u32,
    },
    /// A point in the page, in points.
    Hit {
        generation: u64,
        x: f32,
        y: f32,
    },
    /// The text between two points in the page, in points; the same point twice clears the selection.
    Select {
        generation: u64,
        from: egui::Pos2,
        to: egui::Pos2,
    },
}

// Without the `html` feature there is no render thread to read or build these.
#[cfg_attr(not(feature = "html"), allow(dead_code))]
impl Request {
    pub fn generation(&self) -> u64 {
        match self {
            Request::Load(load) => load.generation,
            Request::Paint { generation, .. }
            | Request::Hit { generation, .. }
            | Request::Select { generation, .. } => *generation,
        }
    }
}

// Without the `html` feature there is no render thread to read or build these.
#[cfg_attr(not(feature = "html"), allow(dead_code))]
pub(crate) enum Reply {
    Failed {
        generation: u64,
    },
    /// The page size in points; `remote` is set when it asked for remote content.
    Laid {
        generation: u64,
        height: f32,
        remote: bool,
        width: f32,
    },
    Link {
        generation: u64,
        href: Option<String>,
    },
    /// The selected text; the strips painted before it lack its highlight.
    Selected {
        generation: u64,
        text: Option<String>,
    },
    Strip {
        generation: u64,
        image: egui::ColorImage,
        strip: u32,
    },
}

impl Reply {
    pub fn generation(&self) -> u64 {
        match self {
            Reply::Failed { generation }
            | Reply::Laid { generation, .. }
            | Reply::Link { generation, .. }
            | Reply::Selected { generation, .. }
            | Reply::Strip { generation, .. } => *generation,
        }
    }
}

/// The HTML view of the shown message: what is laid out, which strips are painted, what the pointer is over.
pub(crate) struct HtmlState {
    /// The layout in flight or shown; replies for any other generation are stale.
    pub generation: u64,
    pub hover: Option<String>,
    /// The last point sent as a hit test, so a still pointer sends nothing.
    pub hovered_at: Option<egui::Pos2>,
    /// Set once the current generation is laid out: the page size in points.
    pub page: Option<egui::Vec2>,
    pub remote: bool,
    pub requested: BTreeSet<u32>,
    /// The two points of the last `Select`, so a still drag sends nothing.
    pub selected_at: Option<(egui::Pos2, egui::Pos2)>,
    pub selection: Option<String>,
    /// The scale and width (points) of the last `Load`.
    pub sent: (f32, f32),
    pub strips: BTreeMap<u32, egui::TextureHandle>,
    /// A different width or scale than `sent`, and when it was first seen.
    pub wanted: Option<((f32, f32), f64)>,
}

impl HtmlState {
    pub fn new(generation: u64, scale: f32, width: f32) -> HtmlState {
        HtmlState {
            generation,
            hover: None,
            hovered_at: None,
            page: None,
            remote: false,
            requested: BTreeSet::new(),
            selected_at: None,
            selection: None,
            sent: (scale, width),
            strips: BTreeMap::new(),
            wanted: None,
        }
    }
}

/// The strips covering `top..bottom` (points) plus one screen either side, within a page `height` points tall.
pub(crate) fn wanted_strips(top: f32, bottom: f32, height: f32, scale: f32) -> Range<u32> {
    let screen = (bottom - top).max(1.0);
    let points = STRIP as f32 / scale.max(0.5);
    let first = ((top - screen).max(0.0) / points).floor() as u32;
    let end = ((bottom + screen).min(height) / points).ceil() as u32;
    first..end.max(first)
}

/// Strips more than three screens from `top..bottom` (points), whose textures can go.
pub(crate) fn far_strips(
    painted: impl Iterator<Item = u32>,
    top: f32,
    bottom: f32,
    scale: f32,
) -> Vec<u32> {
    let screen = (bottom - top).max(1.0);
    let points = STRIP as f32 / scale.max(0.5);
    painted
        .filter(|&strip| {
            let (start, end) = (strip as f32 * points, (strip + 1) as f32 * points);
            end < top - 3.0 * screen || start > bottom + 3.0 * screen
        })
        .collect()
}

/// A link the HTML view may open: absolute, with one of the text view's schemes.
pub(crate) fn openable(href: &str) -> Option<&str> {
    let href = href.trim();
    body::has_target(href).then_some(href)
}

/// The page `width` points wide, its note, and what the pointer does over it; "Rendering…" until it is laid out.
pub(crate) fn show(state: Option<&HtmlState>, width: f32, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let width = width.max(MIN_WIDTH);
    let scale = ui.ctx().pixels_per_point();
    actions.push(UiAction::HtmlWidth { scale, width });
    let Some((state, page)) = state.and_then(|s| Some((s, s.page?))) else {
        ui.weak("Rendering…");
        return actions;
    };
    if state.remote {
        ui.weak("Remote content not loaded.");
    }
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(page.x.max(width), page.y),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::WHITE);
    let points = STRIP as f32 / scale;
    for (&strip, texture) in &state.strips {
        let size = texture.size_vec2() / scale;
        let min = rect.min + egui::vec2(0.0, strip as f32 * points);
        painter.image(
            texture.id(),
            egui::Rect::from_min_size(min, size),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    painter.rect_stroke(
        rect,
        0.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let visible = ui.clip_rect().intersect(rect);
    if visible.is_positive() {
        actions.push(UiAction::HtmlVisible {
            top: visible.min.y - rect.min.y,
            bottom: visible.max.y - rect.min.y,
        });
    }
    let in_page = |pos: egui::Pos2| (pos - rect.min).to_pos2();
    let pointer = response.hover_pos().map(in_page);
    if pointer != state.hovered_at {
        actions.push(UiAction::HtmlHover(pointer));
    }
    actions.extend(select(state, &response, in_page, ui));
    let link = state.hover.as_deref().filter(|_| pointer.is_some());
    if let Some(href) = link {
        let response = response.on_hover_text_at_pointer(crate::message::clean(href, false));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if response.clicked()
            && let Some(url) = openable(href)
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    } else if pointer.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    actions
}

/// A drag selects the page's text, a click clears it, and Copy copies it unless a widget has the keyboard.
fn select(
    state: &HtmlState,
    response: &egui::Response,
    in_page: impl Fn(egui::Pos2) -> egui::Pos2,
    ui: &egui::Ui,
) -> Option<UiAction> {
    let copy = ui.input(|input| input.events.contains(&egui::Event::Copy));
    if let Some(text) = &state.selection
        && copy
        && ui.memory(|memory| memory.focused().is_none())
    {
        ui.ctx().copy_text(text.clone());
    }
    let origin = ui.input(|input| input.pointer.press_origin());
    let at = match (origin, response.interact_pointer_pos()) {
        (Some(from), Some(to)) if response.dragged() => (in_page(from), in_page(to)),
        (_, Some(to)) if response.clicked() && state.selection.is_some() => {
            (in_page(to), in_page(to))
        }
        _ => return None,
    };
    (state.selected_at != Some(at)).then_some(UiAction::HtmlSelect {
        from: at.0,
        to: at.1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wanted_strips_cover_the_view_and_a_screen_either_side() {
        // At scale 1 a strip is 512 points; the view shows 1000..1400 of a 5000-point page.
        assert_eq!(wanted_strips(1000.0, 1400.0, 5000.0, 1.0), 1..4);
        // At the top nothing above, and at scale 2 strips are 256 points.
        assert_eq!(wanted_strips(0.0, 400.0, 5000.0, 2.0), 0..4);
        // A short page stops at its end.
        assert_eq!(wanted_strips(0.0, 800.0, 300.0, 1.0), 0..1);
    }

    #[test]
    fn far_strips_are_three_screens_away() {
        // Screen 400 points at 4000..4400: strips ending before 2800 or starting after 5600 go.
        let far = far_strips(0..12, 4000.0, 4400.0, 1.0);
        assert_eq!(far, [0, 1, 2, 3, 4, 11]);
    }

    #[test]
    fn only_absolute_http_https_and_mailto_links_open() {
        assert_eq!(
            openable(" https://example.com/a "),
            Some("https://example.com/a")
        );
        assert_eq!(
            openable("mailto:me@example.com"),
            Some("mailto:me@example.com")
        );
        for refused in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "/relative",
            "https://",
        ] {
            assert_eq!(openable(refused), None, "{refused}");
        }
    }
}
