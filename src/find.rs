//! Free-text lookup: "smoke t spawn ct mirage", "mirage ct smoke from t spawn", "window smoke".
//! Word order doesn't matter; map, side and grenade words narrow the search, the rest is matched
//! (loosely, speech-to-text spells things oddly) against lineup names and notes.

use crate::library::{Lineup, Usage};
use crate::snapshot::Kind;

/// What a request asks for.
#[derive(Debug, Default, PartialEq)]
pub struct Query {
    pub map: Option<String>,
    pub kind: Option<Kind>,
    /// The words left to match against names and notes (sides stay in here: "t spawn").
    pub words: Vec<String>,
}

const MAPS: &[(&str, &[&str])] = &[
    ("de_mirage", &["mirage", "mirrage", "mirage's"]),
    ("de_inferno", &["inferno"]),
    ("de_dust2", &["dust2", "dust", "d2"]),
    ("de_nuke", &["nuke"]),
    ("de_ancient", &["ancient"]),
    ("de_anubis", &["anubis"]),
    ("de_vertigo", &["vertigo"]),
    ("de_overpass", &["overpass"]),
    ("de_train", &["train"]),
    ("cs_office", &["office"]),
    ("cs_italy", &["italy"]),
];

const KINDS: &[(Kind, &[&str])] = &[
    (Kind::Smoke, &["smoke", "smokes", "smoked", "smog"]),
    (Kind::Flash, &["flash", "flashes", "flashbang", "flashbangs", "popflash"]),
    (Kind::He, &["he", "nade", "frag", "hegrenade"]),
    (
        Kind::Molotov,
        &["molly", "mollies", "molotov", "molotovs", "molo", "incendiary", "incend", "inc", "fire"],
    ),
    (Kind::Decoy, &["decoy", "decoys"]),
];

/// Words that carry no meaning in a request.
const FILLER: &[&str] = &[
    "magi", "maggie", "show", "me", "the", "a", "an", "for", "on", "of", "my", "lineup", "lineups", "line",
    "up", "please", "get", "give", "find", "pull", "open", "what's", "whats", "is", "how", "do", "i",
    "throw", "grenade", "nadecue", "side", "to", "into",
];

/// Lowercase words, with speech-to-text spellings folded: "C.T." → ct, "tea" → t, "dust two" → dust2.
pub fn words(text: &str) -> Vec<String> {
    let cleaned: String =
        text.to_lowercase().chars().map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' }).collect();
    let raw: Vec<&str> = cleaned.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let (w, next) = (raw[i], raw.get(i + 1).copied());
        let joined = match (w, next) {
            ("c" | "see" | "sea", Some("t" | "tea" | "tee")) => Some("ct"),
            ("dust", Some("2" | "two" | "to" | "too")) => Some("dust2"),
            ("he", Some("grenade" | "nade")) => Some("he"),
            ("pop", Some("flash")) => Some("popflash"),
            _ => None,
        };
        if let Some(j) = joined {
            out.push(j.into());
            i += 2;
            continue;
        }
        out.push(
            match w {
                "tea" | "tee" | "tees" | "ts" | "terrorist" | "terrorists" => "t",
                "cts" | "city" | "counter" => "ct",
                "dust2's" => "dust2",
                other => other,
            }
            .to_string(),
        );
        i += 1;
    }
    // "counter terrorist" → ct (the "counter" already became ct)
    out.dedup_by(|b, a| a == "ct" && (b == "terrorist" || b == "t"));
    out
}

