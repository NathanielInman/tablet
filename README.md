<p align="center">
  <img src="docs/assets/mark.svg" width="46" alt="">
</p>

<h1 align="center">Tablet</h1>

<p align="center">
  <img src="docs/assets/hero.svg" width="100%" alt="A floating waybar of frosted pills: workspaces, the window title, Tannoy&#x27;s song, then the lamps, where Tablet&#x27;s network lamp is dim. In a terminal an agent runs tablet need work for a Gradle build, and the lamp turns amber with the word asked. A right-click opens the Tablet drawer out of it: the agent&#x27;s request with its command, directory and unverified reason, and Connect work. A click on Connect turns the lamp into a pulsing ellipsis, then sign in while a browser finishes the web sign-in, then frost: the drawer holds work with its traffic graph and both probes answering, and the build runs in the terminal. The server goes quiet: the probes fail twice, the lamp says stale in orange and the drawer explains that openvpn3 says connected but nothing answers. A click on Reconnect brings it back. The Checks tab shows both probes, the resolvers and routing-only zones on tun0 and openvpn3&#x27;s counters; the History tab shows connected, stale, reconnected and an older dropped session. A click outside closes the drawer.">
</p>

<p align="center">
  <b>A VPN lamp in your bar that only says connected when the tunnel actually works.</b><br>
  openvpn3 sessions followed over D-Bus, probes that catch a tunnel gone stale,<br>
  a drawer that says why, and a way for your agents to ask for the VPN instead of guessing.
</p>

<p align="center">
  <a href="https://github.com/NathanielInman/tablet/actions/workflows/ci.yml"><img src="https://github.com/NathanielInman/tablet/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/status-early%20(0.1)-ebcb8b" alt="Early, version 0.1">
  <img src="https://img.shields.io/badge/Arch%20%2B%20Hyprland%20first-Linux%20%C2%B7%20Wayland-a3be8c" alt="Arch and Hyprland first; Linux with Wayland">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-b48ead" alt="MIT license"></a>
</p>

