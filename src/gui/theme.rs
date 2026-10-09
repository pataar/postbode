//! Ossenbloed: per mode a pane palette and an oxblood chrome palette (toolbar, folders, status bar) around it.
use eframe::egui::{self, Color32, Stroke};

pub(crate) struct Palette {
    pub accent: Color32,
    pub background: Color32,
    pub border: Color32,
    pub error: Color32,
    pub faint: Color32,
    /// The reader's subject.
    pub heading: Color32,
    pub highlight: Color32,
    pub hover: Color32,
    pub muted: Color32,
    /// Text on `selected`.
    pub on_accent: Color32,
    /// Unique per palette: `palette` finds a ui's palette by it.
    pub panel: Color32,
    /// Behind a widget while it is pressed or dragged; `strong` is drawn on it.
    pub pressed: Color32,
    pub secondary: Color32,
    /// Behind the selected row, folder and text.
    pub selected: Color32,
    /// Bold text, such as account headings and header labels: more contrast than `text`. egui draws it in the
    /// pressed widget's foreground, so it must also read on `pressed`.
    pub strong: Color32,
    pub text: Color32,
    pub warning: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    let [_, red, green, blue] = rgb.to_be_bytes();
    Color32::from_rgb(red, green, blue)
}

/// Height of buttons and fields, and the width of icon buttons.
pub(crate) const CONTROL: f32 = 28.0;
/// Height of the toolbar.
pub(crate) const TOP_BAR: f32 = 40.0;
/// Height of rows, text buttons and fields.
pub(crate) const ROW: f32 = 24.0;
/// Space between a row's edge and its content; a pane's edge and its rows are 8 px apart too, so content sits 16 px in.
pub(crate) const PAD: f32 = 8.0;
/// Space between a pane's edge and content that is not in a row.
pub(crate) const INSET: f32 = 2.0 * PAD;
/// Size of the icons on icon buttons.
pub(crate) const ICON: f32 = 16.0;
/// Corner radius of the panes where they meet the frame.
pub(crate) const RADIUS: u8 = 10;

/// Light panes.
pub(crate) const PAPER: Palette = Palette {
    accent: hex(0x9a4a1c),
    background: hex(0xfbf8f3),
    border: hex(0xe3d8cb),
    error: hex(0xa8202e),
    faint: hex(0xfffdfa),
    heading: hex(0x3b1418),
    highlight: hex(0x8c6720),
    hover: hex(0xeadfd0),
    muted: hex(0x6e5958),
    on_accent: hex(0xf1e4d8),
    panel: hex(0xf3ece2),
    pressed: hex(0xe3d8cb),
    secondary: hex(0x5e4a4b),
    selected: hex(0x3b1418),
    strong: hex(0x140b0c),
    text: hex(0x2b1a1c),
    warning: hex(0x8a5a00),
};

/// Dark panes.
pub(crate) const NIGHT: Palette = Palette {
    accent: hex(0xe9c9b0),
    background: hex(0x231919),
    border: hex(0x3a2b2c),
    error: hex(0xf08a8a),
    faint: hex(0x140e0f),
    heading: hex(0xe9c9b0),
    highlight: hex(0xc99a3e),
    hover: hex(0x2e2223),
    muted: hex(0xb09a93),
    on_accent: hex(0xf1e4d8),
    panel: hex(0x1a1213),
    pressed: hex(0x3a2b2c),
    secondary: hex(0xcdb8af),
    selected: hex(0x5a1f25),
    strong: hex(0xffffff),
    text: hex(0xefe3d6),
    warning: hex(0xe6b85c),
};

/// The frame around the light panes.
pub(crate) const FRAME: Palette = Palette {
    accent: hex(0xd9b36a),
    background: hex(0x3b1418),
    border: hex(0x5a2a2f),
    error: hex(0xffb4a8),
    faint: hex(0x4e2629),
    heading: hex(0xf1e4d8),
    highlight: hex(0xd9b36a),
    hover: hex(0x4a1d22),
    muted: hex(0xc29a92),
    on_accent: hex(0xffffff),
    panel: hex(0x3b1418),
    pressed: hex(0x5e2a30),
    secondary: hex(0xe0cbbe),
    selected: hex(0x552a2b),
    strong: hex(0xffffff),
    text: hex(0xf1e4d8),
    warning: hex(0xf2cc7a),
};

