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

## Desktop entry on Linux

The channels above install only the binary. To get Postbode into your app launcher with its icon, run this from a checkout:

```sh
packaging/linux/install.sh                                # into ~/.local/share, for you
sudo packaging/linux/install.sh --prefix /usr/local/share # for everyone
packaging/linux/install.sh --uninstall                    # add the same --prefix if you used one
```

The entry runs `postbode gui`, so `postbode` must be on your `PATH`. The window's Wayland app id, `io.github.pataar.postbode`, matches the entry, so docks and task switchers show the right name and icon.
