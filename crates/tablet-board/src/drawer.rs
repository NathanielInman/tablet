//! The drawer: an agent's request when one waits, then four tabs. Now has
//! each profile's state, its traffic and the buttons that fit it; Checks
//! has the probes, the tunnel's DNS and openvpn3's counters; Doctor has
//! openvpn3's health; History has what happened.

use crate::anim::{Motion, Spring};
use crate::stage::Stage;
use bar_anchor::Anchor;
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge as LsEdge, KeyboardMode, Layer, LayerShell};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Duration;
use tablet_core::config::Config;
use tablet_core::model::{Phase, ProfileView, Snapshot};
use tablet_core::protocol::{Event, Request};
use tablet_core::text::{ago, bytes, rate, short_when, span, when};

pub enum Msg {
    Event(Event),
    /// A watched stylesheet or the config changed.
    Restyle,
    /// A command the user gave failed.
    Error(String),
    /// A command said what it did.
    Said(String),
}

const DOT: &str = "\u{25cf}";
const OK: &str = "\u{2713}";
const BAD: &str = "\u{2717}";
/// Seconds of traffic the graph keeps.
const GRAPH_SECS: usize = 60;
/// How long a command's message stays in the notice line.
const NOTICE_FOR: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Now,
    Checks,
    Doctor,
    History,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Now, Tab::Checks, Tab::Doctor, Tab::History];

    fn label(self) -> &'static str {
        match self {
            Tab::Now => "Now",
            Tab::Checks => "Checks",
            Tab::Doctor => "Doctor",
            Tab::History => "History",
        }
    }
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn wrapped(text: &str, classes: &[&str]) -> gtk::Label {
    let l = label(text, classes);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

fn card(classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.add_css_class("card");
    for c in classes {
        b.add_css_class(c);
    }
    b
}

/// A row of a small uppercase label and something on the right.
fn heading(left: &str, right: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let l = label(left, &["t-label"]);
    l.set_hexpand(true);
    row.append(&l);
    if !right.is_empty() {
        row.append(&label(right, &["t-label"]));
    }
    row
}

/// A check line: a mark, the text, and a value on the right.
fn mark_row(ok: Option<bool>, text: &str, right: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let (glyph, class) = match ok {
        Some(true) => (OK, "ok"),
        Some(false) => (BAD, "bad"),
        None => (DOT, "dim"),
    };
    let m = label(glyph, &["mark", class]);
    m.set_valign(gtk::Align::Start);
    row.append(&m);
    let t = label(text, &[]);
    t.set_hexpand(true);
    t.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    t.set_tooltip_text(Some(text));
    row.append(&t);
    if !right.is_empty() {
        let r = label(right, &["dim", "num"]);
        r.set_xalign(1.0);
        row.append(&r);
    }
    row
}

fn phase_class(p: Phase) -> &'static str {
    match p {
        Phase::Held => "held",
        Phase::Off => "off",
        Phase::Starting | Phase::Reconnecting | Phase::SignIn => "wait",
        Phase::Stale => "stale",
        Phase::Failed | Phase::Dropped => "bad",
    }
}

fn lamp_class(lamp: &str) -> &'static str {
    match lamp {
        "held" => "held",
        "degraded" | "connecting" | "signin" | "asking" => "wait",
        "stale" => "stale",
        "failed" | "dropped" | "broken" => "bad",
        _ => "off",
    }
}

fn set_state(w: &impl IsA<gtk::Widget>, class: &str) {
    for c in ["held", "off", "wait", "stale", "bad"] {
        w.remove_css_class(c);
    }
    w.add_css_class(class);
}

fn find_monitor(name: &str) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let list = display.monitors();
    (0..list.n_items())
        .filter_map(|i| list.item(i).and_downcast::<gdk::Monitor>())
        .find(|m| m.connector().is_some_and(|c| c == name))
}

/// Sends a request the drawer does not need an answer to.
fn send_quiet(req: Request) {
    std::thread::spawn(move || {
        let _ = tablet_core::protocol::request(&req);
    });
}

/// Sends a command the user gave and reports what it did, or why it could
/// not, in the notice line.
fn send_with(tx: &async_channel::Sender<Msg>, req: Request) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let msg = match tablet_core::protocol::request(&req) {
            Ok(r) => match r.error {
                Some(e) => Msg::Error(e),
                None => match r.message {
                    Some(m) => Msg::Said(m),
                    None => return,
                },
            },
            Err(e) => Msg::Error(format!("{e:#}")),
        };
        let _ = tx.send_blocking(msg);
    });
}

/// Bytes in and out of a network device, from sysfs.
fn device_bytes(dev: &str) -> Option<(u64, u64)> {
    if dev.is_empty() || dev.contains('/') {
        return None;
    }
    let read = |what: &str| {
        std::fs::read_to_string(format!("/sys/class/net/{dev}/statistics/{what}"))
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
    };
    Some((read("rx_bytes")?, read("tx_bytes")?))
}

/// A device's traffic, a second at a time, for the graph.
#[derive(Default)]
struct Traffic {
    last: Option<(u64, u64)>,
    /// (in, out) bytes a second, oldest first.
    rates: VecDeque<(f64, f64)>,
}

