//! CS2 Game State Integration: the config CS2 reads at launch and the HTTP listener it posts to.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::Event;
use crate::snapshot::Snapshot;

pub const TOKEN: &str = "nadecue";
pub const CFG_NAME: &str = "gamestate_integration_nadecue.cfg";
const MAX_BODY: usize = 1024 * 1024;

/// No buffer and a short throttle so the card follows weapon switches and steps right away.
pub fn cfg_text(port: u16) -> String {
    format!(
        r#""nadecue"
{{
    "uri"       "http://127.0.0.1:{port}/"
    "timeout"   "1.0"
    "buffer"    "0.0"
    "throttle"  "0.05"
    "heartbeat" "5.0"
    "auth"
    {{
        "token" "{TOKEN}"
    }}
    "data"
    {{
        "provider"         "1"
        "map"              "1"
        "player_id"        "1"
        "player_state"     "1"
        "player_weapons"   "1"
        "player_position"  "1"
    }}
}}
"#
    )
}

/// Library paths listed in a Steam `libraryfolders.vdf`.
pub fn vdf_paths(vdf: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut rest = vdf;
    while let Some(i) = rest.find("\"path\"") {
        rest = &rest[i + 6..];
        let trimmed = rest.trim_start();
        if trimmed.len() == rest.len() || !trimmed.starts_with('"') {
            continue;
        }
        let value = &trimmed[1..];
        if let Some(end) = value.find('"')
            && end > 0
        {
            paths.push(PathBuf::from(value[..end].replace("\\\\", "\\")));
            rest = &value[end + 1..];
        }
    }
    paths
}

/// CS2's cfg folder in every Steam library (native, Flatpak and Snap Steam).
pub fn cfg_dirs() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let roots = [
        ".local/share/Steam",
        ".steam/steam",
        ".steam/root",
        ".var/app/com.valvesoftware.Steam/.local/share/Steam",
        "snap/steam/common/.local/share/Steam",
    ];
    let mut libraries = BTreeSet::new();
    for root in roots.map(|r| home.join(r)) {
        let Ok(bytes) = fs::read(root.join("steamapps/libraryfolders.vdf")) else {
            continue;
        };
        let vdf = String::from_utf8_lossy(&bytes);
        libraries.insert(fs::canonicalize(&root).unwrap_or(root));
        for path in vdf_paths(&vdf) {
            libraries.insert(fs::canonicalize(&path).unwrap_or(path));
        }
    }
    let mut found = BTreeSet::new();
    for library in libraries {
        let cfg = library.join("steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg");
        if cfg.is_dir() {
            found.insert(fs::canonicalize(&cfg).unwrap_or(cfg));
        }
    }
    found.into_iter().collect()
}

/// Put the config in every CS2 install; CS2 reads it when it starts.
pub fn install_cfg(port: u16) {
    let cfg = cfg_text(port);
    for folder in cfg_dirs() {
        let target = folder.join(CFG_NAME);
        if fs::read_to_string(&target).is_ok_and(|current| current == cfg) {
            continue;
        }
        match fs::write(&target, &cfg) {
            Ok(()) => {
                println!("Installed Game State Integration config in {} (restart CS2 once)", folder.display())
            }
            Err(error) => println!("Could not write {}: {error}", target.display()),
        }
    }
}

/// Listen on 127.0.0.1:`port`; every accepted post becomes an `Event::Game`.
pub fn start(port: u16, tx: Sender<Event>) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = handle(stream, &tx);
        }
    });
    Ok(())
}

fn handle(stream: TcpStream, tx: &Sender<Event>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let is_post = line.starts_with("POST ");
    let mut length = 0usize;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line.trim_end().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let status = if !is_post {
        "405 Method Not Allowed"
    } else if length > MAX_BODY {
        "413 Content Too Large"
    } else {
        let mut body = vec![0; length];
        reader.read_exact(&mut body)?;
        match accept(&body) {
            Some(data) => {
                if let Some(path) = std::env::var_os("NADECUE_DUMP") {
                    let _ = fs::write(path, &body);
                }
                let _ = tx.send(Event::Game(Box::new(Snapshot::from_gsi(&data))));
                "200 OK"
            }
            None => "403 Forbidden",
        }
    };
    (&stream)
        .write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())
}

/// The posted game state, if it is a JSON object carrying our token.
pub fn accept(body: &[u8]) -> Option<Value> {
    let data: Value = serde_json::from_slice(body).ok()?;
    (data.pointer("/auth/token")?.as_str()? == TOKEN).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_the_token() {
        assert!(accept(br#"{"auth":{"token":"nadecue"},"map":{}}"#).is_some());
        assert!(accept(br#"{"auth":{"token":"other"}}"#).is_none());
        assert!(accept(b"not json").is_none());
    }

    #[test]
    fn cfg_asks_for_position_and_weapons() {
        let cfg = cfg_text(4000);
        assert!(cfg.contains("http://127.0.0.1:4000/"));
        assert!(cfg.contains("\"player_position\"  \"1\""));
        assert!(cfg.contains("\"player_weapons\"   \"1\""));
    }

    #[test]
    fn serves_posts() {
        let (tx, rx) = std::sync::mpsc::channel();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        start(port, tx).unwrap();
        let body = r#"{"auth":{"token":"nadecue"},"map":{"name":"de_nuke"}}"#;
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut reply = String::new();
        s.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 200"));
        match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Game(snap) => assert_eq!(snap.map.as_deref(), Some("de_nuke")),
            _ => panic!("expected a game event"),
        }
    }
}
