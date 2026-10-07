//! Installs `postbode run` as a login service: a launchd agent on macOS, a systemd user unit on Linux.
use std::fmt::Write as _;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::Client;
use crate::paths::{self, Paths};

const LAUNCHD_LABEL: &str = "nl.pataar.postbode";
const SYSTEMD_UNIT: &str = "postbode";
const UNSUPPORTED: &str = "postbode service supports macOS and Linux";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Launchd,
    Systemd,
}

/// One command to run; a tolerated failure is not an error (booting out a service that is not loaded).
struct Step {
    args: Vec<String>,
    program: &'static str,
    tolerate_failure: bool,
}

impl Step {
    fn new(program: &'static str, args: &[&str], tolerate_failure: bool) -> Step {
        Step {
            args: args.iter().map(|arg| arg.to_string()).collect(),
            program,
            tolerate_failure,
        }
    }

    fn shown(&self) -> String {
        format!("{} {}", self.program, self.args.join(" "))
    }

    fn run(&self) -> Result<()> {
        let status = Command::new(self.program)
            .args(&self.args)
            .status()
            .with_context(|| format!("running `{}`", self.shown()))?;
        if !status.success() && !self.tolerate_failure {
            bail!("`{}` failed ({status})", self.shown());
        }
        Ok(())
    }
}

impl Target {
    pub fn current() -> Result<Target> {
        if cfg!(target_os = "macos") {
            Ok(Target::Launchd)
        } else if cfg!(target_os = "linux") {
            Ok(Target::Systemd)
        } else {
            bail!(UNSUPPORTED)
        }
    }

    pub fn file_name(self) -> &'static str {
        match self {
            Target::Launchd => "nl.pataar.postbode.plist",
            Target::Systemd => "postbode.service",
        }
    }

    pub fn file(self, home: &Path) -> PathBuf {
        match self {
            Target::Launchd => home.join("Library/LaunchAgents").join(self.file_name()),
            Target::Systemd => home.join(".config/systemd/user").join(self.file_name()),
        }
    }

    fn text(self, exe: &Path, log: &Path) -> String {
        match self {
            Target::Launchd => plist(exe, log),
            Target::Systemd => unit(exe),
        }
    }

    fn install_steps(self, home: &Path, file: &Path) -> Result<Vec<Step>> {
        Ok(match self {
            Target::Launchd => {
                let domain = launchd_domain(home)?;
                let file = file.to_string_lossy();
                vec![
                    Step::new(
                        "launchctl",
                        &["bootout", &format!("{domain}/{LAUNCHD_LABEL}")],
                        true,
                    ),
                    Step::new("launchctl", &["bootstrap", &domain, &file], false),
                ]
            }
            Target::Systemd => vec![
                Step::new("systemctl", &["--user", "daemon-reload"], false),
                Step::new(
                    "systemctl",
                    &["--user", "enable", "--now", SYSTEMD_UNIT],
                    false,
                ),
            ],
        })
    }

    /// The first step stops the service; the rest run after the file is deleted.
    fn remove_steps(self, home: &Path) -> Result<Vec<Step>> {
        Ok(match self {
            Target::Launchd => vec![Step::new(
                "launchctl",
                &[
                    "bootout",
                    &format!("{}/{LAUNCHD_LABEL}", launchd_domain(home)?),
                ],
                true,
            )],
            Target::Systemd => vec![
                Step::new(
                    "systemctl",
                    &["--user", "disable", "--now", SYSTEMD_UNIT],
                    true,
                ),
                Step::new("systemctl", &["--user", "daemon-reload"], true),
            ],
        })
    }
}

/// The text to print; with `dry_run` nothing is written, stopped or run.
pub fn install(paths: &Paths, dry_run: bool) -> Result<String> {
    let target = Target::current()?;
    let home = home_dir()?;
    let file = target.file(&home);
    let exe = std::env::current_exe().context("finding this postbode")?;
    let text = target.text(&exe, &paths.daemon_log());
    let steps = target.install_steps(&home, &file)?;
    if !dry_run {
        stop_running_daemon(paths)?;
        paths::write_atomic_keeping_dir_mode(&file, text.as_bytes())
            .with_context(|| format!("writing {}", file.display()))?;
        steps.iter().try_for_each(Step::run)?;
    }
    Ok(describe(
        dry_run,
        ("would write", "wrote"),
        &file,
        Some(&text),
        &steps,
    ))
}

/// The text to print; with `dry_run` nothing is stopped, run or deleted.
pub fn remove(dry_run: bool) -> Result<String> {
    let target = Target::current()?;
    let home = home_dir()?;
    let file = target.file(&home);
    let steps = target.remove_steps(&home)?;
    if !dry_run {
        steps[0].run()?;
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("deleting {}", file.display())),
        }
        steps[1..].iter().try_for_each(Step::run)?;
    }
    Ok(describe(
        dry_run,
        ("would delete", "deleted"),
        &file,
        None,
        &steps,
    ))
}

/// A daemon already running holds the lock the service's daemon needs.
fn stop_running_daemon(paths: &Paths) -> Result<()> {
    if let Some(client) = Client::connect_any_version(paths)? {
        client.stop_and_wait(paths)?;
    }
    Ok(())
}

