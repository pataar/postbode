//! The mail window: folders, threads and the message body over the local store, as a client of the daemon that syncs.
mod app;
mod body;
mod folders;
mod html;
mod icons;
mod list;
mod numbers;
mod rules;
#[cfg(test)]
mod snapshots;
mod startup;
mod status;
#[cfg(test)]
mod test_support;
mod theme;
mod toolbar;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{Result, bail};
use eframe::egui;

use crate::config::Config;
use crate::paths::Paths;

pub use app::App;

/// The window icon: `assets/icon.svg` rendered to PNG by `packaging/icons.sh`.
pub(crate) const ICON: &[u8] = include_bytes!("../../assets/icon.png");

/// macOS shows the window icon in the Dock, which expects Apple's grid: the tile at 824/1024 of the canvas.
#[cfg(target_os = "macos")]
const WINDOW_ICON: &[u8] = include_bytes!("../../assets/macos.iconset/icon_512x512@2x.png");
#[cfg(not(target_os = "macos"))]
const WINDOW_ICON: &[u8] = ICON;

/// The Wayland app id and X11 WM class; the Linux desktop entry and icons are named after it, so the compositor can
/// match the window to them.
const APP_ID: &str = "io.github.postvak_app.postvak";

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

/// The window options for a window of `size`.
fn native_options(size: [f32; 2]) -> eframe::NativeOptions {
    let options = eframe::NativeOptions {
        viewport: viewport(size),
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    let options = prefer_integrated_gpu(options);
    options
}

/// egui-wgpu asks for the discrete GPU by default, which keeps it powered on a dual-GPU Intel MacBook Pro for as long
/// as the window is open; egui draws fine on the integrated one. `WGPU_POWER_PREF` still overrides.
#[cfg(target_os = "macos")]
fn prefer_integrated_gpu(mut options: eframe::NativeOptions) -> eframe::NativeOptions {
    use eframe::wgpu::PowerPreference;
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.power_preference = PowerPreference::from_env().unwrap_or(PowerPreference::LowPower);
    }
    options
}

/// A window with the icon, app id and title, at `size`.
fn viewport(size: [f32; 2]) -> egui::ViewportBuilder {
    // WINDOW_ICON is embedded at compile time and decoded by `the_window_icon_is_a_valid_png`.
    #[allow(clippy::expect_used)]
    let icon =
        eframe::icon_data::from_png_bytes(WINDOW_ICON).expect("the window icon is a valid PNG");
    egui::ViewportBuilder::default()
        .with_icon(icon)
        .with_app_id(APP_ID)
        .with_inner_size(size)
        .with_title("Postvak")
}

/// Shows why the mail window could not start until the user closes it, then hands the error back for stderr and the
/// exit code. Without a display there is no window, and stderr is all there is.
fn explain(error: anyhow::Error, log: Option<&Path>) -> anyhow::Error {
    let problem = startup::Problem::of(&error, log);
    let shown = eframe::run_native(
        "Postvak",
        explain_options(),
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

/// The error window remembers nothing. eframe reads a saved window size even when it does not save one, so this
/// window must not share the mail window's file; the empty path also keeps eframe from making a folder of its own.
fn explain_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        persist_window: false,
        persistence_path: Some(std::path::PathBuf::new()),
        ..native_options([520.0, 260.0])
    }
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

    fn persist_egui_memory(&self) -> bool {
        false
    }
}

fn open(
    config: Config,
    paths: Paths,
    (client, events, states): app::Connection,
    window: &OnceLock<egui::Context>,
) -> Result<()> {
    let options = eframe::NativeOptions {
        // The window size and position and the pane widths; egui never saves what is typed in a field.
        persistence_path: Some(paths.state_dir.join("window.ron")),
        ..native_options([1280.0, 800.0])
    };
    eframe::run_native(
        "Postvak",
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
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_windows_prefer_the_integrated_gpu() {
        use eframe::egui_wgpu::WgpuSetup;
        use eframe::wgpu::PowerPreference;
        if std::env::var_os("WGPU_POWER_PREF").is_some() {
            return;
        }
        let options = super::native_options([100.0, 100.0]);
        let WgpuSetup::CreateNew(setup) = options.wgpu_options.wgpu_setup else {
            panic!("eframe's default wgpu setup creates a new instance");
        };
        assert_eq!(setup.power_preference, PowerPreference::LowPower);
    }

    #[test]
    fn the_icon_is_a_square_png_with_transparent_corners() {
        let icon = eframe::icon_data::from_png_bytes(super::ICON).unwrap();
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba[3], 0, "the top-left pixel must be transparent");
    }

    #[test]
    fn the_window_icon_is_a_valid_png() {
        let icon = eframe::icon_data::from_png_bytes(super::WINDOW_ICON).unwrap();
        assert_eq!(icon.width, icon.height);
        if cfg!(target_os = "macos") {
            let left_middle = (icon.height / 2 * icon.width + icon.width / 20) as usize * 4;
            assert_eq!(
                icon.rgba[left_middle + 3],
                0,
                "the Dock icon needs Apple's transparent margin"
            );
        }
    }

    #[test]
    fn the_error_window_saves_nothing() {
        let options = super::explain_options();
        assert!(!options.persist_window);
        assert_eq!(options.persistence_path, Some(std::path::PathBuf::new()));
        let problem = super::startup::Problem::of(&anyhow::anyhow!("no display"), None);
        assert!(!eframe::App::persist_egui_memory(&super::Explain(problem)));
    }

    #[test]
    fn the_desktop_entry_names_the_app_id() {
        let entry = include_str!("../../packaging/linux/io.github.postvak_app.postvak.desktop");
        for key in ["Icon", "StartupWMClass"] {
            let line = format!("{key}={}", super::APP_ID);
            assert!(
                entry.lines().any(|l| l == line),
                "the desktop entry lacks {line}"
            );
        }
    }
}
