//! The system bus: openvpn3's session manager and systemd-resolved. zbus is
//! async; every function here drives its call to the end on the calling
//! thread (`zbus::block_on`) with a 5 second method timeout, so a service
//! that hangs holds up only the thread that asked it. Signal streams are
//! plain iterators ([`Signals`]) for the daemon's watcher threads.
//!
//! The bus is the system bus, or whatever `DBUS_SYSTEM_BUS_ADDRESS` names
//! (the end-to-end test runs a fake openvpn3 on a private bus).

use anyhow::{Result, anyhow};
use futures_core::Stream;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::time::Duration;
use tablet_core::checks::Dns;
use tablet_core::openvpn::{BUS, IFACE, MANAGER_PATH, Props, Session, Value};
use zbus::{Connection, DynamicType, MatchRule, Message, MessageStream, OwnedVariant, Variant};

const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
pub const RESOLVED: &str = "org.freedesktop.resolve1";
const RESOLVED_PATH: &str = "/org/freedesktop/resolve1";
const RESOLVED_MANAGER: &str = "org.freedesktop.resolve1.Manager";
const RESOLVED_LINK: &str = "org.freedesktop.resolve1.Link";

/// A system-bus connection whose method calls give up after 5 seconds.
pub fn connect() -> Result<Connection> {
    Ok(zbus::block_on(
        zbus::connection::Builder::system()
            .method_timeout(Duration::from_secs(5))
            .build(),
    )?)
}

fn call<B>(
    conn: &Connection,
    bus: &str,
    path: &str,
    iface: &str,
    method: &str,
    body: &B,
) -> zbus::Result<Message>
where
    B: serde::ser::Serialize + DynamicType,
{
    zbus::block_on(conn.call_method(Some(bus), path, Some(iface), method, body))
}

/// The signals a match rule selects, as a blocking iterator. Ends when the
/// connection does.
pub struct Signals(MessageStream);

impl Iterator for Signals {
    type Item = zbus::Result<Message>;

    fn next(&mut self) -> Option<Self::Item> {
        let stream = &mut self.0;
        zbus::block_on(std::future::poll_fn(|cx| {
            Pin::new(&mut *stream).poll_next(cx)
        }))
    }
}

pub fn signals(conn: &Connection, rule: MatchRule<'static>, queued: usize) -> Result<Signals> {
    Ok(Signals(zbus::block_on(MessageStream::for_match_rule(
        rule,
        conn,
        NonZeroUsize::new(queued),
    ))?))
}

/// A D-Bus value as the parsers' [`Value`].
pub fn value(v: &Variant) -> Value {
    match v {
        Variant::Bool(b) => Value::Bool(*b),
        Variant::U8(x) => Value::UInt(*x as u64),
        Variant::U16(x) => Value::UInt(*x as u64),
        Variant::U32(x) => Value::UInt(*x as u64),
        Variant::U64(x) => Value::UInt(*x),
        Variant::I16(x) => Value::Int(*x as i64),
        Variant::I32(x) => Value::Int(*x as i64),
        Variant::I64(x) => Value::Int(*x),
        Variant::Str(s) => Value::Str(s.to_string()),
        Variant::ObjectPath(p) => Value::Str(p.to_string()),
        Variant::Variant(inner) => value(inner),
        Variant::Array(a) => {
            let items = a.inner();
            if !items.is_empty() && items.iter().all(|x| matches!(x, Variant::U8(_))) {
                Value::Bytes(
                    items
                        .iter()
                        .filter_map(|x| match x {
                            Variant::U8(b) => Some(*b),
                            _ => None,
                        })
                        .collect(),
                )
            } else {
                Value::List(items.iter().map(value).collect())
            }
        }
        Variant::Structure(s) => Value::List(s.fields().iter().map(value).collect()),
        Variant::Dict(d) => Value::Dict(
            d.iter()
                .filter_map(|(k, v)| match k {
                    Variant::Str(k) => Some((k.to_string(), value(v))),
                    _ => None,
                })
                .collect(),
        ),
        _ => Value::Other,
    }
}

pub fn props(map: &HashMap<String, OwnedVariant>) -> Props {
    map.iter().map(|(k, v)| (k.clone(), value(v))).collect()
}

fn dbus_call<B>(conn: &Connection, method: &str, body: &B) -> Result<Message>
where
    B: serde::ser::Serialize + DynamicType,
{
    Ok(call(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        method,
        body,
    )?)
}

/// The bus still answers.
pub fn bus_alive(conn: &Connection) -> bool {
    dbus_call(conn, "GetId", &()).is_ok()
}

/// Someone owns the name. Asked before calling openvpn3, because a call to
/// a name nobody owns makes the bus start the service, and Tablet only
/// starts openvpn3 when you connect.
pub fn has_owner(conn: &Connection, name: &str) -> bool {
    dbus_call(conn, "NameHasOwner", &(name,))
        .ok()
        .and_then(|m| m.body().deserialize::<bool>().ok())
        .unwrap_or(false)
}