/// `verbs` is the (dry run, done) wording of the file change.
fn describe(
    dry_run: bool,
    verbs: (&str, &str),
    file: &Path,
    text: Option<&str>,
    steps: &[Step],
) -> String {
    let (verb, run) = if dry_run {
        (verbs.0, "would run")
    } else {
        (verbs.1, "ran")
    };
    let mut out = format!("{verb} {}", file.display());
    if let Some(text) = text {
        let _ = write!(out, ":\n{}", text.trim_end());
    }
    for step in steps {
        let _ = write!(out, "\n{run}: {}", step.shown());
    }
    out
}

fn home_dir() -> Result<PathBuf> {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .context("no home directory found")
}

/// `gui/<uid>`; the uid is the home directory's owner, which avoids a libc call.
fn launchd_domain(home: &Path) -> Result<String> {
    let uid = std::fs::metadata(home)
        .with_context(|| format!("reading {}", home.display()))?
        .uid();
    Ok(format!("gui/{uid}"))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn plist(exe: &Path, log: &Path) -> String {
    plist_with_path(exe, log, std::env::var("PATH").ok().as_deref())
}

fn plist_with_path(exe: &Path, log: &Path, path: Option<&str>) -> String {
    let exe = xml_escape(&exe.to_string_lossy());
    let log = xml_escape(&log.to_string_lossy());
    let environment = path.map_or_else(String::new, |path| {
        format!(
            "    <key>EnvironmentVariables</key>\n    <dict>\n        <key>PATH</key>\n        <string>{}</string>\n    </dict>\n",
            xml_escape(path)
        )
    });
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
{environment}</dict>
</plist>
"#
    )
}

/// systemd expands `%` specifiers and unquotes `"` and `\` in ExecStart, so a path needs all three escaped.
pub fn unit(exe: &Path) -> String {
    let exe = exe
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!(
        "[Unit]\nDescription=Postbode mail sync\n\n[Service]\nExecStart=\"{exe}\" run\nRestart=on-failure\nRestartSec=10\n\n[Install]\nWantedBy=default.target\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launchd_plist_runs_the_binary_at_login_and_keeps_it_alive() {
        let text = plist(
            Path::new("/opt/homebrew/bin/postbode"),
            Path::new("/Users/me/Library/Application Support/postbode/daemon.log"),
        );
        for needle in [
            "<string>nl.pataar.postbode</string>",
            "<string>/opt/homebrew/bin/postbode</string>",
            "<string>run</string>",
            "<key>RunAtLoad</key>",
            "<key>KeepAlive</key>",
            "<key>EnvironmentVariables</key>",
            "daemon.log",
        ] {
            assert!(text.contains(needle), "{needle} missing:\n{text}");
        }
    }

    #[test]
    fn the_plist_escapes_xml_in_paths() {
        let text = plist(Path::new("/a&b/postbode"), Path::new("/x/<log>"));
        assert!(text.contains("/a&amp;b/postbode"), "{text}");
        assert!(text.contains("/x/&lt;log&gt;"), "{text}");
    }

    #[test]
    fn the_systemd_unit_restarts_on_failure() {
        let text = unit(Path::new("/home/me/.cargo/bin/postbode"));
        assert!(text.contains("ExecStart=\"/home/me/.cargo/bin/postbode\" run"));
        assert!(text.contains("Restart=on-failure"));
        assert!(text.contains("RestartSec=10"));
        assert!(text.contains("WantedBy=default.target"));
    }

    #[test]
    fn the_systemd_exec_start_survives_spaces_and_percent_signs() {
        let text = unit(Path::new("/home/me/my bin/50%/postbode"));
        assert!(
            text.contains("ExecStart=\"/home/me/my bin/50%%/postbode\" run"),
            "{text}"
        );
    }

    #[test]
    fn the_plist_has_no_environment_without_a_path() {
        let text = plist_with_path(Path::new("/bin/postbode"), Path::new("/x/log"), None);
        assert!(!text.contains("EnvironmentVariables"), "{text}");
        assert!(text.contains("</dict>\n</plist>"), "{text}");
    }

    #[test]
    fn removing_the_systemd_unit_reloads_systemd_after_the_file_is_gone() {
        let steps = Target::Systemd.remove_steps(Path::new("/home/me")).unwrap();
        let shown: Vec<_> = steps.iter().map(Step::shown).collect();
        assert_eq!(
            shown,
            [
                "systemctl --user disable --now postbode",
                "systemctl --user daemon-reload"
            ]
        );
        assert!(steps.iter().all(|step| step.tolerate_failure));
    }

    #[test]
    fn a_dry_run_install_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let before = target_stamp();
        let text = install(&paths, true).unwrap();
        assert_eq!(target_stamp(), before);
        assert!(
            text.contains(Target::current().unwrap().file_name()),
            "{text}"
        );
        assert!(
            text.contains("launchctl") || text.contains("systemctl"),
            "{text}"
        );
        assert!(text.contains("run"), "{text}");
    }

    #[test]
    fn a_dry_run_remove_names_the_file_and_the_command() {
        let before = target_stamp();
        let text = remove(true).unwrap();
        assert_eq!(target_stamp(), before);
        assert!(
            text.contains(Target::current().unwrap().file_name()),
            "{text}"
        );
        assert!(
            text.contains("bootout") || text.contains("disable"),
            "{text}"
        );
    }

    /// Whether the real service file exists and when it last changed; a dry run must leave both alone.
    fn target_stamp() -> Option<std::time::SystemTime> {
        let target = Target::current().unwrap();
        std::fs::metadata(target.file(&home_dir().unwrap()))
            .and_then(|meta| meta.modified())
            .ok()
    }
}
