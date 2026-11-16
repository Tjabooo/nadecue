//! Which lineups fit the moment, and how to get from here onto one.

use crate::library::{Lineup, Usage};
use crate::snapshot::{Kind, Snapshot};

/// Close enough to the saved spot to throw (game units; a player is 32 wide).
pub const ON_SPOT: f64 = 14.0;
/// Aim closer than this (degrees) counts as matched. GSI reports the view vector with two
/// decimals, so finer than ~0.6° can't be told apart anyway: the picture does the rest.
pub const AIM_OK: f64 = 1.0;
/// Game units to metres (1 unit is one inch).
pub const METRES: f64 = 0.0254;

/// Lineups that fit what we hold, as indexes into `lineups`. `kind` None = every kind (browsing).
///
/// CS2 only reports the player's position to spectators, so in a real match the order is
/// "thrown most recently first" (people throw the same lineups for their role every round),
/// then by name. When a position is known (both ours and the lineup's), nearby ones come first
/// and ones further than `radius` are left out.
pub fn candidates(
    lineups: &[Lineup],
    snap: &Snapshot,
    kind: Option<Kind>,
    radius: f64,
    used: &Usage,
) -> Vec<usize> {
    let mut out: Vec<(usize, Option<f64>, u64)> = lineups
        .iter()
        .enumerate()
        .filter(|(_, l)| kind.is_none_or(|k| l.kind == k))
        .filter(|(_, l)| {
            l.side.is_empty() || snap.side.as_ref().is_none_or(|s| s.eq_ignore_ascii_case(&l.side))
        })
        .map(|(i, l)| (i, snap.pos.zip(l.pos).map(|(a, b)| distance(a, b)), used.get(l)))
        .filter(|(_, d, _)| d.is_none_or(|d| d <= radius))
        .collect();
    // Stable sort: lineups arrive sorted by name, which breaks ties.
    out.sort_by(|a, b| match (a.1, b.1) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.2.cmp(&a.2),
    });
    out.into_iter().map(|(i, _, _)| i).collect()
}

pub fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Yaw in degrees (counter-clockwise from +x, like the game's).
pub fn yaw(f: [f64; 3]) -> f64 {
    f[1].atan2(f[0]).to_degrees()
}

/// Elevation in degrees (up is positive).
pub fn elevation(f: [f64; 3]) -> f64 {
    f[2].clamp(-1.0, 1.0).asin().to_degrees()
}

/// Wrap an angle into (-180, 180].
pub fn wrap(mut deg: f64) -> f64 {
    while deg > 180.0 {
        deg -= 360.0;
    }
    while deg <= -180.0 {
        deg += 360.0;
    }
    deg
}

/// Screen arrow for a direction `rel` degrees left of where we look.
pub fn arrow(rel: f64) -> &'static str {
    const ARROWS: [&str; 8] = ["↑", "↖", "←", "↙", "↓", "↘", "→", "↗"];
    let sector = (wrap(rel) / 45.0).round() as i64;
    ARROWS[sector.rem_euclid(8) as usize]
}