impl Traffic {
    fn sample(&mut self, now: (u64, u64)) {
        if let Some((rx, tx)) = self.last {
            let r = (
                now.0.saturating_sub(rx) as f64,
                now.1.saturating_sub(tx) as f64,
            );
            self.rates.push_back(r);
            while self.rates.len() > GRAPH_SECS {
                self.rates.pop_front();
            }
        }
        self.last = Some(now);
    }

    fn latest(&self) -> (f64, f64) {
        self.rates.back().copied().unwrap_or((0.0, 0.0))
    }
}

type Graphs = Rc<RefCell<HashMap<String, Traffic>>>;

/// A label whose text follows the clock (durations, "checked 40s ago").
struct Ticker {
    label: gtk::Label,
    text: Box<dyn Fn(f64) -> String>,
}

pub struct Drawer {
    window: gtk::ApplicationWindow,
    tx: async_channel::Sender<Msg>,
    stage: Stage,
    spring: Spring,
    visible: Cell<bool>,
    snap: RefCell<Option<Snapshot>>,
    cfg: RefCell<Config>,
    tab: Cell<Tab>,
    /// What the content was last built from, so it is rebuilt only when
    /// something shown changed.
    built: RefCell<String>,

    lamp: gtk::Label,
    sub: gtk::Label,
    chip: gtk::Label,
    notice: gtk::Label,
    message: RefCell<Option<(String, bool)>>,
    message_timer: RefCell<Option<glib::SourceId>>,
    ask: gtk::Box,
    tabs: Vec<(Tab, gtk::Button)>,
    content: gtk::Box,
    tickers: RefCell<Vec<Ticker>>,
    graphs: Graphs,
    areas: RefCell<Vec<gtk::DrawingArea>>,
}

