#!/usr/bin/env python3
"""A stand-in for openvpn3, for tools/e2e.sh. Needs python-gobject.

    fake-openvpn3.py serve
        Owns net.openvpn.v3.sessions on the bus in DBUS_SYSTEM_BUS_ADDRESS
        (the session bus when that is unset) and behaves like openvpn3's
        session manager: FetchAvailableSessions, per-session properties,
        Disconnect and Restart, SessionManagerEvent and StatusChange
        signals. A test interface on /net/openvpn/v3/test makes sessions
        and moves them along.

    fake-openvpn3.py session-start --config <file>
        What Tablet runs to connect (openvpn3.command). Makes a session for
        <file>; FAKE_OPENVPN3_MODE, or the first word of the file named by
        FAKE_OPENVPN3_MODE_FILE, says what happens next:
          connect      connecting, then connected (the default)
          quiet        connected, without a StatusChange signal
          auth-url     waits on a web sign-in
          auth-failed  stays connecting with AUTH_FAILED = 1
          broken       makes no session and fails, as an openvpn3 whose
                       backend cannot load its libraries does

    fake-openvpn3.py ctl <status|stats|remove> <path> [args]
        Moves a session along by hand (status <major> <minor> [message],
        stats <bytes_in> <bytes_out>, remove).
"""

import os
import sys
import time

from gi.repository import Gio, GLib

BUS_NAME = "net.openvpn.v3.sessions"
IFACE = "net.openvpn.v3.sessions"
MANAGER = "/net/openvpn/v3/sessions"
TEST_PATH = "/net/openvpn/v3/test"
TEST_IFACE = "net.openvpn.v3.test"

XML = f"""
<node>
  <interface name="{IFACE}">
    <method name="FetchAvailableSessions"><arg type="ao" direction="out"/></method>
    <method name="Connect"/>
    <method name="Disconnect"/>
    <method name="Restart"/>
    <signal name="SessionManagerEvent"><arg type="o"/><arg type="q"/><arg type="u"/></signal>
    <signal name="StatusChange"><arg type="u"/><arg type="u"/><arg type="s"/></signal>
    <property name="config_name" type="s" access="read"/>
    <property name="session_name" type="s" access="read"/>
    <property name="device_name" type="s" access="read"/>
    <property name="session_created" type="t" access="read"/>
    <property name="status" type="(uus)" access="read"/>
    <property name="connected_to" type="(ssu)" access="read"/>
    <property name="statistics" type="a{{sx}}" access="read"/>
    <property name="backend_pid" type="u" access="read"/>
    <property name="owner" type="u" access="read"/>
  </interface>
  <interface name="{TEST_IFACE}">
    <method name="New"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
    <method name="Status"><arg type="o" direction="in"/><arg type="u" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/></method>
    <method name="Stats"><arg type="o" direction="in"/><arg type="x" direction="in"/><arg type="x" direction="in"/></method>
    <method name="Remove"><arg type="o" direction="in"/></method>
  </interface>
</node>
"""

CONNECTING, CONNECTED, DISCONNECTED, RECONNECTING, AUTH_URL = 6, 7, 9, 12, 22


def bus():
    addr = os.environ.get("DBUS_SYSTEM_BUS_ADDRESS")
    if addr:
        return Gio.DBusConnection.new_for_address_sync(
            addr,
            Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
            None,
            None,
        )
    return Gio.bus_get_sync(Gio.BusType.SESSION, None)


