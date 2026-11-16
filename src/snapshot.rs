//! The parts of a CS2 Game State Integration post that nadecue cares about.

use serde_json::Value;

/// Grenade families. Molotov and incendiary share lineups, so they are one kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Smoke,
    Flash,
    He,
    Molotov,
    Decoy,
}

impl Kind {
    /// The kind of a GSI weapon name such as `weapon_smokegrenade`.
    pub fn from_weapon(name: &str) -> Option<Kind> {
        Some(match name.strip_prefix("weapon_").unwrap_or(name) {
            "smokegrenade" => Kind::Smoke,
            "flashbang" => Kind::Flash,
            "hegrenade" => Kind::He,
            "molotov" | "incgrenade" => Kind::Molotov,
            "decoy" => Kind::Decoy,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Smoke => "SMOKE",
            Kind::Flash => "FLASH",
            Kind::He => "HE",
            Kind::Molotov => "MOLLY",
            Kind::Decoy => "DECOY",
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Kind::Smoke => "Smoke",
            Kind::Flash => "Flash",
            Kind::He => "HE",
            Kind::Molotov => "Molly",
            Kind::Decoy => "Decoy",
        }
    }
}

/// What the local player is doing right now.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Snapshot {
    /// Map file name, e.g. `de_mirage`.
    pub map: Option<String>,
    /// `T` or `CT`.
    pub side: Option<String>,
    /// True when the player block is us (not someone we're spectating).
    pub me: bool,
    pub alive: bool,
    /// The grenade in hand, if any.
    pub held: Option<Kind>,
    /// How many grenades of each kind we carry (a throw lowers it).
    pub nades: usize,
    /// World position in game units (1 unit ≈ 1 inch).
    pub pos: Option<[f64; 3]>,
    /// Unit view vector.
    pub fwd: Option<[f64; 3]>,
}

fn vec3(value: &Value) -> Option<[f64; 3]> {
    let mut parts = value.as_str()?.split(',').map(|p| p.trim().parse::<f64>());
    let v = [parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?];
    parts.next().is_none().then_some(v)
}

impl Snapshot {
    pub fn from_gsi(data: &Value) -> Snapshot {
        let player = &data["player"];
        let me = match (data["provider"]["steamid"].as_str(), player["steamid"].as_str()) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        };
        let map = data["map"]["name"].as_str().filter(|m| !m.is_empty()).map(|m| {
            // Workshop maps arrive as "workshop/123/de_name".
            m.rsplit('/').next().unwrap_or(m).to_string()
        });
        let alive = player["state"]["health"].as_i64().is_some_and(|h| h > 0);
        let mut held = None;
        let mut nades = 0;
        if let Some(weapons) = player["weapons"].as_object() {
            for weapon in weapons.values() {
                let Some(kind) = weapon["name"].as_str().and_then(Kind::from_weapon) else {
                    continue;
                };
                nades += weapon["ammo_reserve"].as_u64().unwrap_or(1).max(1) as usize;
                if weapon["state"].as_str() == Some("active") {
                    held = Some(kind);
                }
            }
        }
        Snapshot {
            map,
            side: player["team"].as_str().map(str::to_string),
            me,
            alive,
            held,
            nades,
            pos: vec3(&player["position"]),
            fwd: vec3(&player["forward"]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_a_live_post() {
        let data = json!({
            "provider": {"steamid": "1"},
            "map": {"name": "de_mirage"},
            "player": {
                "steamid": "1", "team": "T",
                "state": {"health": 100},
                "position": "-1102.21, -500.03, -167.97",
                "forward": "0.71, 0.70, -0.06",
                "weapons": {
                    "weapon_0": {"name": "weapon_knife_t", "state": "holstered"},
                    "weapon_1": {"name": "weapon_flashbang", "ammo_reserve": 2, "state": "holstered"},
                    "weapon_2": {"name": "weapon_smokegrenade", "ammo_reserve": 1, "state": "active"}
                }
            }
        });
        let s = Snapshot::from_gsi(&data);
        assert_eq!(s.map.as_deref(), Some("de_mirage"));
        assert!(s.me && s.alive);
        assert_eq!(s.held, Some(Kind::Smoke));
        assert_eq!(s.nades, 3);
        assert_eq!(s.pos, Some([-1102.21, -500.03, -167.97]));
        assert_eq!(s.fwd, Some([0.71, 0.70, -0.06]));
    }

    #[test]
    fn spectating_is_not_me() {
        let data = json!({"provider": {"steamid": "1"}, "player": {"steamid": "2"}});
        assert!(!Snapshot::from_gsi(&data).me);
    }

    #[test]
    fn workshop_maps_and_incendiaries() {
        let data = json!({"map": {"name": "workshop/123/de_ancient"}});
        assert_eq!(Snapshot::from_gsi(&data).map.as_deref(), Some("de_ancient"));
        assert_eq!(Kind::from_weapon("weapon_incgrenade"), Some(Kind::Molotov));
        assert_eq!(Kind::from_weapon("weapon_ak47"), None);
    }
}