impl Drawer {
    pub fn new(app: &gtk::Application, tx: async_channel::Sender<Msg>, layer: bool) -> Rc<Drawer> {
        let (cfg, _) = Config::load();
        let window = gtk::ApplicationWindow::new(app);
        window.add_css_class("tablet");
        window.set_decorated(false);
        window.set_title(Some("Tablet"));
        if layer {
            window.init_layer_shell();
            window.set_layer(Layer::Overlay);
            window.set_namespace(Some("tablet"));
            for e in [LsEdge::Top, LsEdge::Bottom, LsEdge::Left, LsEdge::Right] {
                window.set_anchor(e, true);
            }
            window.set_exclusive_zone(0);
            window.set_keyboard_mode(KeyboardMode::Exclusive);
        }

        let lamp = label(DOT, &["lamp", "off"]);
        lamp.set_valign(gtk::Align::Center);
        let brand = label("TABLET", &["brand"]);
        let sub = label("", &["sub"]);
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let who = gtk::Box::new(gtk::Orientation::Vertical, 0);
        who.append(&brand);
        who.append(&sub);
        who.set_hexpand(true);
        let chip = label("off", &["chip", "off"]);
        chip.set_valign(gtk::Align::Center);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        header.append(&lamp);
        header.append(&who);
        header.append(&chip);

        let notice = wrapped("", &["notice"]);
        notice.set_visible(false);

        let ask = gtk::Box::new(gtk::Orientation::Vertical, 8);
        ask.set_visible(false);

        let tabrow = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        tabrow.add_css_class("tabs");
        tabrow.set_homogeneous(true);
        let tabs: Vec<(Tab, gtk::Button)> = Tab::ALL
            .iter()
            .map(|t| {
                let b = gtk::Button::with_label(t.label());
                b.set_focus_on_click(false);
                tabrow.append(&b);
                (*t, b)
            })
            .collect();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_propagate_natural_height(true);
        scroll.set_child(Some(&content));

        let foot = label(
            "Enter connect \u{b7} r reconnect \u{b7} k check \u{b7} 1-4 tabs \u{b7} Esc close",
            &["foot"],
        );
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 12);
        panel.add_css_class("panel");
        panel.append(&header);
        panel.append(&notice);
        panel.append(&ask);
        panel.append(&tabrow);
        panel.append(&scroll);
        panel.append(&foot);
        let spring = Spring::new(&panel, Motion::PANEL);
        let stage = Stage::new(
            &spring,
            cfg.drawer.width,
            cfg.drawer.max_height,
            cfg.drawer.gap,
        );
        if !layer {
            // A plain window the panel is centered in.
            window.set_default_size(cfg.drawer.width as i32 + 120, 1000);
        }
        window.set_child(Some(&stage));
        let d = Rc::new(Drawer {
            window,
            tx,
            stage,
            spring,
            visible: Cell::new(false),
            snap: RefCell::new(None),
            cfg: RefCell::new(cfg),
            tab: Cell::new(Tab::Now),
            built: RefCell::new(String::new()),
            lamp,
            sub,
            chip,
            notice,
            message: RefCell::new(None),
            message_timer: RefCell::new(None),
            ask,
            tabs,
            content,
            tickers: RefCell::new(vec![]),
            graphs: Rc::new(RefCell::new(HashMap::new())),
            areas: RefCell::new(vec![]),
        });
        d.connect();
        d
    }

    fn connect(self: &Rc<Self>) {
        {
            let w = Rc::downgrade(self);
            self.stage.connect_outside(move || {
                if let Some(d) = w.upgrade() {
                    d.hide();
                }
            });
            let s = self.spring.clone();
            self.stage.connect_placed(move |(x, y)| s.set_origin(x, y));
        }
        for (tab, b) in &self.tabs {
            let w = Rc::downgrade(self);
            let tab = *tab;
            b.connect_clicked(move |_| {
                if let Some(d) = w.upgrade() {
                    d.set_tab(tab);
                }
            });
        }
        {
            let keys = gtk::EventControllerKey::new();
            let w = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, _| {
                let Some(d) = w.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                d.key(key)
            });
            self.window.add_controller(keys);
        }
        let w = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_secs(1), move || {
            let Some(d) = w.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if d.visible.get() {
                d.tick();
            }
            glib::ControlFlow::Continue
        });
    }

    pub fn handle(self: &Rc<Self>, msg: Msg) {
        match msg {
            Msg::Event(Event::Snapshot { snapshot }) => {
                *self.snap.borrow_mut() = Some(*snapshot);
                if self.visible.get() {
                    self.render();
                }
            }
            Msg::Event(Event::Show { anchor }) => self.show(anchor),
            Msg::Event(Event::Toggle { anchor }) => {
                if self.visible.get() {
                    self.hide();
                } else {
                    self.show(anchor);
                }
            }
            Msg::Event(Event::Hide) => self.hide(),
            Msg::Event(Event::Quit) => std::process::exit(0),
            Msg::Restyle => {
                crate::linux::apply_styles();
                let (cfg, _) = Config::load();
                self.stage
                    .set_geometry(cfg.drawer.width, cfg.drawer.max_height, cfg.drawer.gap);
                *self.cfg.borrow_mut() = cfg;
                self.built.borrow_mut().clear();
                if self.visible.get() {
                    self.render();
                }
            }
            Msg::Error(e) => self.flash(e, true),
            Msg::Said(m) => self.flash(m, false),
        }
    }

    fn send(&self, req: Request) {
        send_with(&self.tx, req);
    }

    /// Draws the panel for `snap` on `tab` into a PNG, then quits.
    pub fn shoot(
        self: &Rc<Self>,
        out: String,
        snap: Option<Snapshot>,
        tab: Option<String>,
        app: gtk::Application,
    ) {
        *self.snap.borrow_mut() = snap;
        self.show(None);
        let tab = match tab.as_deref() {
            Some("checks") => Tab::Checks,
            Some("doctor") => Tab::Doctor,
            Some("history") => Tab::History,
            _ => Tab::Now,
        };
        self.set_tab(tab);
        // Holds the drawer: nothing else does in this mode.
        let d = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1200), move || {
            d.spring.settle();
            d.tick();
            // Take the first frame GTK has a picture of (the clocks keep
            // invalidating it), trying for a few seconds.
            let tries = Rc::new(Cell::new(0));
            glib::timeout_add_local(Duration::from_millis(100), move || {
                let widget: gtk::Widget = d.spring.clone().upcast();
                let (wd, ht) = (widget.width(), widget.height());
                let paintable = gtk::WidgetPaintable::new(Some(&widget));
                let snapshot = gtk::Snapshot::new();
                paintable.snapshot(&snapshot, wd as f64, ht as f64);
                tries.set(tries.get() + 1);
                match (
                    snapshot.to_node(),
                    widget.native().and_then(|n| n.renderer()),
                ) {
                    (Some(node), Some(renderer)) => {
                        let tex = renderer.render_texture(&node, None);
                        match tex.save_to_png(&out) {
                            Ok(()) => println!("{out} {wd}x{ht}"),
                            Err(e) => eprintln!("tablet-board: {e}"),
                        }
                    }
                    _ if tries.get() < 50 => return glib::ControlFlow::Continue,
                    _ => eprintln!("tablet-board: nothing to draw ({wd}x{ht})"),
                }
                app.quit();
                glib::ControlFlow::Break
            });
        });
    }

    fn flash(self: &Rc<Self>, text: String, error: bool) {
        *self.message.borrow_mut() = Some((text, error));
        if let Some(id) = self.message_timer.borrow_mut().take() {
            id.remove();
        }
        let w = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(NOTICE_FOR, move || {
            let Some(d) = w.upgrade() else { return };
            d.message_timer.borrow_mut().take();
            d.message.borrow_mut().take();
            d.render_notice();
        });
        *self.message_timer.borrow_mut() = Some(id);
        self.render_notice();
    }

    /// The notice line: a command's message while it lasts, else a broken
    /// config.
    fn render_notice(&self) {
        let config = self
            .snap
            .borrow()
            .as_ref()
            .and_then(|s| s.config_error.clone());
        let message = self.message.borrow().clone();
        self.notice
            .set_visible(message.is_some() || config.is_some());
        self.notice.remove_css_class("error");
        match (message, config) {
            (Some((m, err)), _) => {
                if err {
                    self.notice.add_css_class("error");
                }
                self.notice.set_label(&m);
            }
            (None, Some(e)) => {
                self.notice.add_css_class("error");
                self.notice
                    .set_label(&format!("Config problem, using defaults: {e}"));
            }
            (None, None) => {}
        }
    }

    fn key(self: &Rc<Self>, key: gdk::Key) -> glib::Propagation {
        let ask = self
            .snap
            .borrow()
            .as_ref()
            .and_then(|s| s.asks.first().map(|a| a.id));
        match key {
            gdk::Key::Escape => self.hide(),
            gdk::Key::Return | gdk::Key::KP_Enter => match ask {
                Some(id) => self.send(Request::Answer { id, connect: true }),
                None => self.send(Request::Flip),
            },
            gdk::Key::n if ask.is_some() => {
                if let Some(id) = ask {
                    self.send(Request::Answer { id, connect: false });
                }
            }
            gdk::Key::r => self.send(Request::Reconnect { profile: None }),
            gdk::Key::k => self.check(None),
            gdk::Key::_1 => self.set_tab(Tab::Now),
            gdk::Key::_2 => self.set_tab(Tab::Checks),
            gdk::Key::_3 => self.set_tab(Tab::Doctor),
            gdk::Key::_4 => self.set_tab(Tab::History),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    fn check(self: &Rc<Self>, profile: Option<String>) {
        self.flash("checking\u{2026}".into(), false);
        // The reply is the snapshot, which arrives on its own; only an
        // error needs saying.
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let msg = match tablet_core::protocol::request(&Request::Check { profile }) {
                Ok(r) => match r.error {
                    Some(e) => Msg::Error(e),
                    None => Msg::Said("checked".into()),
                },
                Err(e) => Msg::Error(format!("{e:#}")),
            };
            let _ = tx.send_blocking(msg);
        });
    }

    fn set_tab(self: &Rc<Self>, tab: Tab) {
        if self.tab.replace(tab) != tab {
            self.built.borrow_mut().clear();
            self.render();
        }
    }

    fn show(self: &Rc<Self>, anchor: Option<Anchor>) {
        if let Some(name) = anchor.as_ref().and_then(|a| a.monitor.clone())
            && let Some(m) = find_monitor(&name)
        {
            self.window.set_monitor(Some(&m));
        }
        self.stage.set_anchor(anchor);
        let was_visible = self.visible.replace(true);
        // Open on what needs you: openvpn3's health when it is broken,
        // else Now.
        if !was_visible {
            let broken = self
                .snap
                .borrow()
                .as_ref()
                .is_some_and(|s| s.lamp == "broken");
            self.tab.set(if broken { Tab::Doctor } else { Tab::Now });
            self.built.borrow_mut().clear();
            self.graphs.borrow_mut().clear();
        }
        self.render();
        self.window.set_visible(true);
        self.window.present();
        if !was_visible {
            self.spring.play_in(0);
            send_quiet(Request::BoardVisible { visible: true });
        }
    }

    fn hide(self: &Rc<Self>) {
        if !self.visible.replace(false) {
            return;
        }
        let w = Rc::downgrade(self);
        self.spring.play_out(move || {
            if let Some(d) = w.upgrade()
                && !d.visible.get()
            {
                d.window.set_visible(false);
            }
        });
        send_quiet(Request::BoardVisible { visible: false });
    }

    /// Every second while open: samples the tunnels' traffic, redraws the
    /// graphs and moves the clocks.
    fn tick(&self) {
        let devices: Vec<String> = self
            .snap
            .borrow()
            .as_ref()
            .map(|s| {
                s.profiles
                    .iter()
                    .filter_map(|p| p.session.as_ref().map(|x| x.device.clone()))
                    .filter(|d| !d.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        {
            let mut g = self.graphs.borrow_mut();
            for dev in &devices {
                if let Some(b) = device_bytes(dev) {
                    g.entry(dev.clone()).or_default().sample(b);
                }
            }
        }
        for a in self.areas.borrow().iter() {
            a.queue_draw();
        }
        let now = tablet_core::now();
        for t in self.tickers.borrow().iter() {
            t.label.set_label(&(t.text)(now));
        }
    }

    fn render(self: &Rc<Self>) {
        let Some(s) = self.snap.borrow().clone() else {
            return;
        };
        self.render_header(&s);
        self.render_notice();
        for (tab, b) in &self.tabs {
            if *tab == self.tab.get() {
                b.add_css_class("on");
            } else {
                b.remove_css_class("on");
            }
            if *tab == Tab::Doctor {
                if s.doctor.broken() {
                    b.add_css_class("alert");
                } else {
                    b.remove_css_class("alert");
                }
            }
        }
        // Rebuild only when something shown changed: rebuilding resets
        // hover and focus.
        let sig = signature(&s, self.tab.get());
        if *self.built.borrow() == sig {
            return;
        }
        *self.built.borrow_mut() = sig;
        self.tickers.borrow_mut().clear();
        self.areas.borrow_mut().clear();
        self.render_ask(&s);
        clear(&self.content);
        match self.tab.get() {
            Tab::Now => self.render_now(&s),
            Tab::Checks => self.render_checks(&s),
            Tab::Doctor => self.render_doctor(&s),
            Tab::History => self.render_history(&s),
        }
        self.tick();
    }

    fn render_header(&self, s: &Snapshot) {
        let class = lamp_class(&s.lamp);
        set_state(&self.lamp, class);
        set_state(&self.chip, class);
        self.chip.set_label(match s.lamp.as_str() {
            "held" => "held",
            "degraded" => "dns missing",
            "connecting" => "connecting",
            "signin" => "sign in",
            "asking" => "asked",
            "stale" => "stale",
            "failed" => "failed",
            "dropped" => "dropped",
            "broken" => "openvpn3 broken",
            _ => "off",
        });
        let addresses = self.cfg.borrow().drawer.addresses;
        let sub = match s.default_profile() {
            Some(p) => {
                let server = p
                    .session
                    .as_ref()
                    .map(|x| x.server.clone())
                    .unwrap_or_default();
                let mut t = p.name.clone();
                if addresses && !server.is_empty() {
                    t.push_str(&format!(" \u{b7} {server}"));
                }
                if s.profiles.len() > 1 {
                    t.push_str(&format!(" \u{b7} {} profiles", s.profiles.len()));
                }
                t
            }
            None if !s.others.is_empty() => format!("{} session without a profile", s.others.len()),
            None => "no profiles yet".into(),
        };
        self.sub.set_label(&sub);
    }

    fn render_ask(self: &Rc<Self>, s: &Snapshot) {
        clear(&self.ask);
        let Some(a) = s.asks.first() else {
            self.ask.set_visible(false);
            return;
        };
        self.ask.set_visible(true);
        let c = card(&["ask"]);
        let row = heading("AGENT ASKING", "");
        let elapsed = label("", &["t-label"]);
        let at = a.at;
        row.append(&elapsed);
        self.tickers.borrow_mut().push(Ticker {
            label: elapsed,
            text: Box::new(move |now| span(now - at)),
        });
        c.append(&row);
        let who = if a.requester.is_empty() {
            "An agent"
        } else {
            &a.requester
        };
        c.append(&wrapped(&format!("{who} wants {}", a.profile), &["title"]));
        if !a.command.is_empty() {
            let cmd = wrapped(&format!("$ {}", a.command), &["mono"]);
            c.append(&cmd);
        }
        if !a.cwd.is_empty() {
            c.append(&wrapped(&format!("in {}", tilde(&a.cwd)), &["dim"]));
        }
        if !a.reason.is_empty() {
            c.append(&wrapped(
                &format!(
                    "\u{201c}{}\u{201d} (the agent's reason, unverified)",
                    a.reason
                ),
                &["dim"],
            ));
        }
        let btns = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        btns.add_css_class("buttons");
        let id = a.id;
        btns.append(&self.button(
            &format!("Connect {}", a.profile),
            true,
            Request::Answer { id, connect: true },
        ));
        btns.append(&self.button("Not now", false, Request::Answer { id, connect: false }));
        c.append(&btns);
        if s.asks.len() > 1 {
            c.append(&label(&format!("and {} more", s.asks.len() - 1), &["dim"]));
        }
        self.ask.append(&c);
    }

    fn button(self: &Rc<Self>, text: &str, primary: bool, req: Request) -> gtk::Button {
        let b = gtk::Button::with_label(text);
        b.set_focus_on_click(false);
        if primary {
            b.add_css_class("primary");
        }
        let w = Rc::downgrade(self);
        b.connect_clicked(move |_| {
            if let Some(d) = w.upgrade() {
                d.send(req.clone());
            }
        });
        b
    }

    fn check_button(self: &Rc<Self>, profile: Option<String>) -> gtk::Button {
        let b = gtk::Button::with_label("Check now");
        b.set_focus_on_click(false);
        let w = Rc::downgrade(self);
        b.connect_clicked(move |_| {
            if let Some(d) = w.upgrade() {
                d.check(profile.clone());
            }
        });
        b
    }

    fn render_now(self: &Rc<Self>, s: &Snapshot) {
        if s.profiles.is_empty() {
            let c = card(&[]);
            c.append(&wrapped(
                "No profiles yet. Add a [[profile]] with a name and its .ovpn file to ~/.config/tablet/config.toml, and the lamp connects it.",
                &[],
            ));
            self.content.append(&c);
        }
        for p in &s.profiles {
            self.content.append(&self.profile_card(p));
        }
        for o in &s.others {
            let c = card(&[]);
            c.append(&heading("SESSION WITHOUT A PROFILE", ""));
            c.append(&wrapped(&o.config_name, &["mono"]));
            c.append(&wrapped(&o.status.describe(), &["dim"]));
            c.append(&self.button(
                "Disconnect all",
                false,
                Request::Disconnect { profile: None },
            ));
            self.content.append(&c);
        }
    }

    fn profile_card(self: &Rc<Self>, p: &ProfileView) -> gtk::Box {
        let c = card(&[phase_class(p.phase)]);
        let name = if p.default {
            format!("{} \u{b7} DEFAULT", p.name.to_uppercase())
        } else {
            p.name.to_uppercase()
        };
        let head = heading(&name, "");
        let phase = label(p.phase.label(), &["t-label", "phase", phase_class(p.phase)]);
        head.append(&phase);
        c.append(&head);

        // The big line: how long it has been held, or since when it is
        // in trouble.
        let big = label("", &["big", phase_class(p.phase)]);
        let (connected, since, ph) = (p.connected_at, p.since, p.phase);
        let text: Box<dyn Fn(f64) -> String> = match ph {
            Phase::Held | Phase::Stale | Phase::Reconnecting if connected.is_some() => {
                let at = connected.unwrap_or_default();
                Box::new(move |now| span(now - at))
            }
            Phase::Off => Box::new(|_| "off".into()),
            Phase::SignIn => Box::new(|_| "waiting on you".into()),
            Phase::Starting => Box::new(move |now| format!("{}\u{2026}", span(now - since))),
            _ => Box::new(move |now| when(since, now)),
        };
        big.set_label(&text(tablet_core::now()));
        self.tickers.borrow_mut().push(Ticker {
            label: big.clone(),
            text,
        });
        c.append(&big);

        if let Some(sess) = &p.session {
            let mut bits = vec![];
            if !sess.device.is_empty() {
                bits.push(sess.device.clone());
            }
            if let Some((proto, host, port)) = &sess.transport {
                if self.cfg.borrow().drawer.addresses {
                    bits.push(format!("{proto} {host}:{port}"));
                } else if !proto.is_empty() {
                    bits.push(proto.clone());
                }
            }
            if sess.created > 0.0 {
                bits.push(format!(
                    "session from {}",
                    when(sess.created, tablet_core::now())
                ));
            }
            if !bits.is_empty() {
                c.append(&label(&bits.join(" \u{b7} "), &["dim"]));
            }
        } else if p.phase == Phase::Off {
            c.append(&label(&p.ovpn, &["dim", "mono"]));
        }
        if let Some(r) = &p.reason
            && p.phase != Phase::Held
        {
            c.append(&wrapped(r, &["reason", phase_class(p.phase)]));
        }
        if p.phase == Phase::Held
            && let Some(ch) = &p.checks
            && !ch.dns_missing.is_empty()
        {
            c.append(&wrapped(
                &format!("DNS is missing {}", ch.dns_missing.join(", ")),
                &["reason", "wait"],
            ));
        }

        // Traffic, while there is a device to read.
        if let Some(dev) = p
            .session
            .as_ref()
            .map(|x| x.device.clone())
            .filter(|d| !d.is_empty())
            && p.phase.up()
        {
            c.append(&self.graph(&dev));
            let stats = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let down = label("", &["num"]);
            let up = label("", &["num", "dim"]);
            let heard = label("", &["num", "dim"]);
            heard.set_hexpand(true);
            heard.set_xalign(1.0);
            stats.append(&down);
            stats.append(&up);
            stats.append(&heard);
            let g = self.graphs.clone();
            let dev2 = dev.clone();
            self.tickers.borrow_mut().push(Ticker {
                label: down,
                text: Box::new(move |_| {
                    let (i, _) = g
                        .borrow()
                        .get(&dev2)
                        .map(|t| t.latest())
                        .unwrap_or_default();
                    format!("\u{2193} {}", rate(i))
                }),
            });
            let g = self.graphs.clone();
            let dev3 = dev.clone();
            self.tickers.borrow_mut().push(Ticker {
                label: up,
                text: Box::new(move |_| {
                    let (_, o) = g
                        .borrow()
                        .get(&dev3)
                        .map(|t| t.latest())
                        .unwrap_or_default();
                    format!("\u{2191} {}", rate(o))
                }),
            });
            let quiet = p.silent_for;
            let checked = p
                .checks
                .as_ref()
                .map(|ch| (ch.at, ch.probes.len(), ch.failed()));
            self.tickers.borrow_mut().push(Ticker {
                label: heard,
                text: Box::new(move |now| match (quiet, checked) {
                    (Some(q), _) => format!("server silent {}", span(q)),
                    (None, Some((at, n, f))) if n > 0 => {
                        format!("{}/{n} probes ok {}", n - f, ago(now - at))
                    }
                    _ => String::new(),
                }),
            });
            c.append(&stats);
        }

        let btns = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        btns.add_css_class("buttons");
        let name = Some(p.name.clone());
        match p.phase {
            Phase::Off => {
                btns.append(&self.button("Connect", true, Request::Connect { profile: name }));
            }
            Phase::Held => {
                btns.append(&self.button(
                    "Disconnect",
                    false,
                    Request::Disconnect {
                        profile: name.clone(),
                    },
                ));
                btns.append(&self.button(
                    "Reconnect",
                    false,
                    Request::Reconnect {
                        profile: name.clone(),
                    },
                ));
                btns.append(&self.check_button(name));
            }
            Phase::Stale => {
                btns.append(&self.button(
                    "Reconnect",
                    true,
                    Request::Reconnect {
                        profile: name.clone(),
                    },
                ));
                btns.append(&self.check_button(name.clone()));
                btns.append(&self.button(
                    "Disconnect",
                    false,
                    Request::Disconnect { profile: name },
                ));
            }
            Phase::Failed | Phase::Dropped => {
                btns.append(&self.button(
                    "Reconnect",
                    true,
                    Request::Reconnect {
                        profile: name.clone(),
                    },
                ));
                btns.append(&self.button("Dismiss", false, Request::Ack { profile: name }));
            }
            Phase::SignIn => {
                if p.signin_url.is_some() {
                    btns.append(&self.button(
                        "Open sign-in",
                        true,
                        Request::OpenSignIn {
                            profile: name.clone(),
                        },
                    ));
                }
                btns.append(&self.button("Cancel", false, Request::Disconnect { profile: name }));
            }
            Phase::Starting | Phase::Reconnecting => {
                btns.append(&self.button("Cancel", false, Request::Disconnect { profile: name }));
            }
        }
        c.append(&btns);
        c
    }

    /// The last minute of a device's traffic: in as a filled area in the
    /// accent, out as a line.
    fn graph(&self, dev: &str) -> gtk::DrawingArea {
        let area = gtk::DrawingArea::new();
        area.add_css_class("graph");
        area.set_content_height(38);
        area.set_hexpand(true);
        let g = self.graphs.clone();
        let dev = dev.to_string();
        area.set_draw_func(move |area, cr, w, h| {
            let (w, h) = (w as f64, h as f64);
            let fg = area.color();
            let graphs = g.borrow();
            let Some(t) = graphs.get(&dev) else { return };
            let pts: Vec<(f64, f64)> = t.rates.iter().copied().collect();
            if pts.len() < 2 {
                return;
            }
            let top = pts
                .iter()
                .map(|(i, o)| i.max(*o))
                .fold(1024.0_f64, f64::max)
                * 1.15;
            let step = w / (GRAPH_SECS - 1) as f64;
            let x0 = w - step * (pts.len() - 1) as f64;
            let y = |v: f64| h - 1.0 - (v / top) * (h - 3.0);
            cr.move_to(x0, h);
            for (i, (rx, _)) in pts.iter().enumerate() {
                cr.line_to(x0 + step * i as f64, y(*rx));
            }
            cr.line_to(w, h);
            cr.close_path();
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.22);
            let _ = cr.fill();
            cr.set_line_width(1.5);
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.95);
            for (i, (rx, _)) in pts.iter().enumerate() {
                let x = x0 + step * i as f64;
                if i == 0 {
                    cr.move_to(x, y(*rx));
                } else {
                    cr.line_to(x, y(*rx));
                }
            }
            let _ = cr.stroke();
            cr.set_line_width(1.0);
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.45);
            for (i, (_, tx)) in pts.iter().enumerate() {
                let x = x0 + step * i as f64;
                if i == 0 {
                    cr.move_to(x, y(*tx));
                } else {
                    cr.line_to(x, y(*tx));
                }
            }
            let _ = cr.stroke();
        });
        self.areas.borrow_mut().push(area.clone());
        area
    }

    fn render_checks(self: &Rc<Self>, s: &Snapshot) {
        let up: Vec<&ProfileView> = s
            .profiles
            .iter()
            .filter(|p| p.phase != Phase::Off)
            .collect();
        if up.is_empty() {
            let c = card(&[]);
            c.append(&wrapped("Nothing is connected. Checks run a few seconds after a tunnel comes up, every couple of minutes while it is up, after a resume, and when you press k.", &["dim"]));
            self.content.append(&c);
            return;
        }
        for p in up {
            let c = card(&[]);
            let row = heading(&format!("{} \u{b7} PROBES", p.name.to_uppercase()), "");
            let when_l = label("", &["t-label"]);
            row.append(&when_l);
            let at = p.checks.as_ref().map(|ch| ch.at);
            self.tickers.borrow_mut().push(Ticker {
                label: when_l,
                text: Box::new(move |now| match at {
                    Some(a) => format!("CHECKED {}", ago(now - a).to_uppercase()),
                    None => "NOT CHECKED YET".into(),
                }),
            });
            c.append(&row);
            match &p.checks {
                Some(ch) if !ch.probes.is_empty() => {
                    for pr in &ch.probes {
                        let right = if pr.ok { format!("{} ms", pr.ms) } else { pr.error.clone().unwrap_or_default() };
                        c.append(&mark_row(Some(pr.ok), &pr.target, &right));
                    }
                }
                Some(_) => c.append(&wrapped("This profile names no probes; add some to tell a stale tunnel from an idle one.", &["dim"])),
                None => c.append(&label(if p.busy.as_deref() == Some("checking") { "checking\u{2026}" } else { "waiting for the first check" }, &["dim"])),
            }
            if let Some(ch) = &p.checks
                && let Some(dev) = p
                    .session
                    .as_ref()
                    .map(|x| x.device.as_str())
                    .filter(|d| !d.is_empty())
            {
                {
                    c.append(&heading(&format!("DNS ON {}", dev.to_uppercase()), ""));
                    match &ch.dns {
                        Some(dns) => {
                            let servers = if dns.servers.is_empty() {
                                "no resolvers".to_string()
                            } else {
                                dns.servers.join(" ")
                            };
                            let domains = if dns.domains.is_empty() {
                                "no domains".to_string()
                            } else {
                                dns.domains.join(" ")
                            };
                            let missing_server =
                                ch.dns_missing.iter().any(|m| m.starts_with("resolver"));
                            let missing_zone = ch.dns_missing.iter().any(|m| m.starts_with("zone"));
                            c.append(&mark_row(Some(!missing_server), &servers, "resolvers"));
                            c.append(&mark_row(Some(!missing_zone), &domains, "domains"));
                            for m in &ch.dns_missing {
                                c.append(&mark_row(Some(false), &format!("missing {m}"), ""));
                            }
                            if ch.dns_applied {
                                c.append(&label("Tablet set these itself (apply_dns)", &["dim"]));
                            }
                            if let Some(e) = &ch.dns_error {
                                c.append(&wrapped(e, &["reason", "bad"]));
                            }
                        }
                        None => c.append(&label("systemd-resolved is not running", &["dim"])),
                    }
                }
            }
            if let Some(sess) = &p.session {
                c.append(&heading("OPENVPN3'S COUNTERS", ""));
                c.append(&mark_row(
                    None,
                    &format!(
                        "{} in, {} out",
                        bytes(sess.stat("BYTES_IN") as f64),
                        bytes(sess.stat("BYTES_OUT") as f64)
                    ),
                    "transport",
                ));
                c.append(&mark_row(
                    Some(p.silent_for.is_none()),
                    &match p.silent_for {
                        Some(q) => format!("nothing from the server for {}", span(q)),
                        None => "the server is talking".into(),
                    },
                    "",
                ));
                c.append(&mark_row(
                    None,
                    &format!(
                        "{} keepalive timeouts, {} reconnects",
                        sess.stat("KEEPALIVE_TIMEOUT"),
                        sess.stat("N_RECONNECT")
                    ),
                    "",
                ));
                if sess.stat("AUTH_FAILED") > 0 {
                    c.append(&mark_row(
                        Some(false),
                        "the server refused a sign-in",
                        "AUTH_FAILED",
                    ));
                }
            }
            let btns = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            btns.add_css_class("buttons");
            btns.append(&self.check_button(Some(p.name.clone())));
            c.append(&btns);
            self.content.append(&c);
        }
    }

    fn render_doctor(self: &Rc<Self>, s: &Snapshot) {
        let d = &s.doctor;
        let c = card(&[if d.broken() { "bad" } else { "plain" }]);
        let row = heading("OPENVPN3", "");
        let state = if d.not_installed {
            "NOT FOUND"
        } else if d.broken() {
            "BROKEN"
        } else {
            "FINE"
        };
        row.append(&label(
            state,
            &[
                "t-label",
                "phase",
                if d.broken() || d.not_installed {
                    "bad"
                } else {
                    "held"
                },
            ],
        ));
        c.append(&row);
        c.append(&wrapped(&d.summary(), &[]));
        if d.at > 0.0 {
            c.append(&label(&format!("looked {}", when(d.at, s.now)), &["dim"]));
        }
        for (bin, lib) in &d.missing {
            let short = bin.rsplit('/').next().unwrap_or(bin);
            c.append(&mark_row(Some(false), lib, short));
        }
        if d.broken() {
            c.append(&wrapped(
                "A library openvpn3 was built against is gone, usually after a system update. Rebuild or reinstall openvpn3; an AUR build needs `paru -S --rebuild openvpn3`.",
                &["dim"],
            ));
        }
        for svc in &d.outdated {
            c.append(&mark_row(None, svc, "old binary"));
        }
        if !d.outdated.is_empty() {
            c.append(&wrapped("These openvpn3 services still run code that has been replaced. They restart when idle; the next session may need them restarted (`sudo pkill -f openvpn3-service-`).", &["dim"]));
        }
        let btns = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        btns.add_css_class("buttons");
        if d.broken() && !self.cfg.borrow().doctor.repair.trim().is_empty() {
            btns.append(&self.button("Repair", true, Request::Repair));
        }
        btns.append(&self.button("Look again", false, Request::Doctor));
        c.append(&btns);
        self.content.append(&c);

        let cfgc = card(&[]);
        cfgc.append(&heading("SETUP", ""));
        match &s.config_error {
            Some(e) => cfgc.append(&mark_row(Some(false), e, "config")),
            None => cfgc.append(&mark_row(
                Some(true),
                &format!(
                    "{} profile{}",
                    s.profiles.len(),
                    if s.profiles.len() == 1 { "" } else { "s" }
                ),
                "config",
            )),
        }
        cfgc.append(&mark_row(
            Some(s.openvpn3 || s.profiles.iter().all(|p| p.session.is_none())),
            if s.openvpn3 {
                "session manager running"
            } else {
                "session manager idle (starts with a session)"
            },
            "",
        ));
        self.content.append(&cfgc);
    }

    fn render_history(&self, s: &Snapshot) {
        let c = card(&[]);
        c.append(&heading("WHAT HAPPENED", "NEWEST FIRST"));
        if s.history.is_empty() {
            c.append(&label("Nothing yet.", &["dim"]));
        }
        for e in s.history.iter().take(30) {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let t = label(&short_when(e.at, s.now), &["dim", "num", "when"]);
            t.set_width_chars(9);
            row.append(&t);
            let class = match e.what.as_str() {
                "connected" | "reconnected" | "recovered" | "repaired" => "held",
                "stale" => "stale",
                "failed" | "dropped" | "broken" => "bad",
                "reconnecting" | "signin" => "wait",
                _ => "off",
            };
            let who = if e.profile.is_empty() {
                "openvpn3".to_string()
            } else {
                e.profile.clone()
            };
            let w = label(&format!("{who} {}", e.what), &["what", class]);
            row.append(&w);
            let detail = label(&e.detail, &["dim"]);
            detail.set_hexpand(true);
            detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
            detail.set_tooltip_text(Some(&e.detail));
            row.append(&detail);
            c.append(&row);
        }
        self.content.append(&c);
    }
}

/// `/home/me/x` as `~/x`.
fn tilde(p: &str) -> String {
    tablet_core::paths::tilde(std::path::Path::new(p))
}

/// What the current tab shows, without the clock: when this is unchanged,
/// the tab is not rebuilt (tickers keep the times moving).
fn signature(s: &Snapshot, tab: Tab) -> String {
    let mut s = s.clone();
    s.now = 0.0;
    for p in &mut s.profiles {
        p.silent_for = p.silent_for.map(|q| (q / 15.0).floor());
        if let Some(sess) = &mut p.session {
            // Counters move all the time; the Checks tab shows them to the
            // nearest change of a whole unit at most.
            if tab != Tab::Checks {
                sess.stats.clear();
            } else {
                for v in sess.stats.values_mut() {
                    *v /= 100_000;
                }
            }
        }
    }
    format!("{tab:?}{}", serde_json::to_string(&s).unwrap_or_default())
}
