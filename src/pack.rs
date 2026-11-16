//! Lineup packs (`nadecue import`) and taking their pictures in the game (`nadecue shoot`).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::library::{self, Lineup};
use crate::{capture, radar};

/// Unit view vector for setang angles (pitch down is positive, like the game).
pub fn forward(ang: [f64; 2]) -> [f64; 3] {
    let (p, y) = (ang[0].to_radians(), ang[1].to_radians());
    let r = |v: f64| (v * 100.0).round() / 100.0;
    [r(p.cos() * y.cos()), r(p.cos() * y.sin()), r(-p.sin())]
}

/// Draw (or redraw) a lineup's radar picture when the radar files and its position are there.
pub fn draw_radar(dir: &Path, lineup: &mut Lineup) -> bool {
    let Some(pos) = lineup.pos else { return false };
    let Some(png) = radar::render(dir, &lineup.map, pos, lineup.to) else { return false };
    let name = format!("{}.radar.png", lineup.stem());
    if std::fs::write(lineup.file.with_file_name(&name), png).is_err() {
        return false;
    }
    lineup.radar = name;
    true
}

/// Add the lineups in a pack (a JSON array of lineups) to the library. A lineup whose `id` is
/// already there gets its text and position updated and keeps its pictures.
pub fn import(file: &Path) -> Result<String, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let pack: Vec<Lineup> = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", file.display()))?;
    let root = library::root();
    let radar_dir = radar::dir();
    let mut by_map: BTreeMap<String, Vec<Lineup>> = BTreeMap::new();
    let (mut added, mut updated, mut radars) = (0, 0, 0);
    for mut new in pack {
        if new.name.is_empty() || new.map.is_empty() {
            continue;
        }
        if new.fwd.is_none() {
            new.fwd = new.ang.map(forward);
        }
        let existing = by_map.entry(new.map.clone()).or_insert_with(|| library::load(&root, &new.map));
        let found = (!new.id.is_empty()).then(|| existing.iter_mut().find(|l| l.id == new.id)).flatten();
        let mut lineup = match found {
            Some(old) => {
                let mut l = new.clone();
                l.file = old.file.clone();
                l.image = old.image.clone();
                l.view = old.view.clone();
                l.radar = old.radar.clone();
                l.store().map_err(|e| e.to_string())?;
                updated += 1;
                l
            }
            None => {
                let mut l = new.clone();
                library::save(&root, &mut l, None).map_err(|e| e.to_string())?;
                existing.push(l.clone());
                added += 1;
                l
            }
        };
        if draw_radar(&radar_dir, &mut lineup) {
            lineup.store().map_err(|e| e.to_string())?;
            radars += 1;
        }
    }
    let mut msg = format!("{added} added, {updated} updated");
    if radars > 0 {
        msg += &format!(", {radars} radar pictures");
    } else {
        msg += &format!(" (no radar pictures: put <map>.png + <map>.txt in {})", radar_dir.display());
    }
    Ok(msg)
}

/// CS2's text console over TCP (start the game with `-netconport PORT`).
struct Console(TcpStream);

impl Console {
    fn connect(port: u16) -> Result<Console, String> {
        let stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| {
            format!("no CS2 console on port {port} ({e}): add -netconport {port} to CS2's launch options")
        })?;
        stream.set_read_timeout(Some(Duration::from_millis(50))).ok();
        Ok(Console(stream))
    }

    fn send(&mut self, command: &str) -> Result<(), String> {
        self.drain();
        writeln!(self.0, "{command}").map_err(|e| format!("CS2 console: {e}"))
    }

    /// Throw away what the console printed (it would fill the socket buffer otherwise).
    fn drain(&mut self) {
        let mut buf = [0u8; 65536];
        while let Ok(n) = self.0.read(&mut buf) {
            if n == 0 {
                break;
            }
        }
    }
}

fn game_map() -> Option<String> {
    let state: serde_json::Value = serde_json::from_str(&crate::ask("state").ok()?).ok()?;
    state["alive"].as_bool()?.then_some(())?;
    state["map"].as_str().map(str::to_string)
}