/// Every session path the session manager lists for this user.
pub fn session_paths(conn: &Connection) -> Result<Vec<String>> {
    let reply = call(
        conn,
        BUS,
        MANAGER_PATH,
        IFACE,
        "FetchAvailableSessions",
        &(),
    )?;
    let paths: Vec<zbus::OwnedObjectPath> = reply.body().deserialize()?;
    Ok(paths.into_iter().map(|p| p.to_string()).collect())
}

/// One session, read whole.
pub fn session(conn: &Connection, path: &str) -> Result<Session> {
    let reply = call(conn, BUS, path, PROPERTIES, "GetAll", &(IFACE,))?;
    let map: HashMap<String, OwnedVariant> = reply.body().deserialize()?;
    Ok(Session::from_props(path, &props(&map)))
}

/// Every session, read whole. A session that vanishes between the list and
/// the read is skipped.
pub fn sessions(conn: &Connection) -> Result<Vec<Session>> {
    Ok(session_paths(conn)?
        .into_iter()
        .filter_map(|p| session(conn, &p).ok())
        .collect())
}

/// Calls a session method with no arguments: `Disconnect`, `Restart`.
pub fn session_call(conn: &Connection, path: &str, method: &str) -> Result<()> {
    call(conn, BUS, path, IFACE, method, &())
        .map_err(|e| anyhow!("openvpn3 refused {method}: {}", short(&e)))?;
    Ok(())
}

fn short(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, Some(msg), _) => format!("{msg} ({name})"),
        zbus::Error::MethodError(name, None, _) => name.to_string(),
        other => other.to_string(),
    }
}

/// The resolved link object for an interface index.
fn link_path(conn: &Connection, ifindex: i32) -> Result<String> {
    let reply = call(
        conn,
        RESOLVED,
        RESOLVED_PATH,
        RESOLVED_MANAGER,
        "GetLink",
        &(ifindex,),
    )?;
    let p: zbus::OwnedObjectPath = reply.body().deserialize()?;
    Ok(p.to_string())
}

/// The interface's index, from sysfs.
pub fn ifindex(device: &str) -> Option<i32> {
    if device.is_empty() || device.contains('/') {
        return None;
    }
    std::fs::read_to_string(format!("/sys/class/net/{device}/ifindex"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// What systemd-resolved has on a device's link. `None` when resolved is
/// not running or does not know the device.
pub fn link_dns(conn: &Connection, device: &str) -> Option<Dns> {
    let idx = ifindex(device)?;
    if !has_owner(conn, RESOLVED) {
        return None;
    }
    let path = link_path(conn, idx).ok()?;
    let reply = call(
        conn,
        RESOLVED,
        &path,
        PROPERTIES,
        "GetAll",
        &(RESOLVED_LINK,),
    )
    .ok()?;
    let map: HashMap<String, OwnedVariant> = reply.body().deserialize().ok()?;
    Some(Dns::from_props(&props(&map)))
}

/// Sets resolvers and routing-only domains on a device's link, and keeps it
/// out of the default route, as `resolvectl dns/domain/default-route` do.
/// systemd-resolved asks polkit; the rule in `packaging/polkit` allows it.
pub fn set_link_dns(
    conn: &Connection,
    device: &str,
    servers: &[String],
    zones: &[String],
) -> Result<()> {
    let idx = ifindex(device).ok_or_else(|| anyhow!("no interface {device}"))?;
    let addrs: Vec<(i32, Vec<u8>)> = servers
        .iter()
        .filter_map(|s| s.trim().parse::<std::net::IpAddr>().ok())
        .map(|ip| match ip {
            std::net::IpAddr::V4(v4) => (libc::AF_INET, v4.octets().to_vec()),
            std::net::IpAddr::V6(v6) => (libc::AF_INET6, v6.octets().to_vec()),
        })
        .collect();
    let domains: Vec<(String, bool)> = zones
        .iter()
        .map(|z| {
            (
                z.trim()
                    .trim_start_matches('~')
                    .trim_matches('.')
                    .to_string(),
                true,
            )
        })
        .filter(|(z, _)| !z.is_empty())
        .collect();
    let denied = |e: zbus::Error| {
        let s = short(&e);
        if s.contains("AccessDenied")
            || s.contains("InteractiveAuthorizationRequired")
            || s.contains("NotAuthorized")
        {
            anyhow!(
                "systemd-resolved did not allow it; install packaging/polkit/50-tablet.rules ({s})"
            )
        } else {
            anyhow!("systemd-resolved refused: {s}")
        }
    };
    if !addrs.is_empty() {
        call(
            conn,
            RESOLVED,
            RESOLVED_PATH,
            RESOLVED_MANAGER,
            "SetLinkDNS",
            &(idx, addrs),
        )
        .map_err(denied)?;
    }
    if !domains.is_empty() {
        call(
            conn,
            RESOLVED,
            RESOLVED_PATH,
            RESOLVED_MANAGER,
            "SetLinkDomains",
            &(idx, domains),
        )
        .map_err(denied)?;
        call(
            conn,
            RESOLVED,
            RESOLVED_PATH,
            RESOLVED_MANAGER,
            "SetLinkDefaultRoute",
            &(idx, false),
        )
        .map_err(denied)?;
    }
    Ok(())
}
