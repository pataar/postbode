//! The render thread: owns the Blitz document of the shown message, lays it out, paints strips and answers hit tests.
//! Blitz documents are not `Send`, so every one is made, used and dropped here.
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DEFAULT_CSS, DocumentConfig, FontContext, local_name};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use eframe::egui;

use super::net;
use super::{Load, Reply, Request, STRIP};

/// The thread's name, which the panic hook uses to keep Blitz's panic messages, which can quote the mail, off stderr.
const THREAD: &str = "html-render";

/// A hierarchical base, so relative URLs resolve (Blitz panics on one it cannot resolve) to something the net
/// provider refuses.
const BASE: &str = "postvak://mail/";

/// Mail is written for a 600 px column and assumes nothing about the window: keep images and long words inside it.
const MAIL_CSS: &str = "img { max-width: 100%; height: auto; } body { overflow-wrap: anywhere; }";

/// The render thread and its two channels. Dropping it ends the thread once it finishes its current job.
pub(crate) struct Renderer {
    ctx: egui::Context,
    replies: Receiver<Reply>,
    requests: Sender<Request>,
}

impl Renderer {
    pub fn start(ctx: egui::Context) -> Renderer {
        quiet_panics();
        let (requests, jobs) = mpsc::channel();
        let (done, replies) = mpsc::channel();
        let waker = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name(THREAD.into())
            .spawn(move || run(&jobs, &done, &waker, handle));
        if let Err(e) = spawned {
            log::warn!("could not start the HTML renderer: {e}");
        }
        Renderer {
            ctx,
            replies,
            requests,
        }
    }

    /// Sends a job; when the thread has gone, starts a new one for a `Load`, as the old document went with it.
    pub fn send(&mut self, request: Request) {
        if let Err(mpsc::SendError(request)) = self.requests.send(request)
            && matches!(request, Request::Load(_))
        {
            *self = Renderer::start(self.ctx.clone());
            let _ = self.requests.send(request);
        }
    }

    pub fn replies(&self) -> Vec<Reply> {
        self.replies.try_iter().collect()
    }
}

/// The document being shown, laid out at one width and scale.
struct Current {
    doc: HtmlDocument,
    generation: u64,
    height: u32,
    raster: VelloCpuImageRenderer,
    scale: f32,
    width: u32,
}

