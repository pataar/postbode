//! The Catppuccin palette: Mocha for dark, Latte for light. Views take colours from here or from egui's visuals.
use eframe::egui::{self, Color32, Stroke};

pub(crate) struct Palette {
    pub accent: Color32,
    pub background: Color32,
    pub border: Color32,
    pub error: Color32,
    pub faint: Color32,
    pub highlight: Color32,
    pub hover: Color32,
    pub muted: Color32,
    pub on_accent: Color32,
    pub panel: Color32,
    pub secondary: Color32,
    pub success: Color32,
    pub text: Color32,
    pub warning: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    let [_, red, green, blue] = rgb.to_be_bytes();
    Color32::from_rgb(red, green, blue)
}

pub(crate) const MOCHA: Palette = Palette {
    accent: hex(0x94e2d5),
    background: hex(0x1e1e2e),
    border: hex(0x45475a),
    error: hex(0xf38ba8),
    faint: hex(0x11111b),
    highlight: hex(0xfab387),
    hover: hex(0x313244),
    muted: hex(0x7f849c),
    on_accent: hex(0x11111b),
    panel: hex(0x181825),
    secondary: hex(0xa6adc8),
    success: hex(0xa6e3a1),
    text: hex(0xcdd6f4),
    warning: hex(0xf9e2af),
};

pub(crate) const LATTE: Palette = Palette {
    accent: hex(0x179299),
    background: hex(0xeff1f5),
    border: hex(0xbcc0cc),
    error: hex(0xd20f39),
    faint: hex(0xdce0e8),
    highlight: hex(0xfe640b),
    hover: hex(0xccd0da),
    muted: hex(0x8c8fa1),
    on_accent: hex(0x11111b),
    panel: hex(0xe6e9ef),
    secondary: hex(0x6c6f85),
    success: hex(0x40a02b),
    text: hex(0x4c4f69),
    warning: hex(0xdf8e1d),
};

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
        visuals.selection.bg_fill = self.accent;
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
        widgets.active.bg_fill = self.accent;
        widgets.active.weak_bg_fill = self.accent;
        widgets.active.fg_stroke.color = self.on_accent;
        visuals
    }
}

/// Gives egui both palettes; the theme preference picks one, following the OS when it is System.
pub(crate) fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, MOCHA.visuals(true));
    ctx.set_visuals_of(egui::Theme::Light, LATTE.visuals(false));
}

/// The palette of the mode `ui` is drawn in.
pub(crate) fn palette(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode {
        &MOCHA
    } else {
        &LATTE
    }
}

/// Runs `add` with the focus ring of a text field in the accent: egui draws it with `selection.stroke`, which stays
/// `on_accent` for the text of selected rows.
pub(crate) fn text_field<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        ui.visuals_mut().selection.stroke = Stroke::new(1.0, palette(ui).accent);
        add(ui)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Color32};

    use super::*;
    use crate::gui::test_support::Fixture;

    #[test]
    fn the_palettes_are_catppuccin() {
        assert_eq!(MOCHA.accent, Color32::from_rgb(0x94, 0xe2, 0xd5));
        assert_eq!(MOCHA.background, Color32::from_rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(MOCHA.highlight, Color32::from_rgb(0xfa, 0xb3, 0x87));
        assert_eq!(LATTE.accent, Color32::from_rgb(0x17, 0x92, 0x99));
        assert_eq!(LATTE.on_accent, Color32::from_rgb(0x11, 0x11, 0x1b));
        assert_eq!(LATTE.text, Color32::from_rgb(0x4c, 0x4f, 0x69));
    }

    #[test]
    fn the_app_installs_both_palettes_flat() {
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
            (MOCHA.accent, MOCHA.accent, MOCHA.panel)
        );
        assert_eq!(
            (light.hyperlink_color, light.window_fill),
            (LATTE.accent, LATTE.background)
        );
        assert_eq!(
            (dark.window_shadow, dark.popup_shadow),
            (egui::Shadow::NONE, egui::Shadow::NONE)
        );
    }

    #[test]
    fn text_fields_get_an_accent_focus_ring_without_changing_selection_text() {
        for (dark, palette) in [(true, &MOCHA), (false, &LATTE)] {
            let mut harness = egui_kittest::Harness::new_ui(move |ui| {
                *ui.visuals_mut() = palette.visuals(dark);
                let inside = text_field(ui, |ui| ui.visuals().selection.stroke.color);
                assert_eq!(inside, palette.accent);
                assert_eq!(ui.visuals().selection.stroke.color, palette.on_accent);
            });
            harness.run();
        }
    }
}