/// The frame around the dark panes: deeper, so it does not glow at night.
pub(crate) const FRAME_NIGHT: Palette = Palette {
    accent: hex(0xd9b36a),
    background: hex(0x2a0e11),
    border: hex(0x4a2226),
    error: hex(0xffb4a8),
    faint: hex(0x3b1d20),
    heading: hex(0xf1e4d8),
    highlight: hex(0xd9b36a),
    hover: hex(0x3a171b),
    muted: hex(0xc29a92),
    on_accent: hex(0xffffff),
    panel: hex(0x2a0e11),
    pressed: hex(0x4c2227),
    secondary: hex(0xe0cbbe),
    selected: hex(0x44262a),
    strong: hex(0xffffff),
    text: hex(0xf1e4d8),
    warning: hex(0xf2cc7a),
};

const PALETTES: [&Palette; 4] = [&PAPER, &NIGHT, &FRAME, &FRAME_NIGHT];

impl Palette {
    /// egui's own visuals for that mode, recoloured; no shadows, since the brand is flat.
    pub fn visuals(&self, dark: bool) -> egui::Visuals {
        let mut visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.error_fg_color = self.error;
        visuals.extreme_bg_color = self.faint;
        visuals.faint_bg_color = self.faint;
        visuals.hyperlink_color = self.accent;
        visuals.panel_fill = self.panel;
        visuals.popup_shadow = egui::Shadow::NONE;
        visuals.selection.bg_fill = self.selected;
        visuals.selection.stroke = Stroke::new(1.0, self.on_accent);
        visuals.warn_fg_color = self.warning;
        visuals.weak_text_color = Some(self.muted);
        visuals.window_fill = self.background;
        visuals.window_shadow = egui::Shadow::NONE;
        visuals.window_stroke = Stroke::new(1.0, self.border);
        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = self.background;
        widgets.noninteractive.bg_stroke.color = self.border;
        widgets.noninteractive.fg_stroke.color = self.text;
        widgets.inactive.bg_fill = self.hover;
        widgets.inactive.weak_bg_fill = self.hover;
        widgets.inactive.fg_stroke.color = self.text;
        widgets.hovered.bg_fill = self.hover;
        widgets.hovered.weak_bg_fill = self.hover;
        widgets.hovered.bg_stroke.color = self.accent;
        widgets.hovered.fg_stroke.color = self.text;
        // Window title bars and open menus.
        widgets.open.bg_fill = self.hover;
        widgets.open.weak_bg_fill = self.hover;
        widgets.open.fg_stroke.color = self.text;
        // egui takes `strong_text_color` (and the spinner and resize handles) from the pressed widget's foreground,
        // so that colour has to read both on the panels and on the pressed fill.
        widgets.active.bg_fill = self.pressed;
        widgets.active.weak_bg_fill = self.pressed;
        widgets.active.fg_stroke.color = self.strong;
        visuals
    }
}

const TEXT_FONT: &str = "HankenGrotesk-Medium";
const MONO_FONT: &str = "SplineSansMono-Medium";
const SERIF_FONT: &str = "Newsreader-Medium";
const BOLD_FONT: &str = "HankenGrotesk-SemiBold";
/// The font family of unread senders, account names and the reader's sender.
pub(crate) const BOLD: &str = "bold";
/// The font family of headings and the wordmark.
pub(crate) const SERIF: &str = "serif";