class Manager:
    def __init__(self, conn):
        self.conn = conn
        self.node = Gio.DBusNodeInfo.new_for_xml(XML)
        self.sessions = {}
        self.regs = {}
        self.count = 0
        conn.register_object(MANAGER, self.node.interfaces[0], self.call, self.get, None)
        conn.register_object(TEST_PATH, self.node.interfaces[1], self.call, None, None)

    def emit_status(self, path, major, minor, msg=""):
        s = self.sessions[path]
        s["status"] = (major, minor, msg)
        self.conn.emit_signal(None, path, IFACE, "StatusChange", GLib.Variant("(uus)", (major, minor, msg)))

    def new(self, config, mode):
        self.count += 1
        path = f"{MANAGER}/fake{self.count}"
        self.sessions[path] = {
            "config_name": config,
            "session_name": "vpn.example.com",
            "device_name": "",
            "session_created": int(time.time()),
            "status": (2, CONNECTING, ""),
            "connected_to": ("udp", "198.51.100.7", 1194),
            "statistics": {"BYTES_IN": 100, "BYTES_OUT": 100},
            "backend_pid": 4242,
            "owner": os.getuid(),
        }
        self.regs[path] = self.conn.register_object(path, self.node.interfaces[0], self.call, self.get, None)
        self.conn.emit_signal(None, MANAGER, IFACE, "SessionManagerEvent", GLib.Variant("(oqu)", (path, 1, os.getuid())))

        def later():
            if path not in self.sessions:
                return False
            s = self.sessions[path]
            if mode == "connect":
                s["device_name"] = "tun9"
                self.emit_status(path, 2, CONNECTED)
            elif mode == "quiet":
                s["device_name"] = "tun9"
                s["status"] = (2, CONNECTED, "")
            elif mode == "auth-url":
                self.emit_status(path, 3, AUTH_URL, "https://login.example.com/auth?state=x")
            elif mode == "auth-failed":
                s["statistics"]["AUTH_FAILED"] = 1
            return False

        GLib.timeout_add(400, later)
        return path

    def remove(self, path, signal=True):
        if path not in self.sessions:
            return
        del self.sessions[path]
        self.conn.unregister_object(self.regs.pop(path))
        if signal:
            self.conn.emit_signal(None, MANAGER, IFACE, "SessionManagerEvent", GLib.Variant("(oqu)", (path, 2, os.getuid())))

    def call(self, conn, sender, path, iface, method, params, inv):
        args = params.unpack()
        if iface == TEST_IFACE:
            if method == "New":
                inv.return_value(GLib.Variant("(o)", (self.new(args[0], args[1]),)))
                return
            target = args[0]
            if target not in self.sessions:
                inv.return_dbus_error("net.openvpn.v3.error", "no such session")
                return
            if method == "Status":
                self.emit_status(target, args[1], args[2], args[3])
            elif method == "Stats":
                self.sessions[target]["statistics"].update({"BYTES_IN": args[1], "BYTES_OUT": args[2]})
            elif method == "Remove":
                self.remove(target)
            inv.return_value(None)
            return
        if path == MANAGER and method == "FetchAvailableSessions":
            inv.return_value(GLib.Variant("(ao)", (sorted(self.sessions),)))
            return
        if path not in self.sessions:
            inv.return_dbus_error("net.openvpn.v3.error", "no such session")
            return
        if method == "Disconnect":
            self.emit_status(path, 2, DISCONNECTED)
            inv.return_value(None)
            GLib.timeout_add(150, lambda: self.remove(path) or False)
        elif method == "Restart":
            self.emit_status(path, 2, RECONNECTING)
            GLib.timeout_add(300, lambda: path in self.sessions and self.emit_status(path, 2, CONNECTED) and False)
            inv.return_value(None)
        else:
            inv.return_value(None)

    def get(self, conn, sender, path, iface, name):
        s = self.sessions.get(path)
        if s is None:
            return None
        types = {
            "config_name": "s",
            "session_name": "s",
            "device_name": "s",
            "session_created": "t",
            "status": "(uus)",
            "connected_to": "(ssu)",
            "statistics": "a{sx}",
            "backend_pid": "u",
            "owner": "u",
        }
        return GLib.Variant(types[name], s[name])


def serve():
    conn = bus()
    m = Manager(conn)
    loop = GLib.MainLoop()

    def lost(*_):
        loop.quit()

    Gio.bus_own_name_on_connection(conn, BUS_NAME, Gio.BusNameOwnerFlags.NONE, None, lost)
    m.conn = conn
    loop.run()


def test_call(method, sig, args, out=None):
    conn = bus()
    reply = conn.call_sync(BUS_NAME, TEST_PATH, TEST_IFACE, method, GLib.Variant(sig, args),
                           GLib.VariantType(out) if out else None, Gio.DBusCallFlags.NONE, 5000, None)
    return reply.unpack() if reply is not None else None


def session_start(argv):
    config = argv[argv.index("--config") + 1]
    mode = os.environ.get("FAKE_OPENVPN3_MODE", "connect")
    mode_file = os.environ.get("FAKE_OPENVPN3_MODE_FILE")
    if mode_file and os.path.exists(mode_file):
        mode = (open(mode_file).read().split() or ["connect"])[0]
    if mode == "broken":
        print("** ERROR ** New tunnel did not respond", file=sys.stderr)
        sys.exit(1)
    print(f"Using configuration profile from file: {config}")
    (path,) = test_call("New", "(ss)", (config, mode), "(o)")
    print(f"Session path: {path}")
    if mode == "auth-url":
        print("Web based authentication required.")
        print("Open this URL: https://login.example.com/auth?state=x")


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "serve":
        serve()
    elif cmd == "session-start":
        session_start(sys.argv[2:])
    elif cmd == "ctl":
        what, path, rest = sys.argv[2], sys.argv[3], sys.argv[4:]
        if what == "status":
            test_call("Status", "(ouus)", (path, int(rest[0]), int(rest[1]), rest[2] if len(rest) > 2 else ""))
        elif what == "stats":
            test_call("Stats", "(oxx)", (path, int(rest[0]), int(rest[1])))
        elif what == "remove":
            test_call("Remove", "(o)", (path,))
    else:
        print(__doc__, file=sys.stderr)
        sys.exit(2)


if __name__ == "__main__":
    main()