#[derive(Clone, Debug, PartialEq)]
pub enum Guide {
    /// Walk this way (arrow) this far (metres).
    Walk { arrow: &'static str, metres: f64 },
    /// Standing on it: aim `up` degrees up and `left` degrees left (negative = down/right).
    Aim { up: f64, left: f64 },
    /// No position from the game: just show the picture.
    Unknown,
}

pub fn guide(snap: &Snapshot, lineup: &Lineup) -> Guide {
    let (Some(pos), Some(fwd), Some(spot)) = (snap.pos, snap.fwd, lineup.pos) else {
        return Guide::Unknown;
    };
    if distance(pos, spot) <= ON_SPOT {
        return match lineup.fwd {
            Some(aim) => Guide::Aim { up: elevation(aim) - elevation(fwd), left: wrap(yaw(aim) - yaw(fwd)) },
            None => Guide::Aim { up: 0.0, left: 0.0 },
        };
    }
    let bearing = (spot[1] - pos[1]).atan2(spot[0] - pos[0]).to_degrees();
    let flat = ((spot[0] - pos[0]).powi(2) + (spot[1] - pos[1]).powi(2)).sqrt();
    Guide::Walk { arrow: arrow(bearing - yaw(fwd)), metres: flat * METRES }
}

/// The aim hint text: "↑ 2.5°  ← 1.0°", or None once it matches.
pub fn aim_text(up: f64, left: f64) -> Option<String> {
    let mut parts = Vec::new();
    if up.abs() >= AIM_OK {
        parts.push(format!("{} {:.1}°", if up > 0.0 { "↑" } else { "↓" }, up.abs()));
    }
    if left.abs() >= AIM_OK {
        parts.push(format!("{} {:.1}°", if left > 0.0 { "←" } else { "→" }, left.abs()));
    }
    (!parts.is_empty()).then(|| parts.join("  "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn lineup(name: &str, kind: Kind, side: &str, pos: [f64; 3], fwd: [f64; 3]) -> Lineup {
        Lineup {
            name: name.into(),
            map: "de_test".into(),
            kind,
            side: side.into(),
            pos: Some(pos),
            fwd: Some(fwd),
            throw: String::new(),
            image: String::new(),
            view: String::new(),
            note: String::new(),
            file: PathBuf::from(format!("/x/{name}.json")),
            ..Default::default()
        }
    }

    fn snap(pos: [f64; 3], fwd: [f64; 3]) -> Snapshot {
        Snapshot {
            side: Some("T".into()),
            me: true,
            alive: true,
            pos: Some(pos),
            fwd: Some(fwd),
            ..Default::default()
        }
    }

    #[test]
    fn arrows_follow_the_view() {
        let east = [1.0, 0.0, 0.0];
        let at = |target: [f64; 3]| match guide(
            &snap([0.0; 3], east),
            &lineup("x", Kind::Smoke, "", target, east),
        ) {
            Guide::Walk { arrow, .. } => arrow,
            other => panic!("{other:?}"),
        };
        assert_eq!(at([100.0, 0.0, 0.0]), "↑");
        assert_eq!(at([0.0, 100.0, 0.0]), "←"); // +y is to the left when facing +x
        assert_eq!(at([0.0, -100.0, 0.0]), "→");
        assert_eq!(at([-100.0, 0.0, 0.0]), "↓");
        assert_eq!(at([100.0, 100.0, 0.0]), "↖");
    }

    #[test]
    fn on_spot_gives_aim_deltas() {
        let target = [0.0, 1.0, 0.0]; // look north
        let me = [0.9962, 0.0872, 0.0]; // ~5° left of east
        match guide(&snap([3.0, 0.0, 0.0], me), &lineup("x", Kind::Smoke, "", [0.0; 3], target)) {
            Guide::Aim { up, left } => {
                assert!(up.abs() < 0.01);
                assert!((left - 85.0).abs() < 0.1, "{left}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(aim_text(0.3, -0.2), None);
        assert_eq!(aim_text(2.04, -1.5).as_deref(), Some("↑ 2.0°  → 1.5°"));
    }

    #[test]
    fn candidates_filter_and_sort() {
        let f = [1.0, 0.0, 0.0];
        let all = [
            lineup("far", Kind::Smoke, "T", [5000.0, 0.0, 0.0], f),
            lineup("near", Kind::Smoke, "", [100.0, 0.0, 0.0], f),
            lineup("mid", Kind::Smoke, "T", [400.0, 0.0, 0.0], f),
            lineup("ct", Kind::Smoke, "CT", [10.0, 0.0, 0.0], f),
            lineup("flash", Kind::Flash, "T", [10.0, 0.0, 0.0], f),
        ];
        let s = snap([0.0; 3], f);
        let names = |c: Vec<usize>| c.iter().map(|i| all[*i].name.as_str()).collect::<Vec<_>>();
        let mut used = Usage::default();
        assert_eq!(names(candidates(&all, &s, Some(Kind::Smoke), 1200.0, &used)), ["near", "mid"]);
        assert_eq!(names(candidates(&all, &s, None, f64::INFINITY, &used)), ["flash", "near", "mid", "far"]);
        // A real match: no position, so last thrown first, then the list order.
        let blind = Snapshot { pos: None, ..s };
        used.last.insert(all[2].key(), 50);
        used.last.insert(all[0].key(), 10);
        assert_eq!(names(candidates(&all, &blind, Some(Kind::Smoke), 1200.0, &used)), ["mid", "far", "near"]);
    }
}
