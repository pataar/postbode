//! The mail window: folders, threads and the message body over the local store, as a client of the daemon that syncs.
mod app;
mod body;
mod folders;
mod list;
mod rules;
#[cfg(test)]
mod snapshots;
mod startup;
mod status;
#[cfg(test)]
mod test_support;
mod theme;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{Result, bail};
use eframe::egui;

use crate::config::Config;
use crate::paths::Paths;

pub use app::App;

/// The window icon: `assets/icon.svg` rendered to PNG by `packaging/icons.sh`.
const ICON: &[u8] = include_bytes!("../../assets/icon.png");

/// The Wayland app id and X11 WM class; the Linux desktop entry and icons are named after it, so the compositor can
/// match the window to them.
const APP_ID: &str = "io.github.pataar.postbode";

/// Opens the window and returns when it closes. When it cannot start, a small window says why before the error is
/// returned, since a launch from a desktop entry or Finder has no terminal for stderr.
pub fn run(paths: Result<Paths>) -> Result<()> {
    // Events can arrive before the window has a context; they wait in the channel for its first frame.
    let window: Arc<OnceLock<egui::Context>> = Arc::default();
    let error = match paths {
        Ok(paths) => match start(&paths, window.clone()) {
            Ok((config, connection)) => return open(config, paths, connection, &window),
            Err(e) => explain(e, Some(&paths.daemon_log())),
        },
        Err(e) => explain(e, None),
    };
    Err(error)
}

/// What the mail window needs before it opens: a config with an account, and the daemon, whose events repaint
/// `window` once it has a context.
fn start(paths: &Paths, window: Arc<OnceLock<egui::Context>>) -> Result<(Config, app::Connection)> {
    let config = Config::load(&paths.config_file())?;
    if config.accounts.is_empty() {
        bail!(startup::NoAccounts);
    }
    let wake = Box::new(move || {
        if let Some(ctx) = window.get() {
            ctx.request_repaint();
        }
    });
    Ok((config, app::connect(paths, wake)?))
}

/// A window with the icon, app id and title, at `size`.
fn viewport(size: [f32; 2]) -> egui::ViewportBuilder {
    // ICON is embedded at compile time and decoded by `the_icon_is_a_square_png_with_transparent_corners`.
    #[allow(clippy::expect_used)]
    let icon = eframe::icon_data::from_png_bytes(ICON).expect("assets/icon.png is a valid PNG");
    egui::ViewportBuilder::default()
        .with_icon(icon)
        .with_app_id(APP_ID)
        .with_inner_size(size)
        .with_title("Postbode")
}

/// Shows why the mail window could not start until the user closes it, then hands the error back for stderr and the
/// exit code. Without a display there is no window, and stderr is all there is.
fn explain(error: anyhow::Error, log: Option<&Path>) -> anyhow::Error {
    let problem = startup::Problem::of(&error, log);
    let options = eframe::NativeOptions {
        viewport: viewport([520.0, 260.0]),
        ..Default::default()
    };
    let shown = eframe::run_native(
        "Postbode",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(Explain(problem)))
        }),
    );
    if let Err(e) = shown {
        log::debug!("could not show the startup error in a window: {e}");
    }
    error
}

/// The window in place of the mail window when that cannot start.
pub(crate) struct Explain(startup::Problem);

impl Explain {
    fn show(&mut self, ui: &mut egui::Ui) {
        if startup::show(&self.0, ui) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for Explain {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

fn open(
    config: Config,
    paths: Paths,
    (client, events, states): app::Connection,
    window: &OnceLock<egui::Context>,
) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: viewport([1280.0, 800.0]),
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
