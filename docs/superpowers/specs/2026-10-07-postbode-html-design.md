# Postbode HTML rendering design

Date: 2026-10-07. Status: draft for review. Builds on `2026-10-06-postbode-gui-design.md` §6 (body panel) and replaces its §11 entry "HTML bodies through `wry`". Core §17's constraints (no bodies in logs, no `unwrap` outside tests) apply.

## 1. Purpose and scope

HTML-only and multipart mail render as their author laid them out, inside the egui body panel, with no remote content and no scripts.

In scope:
- An HTML view in the body panel for messages with an HTML part, rendered by Blitz on a dedicated thread into an egui texture.
- Inline images from `cid:` parts and `data:` URIs.
- Clickable links under the existing scheme rule, with the target shown on hover.
- A key to switch between the HTML view and the text view.
- A default-on `html` cargo feature.

Out of scope, recorded in §9: loading remote images, text selection inside the HTML view, dark-mode adaptation of mail, a persisted view preference, HTML in compose.

## 2. Decisions log

| Decision | Choice | Rejected |
|---|---|---|
| Engine | Blitz (`blitz-dom`, `blitz-html`, `blitz-paint`) | `wry` (system webview: child windows are X11-only on Linux, needs WebKitGTK at runtime, positioned by hand each frame, invisible to snapshot tests); `servo` (JavaScript engine, monthly breaking releases, far beyond email); `litehtml` (C++17 toolchain, one maintainer, ignored `align="center"` and inline link colours in the comparison); our own HTML-to-egui mapping (table layouts and inline CSS out of reach) |
| Raster | CPU through `anyrender_vello_cpu`, uploaded as an egui texture | Vello on eframe's wgpu device (ties the wgpu version to eframe's, adds a GPU path the tests cannot see) |
| Thread | One render thread owning every Blitz document; the UI thread sends jobs and receives pixels | Render on the UI thread (15–25 ms per layout stalls `j`/`k`); one thread per message (Blitz documents are not `Send`, so each would live and die on its thread anyway) |
| Tall mail | Laid out once per width, painted in 512 px strips as they scroll into view | One texture per message (long newsletters exceed GPU texture limits) |
| Remote content | Never fetched; only `cid:` and `data:` resolve | Fetch by default; per-sender allow list now |
| Colour scheme | Light, on a white page, in both app themes | Follow the app theme (most mail hardcodes dark text on no background) |
| Default view | HTML when the message has an HTML part, else text | Text first; remember per sender |
| Versions | Exact pins (`=0.3.0-beta.2`) for the Blitz crates and the matching `anyrender` | Caret ranges (the beta crates already disagree on `anyrender` between minor versions) |

Measured on one newsletter fixture at 700 px, Linux x86_64, 4 cores, release build: first document 24 ms parse, style and layout, later ones 15 ms, paint 5 ms. A standalone renderer binary is 22 MB over 288 crates. Ran on Linux; not yet on macOS.

## 3. Packaging

- Cargo feature `html = ["gui", "dep:blitz-dom", "dep:blitz-html", "dep:blitz-paint", "dep:blitz-traits", "dep:anyrender", "dep:anyrender_vello_cpu"]`, in `default`. Without it the body panel is the text view of GUI §6, unchanged.
- Stylo, Blitz's CSS engine, is MPL-2.0. File-level copyleft: shipping a binary that links it is fine; changes to its own files would be published. Noted in the licence section of `docs/src/index.md`, and so of `README.md`.
- `cargo audit` and `cargo machete` cover the new crates as they cover the rest.

## 4. Architecture

```
src/gui/html/
  mod.rs       HtmlView: per-message state the app holds; strip cache; drawing; link hover and click
  render.rs    the render thread: owns Blitz documents, runs jobs, catches panics
  net.rs       the net provider: cid: and data: from memory, everything else refused
src/message.rs html_body(raw) -> Option<HtmlBody { html, inline: Vec<(cid, mime, bytes)> }>
```

- `message::html_body` uses `mail-parser`: the first `text/html` part, and every part with a `Content-ID`. It returns `None` for messages with no HTML part. No store change: the raw message is already stored and read by `App::read_stored`.
- `body.rs` stays the panel: headers, then either the text view or `html::show`, then attachments. Views still return `UiAction`s; `app.rs` still applies them.
- The render thread starts with the GUI and lives until it closes. It talks over two channels:
  - in: `Load { generation, html, inline, width, scale }`, `Paint { generation, strip }`, `Hit { generation, x, y }`, `Drop { generation }`
  - out: `Laid { generation, height }`, `Strip { generation, strip, rgba }`, `Link { generation, href: Option<String> }`, `Failed { generation }`
