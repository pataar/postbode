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

use std::sync::mpsc::{self, Receiver};

use anyhow::Result;
use eframe::egui;

use crate::config::Config;
use crate::paths::Paths;
use crate::sync::Event;

pub use app::App;

/// The window icon: `assets/icon.svg` rendered to PNG by `packaging/icons.sh`.
const ICON: &[u8] = include_bytes!("../../assets/icon.png");

/// Opens the window and returns when it closes.
pub fn run(config: &Config, paths: &Paths) -> Result<()> {
    let (client, events, states) = app::connect(paths)?;
    let (config, paths) = (config.clone(), paths.clone());
    // ICON is embedded at compile time and decoded by `the_icon_is_a_square_png_with_transparent_corners`.
    #[allow(clippy::expect_used)]
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(
                eframe::icon_data::from_png_bytes(ICON).expect("assets/icon.png is a valid PNG"),
            )
            .with_inner_size([1280.0, 800.0])
            .with_title("Postbode"),
        ..Default::default()
    };
    eframe::run_native(
        "Postbode",
        options,
        Box::new(move |cc| {
            let events = forward(events, cc.egui_ctx.clone())?;
            Ok(Box::new(App::new(&config, paths, client, events, states)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("the window failed: {e}"))
}

/// Passes the daemon's events on and repaints for each, so an idle window still shows them; the returned receiver
/// disconnects when the daemon's does.
fn forward(events: Receiver<Event>, ctx: egui::Context) -> std::io::Result<Receiver<Event>> {
    let (forward, received) = mpsc::channel();
    std::thread::Builder::new()
        .name("gui-events".into())
        .spawn(move || {
            for event in events {
                if forward.send(event).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
            ctx.request_repaint();
        })?;
    Ok(received)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_icon_is_a_square_png_with_transparent_corners() {
        let icon = eframe::icon_data::from_png_bytes(super::ICON).unwrap();
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba[3], 0, "the top-left pixel must be transparent");
    }
}
