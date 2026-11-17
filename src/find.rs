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
    /// Words naming where it should land ("CT" in "CT smoke from T spawn").
    pub land: Vec<String>,
    /// Words naming where it's thrown from (after "from", or a lone "T/CT spawn").
    pub origin: Vec<String>,
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
    ("de_cache", &["cache", "cash"]),
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
    "hey", "show", "me", "the", "a", "an", "for", "on", "of", "my", "lineup", "lineups", "line", "up",
    "please", "get", "give", "find", "pull", "open", "what's", "whats", "is", "how", "do", "i", "throw",
    "grenade", "nadecue", "side", "to", "into",
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
    let mut after_from = false;
    for w in words(text) {
        if let Some((map, _)) = MAPS.iter().find(|(_, names)| names.contains(&w.as_str())) {
            q.map = Some(map.to_string());
        } else if let Some((kind, _)) = KINDS.iter().find(|(_, names)| names.contains(&w.as_str())) {
            q.kind = Some(*kind);
        } else if w == "from" {
            after_from = true;
            q.words.push(w);
        } else if !FILLER.contains(&w.as_str()) {
            if after_from {
                q.origin.push(w.clone());
            } else {
                q.land.push(w.clone());
            }
            q.words.push(w);
        }
    }
    // No "from": a "T spawn" / "CT spawn" next to another place is where it's thrown from
    // ("smoke t spawn ct" = a CT smoke from T spawn).
    if q.origin.is_empty()
        && let Some(i) = q.land.windows(2).position(|p| (p[0] == "t" || p[0] == "ct") && p[1] == "spawn")
        && q.land.len() > 2
    {
        q.origin = q.land.drain(i..i + 2).collect();
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
    // Names read "<lands> from <thrown from>". A request word scores most in the matching part,
    // a little in the other part, and a note word ("stand in the corner against stairs") only a
    // little, so a note never makes a lineup look like a sure hit.
    let (land_part, origin_part) = lineup.name.split_once(" from ").unwrap_or((&lineup.name, ""));
    let mut land = words(land_part);
    let mut origin = words(origin_part);
    let mut note = words(&lineup.note);
    let side = lineup.side.to_lowercase();
    let take = |list: &mut Vec<String>, w: &str| -> bool {
        match list.iter().position(|h| close(w, h)) {
            Some(i) => {
                list.remove(i);
                true
            }
            None => false,
        }
    };
    let mut total = 0.0;
    let mut landed = false;
    for (w, wants_land) in q.land.iter().map(|w| (w, true)).chain(q.origin.iter().map(|w| (w, false))) {
        let (home, other) = if wants_land { (&mut land, &mut origin) } else { (&mut origin, &mut land) };
        total += if take(home, w) {
            landed |= wants_land;
            if wants_land { 1.5 } else { 1.0 }
        } else if take(other, w) {
            0.3
        } else if (!side.is_empty() && *w == side) || take(&mut note, w) {
            0.4
        } else {
            -0.2
        };
    }
    if landed {
        total += 1.2;
    }
    // Name words nobody asked for ("Deep" in "Deep Stairs") count a little against it.
    total -= 0.05 * (land.len() + origin.iter().filter(|w| *w != "from").count()) as f64;
    total
}

/// True when every name word was asked for and every content word of the request is in the name.
fn exact(q: &Query, lineup: &Lineup) -> bool {
    let mut name: Vec<String> = words(&lineup.name).into_iter().filter(|w| w != "from").collect();
    let asked: Vec<&String> = q.land.iter().chain(q.origin.iter()).collect();
    for w in &asked {
        match name.iter().position(|h| close(w, h)) {
            Some(i) => {
                name.remove(i);
            }
            None => return false,
        }
    }
    name.is_empty() && !asked.is_empty()
}

/// Sure enough to skip the language model: a clear winner over the best lineup with another name.
pub fn sure(q: &Query, ranked: &[(usize, f64)], lineups: &[Lineup]) -> bool {
    let Some(&(first, top)) = ranked.first() else { return false };
    if q.land.is_empty() && q.origin.is_empty() {
        return true; // just "mirage smoke": the most recent one
    }
    if exact(q, &lineups[first]) {
        return true;
    }
    let name = lineups[first].name.to_lowercase();
    let rival = ranked.iter().find(|(i, _)| lineups[*i].name.to_lowercase() != name).map(|(_, s)| *s);
    top >= 1.5 && rival.is_none_or(|r| top - r >= 0.4)
}

/// Lineups matching `q`, best first (ties: thrown most recently, then name).
pub fn rank(q: &Query, lineups: &[Lineup], used: &Usage) -> Vec<(usize, f64)> {
    let mut out: Vec<(usize, f64)> = lineups
        .iter()
        .enumerate()
        .filter(|(_, l)| q.kind.is_none_or(|k| l.kind == k))
        .map(|(i, l)| (i, score(q, l)))
        .collect();
    let content = !q.land.is_empty() || !q.origin.is_empty();
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
            ..Default::default()
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
        let q = parse("hey smoke t spawn ct mirage");
        assert_eq!(q.map.as_deref(), Some("de_mirage"));
        assert_eq!(q.kind, Some(Kind::Smoke));
        assert_eq!(q.words, ["t", "spawn", "ct"]);
        assert_eq!(
            (q.land.as_slice(), q.origin.as_slice()),
            (&["ct".to_string()][..], &["t".to_string(), "spawn".to_string()][..])
        );
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
        // Landing spot beats a throw spot with the same word: a CT smoke, not one thrown from CT.
        let real = [
            l("Palace from CT Spawn", Kind::Smoke, "CT", ""),
            l("CT from Palace Alley", Kind::Smoke, "", ""),
        ];
        assert_eq!(best("mirage ct smoke from t spawn", &real).as_deref(), Some("CT from Palace Alley"));
        let q = parse("mirage ct smoke from t spawn");
        assert!(sure(&q, &rank(&q, &real, &Usage::default()), &real));
        // An exact name beats a longer one with the same words.
        let stairs = [
            l("Deep Stairs from T Spawn", Kind::Smoke, "T", ""),
            l("Stairs from T Spawn", Kind::Smoke, "T", ""),
        ];
        assert_eq!(best("mirage stairs smoke from t spawn", &stairs).as_deref(), Some("Stairs from T Spawn"));
        let q = parse("mirage stairs smoke from t spawn");
        assert!(sure(&q, &rank(&q, &stairs, &Usage::default()), &stairs));
        // Just a grenade (and map): everything of that kind, most recent first.
        assert_eq!(rank(&parse("mirage flash"), &all, &Usage::default()).len(), 1);
    }
}