/// Load `map` on a local server and wait until we stand in it.
fn enter(console: &mut Console, map: &str) -> Result<(), String> {
    if game_map().as_deref() != Some(map) {
        println!("Loading {map}...");
        console.send(&format!("map {map}"))?;
        let start = Instant::now();
        sleep(Duration::from_secs(5));
        while game_map().as_deref() != Some(map) {
            if start.elapsed() > Duration::from_secs(120) {
                return Err(format!("{map} didn't load within 2 minutes"));
            }
            sleep(Duration::from_secs(1));
        }
    }
    // A clean, quiet server: no bots, no round end, no HUD but the crosshair, no weapon in view.
    for command in [
        "sv_cheats 1",
        "bot_kick",
        "mp_warmup_end",
        "mp_ignore_round_win_conditions 1",
        "mp_roundtime 60",
        "mp_roundtime_defuse 60",
        "mp_freezetime 0",
        "mp_restartgame 1",
    ] {
        console.send(command)?;
        sleep(Duration::from_millis(80));
    }
    sleep(Duration::from_secs(3));
    for command in ["god", "cl_draw_only_deathnotices 1", "r_drawviewmodel 0"] {
        console.send(command)?;
        sleep(Duration::from_millis(80));
    }
    Ok(())
}

/// Take the aim and view pictures of every lineup that has exact angles but no pictures yet
/// (`all` = retake them too), by standing on each spot in the running game.
pub fn shoot(cfg: &Config, maps: &[String], all: bool) -> Result<(), String> {
    let root = library::root();
    let radar_dir = radar::dir();
    let wanted: Vec<String> = if maps.is_empty() {
        let mut found: Vec<String> = std::fs::read_dir(&root)
            .map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
            .unwrap_or_default();
        found.sort();
        found
    } else {
        maps.iter().map(|m| if m.contains('_') { m.clone() } else { format!("de_{m}") }).collect()
    };
    let mut console = Console::connect(cfg.netcon_port)?;
    let mut done = 0;
    for map in wanted {
        let todo: Vec<Lineup> = library::load(&root, &map)
            .into_iter()
            .filter(|l| l.pos.is_some() && l.ang.is_some() && (all || l.image_path().is_none()))
            .collect();
        if todo.is_empty() {
            continue;
        }
        enter(&mut console, &map)?;
        println!("{map}: {} lineups", todo.len());
        for mut lineup in todo {
            let (p, a) = (lineup.pos.unwrap_or_default(), lineup.ang.unwrap_or_default());
            console.send(&format!("setpos {} {} {};setang {} {} 0", p[0], p[1], p[2], a[0], a[1]))?;
            sleep(Duration::from_millis(cfg.shoot_settle_ms));
            let (aim, view) = capture::pictures(&cfg.output)?;
            let stem = lineup.stem();
            lineup.image = format!("{stem}.png");
            lineup.view = format!("{stem}.view.png");
            std::fs::write(lineup.file.with_file_name(&lineup.image), aim).map_err(|e| e.to_string())?;
            std::fs::write(lineup.file.with_file_name(&lineup.view), view).map_err(|e| e.to_string())?;
            draw_radar(&radar_dir, &mut lineup);
            lineup.store().map_err(|e| e.to_string())?;
            done += 1;
            println!("  {}", lineup.name);
        }
    }
    for command in ["cl_draw_only_deathnotices 0", "r_drawviewmodel 1"] {
        console.send(command)?;
    }
    let _ = crate::ask("reload");
    println!("{done} lineups photographed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_to_vectors() {
        assert_eq!(forward([0.0, 0.0]), [1.0, 0.0, -0.0]);
        assert_eq!(forward([0.0, 90.0]), [0.0, 1.0, -0.0]);
        // Looking 30° up (negative pitch) points the vector up.
        let f = forward([-30.0, 0.0]);
        assert!(f[2] > 0.49 && f[2] < 0.51);
    }
}
