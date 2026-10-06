//! The mail window: folders, threads and the message body over the local store, with the account threads running in
//! this process.
mod app;
mod folders;
mod list;
mod status;
#[cfg(test)]
mod test_support;

use std::sync::mpsc;

use anyhow::Result;
use eframe::egui;

use crate::config::Config;
use crate::engine::Engine;
use crate::paths::Paths;

pub use app::App;

/// Opens the window and returns when it closes.
pub fn run(config: &Config, paths: &Paths) -> Result<()> {
    let (engine, events) = Engine::start(config, paths);
    let (config, paths) = (config.clone(), paths.clone());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Postbode"),
        ..Default::default()
    };
    eframe::run_native(
        "Postbode",
        options,
        Box::new(move |cc| {
            let (forward, received) = mpsc::channel();
            let ctx = cc.egui_ctx.clone();
            std::thread::Builder::new()
                .name("gui-events".into())
                .spawn(move || {
                    for event in events {
                        if forward.send(event).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                })?;
            Ok(Box::new(App::new(&config, paths, engine, received)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("the window failed: {e}"))
}
