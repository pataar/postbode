//! The HTML view in the running app: toggling to text, links, notes, and the render thread's failures.
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::{FAILED, MAX_HTML, Renderer, Reply, Request, TOO_LARGE};
use crate::gui::App;
use crate::gui::test_support::{Fixture, Wires, message};

pub(crate) const NEWSLETTER: &str = include_str!("../../../tests/fixtures/newsletter.html");

/// A 1×1 PNG, for the `cid:` image.
const DOT: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

/// A raw message whose HTML is `html`, with a `cid:dot@example.com` image beside it.
pub(crate) fn raw(html: &str) -> Vec<u8> {
    format!(
        "From: news@example.org\r\nSubject: news\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/related; boundary=\"b\"\r\n\r\n\
--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n\
--b\r\nContent-Type: image/png\r\nContent-ID: <dot@example.com>\r\nContent-Transfer-Encoding: base64\r\n\r\n{DOT}\r\n--b--\r\n"
    )
    .into_bytes()
}

/// Adds message `uid` to work's INBOX with `html` as its body and `text` as its extracted text.
pub(crate) fn add_html(fx: &Fixture, uid: u32, html: &str, text: &str) {
    let mut m = message("INBOX", uid, &format!("news {uid}"));
    m.body_text = Some(text.into());
    fx.add("work", m);
    fx.store("work")
        .set_raw("INBOX", uid, &raw(html), text)
        .unwrap();
}

/// Steps the app until `done` holds; the render thread answers in real time, so `run` before this could exceed its
/// step limit while replies arrive frame after frame.
pub(crate) fn wait_for(harness: &mut Harness<'static, App>, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(harness.state()) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the renderer"
        );
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    harness.run();
}

/// True once the shown message is laid out and every strip it asked for is painted.
pub(crate) fn painted(app: &App) -> bool {
    app.body
        .as_ref()
        .and_then(|b| b.html_view.as_ref())
        .is_some_and(|v| {
            v.page.is_some()
                && !v.requested.is_empty()
                && v.requested.iter().all(|s| v.strips.contains_key(s))
        })
}

/// One frame applies the key, the next draws it; `run` instead would race the render thread's repaints.
fn press(harness: &mut Harness<'static, App>, text: &str) {
    harness.event(egui::Event::Text(text.into()));
    harness.run_steps(2);
}

fn opened(harness: &Harness<'static, App>) -> Vec<String> {
    harness
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
            _ => None,
        })
        .collect()
}

/// Moves the pointer into the body pane, waits for the link answer, and clicks.
fn click_page(harness: &mut Harness<'static, App>) -> Option<String> {
    let at = egui::pos2(1000.0, 600.0);
    harness.event(egui::Event::PointerMoved(at));
    wait_for(harness, |app| {
        app.body
            .as_ref()
            .and_then(|b| b.html_view.as_ref())
            .is_some_and(|v| v.hovered_at.is_some() && v.hover.is_some())
    });
    let hover = harness
        .state()
        .body
        .as_ref()?
        .html_view
        .as_ref()?
        .hover
        .clone();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    harness.step();
    hover
}

/// A link filling the page, so any point in the body pane is over it.
fn page_link(href: &str) -> String {
    format!("<a href=\"{href}\" style=\"display:block;height:3000px\">link</a>")
}

#[test]
fn html_mail_shows_as_html_and_v_switches_that_message_to_text() {
    let fx = Fixture::new(&["work"]);
    add_html(&fx, 1, "<p>older</p>", "older as text");
    add_html(&fx, 2, NEWSLETTER, "newer as text");
    let (mut harness, _wires) = fx.harness();
    wait_for(&mut harness, painted);
    assert!(harness.query_by_label("newer as text").is_none());
    assert!(
        harness
            .query_by_label("Remote content not loaded.")
            .is_some()
    );
    press(&mut harness, "v");
    assert!(harness.query_by_label("newer as text").is_some());
    press(&mut harness, "v");
    assert!(harness.query_by_label("newer as text").is_none());
    press(&mut harness, "v");
    press(&mut harness, "j");
    wait_for(&mut harness, painted);
    assert!(harness.query_by_label("older as text").is_none());
    assert!(
        harness
            .query_by_label("Remote content not loaded.")
            .is_none()
    );
}

#[test]
fn v_on_plain_text_mail_changes_nothing() {
    let fx = Fixture::new(&["work"]);
    fx.add("work", message("INBOX", 1, "plain"));
    let (mut harness, _wires) = fx.harness();
    harness.run();
    press(&mut harness, "v");
    assert!(!harness.state().body.as_ref().unwrap().show_text);
    assert!(harness.query_by_label("Body of 1").is_some());
    assert!(harness.state().renderer.is_none());
}

#[test]
fn an_https_link_opens_and_shows_its_target() {
    let fx = Fixture::new(&["work"]);
    add_html(&fx, 1, &page_link("https://example.com/x"), "link");
    let (mut harness, _wires) = fx.harness();
    wait_for(&mut harness, painted);
    let hover = click_page(&mut harness);
    assert_eq!(hover.as_deref(), Some("https://example.com/x"));
    assert_eq!(opened(&harness), ["https://example.com/x"]);
}

