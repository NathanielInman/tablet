# Tablet

`docs/design.md` is the spec; the README is the tour. Read the spec section
for whatever you change.

## Crates

- `bar-anchor` is no longer in this repo: it's the shared
  [bar-anchor](https://github.com/NathanielInman/bar-anchor) crate, pinned by tag in the root `Cargo.toml`.
  Change it there and move the tag; its `Anchor` JSON must stay
  identical across the apps.
- `tablet-core`: openvpn3's statuses and sessions (`openvpn.rs`), the checks
  (`checks.rs`), the doctor (`doctor.rs`), names to profiles (`route.rs`),
  the daemon's state and its transitions (`state.rs`: what happened in,
  effects out), the bar line (`bar.rs`), config, the socket protocol and the
  waybar reader. No D-Bus, no UI. A new rule about a profile's phase belongs
  in `state.rs` with a test.
- `tablet`: the daemon (`daemon.rs`: threads, sockets, `session-start`,
  probes and D-Bus calls around `state::Model`), the zbus glue (`dbus.rs`),
  the CLI and the waybar module (one binary, `tablet`).
- `tablet-board`: the Linux GTK4 layer-shell drawer. `anim.rs` and
  `stage.rs` came from Tannoy's board.

## Build and test

`cargo build --workspace --locked`, `cargo test --workspace --locked`,
`cargo clippy --workspace --all-targets -- -D warnings` and `tools/e2e.sh`
(the real daemon, bar and CLI against `tools/fake-openvpn3.py` on a private
bus; CI runs it too). CI also runs `cargo deny check`. The drawer needs
gtk4-layer-shell; `.github/scripts/gtk4-layer-shell.sh` is how CI gets it.

Parsers are pure functions over plain data and are tested against recorded
samples in `crates/tablet-core/fixtures/` (busctl `GetAll` dumps of
openvpn3 sessions and a resolved link, waybar CSS). Add a recorded sample
when you touch a parser, and replace every address, host, path and id
first.

To look at the drawer without a desktop, render it under GTK's broadway
backend: `gtk4-broadwayd :7 &`, then
`GDK_BACKEND=broadway BROADWAY_DISPLAY=:7 target/debug/tablet-board --shot
out.png snapshot.json [now|checks|doctor|history]`, with a snapshot from
`tablet status --json`.

## Never touch a real VPN

The machine you work on may hold a real tunnel someone is using. Tests and
experiments never connect, disconnect or reconnect a real session.

- Use `tools/e2e.sh`, or run a dev daemon against the fake: start
  `dbus-run-session`, export `DBUS_SYSTEM_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS`,
  run `tools/fake-openvpn3.py serve`, and set `openvpn3.command` to
  `python3 tools/fake-openvpn3.py`.
- A dev daemon pointed at the real system bus may read (`status`,
  `check`, `why`, `doctor`) but never send `connect`, `disconnect`,
  `reconnect`, `flip`, `ack` or `answer`.
- The daemon runs `session-start` with its own environment, not the
  command's that asked; the fake reads its mode from
  `FAKE_OPENVPN3_MODE_FILE` for that reason.

## Isolation

Point a dev build away from an installed Tablet with `TABLET_RUNTIME_DIR`
(keep it short: socket paths have a length limit), `TABLET_STATE_DIR` and
`TABLET_CONFIG_DIR`.

## Privacy

The repo is public; VPNs are not.

- Never commit a real server, host, zone, resolver, `.ovpn` path or company
  name. Use `vpn.example.com`, `corp.example.com`, `10.8.0.2` and
  `198.51.100.7` (the documentation range).
- Sign-in URLs carry one-time tokens: never log them. The daemon replaces
  any URL in `session-start`'s output with `<sign-in page>`.
- The drawer shows the server's address only with `drawer.addresses`.

## Rules

- Never start openvpn3 just to ask it something: no manager call while
  nobody owns `net.openvpn.v3.sessions`.
- Nothing may stall the bar stream. D-Bus calls have a 5 second timeout and
  run on helper or connection threads; probes and `session-start` run on
  their own threads; the lock that feeds the bar is held for memory work
  only.
- Nothing connects without a person: `ask` waits for Connect, and no timer
  reconnects by itself.
- Red stays red until someone acts (reconnect or dismiss).

## When behavior changes

Update in the same commit:

- `docs/design.md` for a new config key, protocol message, phase or rule.
- The README for anything a new user meets (CLI, keys, install, config).
- `packaging/man/tablet.1` and the three files in `packaging/completions`
  for a new command or flag (a test fails until each one mentions it).
- `docs/assets/hero.py` when the drawer or the bar module looks different,
  then run `python3 docs/assets/hero.py` to rewrite `hero.svg` (never edit
  the SVG by hand).

Commits are conventional, scoped by area: `anchor`, `core`, `openvpn`,
`checks`, `doctor`, `state`, `cli`, `daemon`, `board`, `docs`, `ci`.
