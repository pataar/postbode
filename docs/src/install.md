# Install

Postbode has no release yet. Build it from source:

```sh
git clone https://github.com/pataar/postbode
cd postbode
mise install rust         # the pinned Rust toolchain; rustup works too
cargo install --path .
```

From the first release on, these channels will work:

| Channel | Command |
|---|---|
| crates.io | `cargo install postbode` |
| mise | `mise use ubi:pataar/postbode` |
| Homebrew | `brew install pataar/tap/postbode` |

On Linux the password keyring is the Secret Service (KDE Wallet or GNOME Keyring), reached over D-Bus. On macOS it is the login Keychain; a new, unsigned binary (every upgrade) asks again for Keychain access, so choose "Always Allow".

## macOS app

Postbode also ships as `Postbode.app` on a DMG, attached to each [GitHub release](https://github.com/pataar/postbode/releases). Homebrew installs it as a cask:

```sh
brew install --cask pataar/tap/postbode
```

The cask puts `Postbode.app` in `/Applications` and links the app's own binary as the `postbode` command, so the CLI and the MCP server come with it. Opening the app from Finder or the Dock opens the mail window; `postbode` in a terminal works as usual.

The app is not notarized by Apple yet; it is signed ad hoc. The cask clears the quarantine flag on install, so it opens normally. A DMG downloaded by hand is blocked by Gatekeeper the first time: clear the flag once with `xattr -dr com.apple.quarantine /Applications/Postbode.app`, or allow it under System Settings → Privacy & Security. Each upgrade has a new signature, so the Keychain asks again for access to the password.

Install either the cask or the formula (`brew install pataar/tap/postbode`), not both: the window, the CLI and the MCP server share one daemon over one socket, and an older client refuses a newer daemon, so two installs at different versions would get in each other's way. Homebrew refuses the second one. The formula stays for Linux and headless Macs.

## Desktop entry on Linux

The channels above install only the binary. To get Postbode into your app launcher with its icon, run this from a checkout:

```sh
packaging/linux/install.sh                                # into ~/.local/share, for you
sudo packaging/linux/install.sh --prefix /usr/local/share # for everyone
packaging/linux/install.sh --uninstall                    # add the same --prefix if you used one
```

The entry runs `postbode gui`, so `postbode` must be on your `PATH`. The window's Wayland app id, `io.github.pataar.postbode`, matches the entry, so docks and task switchers show the right name and icon.