/// Hanken Grotesk for text, Spline Sans Mono for monospace (Hack behind it for the arrows it lacks), Newsreader for
/// headings, and the Phosphor icons as the first fallback, so icons mix into ordinary labels.
pub(crate) fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        (
            MONO_FONT,
            &include_bytes!("../../assets/fonts/SplineSansMono-Medium.ttf")[..],
        ),
        (
            SERIF_FONT,
            &include_bytes!("../../assets/fonts/Newsreader-Medium.ttf")[..],
        ),
        (
            TEXT_FONT,
            &include_bytes!("../../assets/fonts/HankenGrotesk-Medium.ttf")[..],
        ),
        (
            BOLD_FONT,
            &include_bytes!("../../assets/fonts/HankenGrotesk-SemiBold.ttf")[..],
        ),
    ] {
        let data = egui::FontData::from_static(bytes);
        fonts
            .font_data
            .insert(name.into(), std::sync::Arc::new(data));
    }
    for family in fonts.families.values_mut() {
        for name in family.iter_mut().filter(|name| *name == "Ubuntu-Light") {
            *name = TEXT_FONT.into();
        }
    }
    fonts.font_data.remove("Ubuntu-Light");
    if let Some(monospace) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        monospace.insert(0, MONO_FONT.into());
    }
    let proportional = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (family, font) in [(BOLD, BOLD_FONT), (SERIF, SERIF_FONT)] {
        let mut names = vec![font.to_string()];
        names.extend(proportional.iter().cloned());
        fonts.families.insert(named(family), names);
    }
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    fonts
}

/// `BOLD` or `SERIF` as an egui font family.
pub(crate) fn named(family: &str) -> egui::FontFamily {
    egui::FontFamily::Name(family.into())
}

/// Gives egui both pane palettes; the theme preference picks one, following the OS when it is System.
pub(crate) fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_visuals_of(egui::Theme::Dark, NIGHT.visuals(true));
    ctx.set_visuals_of(egui::Theme::Light, PAPER.visuals(false));
    ctx.all_styles_mut(|style| {
        style.spacing.button_padding = egui::vec2(PAD, 4.0);
        style.spacing.interact_size.y = ROW;
        #[allow(clippy::cast_possible_truncation, reason = "a margin of a few points")]
        let inset = INSET as i8;
        style.spacing.window_margin = egui::Margin::same(inset);
        if let Some(heading) = style.text_styles.get_mut(&egui::TextStyle::Heading) {
            heading.family = egui::FontFamily::Name(SERIF.into());
        }
    });
}

/// The palette `ui` is drawn in: a pane's, or the chrome's inside `chrome`.
pub(crate) fn palette(ui: &egui::Ui) -> &'static Palette {
    let visuals = ui.visuals();
    PALETTES
        .into_iter()
        .find(|palette| palette.panel == visuals.panel_fill)
        .unwrap_or(if visuals.dark_mode { &NIGHT } else { &PAPER })
}

/// The chrome palette of `palette`'s mode; a chrome palette's is itself.
fn chrome_of(palette: &Palette) -> &'static Palette {
    if palette.panel == NIGHT.panel || palette.panel == FRAME_NIGHT.panel {
        &FRAME_NIGHT
    } else {
        &FRAME
    }
}

/// The frame of a chrome bar or pane: the chrome's fill, no outline.
pub(crate) fn chrome_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::side_top_panel(ui.style())
        .fill(chrome_of(palette(ui)).panel)
        .stroke(Stroke::NONE)
}

/// Draws the rest of `ui` in the chrome's colours.
pub(crate) fn chrome(ui: &mut egui::Ui) {
    *ui.visuals_mut() = chrome_of(palette(ui)).visuals(true);
}

/// A grid whose columns are an inset apart, so neighbouring cells never read as one.
pub(crate) fn grid(id: &str, ui: &egui::Ui) -> egui::Grid {
    egui::Grid::new(id).spacing(egui::vec2(INSET, ui.spacing().item_spacing.y))
}

/// `frame` with an accent outline when its pane has the keyboard focus.
pub(crate) fn pane(frame: egui::Frame, ui: &egui::Ui, focused: bool) -> egui::Frame {
    if focused {
        frame.stroke(Stroke::new(1.0, palette(ui).accent))
    } else {
        frame
    }
}