/// Runs jobs until the app hangs up. A panic in `handle` drops the document and replies `Failed`.
fn run(
    jobs: &Receiver<Request>,
    done: &Sender<Reply>,
    ctx: &egui::Context,
    mut handle: impl FnMut(&mut Option<Current>, Request) -> Option<Reply>,
) {
    let mut current: Option<Current> = None;
    while let Ok(first) = jobs.recv() {
        let mut batch = vec![first];
        batch.extend(jobs.try_iter());
        // Only the newest message matters: skip every job before the last `Load`.
        if let Some(last) = batch.iter().rposition(|r| matches!(r, Request::Load(_))) {
            batch.drain(..last);
        }
        for request in batch {
            let generation = request.generation();
            let reply =
                match panic::catch_unwind(AssertUnwindSafe(|| handle(&mut current, request))) {
                    Ok(reply) => reply,
                    Err(_) => {
                        current = None;
                        Some(Reply::Failed { generation })
                    }
                };
            if let Some(reply) = reply {
                if done.send(reply).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        }
    }
}

fn handle(current: &mut Option<Current>, request: Request) -> Option<Reply> {
    match request {
        Request::Load(load) => {
            let (laid, reply) = layout(load);
            *current = Some(laid);
            Some(reply)
        }
        Request::Paint { generation, strip } => {
            let current = current.as_mut().filter(|c| c.generation == generation)?;
            Some(Reply::Strip {
                generation,
                strip,
                image: paint(current, strip)?,
            })
        }
        Request::Hit { generation, x, y } => {
            let current = current.as_ref().filter(|c| c.generation == generation)?;
            Some(Reply::Link {
                generation,
                href: link_at(&current.doc, x, y),
            })
        }
    }
}

fn layout(load: Load) -> (Current, Reply) {
    let scale = load.scale.max(0.5);
    let width = physical(load.width, scale).max(1);
    let (provider, remote) = net::Inline::new(load.inline);
    let config = DocumentConfig {
        viewport: Some(Viewport::new(
            width,
            physical(800.0, scale),
            scale,
            ColorScheme::Light,
        )),
        base_url: Some(BASE.into()),
        ua_stylesheets: Some(vec![DEFAULT_CSS.into(), MAIL_CSS.into()]),
        net_provider: Some(Arc::new(provider)),
        font_ctx: fonts(),
        ..Default::default()
    };
    let mut doc = HtmlDocument::from_html(&load.html, config);
    // The provider answered every request synchronously; take its answers before the layout.
    doc.handle_messages();
    doc.resolve(0.0);
    let layout = doc.root_element().final_layout();
    let overflow = layout.scrollable_overflow_rect;
    let laid_width = overflow.right.max(load.width);
    let height = layout.size.height.max(overflow.bottom);
    let current = Current {
        doc,
        generation: load.generation,
        height: physical(height, scale),
        raster: VelloCpuImageRenderer::new(1, 1),
        scale,
        width: physical(laid_width, scale).max(width),
    };
    let reply = Reply::Laid {
        generation: load.generation,
        height,
        remote: remote.load(Ordering::Relaxed),
        width: laid_width,
    };
    (current, reply)
}

fn paint(current: &mut Current, strip: u32) -> Option<egui::ColorImage> {
    let top = strip.checked_mul(STRIP)?;
    let height = current.height.checked_sub(top)?.min(STRIP);
    if height == 0 {
        return None;
    }
    let (width, scale) = (current.width, f64::from(current.scale));
    current.raster.resize(width, height);
    let mut rgba = Vec::new();
    let doc = &mut current.doc;
    // Blitz's own offsets place the document on the canvas; a strip lower down is the viewport scrolled to it.
    // Hit tests take document coordinates and ignore this scroll.
    doc.set_viewport_scroll(blitz_dom::Point {
        x: 0.0,
        y: f64::from(top) / scale,
    });
    current.raster.render_to_vec(
        |scene| blitz_paint::paint_scene(scene, doc, scale, width, height, 0, 0),
        &mut rgba,
    );
    let size = [usize::try_from(width).ok()?, usize::try_from(height).ok()?];
    (rgba.len() == size[0] * size[1] * 4)
        .then(|| egui::ColorImage::from_rgba_premultiplied(size, &rgba))
}

/// The `href` of the link under a point in CSS pixels, from the innermost `<a>` around the hit node.
fn link_at(doc: &HtmlDocument, x: f32, y: f32) -> Option<String> {
    let hit = doc.hit(x, y)?;
    let mut node = doc.get_node(hit.node_id);
    while let Some(n) = node {
        if n.data.is_element_with_tag_name(&local_name!("a")) {
            return n.attr(local_name!("href")).map(str::to_string);
        }
        node = n.parent.and_then(|parent| doc.get_node(parent));
    }
    None
}

/// Points to whole pixels; a float-to-int `as` saturates, and NaN becomes 0.
fn physical(points: f32, scale: f32) -> u32 {
    (points * scale).ceil() as u32
}

/// System fonts, except in tests: there the font egui bundles, so a render looks the same on every machine.
fn fonts() -> Option<FontContext> {
    #[cfg(test)]
    {
        let fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .get("Ubuntu-Light")
            .map(|data| blitz_dom::build_single_font_ctx(&data.font))
    }
    #[cfg(not(test))]
    None
}

/// Blitz panics on some input, and its messages can quote the mail; a panic on the render thread logs only that it
/// happened. Other threads keep the previous hook.
fn quiet_panics() {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some(THREAD) {
            log::warn!("the HTML renderer failed; showing text instead");
        } else {
            previous(info);
        }
    }));
}

