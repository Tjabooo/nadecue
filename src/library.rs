//! Saved lineups: one JSON file (plus an optional aim picture) per lineup, one folder per map.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::snapshot::Kind;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lineup {
    pub name: String,
    pub map: String,
    pub kind: Kind,
    /// `T`, `CT` or empty for both.
    #[serde(default)]
    pub side: String,
    /// Where to stand (game units), when CS2 reported it (practice servers with spectating, demos).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f64; 3]>,
    /// Where to look (unit vector), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fwd: Option<[f64; 3]>,
    /// How to throw it: "Left click", "Jump throw", ...
    #[serde(default)]
    pub throw: String,
    /// Close-up of the crosshair (where to aim), next to the JSON file.
    #[serde(default)]
    pub image: String,
    /// The whole view when saved (where you stand and look from).
    #[serde(default)]
    pub view: String,
    /// Free text, e.g. "stand in the corner by the bin".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The JSON file this came from (not stored).
    #[serde(skip)]
    pub file: PathBuf,
}

impl Lineup {
    fn beside(&self, name: &str) -> Option<PathBuf> {
        (!name.is_empty()).then(|| self.file.with_file_name(name)).filter(|p| p.is_file())
    }

    pub fn image_path(&self) -> Option<PathBuf> {
        self.beside(&self.image)
    }

    pub fn view_path(&self) -> Option<PathBuf> {
        self.beside(&self.view)
    }

    /// `<map>/<file stem>`: the key usage is remembered under.
    pub fn key(&self) -> String {
        let stem = self.file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        format!("{}/{stem}", self.map)
    }
}

/// Lineups folder: `$NADECUE_DIR`, else `$XDG_DATA_HOME/nadecue/lineups`.
pub fn root() -> PathBuf {
    if let Some(dir) = std::env::var_os("NADECUE_DIR") {
        return PathBuf::from(dir);
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    data.join("nadecue/lineups")
}

/// Every lineup for `map`, sorted by name.
pub fn load(root: &Path, map: &str) -> Vec<Lineup> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root.join(map)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        match fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<Lineup>(&b).map_err(|e| e.to_string()))
        {
            Ok(mut lineup) => {
                lineup.file = path;
                out.push(lineup);
            }
            Err(error) => eprintln!("Skipping {}: {error}", path.display()),
        }
    }
    out.sort_by_key(|l| l.name.to_lowercase());
    out
}

/// A file-name-safe version of `name`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "lineup".into() } else { out }
}

/// A free `<slug>`, `<slug>-2`, ... in the map folder.
pub fn free_stem(dir: &Path, name: &str) -> String {
    let base = slug(name);
    let mut stem = base.clone();
    let mut n = 2;
    while dir.join(format!("{stem}.json")).exists() {
        stem = format!("{base}-{n}");
        n += 1;
    }
    stem
}

/// Write `lineup` (and its pictures, PNG bytes: crosshair close-up and whole view) to
/// `<root>/<map>/<stem>.json`.
pub fn save(root: &Path, lineup: &mut Lineup, pictures: Option<(&[u8], &[u8])>) -> std::io::Result<PathBuf> {
    let dir = root.join(&lineup.map);
    fs::create_dir_all(&dir)?;
    let stem = free_stem(&dir, &lineup.name);
    if let Some((aim, view)) = pictures {
        lineup.image = format!("{stem}.png");
        lineup.view = format!("{stem}.view.png");
        fs::write(dir.join(&lineup.image), aim)?;
        fs::write(dir.join(&lineup.view), view)?;
    }
    let file = dir.join(format!("{stem}.json"));
    fs::write(&file, serde_json::to_vec_pretty(lineup).map_err(std::io::Error::other)?)?;
    lineup.file = file.clone();
    Ok(file)
}

/// Delete a lineup and its picture.
pub fn forget(lineup: &Lineup) -> std::io::Result<()> {
    for picture in [lineup.image_path(), lineup.view_path()].into_iter().flatten() {
        let _ = fs::remove_file(picture);
    }
    fs::remove_file(&lineup.file)
}

/// When each lineup was last thrown (Unix seconds), by `Lineup::key`. Lives in
/// `$XDG_STATE_HOME/nadecue/used.json`; the one you threw last comes up first next time.
#[derive(Default)]
pub struct Usage {
    path: PathBuf,
    pub last: BTreeMap<String, u64>,
}

impl Usage {
    pub fn load() -> Usage {
        let path = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("nadecue/used.json");
        let last = fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Usage { path, last }
    }

    pub fn mark(&mut self, lineup: &Lineup) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.last.insert(lineup.key(), now);
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec(&self.last) {
            let _ = fs::write(&self.path, bytes);
        }
    }

    pub fn get(&self, lineup: &Lineup) -> u64 {
        self.last.get(&lineup.key()).copied().unwrap_or(0)
    }
}

/// How many lineups of `kind` exist for `map` (used to auto-name new ones).
pub fn count(root: &Path, map: &str, kind: Kind) -> usize {
    load(root, map).iter().filter(|l| l.kind == kind).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nadecue-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Window from T spawn!"), "window-from-t-spawn");
        assert_eq!(slug("  ??  "), "lineup");
        assert_eq!(slug("Jungle / Stairs"), "jungle-stairs");
    }

    #[test]
    fn round_trip() {
        let root = temp();
        let mut a = Lineup {
            name: "Window".into(),
            map: "de_mirage".into(),
            kind: Kind::Smoke,
            side: "T".into(),
            pos: Some([1.0, 2.0, 3.0]),
            fwd: None,
            throw: "Jump throw".into(),
            image: String::new(),
            view: String::new(),
            note: String::new(),
            file: PathBuf::new(),
        };
        save(&root, &mut a, Some((b"png", b"view"))).unwrap();
        assert_eq!(a.key(), "de_mirage/window");
        let mut b = a.clone();
        save(&root, &mut b, None).unwrap();
        assert!(b.file.ends_with("window-2.json"));
        let all = load(&root, "de_mirage");
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|l| l.image_path().is_some() && l.view_path().is_some()));
        assert!(!fs::read_to_string(&b.file).unwrap().contains("fwd"));
        assert_eq!(count(&root, "de_mirage", Kind::Smoke), 2);
        forget(&all[0]).unwrap();
        assert_eq!(load(&root, "de_mirage").len(), 1);
        let _ = fs::remove_dir_all(&root);
    }
}