#[test]
fn a_javascript_link_does_nothing() {
    let fx = Fixture::new(&["work"]);
    add_html(&fx, 1, &page_link("javascript:alert(1)"), "link");
    let (mut harness, _wires) = fx.harness();
    wait_for(&mut harness, painted);
    assert_eq!(
        click_page(&mut harness).as_deref(),
        Some("javascript:alert(1)")
    );
    assert!(opened(&harness).is_empty());
}

#[test]
fn html_over_the_limit_shows_text_with_a_note() {
    let fx = Fixture::new(&["work"]);
    let huge = format!("<p>{}</p>", "x".repeat(MAX_HTML));
    add_html(&fx, 1, &huge, "the text");
    let (mut harness, _wires) = fx.harness();
    harness.run();
    assert!(harness.query_by_label(TOO_LARGE).is_some());
    assert!(harness.query_by_label("the text").is_some());
    assert!(harness.state().renderer.is_none());
}

/// The app with a fake renderer, and the generation of the first layout it asked for.
struct Faked {
    harness: Harness<'static, App>,
    jobs: Receiver<Request>,
    replies: Sender<Reply>,
    generation: u64,
    _wires: Wires,
}

fn faked(fx: &Fixture) -> Faked {
    let (mut harness, wires) = fx.harness();
    let (renderer, jobs, replies) = Renderer::fake(egui::Context::default());
    // Building the harness drew a first frame with a real renderer; start the view again on the fake one.
    let app = harness.state_mut();
    app.renderer = Some(renderer);
    if let Some(body) = app.body.as_mut() {
        body.html_view = None;
    }
    harness.run();
    let generation = jobs
        .try_iter()
        .find_map(|job| match job {
            Request::Load(load) => Some(load.generation),
            _ => None,
        })
        .expect("a layout was asked for");
    Faked {
        harness,
        jobs,
        replies,
        generation,
        _wires: wires,
    }
}

fn page(harness: &Harness<'static, App>) -> Option<egui::Vec2> {
    harness.state().body.as_ref()?.html_view.as_ref()?.page
}

#[test]
fn a_render_failure_shows_text_with_a_note() {
    let fx = Fixture::new(&["work"]);
    add_html(&fx, 1, "<p>x</p>", "fallback text");
    let Faked {
        mut harness,
        replies,
        generation,
        ..
    } = faked(&fx);
    replies.send(Reply::Failed { generation }).unwrap();
    harness.run();
    assert!(harness.query_by_label(FAILED).is_some());
    assert!(harness.query_by_label("fallback text").is_some());
}

#[test]
fn a_reply_for_an_older_layout_is_dropped() {
    let fx = Fixture::new(&["work"]);
    add_html(&fx, 1, "<p>x</p>", "text");
    let Faked {
        mut harness,
        replies,
        generation,
        ..
    } = faked(&fx);
    let laid = |generation, height| Reply::Laid {
        generation,
        height,
        remote: false,
        width: 400.0,
    };
    replies.send(laid(generation - 1, 900.0)).unwrap();
    harness.run();
    assert_eq!(page(&harness), None);
    replies.send(laid(generation, 300.0)).unwrap();
    harness.run();
    assert_eq!(page(&harness), Some(egui::vec2(400.0, 300.0)));
}

#[test]
fn a_page_over_the_height_limit_shows_text_with_a_note() {
    let fx = Fixture::new(&["work"]);
    add_html(&fx, 1, "<p>x</p>", "tall text");
    let Faked {
        mut harness,
        replies,
        generation,
        ..
    } = faked(&fx);
    let laid = Reply::Laid {
        generation,
        height: super::MAX_HEIGHT + 1.0,
        remote: false,
        width: 400.0,
    };
    replies.send(laid).unwrap();
    harness.run();
    assert!(harness.query_by_label(TOO_LARGE).is_some());
    assert!(harness.query_by_label("tall text").is_some());
}

#[test]
fn a_new_width_lays_out_again_once_it_held_still() {
    let fx = Fixture::new(&["work"]);
    add_html(&fx, 1, "<p>x</p>", "text");
    let Faked {
        mut harness,
        jobs,
        generation: first,
        ..
    } = faked(&fx);
    harness.set_size(egui::vec2(1180.0, 800.0));
    harness.step();
    assert!(jobs.try_iter().all(|job| !matches!(job, Request::Load(_))));
    let start = harness
        .state()
        .body
        .as_ref()
        .unwrap()
        .html_view
        .as_ref()
        .unwrap()
        .wanted;
    assert!(start.is_some(), "the new width is waiting to settle");
    for _ in 0..15 {
        harness.step();
    }
    let loads: Vec<u64> = jobs
        .try_iter()
        .filter_map(|job| match job {
            Request::Load(load) => Some(load.generation),
            _ => None,
        })
        .collect();
    assert_eq!(loads, [first + 1]);
}