#[cfg(test)]
impl Renderer {
    /// A renderer with no thread: the test reads what the app sent and answers in its place.
    pub fn fake(ctx: egui::Context) -> (Renderer, Receiver<Request>, Sender<Reply>) {
        let (requests, jobs) = mpsc::channel();
        let (done, replies) = mpsc::channel();
        let renderer = Renderer {
            ctx,
            replies,
            requests,
        };
        (renderer, jobs, done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEWSLETTER: &str = include_str!("../../../tests/fixtures/newsletter.html");

    fn load(generation: u64, html: &str, width: f32) -> Request {
        Request::Load(Load {
            generation,
            html: html.into(),
            inline: Vec::new(),
            scale: 1.0,
            width,
        })
    }

    /// Runs `requests` through the loop on this thread and returns the replies.
    fn replies(
        requests: Vec<Request>,
        handler: impl FnMut(&mut Option<Current>, Request) -> Option<Reply>,
    ) -> Vec<Reply> {
        let (send, jobs) = mpsc::channel();
        for request in requests {
            send.send(request).unwrap();
        }
        drop(send);
        let (done, replies) = mpsc::channel();
        run(&jobs, &done, &egui::Context::default(), handler);
        replies.try_iter().collect()
    }

    #[test]
    fn a_newsletter_lays_out_paints_and_finds_its_links() {
        let mut current = None;
        let Some(Reply::Laid {
            height,
            remote,
            width,
            ..
        }) = handle(&mut current, load(1, NEWSLETTER, 700.0))
        else {
            panic!("no layout");
        };
        assert!((700.0..800.0).contains(&width), "width {width}");
        assert!(height > 500.0, "height {height}");
        assert!(remote, "the tracking pixel and stylesheet were asked for");
        let Some(Reply::Strip { image, .. }) = handle(
            &mut current,
            Request::Paint {
                generation: 1,
                strip: 0,
            },
        ) else {
            panic!("no strip");
        };
        assert_eq!(image.size, [700, 512]);
        // The header bar is dark blue, the page around it light grey.
        let at = |x: usize, y: usize| image.pixels[y * 700 + x];
        assert_eq!(at(10, 10).to_array()[..3], [0xf2, 0xf4, 0xf6]);
        assert!(at(100, 60).b() > at(100, 60).r() + 20, "{:?}", at(100, 60));
        // The second strip holds the quote, the code block and the footer, not blank page.
        let Some(Reply::Strip { image, .. }) = handle(
            &mut current,
            Request::Paint {
                generation: 1,
                strip: 1,
            },
        ) else {
            panic!("no second strip");
        };
        let colours: std::collections::HashSet<_> = image.pixels.iter().collect();
        assert!(colours.len() > 20, "{} colours", colours.len());
        // A stale generation gets no answer; a point over the button finds its link.
        let stale = Request::Paint {
            generation: 0,
            strip: 0,
        };
        assert!(handle(&mut current, stale).is_none());
        let mut link = |x: f32, y: f32| match handle(
            &mut current,
            Request::Hit {
                generation: 1,
                x,
                y,
            },
        ) {
            Some(Reply::Link { href, .. }) => href,
            _ => panic!("no hit answer"),
        };
        assert_eq!(link(5.0, 5.0), None);
        let button = (0..height as u32)
            .step_by(4)
            .find_map(|y| link(350.0, y as f32));
        assert_eq!(button.as_deref(), Some("https://example.com/cta"));
    }

    #[test]
    fn a_strip_past_the_end_is_not_painted() {
        let mut current = None;
        handle(&mut current, load(1, "<p>short</p>", 400.0));
        let past = Request::Paint {
            generation: 1,
            strip: 3,
        };
        assert!(handle(&mut current, past).is_none());
    }

    #[test]
    fn relative_urls_and_a_bare_page_do_not_panic() {
        let html = "<img src=\"logo.png\"><a href=\"/x\">x</a><link rel=stylesheet href=\"s.css\"><iframe src=\"f.html\"></iframe>";
        let got = replies(vec![load(1, html, 400.0), load(2, "", 400.0)], handle);
        assert!(matches!(got[..], [Reply::Laid { generation: 2, .. }]));
    }

    #[test]
    fn queued_loads_lay_out_only_the_newest() {
        let requests = vec![
            load(1, "<p>one</p>", 400.0),
            Request::Paint {
                generation: 1,
                strip: 0,
            },
            load(2, "<p>two</p>", 400.0),
            Request::Paint {
                generation: 2,
                strip: 0,
            },
        ];
        let got: Vec<_> = replies(requests, handle)
            .iter()
            .map(|r| match r {
                Reply::Laid { generation, .. } => ("laid", *generation),
                Reply::Strip { generation, .. } => ("strip", *generation),
                _ => ("other", 0),
            })
            .collect();
        assert_eq!(got, [("laid", 2), ("strip", 2)]);
    }

    #[test]
    fn a_panic_drops_the_document_and_replies_failed() {
        let got = replies(vec![load(7, "<p>x</p>", 400.0)], |_, _| {
            panic!("blitz fell over")
        });
        assert!(matches!(got[..], [Reply::Failed { generation: 7 }]));
    }

    /// Responsive mail stacks its columns below about 600 px with `td { display: block }`; Blitz lays such a cell
    /// out but paints nothing of it, so in a narrow body pane those columns vanish.
    #[test]
    #[ignore = "BUG: Blitz 0.3.0-beta.2 does not paint a <td> with display: block"]
    fn a_table_cell_shown_as_a_block_is_painted() {
        let html = "<table width=100%><tr><td style=\"display:block;background:#ff0000\">one</td></tr></table>";
        let mut current = None;
        handle(&mut current, load(1, html, 300.0));
        let Some(Reply::Strip { image, .. }) = handle(
            &mut current,
            Request::Paint {
                generation: 1,
                strip: 0,
            },
        ) else {
            panic!("no strip");
        };
        let red = egui::Color32::from_rgb(255, 0, 0);
        assert!(image.pixels.contains(&red));
    }

    /// Lays out and paints `html`, reporting whether an upstream crate panicked.
    fn panics(html: &str, width: f32, scale: f32) -> bool {
        panic::catch_unwind(|| {
            let mut current = None;
            let load = Request::Load(Load {
                generation: 1,
                html: html.into(),
                inline: Vec::new(),
                scale,
                width,
            });
            handle(&mut current, load);
            for strip in 0..2 {
                handle(
                    &mut current,
                    Request::Paint {
                        generation: 1,
                        strip,
                    },
                );
            }
        })
        .is_err()
    }

    /// Found by `hostile_html_never_panics`. The render thread catches it and shows the text.
    #[test]
    #[ignore = "BUG: parley 0.11.1 line_break.rs:453 asserts on a 1e9px font in a 1px-wide layout"]
    fn a_huge_font_in_a_tiny_width_lays_out() {
        let html = "<div style=\"position:fixed;top:-1e9px\">%</table><br><style>@import url(https://x.test/i.css); * { font-size: 1e9px }</style></table>";
        assert!(!panics(html, 1.0, 1.207_759_3));
    }

    /// Found by `hostile_html_never_panics`. The render thread catches it and shows the text.
    #[test]
    #[ignore = "BUG: vello_common 0.1.0 util.rs:174 unwraps an overflowed tile bound for huge geometry"]
    fn huge_geometry_paints() {
        let html = "<div style=\"position:fixed;top:-1e9px\">aW<div style=\"float:left;width:200%;margin:-9999px\">!<style>@import url(https://x.test/i.css); * { font-size: 1e9px }</style>";
        assert!(!panics(html, 1.0, 1.946_665));
    }

    /// Pieces of hostile mail HTML: broken nesting, odd tables, every kind of URL, and CSS at its extremes.
    const PIECES: &[&str] = &[
        "<table>",
        "</table>",
        "<tr>",
        "<td colspan=0 rowspan=99999>",
        "<td style=\"display:block\">",
        "</td>",
        "<th>",
        "<tbody>",
        "<div style=\"position:fixed;top:-1e9px\">",
        "</div>",
        "<span>",
        "<p>",
        "<br>",
        "<img src=\"cid:\">",
        "<img src=\"https://x.test/a.png\" width=-1 height=1e12>",
        "<img src=\"data:image/png;base64,!!!\">",
        "<img src=\"../../../etc/passwd\">",
        "<img>",
        "<a href=\"javascript:alert(1)\">",
        "<a href=\"\">",
        "</a>",
        "<iframe src=\"https://x.test\">",
        "<link rel=stylesheet href=\"//x.test/s.css\">",
        "<base href=\"https://x.test/\">",
        "<style>@import url(https://x.test/i.css); * { font-size: 1e9px }</style>",
        "<style>td { width: 100% !important; display: block !important }</style>",
        "<style>body { columns: 0; transform: scale(0) rotate(1e9deg) }</style>",
        "<style>@font-face { font-family: x; src: url(data:font/woff2;base64,AAAA) }</style>",
        "<svg><circle r=1e9 /></svg>",
        "<math><mi>x</mi></math>",
        "<select><option>o</select>",
        "<input type=file>",
        "<textarea>",
        "<!--",
        "-->",
        "<![CDATA[",
        "&#x0;",
        "&nbsp;",
        "\u{202e}",
        "<pre>\t\t</pre>",
        "<ul><li><ol><li>",
        "<font size=+99 color=\"#zzz\">",
        "<center>",
        "<div style=\"float:left;width:200%;margin:-9999px\">",
        "<table width=600 style=\"width:auto\">",
    ];

    fn hostile_html() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        let piece = prop_oneof![
            4 => proptest::sample::select(PIECES).prop_map(str::to_string),
            1 => "[ -~]{0,40}",
            1 => ".{0,8}",
        ];
        proptest::collection::vec(piece, 0..40).prop_map(|pieces| pieces.concat())
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: std::env::var("PROPTEST_CASES").ok().and_then(|n| n.parse().ok()).unwrap_or(32),
            failure_persistence: None,
            ..Default::default()
        })]

        /// Hostile HTML through the render loop: every layout is answered, laid out or failed, and nothing takes the
        /// process down. Upstream panics the loop caught are named tests above, ignored until fixed.
        #[test]
        fn hostile_html_never_panics(html in hostile_html(), width in 1.0f32..2000.0, scale in 0.5f32..3.0) {
            let mut requests = vec![Request::Load(Load {
                generation: 1,
                html,
                inline: Vec::new(),
                scale,
                width,
            })];
            for strip in 0..2 {
                requests.push(Request::Paint { generation: 1, strip });
            }
            for (x, y) in [(0.0, 0.0), (width / 2.0, 40.0), (-5.0, 1e9), (f32::NAN, 3.0)] {
                requests.push(Request::Hit { generation: 1, x, y });
            }
            let got = replies(requests, handle);
            let answered = matches!(
                got.first(),
                Some(Reply::Laid { generation: 1, .. } | Reply::Failed { generation: 1 })
            );
            proptest::prop_assert!(answered, "the layout got no answer");
        }
    }

    #[test]
    fn physical_pixels_round_up_and_saturate() {
        assert_eq!(physical(100.4, 1.0), 101);
        assert_eq!(physical(100.0, 2.0), 200);
        assert_eq!(physical(-3.0, 1.0), 0);
        assert_eq!(physical(f32::NAN, 1.0), 0);
        assert_eq!(physical(f32::INFINITY, 1.0), u32::MAX);
    }
}