- `generation` is a counter the app bumps for every new message or width; replies with an old generation are dropped. The thread keeps only the latest document, so moving through mail with `j` never queues layouts: a `Load` replaces whatever was pending.
- After each reply the thread calls `ctx.request_repaint()`.

## 5. Rendering

- **Width.** The body panel's inner width in points, at least 320. A width change re-lays out once the width has held still for 100 ms; meanwhile the old strips are drawn as they are, clipped or padded.
- **Scale.** `pixels_per_point` becomes Blitz's hidpi scale; strips are painted at physical pixels and drawn at points.
- **Strips.** 512 physical pixels tall, the full width. The view asks for the strips intersecting the visible rect plus one screen above and below, and drops textures more than three screens away. A strip not yet painted draws as white.
- **Page.** A white rectangle the width of the panel, a 1 px border from the egui theme, the document painted over it with `ColorScheme::Light`. The rest of the panel follows the app theme.
- **User agent CSS.** Blitz's own, plus `img { max-width: 100%; height: auto }` and `body { overflow-wrap: anywhere }`, so 600 px newsletters and long URLs fit a narrow panel.
- **Fonts.** System fonts through Blitz's `FontContext`. Tests build the context from a font bundled in the test fixtures, so snapshots do not depend on the machine.
- **Limits.** HTML over 2 MiB, or a document taller than 200,000 px, shows the text view with the note "Too large to render; showing text."

## 6. Resources and privacy

- The net provider answers `cid:` from the message's `Content-ID` parts and `data:` by decoding the URI. Everything else, including `http`, `https`, `file` and stylesheet `@import`, completes at once as a failure, so no request leaves the machine and nothing waits on it.
- Blocked images keep their `width`/`height` box. When any were blocked, a line above the page says "Remote images not loaded."
- Blitz runs no scripts; `<form>` controls draw but are never submitted, since the view sends Blitz no input events beyond hit tests.
- Nothing from the body is logged. A render failure logs the account, folder and uid only.

## 7. Interaction

- **Links.** On hover the view sends `Hit` for the pointer position (at most once per frame); the reply's `href`, if any, shows as a tooltip and the pointer becomes a hand. A click opens it through `ctx.open_url` only when its scheme is `http`, `https` or `mailto`, the same rule as the text view; any other scheme does nothing. Relative URLs have no base and do nothing.
- **Toggle.** `v` switches the current message between the HTML and text views. The choice lasts until the next message is shown. Added to the `?` key table. The text view is the existing one, so its selection and copy still work.
- **Scrolling.** The page sits in the body `ScrollArea` and scrolls as the text view does. Wider-than-panel content scrolls horizontally.
- **Read state.** Unchanged: the 1 s timer starts when the message is on screen, whichever view shows it.

## 8. Failure

- The render thread wraps each job in `catch_unwind`. A panic, or `Failed`, shows the text view with "Could not render HTML; showing text." for that message, and the thread drops the document and carries on. If the thread itself has gone, the next `Load` restarts it.
- A message whose body is not downloaded shows "Loading…" as today; the HTML view starts when `BodyReady` arrives.

## 9. Testing

**Unit,** in `message` and `gui::html`:
- `html_body`: `text/html` only, `multipart/alternative`, `multipart/related` with two `cid:` images, plain text only (`None`).
- Net provider: `cid:` and `data:` resolve; `https:`, `http:`, `file:` and `@import` fail without a network call.
- Generations: a `Laid` from an older generation is ignored; two `Load`s in a row lay out only the second.
- Strip selection for a visible rect, and eviction three screens away.
- Size limits fall back to text with the note.

**GUI,** with `egui_kittest`:
- An HTML message shows the HTML view; `v` shows the text; the next message shows HTML again.
- Hovering a link shows its URL; clicking `https://` emits `OpenUrl`, clicking `javascript:` emits nothing.
- A message with a remote `<img>` shows "Remote images not loaded".
- A forced render failure shows the text with the failure note.
- Snapshots in `gui::snapshots`: the newsletter fixture in light and dark app themes, at 700 and 360 px.

**Fuzz,** in `tests/fuzz.rs`: hostile HTML through `html_body` and a full layout and paint never panics the process.

Ran on Linux (CPU raster needs no GPU, so CI covers it); macOS reported separately as compiled on or ran on.

## 10. Later

- Loading remote images on request, per message, then per sender.
- Selecting and copying text in the HTML view (Blitz has selection; wiring it means forwarding pointer drags).
- Dark-mode rendering of mail that declares `color-scheme: dark` or has no colours of its own.
- Remembering the view choice in `[ui]`.
- Vello on the GPU if CPU paint shows up in profiles.
