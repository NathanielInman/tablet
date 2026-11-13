# Fixtures

Recorded from openvpn3-linux v27.1 and systemd-resolved, with every address,
host name, path and id replaced by a placeholder (`vpn.example.com`,
`198.51.100.7` from the documentation range, `/home/user/work.ovpn`).

- `session-connected.json`: `busctl --system --json=short call
  net.openvpn.v3.sessions <path> org.freedesktop.DBus.Properties GetAll s
  net.openvpn.v3.sessions` on a connected session.
- `session-auth-failed.json`: the same shape for a session whose sign-in the
  server refused: it stayed at `CONN_CONNECTING` with `AUTH_FAILED` = 1 in
  its counters until it was disconnected by hand, a day later. Built from the
  counters openvpn3 printed when that session ended.
- `resolved-link.json`: `GetAll s org.freedesktop.resolve1.Link` on a tunnel
  link with two resolvers and two routing-only domains.
- `waybar-platform.css`: the style sheet of a floating, transparent waybar.