pub fn parse(text: &str) -> Query {
    let mut q = Query::default();
    for w in words(text) {
        if let Some((map, _)) = MAPS.iter().find(|(_, names)| names.contains(&w.as_str())) {
            q.map = Some(map.to_string());
        } else if let Some((kind, _)) = KINDS.iter().find(|(_, names)| names.contains(&w.as_str())) {
            q.kind = Some(*kind);
        } else if !FILLER.contains(&w.as_str()) {
            // "from" stays: it tells "CT from T spawn" apart a little, at a low weight.
            q.words.push(w);
        }
    }
    q
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn close(a: &str, b: &str) -> bool {
    a == b
        || (a.len() >= 4 && b.len() >= 4 && (a.starts_with(b) || b.starts_with(a) || edit_distance(a, b) <= 1))
        // plurals: "stairs" / "stair"
        || a.trim_end_matches('s') == b.trim_end_matches('s') && a.len() > 2
}

/// How well `lineup` fits the words (higher is better). Side counts as a word of the lineup.
pub fn score(q: &Query, lineup: &Lineup) -> f64 {
    let mut have = words(&format!("{} {}", lineup.name, lineup.note));
    let side = lineup.side.to_lowercase();
    let mut total = 0.0;
    for w in &q.words {
        if let Some(pos) = have.iter().position(|h| close(w, h)) {
            // Each lineup word answers one request word: "t ... t" needs two t's.
            have.remove(pos);
            total += if w == "from" { 0.25 } else { 1.0 };
        } else if !side.is_empty() && *w == side {
            total += 0.5;
        } else if w != "from" {
            total -= 0.2;
        }
    }
    total
}

/// Lineups matching `q`, best first (ties: thrown most recently, then name).
pub fn rank(q: &Query, lineups: &[Lineup], used: &Usage) -> Vec<(usize, f64)> {
    let mut out: Vec<(usize, f64)> = lineups
        .iter()
        .enumerate()
        .filter(|(_, l)| q.kind.is_none_or(|k| l.kind == k))
        .map(|(i, l)| (i, score(q, l)))
        .collect();
    let content = q.words.iter().any(|w| w != "from");
    if content {
        out.retain(|(_, s)| *s > 0.0);
    }
    out.sort_by(|a, b| {
        b.1.total_cmp(&a.1).then_with(|| used.get(&lineups[b.0]).cmp(&used.get(&lineups[a.0])))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn l(name: &str, kind: Kind, side: &str, note: &str) -> Lineup {
        Lineup {
            name: name.into(),
            map: "de_mirage".into(),
            kind,
            side: side.into(),
            pos: None,
            fwd: None,
            throw: String::new(),
            image: String::new(),
            view: String::new(),
            note: note.into(),
            file: PathBuf::from(format!("/x/{name}.json")),
        }
    }

    fn best(text: &str, all: &[Lineup]) -> Option<String> {
        let q = parse(text);
        rank(&q, all, &Usage::default()).first().map(|(i, _)| all[*i].name.clone())
    }

    #[test]
    fn spoken_variants() {
        assert_eq!(words("Mirage, C.T. smoke from T-spawn"), ["mirage", "ct", "smoke", "from", "t", "spawn"]);
        assert_eq!(words("see tea smoke dust two"), ["ct", "smoke", "dust2"]);
        assert_eq!(words("counter terrorist flash"), ["ct", "flash"]);
        let q = parse("magi smoke t spawn ct mirage");
        assert_eq!(q.map.as_deref(), Some("de_mirage"));
        assert_eq!(q.kind, Some(Kind::Smoke));
        assert_eq!(q.words, ["t", "spawn", "ct"]);
    }

    #[test]
    fn picks_the_right_smoke() {
        let all = [
            l("CT from T spawn", Kind::Smoke, "T", ""),
            l("Window from T spawn", Kind::Smoke, "T", ""),
            l("Jungle", Kind::Smoke, "T", "from top mid"),
            l("Stairs", Kind::Smoke, "T", ""),
            l("CT flash", Kind::Flash, "T", ""),
            l("Connector", Kind::Smoke, "CT", "retake from jungle"),
        ];
        assert_eq!(best("smoke t spawn ct mirage", &all).as_deref(), Some("CT from T spawn"));
        assert_eq!(best("mirage ct smoke from t spawn", &all).as_deref(), Some("CT from T spawn"));
        assert_eq!(best("window smoke", &all).as_deref(), Some("Window from T spawn"));
        assert_eq!(best("smoke window from tea spawn", &all).as_deref(), Some("Window from T spawn"));
        assert_eq!(best("jungle smoke top mid", &all).as_deref(), Some("Jungle"));
        assert_eq!(best("stair smoke", &all).as_deref(), Some("Stairs"));
        assert_eq!(best("ct flash", &all).as_deref(), Some("CT flash"));
        assert_eq!(best("conector smoke", &all).as_deref(), Some("Connector"));
        assert_eq!(best("smoke banana", &all), None);
        // Just a grenade (and map): everything of that kind, most recent first.
        assert_eq!(rank(&parse("mirage flash"), &all, &Usage::default()).len(), 1);
    }
}
