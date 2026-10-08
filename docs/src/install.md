# Install

| Channel | Command |
|---|---|
| Homebrew | `brew install pataar/tap/postbode` |
| crates.io | `cargo install postbode` |
| mise | `mise use ubi:pataar/postbode` |
| AppImage (Linux) | download `postbode-<arch>.AppImage` from the [latest release](https://github.com/pataar/postbode/releases/latest), see [below](#appimage-on-linux) |

Each [GitHub release](https://github.com/pataar/postbode/releases) also has plain binaries for macOS and Linux on x86_64 and aarch64.

To build from source:

```sh
git clone https://github.com/pataar/postbode
cd postbode
mise install rust         # the pinned Rust toolchain; rustup works too
cargo install --path .
```

On Linux the password keyring is the Secret Service (KDE Wallet or GNOME Keyring), reached over D-Bus. On macOS it is the login Keychain; a new, unsigned binary (every upgrade) asks again for Keychain access, so choose "Always Allow".

## AppImage on Linux

Each release has an AppImage for x86_64 and aarch64, which runs on any desktop (KDE Plasma, GNOME and others) with no install:

```sh
chmod +x postbode-x86_64.AppImage
./postbode-x86_64.AppImage                  # double-clicked or run without a terminal it opens the window
./postbode-x86_64.AppImage account add      # with arguments it is the postbode command
./postbode-x86_64.AppImage mcp install      # MCP hosts and `service install` record this file's path
```

Keep the file where you run it from: the daemon it starts, `service install` and `mcp install` all point at it, so after moving it run those two again. To put it in your app launcher, use an AppImage manager such as Gear Lever or AppImageLauncher. The AppImage uses the system's graphics libraries and FUSE (`fusermount`), which most desktops have.

## Desktop entry on Linux

Apart from the AppImage, the channels above install only the binary. To get Postbode into your app launcher with its icon, run this from a checkout:

```sh
packaging/linux/install.sh                                # into ~/.local/share, for you
sudo packaging/linux/install.sh --prefix /usr/local/share # for everyone
packaging/linux/install.sh --uninstall                    # add the same --prefix if you used one
```

The entry runs `postbode gui`, so `postbode` must be on your `PATH`. The window's Wayland app id, `io.github.pataar.postbode`, matches the entry, so docks and task switchers show the right name and icon.