On a single-line railway a driver may only enter the section while carrying
the tablet, a metal token the signal box hands out one at a time, so two
trains never meet in the tunnel. [Switchyard](https://github.com/NathanielInman/switchyard)
dispatches your work, [Roundhouse](https://github.com/NathanielInman/roundhouse)
says whether the machine is fit to run it, [Signal Box](https://github.com/NathanielInman/credential-broker)
hands out secrets, and Tablet says whether you are holding the token for the
private line.

## Why

My bar had a `vpn.sh` that grepped `openvpn3 sessions-list` for `Path:` every
30 seconds. Any session object counted as connected, so a session whose
sign-in the server had refused sat there for more than a day with a frost
lamp while nothing internal resolved. When an update bumped protobuf, the
AUR build of openvpn3 could no longer load its backend and the button did
nothing at all. When the tunnel dropped mid-session, my agents spent their
time on an "expired" Grafana session before anyone looked at `tun0`.
That script is what Tablet replaces.

- **The lamp tells the truth.** The daemon follows every openvpn3 session
  on the system bus (its signals, plus a read of its status every few
  seconds while one exists, since openvpn3 does not promise to signal every
  change). Frost means connected and checked. Amber means it is connecting,
  waiting on a sign-in, or an agent is asking. Red means failed or dropped,
  and stays red until you act.
- **Stale is a state.** openvpn3 can report `CONN_CONNECTED` for a session
  the server stopped answering long ago. A profile names a few hosts that
  must answer through the tunnel (**probes**); when every probe fails twice
  in a row, or the server sends nothing for 90 seconds while traffic keeps
  going out, the tunnel is **stale**: orange lamp, the reason in the drawer,
  Reconnect one click away. Checks run a few seconds after connecting, every
  two minutes, right after a resume from suspend, and whenever you press the
  middle button.
- **It knows openvpn3 is broken before you click.** Tablet reads the
  `DT_NEEDED` libraries of every openvpn3 binary and looks for them where the
  dynamic linker would. When a package update removes one, the lamp says
  `rebuild` and the drawer names the library, the binary and the fix. It also
  spots openvpn3 services still running a binary that was replaced.
- **DNS is checked, and set when you want.** The tunnel's link in
  systemd-resolved must have the profile's resolvers and routing-only zones.
  With `apply_dns` Tablet sets them itself (through polkit) when openvpn3's
  netcfg did not, on whatever interface the session reports, so a switch to
  kernel DCO and a new interface name changes nothing.
- **Agents ask instead of guessing.** `tablet why <host>` says which profile
  a name needs and whether it is up. `tablet need work -- ./gradlew build`
  runs the command once `work` is connected, and while it is not, the lamp
  turns amber and the drawer shows the request with Connect and Not now.
  Nothing connects until you click.
- **It opens from the icon you clicked.** The drawer springs out of the bar
  on whichever monitor you clicked, with the same anchoring code as the other
  lamps, and takes its font and colors from your waybar `style.css`.

## What's in the drawer

**Now** has a card per profile: how long it has been held (or since when it
is in trouble, and why), the tunnel's interface, a graph of the last minute
of traffic, how many probes answered, and the buttons that fit the state:
Connect, Disconnect, Reconnect, Check now, Open sign-in, Dismiss. An agent's
request sits above the tabs with its command, directory and its own
(unverified) reason. **Checks** lists each probe with its time or its error,
the resolvers and domains on the tunnel's link, and openvpn3's counters
(bytes in and out, keepalive timeouts, reconnects, refused sign-ins).
**Doctor** is openvpn3's health and the setup. **History** is what happened,
newest first, kept in `~/.local/state/tablet/history.jsonl`.

<kbd>Enter</kbd> does what a click on the lamp does (or accepts a waiting
request), <kbd>r</kbd> reconnects, <kbd>k</kbd> checks now,
<kbd>1</kbd> to <kbd>4</kbd> switch tabs, and <kbd>Esc</kbd> or a click
outside closes it.

## Install

Tablet is at 0.1. It runs on Linux with Wayland and drives
[openvpn3-linux](https://github.com/OpenVPN/openvpn3-linux) (the client for
OpenVPN's Access Server and CloudConnexa; on Arch it is the `openvpn3` AUR
package). Under Hyprland the drawer opens at the icon you clicked; under sway
and niri it opens on the monitor you clicked. Packages come later; for now
grab a tarball from the [releases](https://github.com/NathanielInman/tablet/releases)
or build from source:

```sh
# Arch: the drawer's libraries
sudo pacman -S --needed gtk4 gtk4-layer-shell

git clone https://github.com/NathanielInman/tablet
cd tablet
cargo build --release
install -Dm755 target/release/tablet target/release/tablet-board -t ~/.local/bin/
tablet doctor
```

To upgrade, install the new binaries the same way. The running daemon,
drawer and bar module notice that their binary was replaced and switch to the
new one on the next event; `tablet restart` does it at once.

To let Tablet set a tunnel's DNS (`apply_dns`), install the polkit rule. It
lets members of `wheel` set DNS servers, domains and the default route on a
link through systemd-resolved, the same calls `resolvectl` makes:

```sh
sudo install -Dm644 packaging/polkit/50-tablet.rules /etc/polkit-1/rules.d/50-tablet.rules
```

The daemon starts the first time waybar runs `tablet bar`. To start it with
your session instead, copy
[`packaging/systemd/tablet.service`](packaging/systemd/tablet.service) to
`~/.config/systemd/user/`, point `ExecStart` at `%h/.local/bin/tablet`, and
`systemctl --user enable --now tablet.service`.

## Waybar

```jsonc
"custom/tablet": {
  "exec": "~/.local/bin/tablet bar",
  "return-type": "json",
  "on-click": "~/.local/bin/tablet flip",
  "on-click-right": "~/.local/bin/tablet toggle",
  "on-click-middle": "~/.local/bin/tablet check"
}
```

A click connects your default profile when it is off and disconnects it
when it is held, reconnects one that is stale, failed or dropped, opens a
waiting sign-in page, or accepts a waiting agent request. A right-click
opens the drawer and a middle-click checks now. The module's classes are the
lamp's state: `off`, `held`, `degraded` (held, but the DNS is missing
something), `connecting`, `signin`, `asking`, `stale`, `failed`, `dropped`,
`broken` (openvpn3 cannot start) and `offline` (no daemon), plus `pulse`
while it waits on something:

```css
#custom-tablet.off, #custom-tablet.offline { color: #616e88; }
#custom-tablet.held { color: #88c0d0; }
#custom-tablet.degraded, #custom-tablet.connecting,
#custom-tablet.signin, #custom-tablet.asking { color: #ebcb8b; }
#custom-tablet.stale { color: #d08770; }
#custom-tablet.failed, #custom-tablet.dropped, #custom-tablet.broken { color: #bf616a; }
#custom-tablet.pulse { animation: tablet-pulse 1.2s ease-in-out infinite alternate; }
@keyframes tablet-pulse { to { opacity: 0.45; } }
```

## Config

`~/.config/tablet/config.toml`. One `[[profile]]` per VPN; everything else
is optional ([`packaging/examples/config.toml`](packaging/examples/config.toml)
has every key with its default).

```toml
[[profile]]
name = "work"
ovpn = "~/work.ovpn"
# What the tunnel's DNS should have; checked while connected.
dns = ["10.8.0.2"]
zones = ["corp.example.com"]
# Set them when openvpn3 did not (needs the polkit rule).
apply_dns = false
# Hosts that must answer through the tunnel; port 443 when left out.
probes = ["git.corp.example.com:22", "wiki.corp.example.com"]

# Names that never go through this tunnel, and what to do instead.
[[profile.outside]]
host = "*.cloud.example.com"
note = "public endpoints; use the cloud gateway, not the VPN"

[checks]
every = 120   # seconds between checks while connected
silence = 90  # seconds of nothing from the server before it is stale
grace = 90    # seconds connecting or reconnecting before it has failed

[doctor]
# What `tablet doctor --repair` and the drawer's Repair button run.
repair = "paru -S --rebuild openvpn3"
terminal = "kitty --hold sh -c"
```

A session is matched to a profile by the file it was started from, so one
you start by hand with `openvpn3 session-start --config ~/work.ovpn` shows
up as `work` too. Sessions no profile claims are listed apart.

The config is reread within a moment of saving it. A mistake (an unknown
key, a probe that is not `host:port`, a resolver that is not an IP address)
keeps the defaults, and the drawer and `tablet doctor` say what is wrong.

## For agents

Three commands, each with an exit status an agent can act on (75 means the
VPN is the problem):

```sh
tablet why https://grafana.corp.example.com/d/abc
# grafana.corp.example.com needs work (zone corp.example.com). work is stale
# since 14:31: openvpn3 says connected, but all 2 probes failed twice in a
# row. This is the VPN, not the service.

tablet need work --reason "Nexus only answers on the VPN" -- ./mvnw verify
# Runs at once when work is held; otherwise asks on the bar and waits (5
# minutes by default, --timeout to change it).

tablet check   # every probe, the DNS and the counters, right now
tablet status --json
```

`tablet need` never connects on its own: the request waits for you to click
Connect, so an agent cannot sign you into the corporate network while you
are away. The drawer shows which program asked, the command, its directory
and the reason it gave.

## Commands

```
tablet connect [profile]          start a session (no profile: the default one)
tablet disconnect [profile|--all] end a profile's session, or every session
tablet reconnect [profile]        end the session and start a new one
tablet flip                       what a click on the lamp does
tablet ack [profile]              dismiss a dropped or failed profile
tablet signin [profile]           open the waiting sign-in page again
tablet check [profile]            check now
tablet why <host|url>             which profile a host needs and whether it is up
tablet need [profile] [--reason <text>] [--timeout <secs>] -- <command>...
tablet status [--json]            every profile and session
tablet history [--count <n>]      what happened, newest first
tablet doctor [--repair]          check openvpn3, the config and the bar
tablet toggle | show | hide       the drawer
tablet bar                        the waybar module
tablet daemon | restart
```

`man tablet` has the details; completions for bash, zsh and fish are in
`packaging/completions`.

## How it works

Tablet does not speak the VPN protocol. It starts sessions with
`openvpn3 session-start --config <file>` (which also opens the web sign-in
page), ends them with the session's `Disconnect` method, and reads the rest
from openvpn3's session manager on the system bus. It never starts openvpn3
just to ask it something: with no session manager running there is nothing to
read. Probes are plain TCP connects through the system resolver, so a probe
that answers also proves its name resolved through the tunnel. The tunnel's
DNS comes from systemd-resolved's D-Bus API. The doctor reads ELF headers
and `/proc`; it runs no programs.

`docs/design.md` is the full spec: the states and what moves a profile
between them, the protocol, every config key and why it works the way it
does.

Sign-in URLs carry one-time tokens, so the daemon never writes them to its
log, and the drawer hides the server's address unless `drawer.addresses` is
on.

## License

MIT
