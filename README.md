# EC

EC 0.3.0 is a terminal mouse click controller for Linux and Wayland. Hold the physical left mouse button and EC generates repeated synthetic clicks while forwarding movement, scrolling, and unrelated mouse buttons through its virtual mouse.

> Synthetic clicking may violate game or service rules.

## Install

```sh
curl -fsSL https://ec.luigiverona.dev/install | sh
```

Configure EC as your normal user:

```sh
ec setup
```

If system access is missing, follow the exact `sudo` command printed by EC. Then run `ec setup` again as your normal user to select the physical mouse. Normal EC operation must not run as root.

Verify readiness and start EC at the default rate:

```sh
ec doctor
ec start
```

The installer supports Linux x86_64 and aarch64. It downloads the matching binary and `SHA256SUMS` from GitHub Releases, verifies the binary with SHA-256, and installs it to `~/.local/bin/ec`. Rerun the installer to upgrade to the latest release.

The installer does not use sudo or configure device permissions. `ec setup` performs the one-time system-access setup and saves the selected physical mouse. `ec doctor` diagnoses access to evdev devices and uinput.

## Usage

```sh
ec start
ec 100
ec 600
ec status
ec stop
ec setup
ec doctor
```

`ec start` uses the default click rate. `ec <CPS>` starts EC at a custom rate from 1–600 CPS. `ec status` reports whether EC is active, `ec stop` stops it, and `ec setup` configures device access and selects the physical mouse.

EC supports Wayland compositors, including Hyprland. The user must have appropriate evdev/uinput access; permission setup varies by Linux distribution.

## Build from source

If a release binary is not suitable, install stable Rust and the required Linux development headers, clone the repository, and build:

```sh
cargo build --release
```

The binary is written to `target/release/ec`. Source builds are the fallback; prebuilt binaries come from GitHub Releases and are checksum-verified for corruption or a mismatched download. Checksums do not protect against compromise of the repository or release account.

## Uninstall

```sh
ec stop
rm -f "$HOME/.local/bin/ec"
```

Optionally remove the saved mouse selection and system access rule:

```sh
rm -f "$HOME/.config/ec/device"
sudo rm -f /etc/udev/rules.d/70-ec.rules
```
