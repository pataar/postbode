//! The mail window: folders, threads and the message body over the local store, as a client of the daemon that syncs.
mod app;
mod body;
mod folders;
mod list;
mod rules;
#[cfg(test)]
mod snapshots;
mod status;
#[cfg(test)]
mod test_support;
mod theme;

use std::sync::{Arc, OnceLock};

use anyhow::Result;
use eframe::egui;

use crate::config::Config;
use crate::paths::Paths;

pub use app::App;

/// The window icon: `assets/icon.svg` rendered to PNG by `packaging/icons.sh`.
const ICON: &[u8] = include_bytes!("../../assets/icon.png");

/// The Wayland app id and X11 WM class; the Linux desktop entry and icons are named after it, so the compositor can
/// match the window to them.
const APP_ID: &str = "io.github.pataar.postbode";

/// Opens the window and returns when it closes.
pub fn run(config: &Config, paths: &Paths) -> Result<()> {
    // Events can arrive before the window has a context; they wait in the channel for its first frame.
    let window: Arc<OnceLock<egui::Context>> = Arc::default();
    let wake = {
        let window = window.clone();
        Box::new(move || {
            if let Some(ctx) = window.get() {
                ctx.request_repaint();
            }
        })
    };
    let (client, events, states) = app::connect(paths, wake)?;
    let (config, paths) = (config.clone(), paths.clone());
    // ICON is embedded at compile time and decoded by `the_icon_is_a_square_png_with_transparent_corners`.
    #[allow(clippy::expect_used)]
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(
                eframe::icon_data::from_png_bytes(ICON).expect("assets/icon.png is a valid PNG"),
            )
            .with_app_id(APP_ID)
            .with_inner_size([1280.0, 800.0])
            .with_title("Postbode"),
        ..Default::default()
    };
    eframe::run_native(
        "Postbode",
        options,
        Box::new(move |cc| {
            let _ = window.set(cc.egui_ctx.clone());
            Ok(Box::new(App::new(&config, paths, client, events, states)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("the window failed: {e}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_icon_is_a_square_png_with_transparent_corners() {
        let icon = eframe::icon_data::from_png_bytes(super::ICON).unwrap();
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba[3], 0, "the top-left pixel must be transparent");
    }

    #[test]
    fn the_desktop_entry_names_the_app_id() {
        let entry = include_str!("../../packaging/linux/io.github.pataar.postbode.desktop");
        for key in ["Icon", "StartupWMClass"] {
            let line = format!("{key}={}", super::APP_ID);
            assert!(
                entry.lines().any(|l| l == line),
                "the desktop entry lacks {line}"
            );
        }
    }
}