/// Adds a text field through `add` and draws its focus ring in the accent.
///
/// egui paints that ring and the selected text both in `selection.stroke`, which stays `on_accent` so selected text
/// stays readable on the selection; the accent ring is painted over egui's.
pub(crate) fn text_field(
    ui: &mut egui::Ui,
    add: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    let response = add(ui);
    if response.has_focus() {
        let visuals = ui.style().interact(&response);
        ui.painter().rect_stroke(
            response.rect.expand(visuals.expansion.round()),
            visuals.corner_radius,
            Stroke::new(1.0, palette(ui).accent),
            egui::StrokeKind::Inside,
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Color32};

    use super::*;
    use crate::gui::test_support::Fixture;

    /// Each palette with the mode its visuals are built in.
    const ALL: [(bool, &Palette); 4] = [
        (false, &PAPER),
        (true, &NIGHT),
        (true, &FRAME),
        (true, &FRAME_NIGHT),
    ];

    #[test]
    fn the_icon_font_falls_back_behind_the_text_font() {
        let fonts = fonts();
        assert!(fonts.font_data.contains_key("phosphor"));
        let proportional = &fonts.families[&egui::FontFamily::Proportional];
        assert_eq!(proportional.get(1).map(String::as_str), Some("phosphor"));
    }

    #[test]
    fn text_is_hanken_grotesk_and_monospace_spline_sans_mono_over_hack() {
        let fonts = fonts();
        let proportional = &fonts.families[&egui::FontFamily::Proportional];
        assert_eq!(proportional.first().map(String::as_str), Some(TEXT_FONT));
        assert!(!proportional.iter().any(|name| name == "Ubuntu-Light"));
        let monospace = &fonts.families[&egui::FontFamily::Monospace];
        assert_eq!(
            monospace.get(..2),
            Some(&[MONO_FONT.to_string(), "Hack".to_string()][..])
        );
    }

    #[test]
    fn headings_and_bold_text_fall_back_to_the_text_font() {
        let fonts = fonts();
        for (family, font) in [(SERIF, SERIF_FONT), (BOLD, BOLD_FONT)] {
            assert_eq!(
                fonts.families[&named(family)].get(..2),
                Some(&[font.to_string(), TEXT_FONT.to_string()][..]),
                "{family}"
            );
        }
        let fx = Fixture::new(&["work"]);
        let (harness, _wires) = fx.harness();
        let heading = egui::TextStyle::Heading.resolve(&harness.ctx.style_of(egui::Theme::Light));
        assert_eq!(heading.family, egui::FontFamily::Name(SERIF.into()));
    }

    #[test]
    fn palette_finds_the_pane_or_chrome_palette_a_ui_is_drawn_in() {
        for (dark, pane) in [(false, &PAPER), (true, &NIGHT)] {
            let mut harness = egui_kittest::Harness::new_ui(move |ui| {
                *ui.visuals_mut() = pane.visuals(dark);
                assert_eq!(palette(ui).panel, pane.panel);
                assert_eq!(chrome_frame(ui).fill, chrome_of(pane).panel);
                chrome(ui);
                assert_eq!(palette(ui).panel, chrome_of(pane).panel);
                chrome(ui);
                assert_eq!(palette(ui).panel, chrome_of(pane).panel, "chrome twice");
            });
            harness.run();
        }
    }

    #[test]
    fn panel_colours_tell_the_palettes_apart() {
        for (i, a) in PALETTES.iter().enumerate() {
            for b in &PALETTES[i + 1..] {
                assert_ne!(a.panel, b.panel);
            }
        }
    }

    #[test]
    fn the_app_installs_both_pane_palettes_flat() {
        let fx = Fixture::new(&["work"]);
        let (harness, _wires) = fx.harness();
        let dark = harness.ctx.style_of(egui::Theme::Dark).visuals.clone();
        let light = harness.ctx.style_of(egui::Theme::Light).visuals.clone();
        assert_eq!(
            (
                dark.hyperlink_color,
                dark.selection.bg_fill,
                dark.panel_fill
            ),
            (NIGHT.accent, NIGHT.selected, NIGHT.panel)
        );
        assert_eq!(
            (light.hyperlink_color, light.window_fill),
            (PAPER.accent, PAPER.background)
        );
        assert_eq!(
            (dark.window_shadow, dark.popup_shadow),
            (egui::Shadow::NONE, egui::Shadow::NONE)
        );
    }

    /// WCAG contrast ratio between two opaque colours, 1 to 21.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let luminance = |c: Color32| {
            let channel = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.040_45 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        };
        let (light, dark) = (
            luminance(a).max(luminance(b)),
            luminance(a).min(luminance(b)),
        );
        (light + 0.05) / (dark + 0.05)
    }

    /// Regression for #41: bold text in the dark theme was drawn in `on_accent`, near black on the dark panels.
    #[test]
    fn strong_text_stands_out_from_normal_text_in_every_palette() {
        for (dark, palette) in ALL {
            let visuals = palette.visuals(dark);
            let strong = visuals.strong_text_color();
            for background in [visuals.panel_fill, visuals.window_fill] {
                let normal = contrast(visuals.text_color(), background);
                let bold = contrast(strong, background);
                assert!(
                    bold >= 7.0,
                    "{:?}: strong text contrast {bold}",
                    palette.panel
                );
                assert!(
                    bold > normal,
                    "{:?}: strong {bold} <= normal {normal}",
                    palette.panel
                );
            }
            let pressed = contrast(strong, visuals.widgets.active.bg_fill);
            assert!(
                pressed >= 4.5,
                "{:?}: pressed widget contrast {pressed}",
                palette.panel
            );
        }
    }

    #[test]
    fn text_on_the_selection_links_and_status_text_pass_aa_in_every_palette() {
        for (_, p) in ALL {
            let name = p.panel;
            let selected = contrast(p.on_accent, p.selected);
            assert!(
                selected >= 4.5,
                "{name:?}: text on the selection {selected}"
            );
            let heading = contrast(p.heading, p.background);
            assert!(heading >= 4.5, "{name:?}: heading {heading}");
            let link = contrast(p.accent, p.background);
            assert!(link >= 4.5, "{name:?}: accent text {link}");
            for (what, color) in [("muted", p.muted), ("error", p.error)] {
                let ratio = contrast(color, p.panel).min(contrast(color, p.hover));
                assert!(ratio >= 4.5, "{name:?}: {what} text {ratio}");
            }
            let marks = contrast(p.highlight, p.panel);
            assert!(marks >= 3.0, "{name:?}: highlight marks {marks}");
        }
    }

    /// Regression: light links were oxblood, which next to the near-black ink read as plain text.
    #[test]
    fn links_stand_apart_from_text_in_every_palette() {
        for (_, p) in ALL {
            let apart = contrast(p.accent, p.text);
            assert!(apart >= 1.2, "{:?}: link against text {apart}", p.panel);
        }
    }

    #[test]
    fn light_strong_text_and_pressed_widgets_keep_their_colours() {
        let light = PAPER.visuals(false);
        assert_eq!(light.strong_text_color(), PAPER.strong);
        assert_eq!(light.widgets.active.bg_fill, PAPER.pressed);
    }

    /// Regression: the accent focus ring went through `selection.stroke`, which egui also uses for selected text.
    #[test]
    fn selected_text_in_text_fields_is_readable_in_every_palette() {
        for (dark, palette) in ALL {
            let mut harness = egui_kittest::Harness::new_ui(move |ui| {
                *ui.visuals_mut() = palette.visuals(dark);
                let mut selection = ui.visuals().selection;
                text_field(ui, |ui| {
                    selection = ui.visuals().selection;
                    ui.label("")
                });
                let ratio = contrast(selection.stroke.color, selection.bg_fill);
                assert!(ratio >= 4.5, "dark={dark}: selected text contrast {ratio}");
            });
            harness.run();
        }
    }

    #[test]
    fn focused_text_fields_get_an_accent_focus_ring() {
        for (dark, palette) in ALL {
            let mut harness = egui_kittest::Harness::new_ui(move |ui| {
                *ui.visuals_mut() = palette.visuals(dark);
                let mut text = String::from("hello");
                text_field(ui, |ui| {
                    let response = ui.text_edit_singleline(&mut text);
                    response.request_focus();
                    response
                });
            });
            harness.run();
            let rings: Vec<_> = harness
                .output()
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(rect) if rect.stroke.width > 0.0 => Some(rect.clone()),
                    _ => None,
                })
                .collect();
            let frame = rings
                .iter()
                .find(|rect| rect.fill == palette.visuals(dark).text_edit_bg_color());
            let ring = rings
                .iter()
                .find(|rect| rect.stroke.color == palette.accent);
            let (Some(frame), Some(ring)) = (frame, ring) else {
                panic!("dark={dark}: no frame or accent ring in {rings:?}");
            };
            assert_eq!(ring.rect, frame.rect, "dark={dark}");
        }
    }
}
