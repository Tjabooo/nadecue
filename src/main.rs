//! nadecue: CS2 grenade lineups that show up when you pull out the grenade and go away when you
//! throw it. `nadecue run` is the daemon; the other commands talk to it over a Unix socket.

mod capture;
mod config;
mod find;
mod gsi;
mod library;
mod llm;
mod overlay;
mod pack;
mod pick;
mod radar;
mod snapshot;

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use config::Config;
use library::Lineup;
use overlay::{Card, Overlay};
use pick::Guide;
use snapshot::{Kind, Snapshot};

/// The game went quiet (CS2 closed or tabbed to the menu without heartbeats).
const STALE: Duration = Duration::from_secs(15);

pub enum Event {
    Game(Box<Snapshot>),
    Command(String, Sender<String>),
}

fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("nadecue.sock")
}

const HELP: &str = "nadecue: CS2 grenade lineups on demand

  nadecue run            the daemon (installs CS2's Game State Integration config)
  nadecue next           show lineups for this map / step to the next one
  nadecue hide           hide the card until you switch grenades
  nadecue find WORDS     find a lineup by name, any order (\"mirage ct smoke from t spawn\")
  nadecue save           save where you stand and aim (hold the grenade) as a lineup
  nadecue forget         delete the lineup on the card
  nadecue import PACK.json  add a lineup pack (JSON array of lineups)
  nadecue shoot [MAP..] [--all]  photograph lineups that have exact angles, in CS2 (needs -netconport)
  nadecue radar          redraw every lineup's radar picture
  nadecue preview MAP    show MAP's lineups without the game (to check the card)
  nadecue list [MAP]     print saved lineups
  nadecue cfg            (re)install the Game State Integration config

Lineups live in $XDG_DATA_HOME/nadecue/lineups/<map>/, settings in ~/.config/nadecue/config.toml.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = Config::load();
    let result = match args.first().map(String::as_str) {
        Some("run") => run(cfg),
        Some("next" | "hide" | "forget" | "reload") => ask(&args[0]).map(|r| print_reply(&r)),
        Some("find") if args.len() > 1 => find_cli(&cfg, &args[1..].join(" ")),
        Some("import") if args.len() > 1 => args[1..]
            .iter()
            .try_for_each(|f| pack::import(std::path::Path::new(f)).map(|m| println!("{f}: {m}"))),
        Some("shoot") => {
            let all = args.iter().any(|a| a == "--all");
            let maps: Vec<String> = args[1..].iter().filter(|a| !a.starts_with("--")).cloned().collect();
            pack::shoot(&cfg, &maps, all)
        }
        Some("radar") => radar_all(),
        Some("preview") => match args.get(1) {
            Some(map) => ask(&format!("preview {map}")).map(|r| print_reply(&r)),
            None => Err("usage: nadecue preview MAP (e.g. de_mirage)".into()),
        },
        Some("save") => save(&cfg),
        Some("list") => {
            list(args.get(1).map(String::as_str));
            Ok(())
        }
        Some("cfg") => {
            gsi::install_cfg(cfg.port);
            Ok(())
        }
        _ => {
            println!("{HELP}");
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("nadecue: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_reply(reply: &str) {
    if !reply.is_empty() {
        println!("{reply}");
    }
}

/// Send one command to the daemon and return its reply.
pub fn ask(command: &str) -> Result<String, String> {
    let mut stream = UnixStream::connect(socket_path())
        .map_err(|e| format!("daemon not running ({e}); start it with `nadecue run`"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    writeln!(stream, "{command}").map_err(|e| e.to_string())?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).map_err(|e| e.to_string())?;
    Ok(reply.trim_end().to_string())
}

fn notify(summary: &str, body: &str) {
    let _ = Command::new("notify-send")
        .args(["-a", "nadecue", "-t", "3500", summary, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Ask with rofi (dmenu mode). None = cancelled (Esc), Some("") = empty answer.
fn prompt(label: &str, options: &[String]) -> Option<String> {
    let mut child = Command::new("rofi")
        .args(["-dmenu", "-i", "-p", label, "-l", &options.len().to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(options.join("\n").as_bytes());
    }
    let out = child.wait_with_output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn save(cfg: &Config) -> Result<(), String> {
    let state = ask("state")?;
    let snap: serde_json::Value = serde_json::from_str(&state).map_err(|e| e.to_string())?;
    let fail = |msg: &str| {
        notify("Lineup not saved", msg);
        Err(msg.to_string())
    };
    let Some(map) = snap["map"].as_str().map(str::to_string) else {
        return fail("No game state from CS2 yet (start CS2 after the daemon, once).");
    };
    let Some(kind) = snap["held"].as_str().and_then(|k| serde_json::from_value::<Kind>(k.into()).ok()) else {
        return fail("Hold the grenade you're lining up, then save.");
    };
    let vec = |key: &str| -> Option<[f64; 3]> { serde_json::from_value(snap[key].clone()).ok() };
    // Pictures first, before any prompt covers the screen.
    let pictures = match capture::pictures(&cfg.output) {
        Ok(pictures) => Some(pictures),
        Err(error) => {
            eprintln!("No pictures: {error}");
            None
        }
    };
    let Some(throw) = prompt("throw", &cfg.throws) else {
        return fail("Cancelled.");
    };
    let root = library::root();
    let auto = format!("{} {}", kind.word(), library::count(&root, &map, kind) + 1);
    let Some(name) = prompt(&format!("name ({auto})"), &[]) else {
        return fail("Cancelled.");
    };
    let note = prompt("where to stand (optional)", &[]).unwrap_or_default();
    let mut lineup = Lineup {
        name: if name.is_empty() { auto } else { name },
        map: map.clone(),
        kind,
        side: snap["side"].as_str().unwrap_or("").to_string(),
        pos: vec("pos"),
        fwd: vec("fwd"),
        throw,
        image: String::new(),
        view: String::new(),
        note,
        file: PathBuf::new(),
        ..Default::default()
    };
    let file =
        library::save(&root, &mut lineup, pictures.as_ref().map(|(a, v)| (a.as_slice(), v.as_slice())))
            .map_err(|e| e.to_string())?;
    let _ = ask("reload");
    notify(&format!("Saved {} {}", pretty_map(&map), kind.word().to_lowercase()), &lineup.name);
    println!("Saved {}", file.display());
    Ok(())
}

/// `find`: word matching in the daemon, the language model (if configured) when that isn't sure.
fn find_cli(cfg: &Config, text: &str) -> Result<(), String> {
    let reply: serde_json::Value =
        serde_json::from_str(&ask(&format!("match {text}"))?).map_err(|e| e.to_string())?;
    if let Some(error) = reply["error"].as_str() {
        notify("No lineup", error);
        return Err(error.to_string());
    }
    let options: Vec<(String, String)> = reply["options"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|o| Some((o["key"].as_str()?.to_string(), o["label"].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let best = reply["best"].as_u64().map(|b| b as usize);
    let sure = reply["sure"].as_bool().unwrap_or(false);
    let mut choice = best;
    if !sure && !cfg.llm_url.is_empty() && !options.is_empty() {
        let labels: Vec<String> = options.iter().map(|(_, l)| l.clone()).collect();
        match llm::pick(cfg, text, &labels) {
            Ok(pick) => choice = pick.or(best),
            Err(error) => eprintln!("language model: {error} (using word matching)"),
        }
    }
    let Some(i) = choice.filter(|i| *i < options.len()) else {
        let map = reply["map"].as_str().map(pretty_map).unwrap_or_default();
        notify("No lineup found", &format!("{map} · \"{text}\""));
        return Err(format!("No {map} lineup matches \"{text}\""));
    };
    print_reply(&ask(&format!("pin {}", options[i].0))?);
    Ok(())
}

fn radar_all() -> Result<(), String> {
    let root = library::root();
    let dir = radar::dir();
    let mut n = 0;
    for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())?.flatten() {
        let map = entry.file_name().to_string_lossy().into_owned();
        for mut lineup in library::load(&root, &map) {
            if pack::draw_radar(&dir, &mut lineup) {
                lineup.store().map_err(|e| e.to_string())?;
                n += 1;
            }
        }
    }
    println!("{n} radar pictures drawn (radar files in {})", dir.display());
    let _ = ask("reload");
    Ok(())
}

fn list(map: Option<&str>) {
    let root = library::root();
    let maps: Vec<String> = match map {
        Some(m) => vec![m.to_string()],
        None => {
            let mut maps: Vec<String> = std::fs::read_dir(&root)
                .map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
                .unwrap_or_default();
            maps.sort();
            maps
        }
    };
    for map in maps {
        let lineups = library::load(&root, &map);
        if lineups.is_empty() {
            continue;
        }
        println!("{map}");
        for l in lineups {
            let side = if l.side.is_empty() { "" } else { &l.side };
            println!("  {:<6} {:<3} {}  ({})", l.kind.label(), side, l.name, l.throw);
        }
    }
}

/// `de_mirage` → `Mirage`.
fn pretty_map(map: &str) -> String {
    let name = map.split_once('_').map_or(map, |(_, rest)| rest);
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => map.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// The daemon.

struct Daemon {
    cfg: Config,
    root: PathBuf,
    overlay: Overlay,
    snap: Snapshot,
    last_post: Option<Instant>,
    lineups: Vec<Lineup>,
    loaded_map: Option<String>,
    /// Steps taken with `next` through the current candidate list.
    index: usize,
    /// Browsing (card up without a grenade in hand) until then.
    browse_until: Option<Instant>,
    /// `hide` pressed: stay hidden until the grenade in hand changes.
    hushed: bool,
    /// Lineup currently on the card.
    shown: Option<usize>,
    used: library::Usage,
    /// Picked with `find`: comes first until it's thrown.
    pin: Option<String>,
}

fn run(cfg: Config) -> Result<(), String> {
    gsi::install_cfg(cfg.port);
    let (tx, rx) = mpsc::channel();
    gsi::start(cfg.port, tx.clone()).map_err(|e| format!("port {} unavailable: {e}", cfg.port))?;
    listen(tx)?;
    let mut overlay = Overlay::new(&cfg);
    overlay.warm_up();
    println!("nadecue listening on 127.0.0.1:{}, lineups in {}", cfg.port, library::root().display());
    let mut daemon = Daemon {
        root: library::root(),
        cfg,
        overlay,
        snap: Snapshot::default(),
        last_post: None,
        lineups: Vec::new(),
        loaded_map: None,
        index: 0,
        browse_until: None,
        hushed: false,
        shown: None,
        used: library::Usage::load(),
        pin: None,
    };
    daemon.serve(rx);
    Ok(())
}

fn listen(tx: Sender<Event>) -> Result<(), String> {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        return Err("another nadecue daemon is already running".into());
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut line = String::new();
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(Event::Command(line.trim().to_string(), reply_tx)).is_err() {
                return;
            }
            if let Ok(reply) = reply_rx.recv_timeout(Duration::from_secs(4)) {
                let _ = stream.write_all(reply.as_bytes());
            }
        }
    });
    Ok(())
}

impl Daemon {
    fn serve(&mut self, rx: Receiver<Event>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(Event::Game(snap)) => self.on_game(*snap),
                Ok(Event::Command(cmd, reply)) => {
                    let answer = self.on_command(&cmd);
                    let _ = reply.send(answer);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.render();
        }
    }

    fn load(&mut self, map: &str) {
        self.lineups = library::load(&self.root, map);
        self.loaded_map = Some(map.to_string());
    }

    fn on_game(&mut self, snap: Snapshot) {
        // Thrown while the card showed a lineup for this grenade: it goes first next time.
        if let Some(held) = self.snap.held
            && snap.nades < self.snap.nades
            && snap.me
            && let Some(lineup) = self.shown.and_then(|i| self.lineups.get(i))
            && lineup.kind == held
        {
            self.used.mark(lineup);
            if self.pin.as_ref() == Some(&lineup.key()) {
                self.pin = None;
            }
        }
        if snap.map != self.loaded_map
            && let Some(map) = &snap.map
        {
            self.load(&map.clone());
        }
        if snap.held != self.snap.held {
            // A new grenade in hand (or a throw): fresh list, picked again, re-read from disk.
            if let Some(map) = snap.map.clone() {
                self.load(&map);
            }
            self.index = 0;
            self.hushed = false;
            if self.snap.held.is_some() {
                self.browse_until = None;
            }
        } else if snap.held.is_some() && snap.nades < self.snap.nades {
            // Threw one of two flashes: same grenade still in hand, card stays, list restarts.
            self.index = 0;
        }
        self.snap = snap;
        self.last_post = Some(Instant::now());
    }

    fn live(&self) -> bool {
        self.last_post.is_some_and(|t| t.elapsed() < STALE)
    }

    fn on_command(&mut self, cmd: &str) -> String {
        let (verb, arg) = cmd.split_once(' ').unwrap_or((cmd, ""));
        match verb {
            "next" => {
                if self.overlay.is_open() {
                    self.index += 1;
                    if self.browse_until.is_some() {
                        self.browse_until = Some(Instant::now() + Duration::from_secs(self.cfg.browse_secs));
                    }
                } else {
                    self.index = 0;
                    self.hushed = false;
                    self.browse_until = Some(Instant::now() + Duration::from_secs(self.cfg.browse_secs));
                }
                String::new()
            }
            "hide" => {
                self.hushed = self.snap.held.is_some();
                self.browse_until = None;
                String::new()
            }
            "reload" => {
                if let Some(map) = self.loaded_map.clone() {
                    self.load(&map);
                }
                format!("{} lineups", self.lineups.len())
            }
            "state" => serde_json::to_string(&self.snap).unwrap_or_default(),
            "match" if !arg.is_empty() => self.matches(arg),
            "pin" if !arg.is_empty() => self.pin(arg),
            "forget" => match self.shown.and_then(|i| self.lineups.get(i)).cloned() {
                Some(lineup) => match library::forget(&lineup) {
                    Ok(()) => {
                        let map = lineup.map.clone();
                        self.load(&map);
                        self.shown = None;
                        notify("Lineup deleted", &lineup.name);
                        format!("Deleted {}", lineup.name)
                    }
                    Err(e) => format!("Could not delete {}: {e}", lineup.file.display()),
                },
                None => "No lineup on the card".into(),
            },
            "preview" if !arg.is_empty() => {
                if !self.live() {
                    self.snap = Snapshot { map: Some(arg.to_string()), ..Default::default() };
                }
                self.load(arg);
                self.index = 0;
                self.browse_until = Some(Instant::now() + Duration::from_secs(self.cfg.browse_secs));
                format!("{} lineups for {arg}", self.lineups.len())
            }
            _ => format!("unknown command: {cmd}"),
        }
    }

    /// The lineups that fit right now (indexes, in order) and which one is on the card.
    fn choose(&mut self) -> Option<(Vec<usize>, usize)> {
        if self.browse_until.is_some_and(|t| Instant::now() >= t) {
            self.browse_until = None;
        }
        let browsing = self.browse_until.is_some();
        let s = &self.snap;
        if !browsing && (!self.live() || !s.me || !s.alive || self.hushed) {
            return None;
        }
        let pinned = self.pin.as_ref().and_then(|key| self.lineups.iter().position(|l| &l.key() == key));
        let browse_kind = pinned.map(|i| self.lineups[i].kind);
        let (kind, radius) = match (s.held, browsing) {
            (Some(kind), _) => (Some(kind), if browsing { f64::INFINITY } else { self.cfg.radius() }),
            (None, true) => (browse_kind, f64::INFINITY),
            (None, false) => return None,
        };
        let mut order = pick::candidates(&self.lineups, s, kind, radius, &self.used);
        if let Some(p) = pinned
            && let Some(at) = order.iter().position(|&i| i == p)
        {
            let p = order.remove(at);
            order.insert(0, p);
        }
        if order.is_empty() {
            return None;
        }
        let at = self.index % order.len();
        Some((order, at))
    }

    /// `match WORDS`: the lineups the words could mean (best first) for `find` to choose from.
    fn matches(&mut self, text: &str) -> String {
        let q = find::parse(text);
        let map = q.map.clone().or_else(|| self.snap.map.clone()).or_else(|| self.loaded_map.clone());
        let Some(map) = map else {
            return serde_json::json!({"error": "Say the map too, e.g. \"mirage window smoke\"."})
                .to_string();
        };
        if self.loaded_map.as_deref() != Some(map.as_str()) {
            self.load(&map);
        }
        let ranked = find::rank(&q, &self.lineups, &self.used);
        let sure = find::sure(&q, &ranked, &self.lineups);
        // Everything of that kind on the map goes to the model, word-match order first.
        let mut order: Vec<usize> = ranked.iter().map(|(i, _)| *i).collect();
        for (i, l) in self.lineups.iter().enumerate() {
            if q.kind.is_none_or(|k| l.kind == k) && !order.contains(&i) {
                order.push(i);
            }
        }
        let options: Vec<serde_json::Value> = order
            .iter()
            .map(|&i| {
                let l = &self.lineups[i];
                let side = if l.side.is_empty() { "any side".to_string() } else { format!("{} side", l.side) };
                serde_json::json!({"key": l.key(), "label": format!("{} ({}, {side})", l.name, l.kind.word())})
            })
            .collect();
        serde_json::json!({
            "map": map,
            "options": options,
            "best": (!ranked.is_empty()).then_some(0),
            "sure": sure,
        })
        .to_string()
    }

    /// `pin KEY`: show that lineup now and first when its grenade comes out, until it's thrown.
    fn pin(&mut self, key: &str) -> String {
        // The game may have reloaded its own map since `match`: load the lineup's map again.
        if !self.lineups.iter().any(|l| l.key() == key)
            && let Some((map, _)) = key.split_once('/')
        {
            self.load(map);
        }
        let Some(lineup) = self.lineups.iter().find(|l| l.key() == key) else {
            return format!("No lineup {key}");
        };
        let map = lineup.map.clone();
        let reply = format!("{} ({} {})", lineup.name, pretty_map(&map), lineup.kind.word().to_lowercase());
        self.pin = Some(key.to_string());
        if !self.live() || self.snap.map.as_deref() != Some(map.as_str()) {
            // Planning from the menu or another map: show it as a browse card.
            self.snap = Snapshot { map: Some(map), ..Default::default() };
        }
        self.index = 0;
        self.hushed = false;
        self.browse_until = Some(Instant::now() + Duration::from_secs(self.cfg.browse_secs.max(20)));
        reply
    }

    /// Up to `LIST` names around the one on the card, for the strip under the picture.
    fn list_json(&self, order: &[usize], at: usize) -> String {
        const LIST: usize = 6;
        if order.len() < 2 {
            return "[]".into();
        }
        let start = at.saturating_sub(LIST / 2).min(order.len().saturating_sub(LIST));
        let items: Vec<serde_json::Value> = order
            .iter()
            .enumerate()
            .skip(start)
            .take(LIST)
            .map(|(n, &i)| serde_json::json!({"name": self.lineups[i].name, "on": n == at}))
            .collect();
        serde_json::Value::Array(items).to_string()
    }

    fn render(&mut self) {
        let Some((order, at)) = self.choose() else {
            self.shown = None;
            self.overlay.hide();
            return;
        };
        let i = order[at];
        self.shown = Some(i);
        let lineup = &self.lineups[i];
        let (arrow, line, state) = match pick::guide(&self.snap, lineup) {
            Guide::Walk { arrow, metres } => {
                let m = if metres < 10.0 { format!("{metres:.1} m") } else { format!("{metres:.0} m") };
                (arrow.to_string(), m, "walk")
            }
            Guide::Aim { up, left } => match pick::aim_text(up, left) {
                Some(text) => ("◎".to_string(), text, "spot"),
                None => ("◉".to_string(), "on spot · match the picture".to_string(), "ready"),
            },
            Guide::Unknown => (String::new(), lineup.throw.to_uppercase(), "blind"),
        };
        let list = self.list_json(&order, at);
        let card = Card {
            kind: lineup.kind.label().to_string(),
            name: lineup.name.clone(),
            side: lineup.side.clone(),
            img: lineup.image_path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            view: if lineup.radar_path().is_some() {
                String::new()
            } else {
                lineup.view_path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
            },
            radar: lineup.radar_path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            note: lineup.note.clone(),
            arrow,
            line,
            state: state.to_string(),
            throw: if state == "blind" { String::new() } else { lineup.throw.to_uppercase() },
            count: if order.len() > 1 { format!("{}/{}", at + 1, order.len()) } else { String::new() },
            list,
        };
        self.overlay.show(&card);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_names() {
        assert_eq!(pretty_map("de_mirage"), "Mirage");
        assert_eq!(pretty_map("cs_office"), "Office");
        assert_eq!(pretty_map("aim"), "Aim");
    }
}
