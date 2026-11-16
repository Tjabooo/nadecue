//! `~/.config/nadecue/config.toml`: a few `key = value` lines, all optional.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Port CS2 posts its game state to.
    pub port: u16,
    /// Output to screenshot when saving a lineup (grim `-o`). Empty = the focused one.
    pub output: String,
    /// Screen to open the card on (`eww open --screen`). Empty = eww's default.
    pub screen: String,
    /// Show lineups within this many metres when a grenade comes out.
    pub radius_m: f64,
    /// Browse mode (SUPER+ALT+U with nothing in hand) closes after this many seconds.
    pub browse_secs: u64,
    /// The eww config folder holding the card.
    pub eww_dir: PathBuf,
    /// Throw styles offered when saving.
    pub throws: Vec<String>,
    /// CS2's console port (`-netconport`), used by `nadecue shoot`.
    pub netcon_port: u16,
    /// Wait after each teleport before the screenshot (`shoot`).
    pub shoot_settle_ms: u64,
    /// OpenAI-compatible endpoint (http://host:port) that helps `find` when word matching isn't sure.
    pub llm_url: String,
    pub llm_model: String,
    /// Extra JSON fields merged into the request, e.g. `{"chat_template_kwargs": {"enable_thinking": false}}`.
    pub llm_extra: Option<serde_json::Value>,
    pub llm_timeout_secs: u64,
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("nadecue")
}

impl Default for Config {
    fn default() -> Self {
        Config {
            port: 3714,
            output: String::new(),
            screen: String::new(),
            radius_m: 30.0,
            browse_secs: 15,
            eww_dir: config_dir().join("eww"),
            throws: [
                "Left click",
                "Jump throw",
                "Run + jump throw",
                "W + jump throw",
                "Right click",
                "Left + right click",
                "Crouch + left click",
            ]
            .map(String::from)
            .to_vec(),
            netcon_port: 2121,
            shoot_settle_ms: 700,
            llm_url: String::new(),
            llm_model: String::new(),
            llm_extra: None,
            llm_timeout_secs: 15,
        }
    }
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v).to_string()
}

impl Config {
    pub fn load() -> Config {
        let text = std::fs::read_to_string(config_dir().join("config.toml")).unwrap_or_default();
        Config::parse(&text)
    }

    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        for line in text.lines() {
            let line = line.split(" #").next().unwrap_or("").trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "port" => c.port = value.parse().unwrap_or(c.port),
                "output" => c.output = unquote(value),
                "screen" => c.screen = unquote(value),
                "radius_m" => c.radius_m = value.parse().unwrap_or(c.radius_m),
                "browse_secs" => c.browse_secs = value.parse().unwrap_or(c.browse_secs),
                "eww_dir" => {
                    let v = unquote(value);
                    c.eww_dir = match (v.strip_prefix("~/"), std::env::var_os("HOME")) {
                        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
                        _ => PathBuf::from(v),
                    };
                }
                "netcon_port" => c.netcon_port = value.parse().unwrap_or(c.netcon_port),
                "shoot_settle_ms" => c.shoot_settle_ms = value.parse().unwrap_or(c.shoot_settle_ms),
                "llm_url" => c.llm_url = unquote(value),
                "llm_model" => c.llm_model = unquote(value),
                "llm_extra" => c.llm_extra = serde_json::from_str(value).ok(),
                "llm_timeout_secs" => c.llm_timeout_secs = value.parse().unwrap_or(c.llm_timeout_secs),
                "throws" => {
                    let list = value.trim_start_matches('[').trim_end_matches(']');
                    let throws: Vec<String> =
                        list.split(',').map(unquote).filter(|t| !t.is_empty()).collect();
                    if !throws.is_empty() {
                        c.throws = throws;
                    }
                }
                _ => {}
            }
        }
        c
    }

    /// Radius in game units.
    pub fn radius(&self) -> f64 {
        self.radius_m / crate::pick::METRES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_example() {
        let c = Config::parse(
            "# comment\nport = 4000\noutput = \"DP-3\"  # main\nscreen = \"XG27AQDMG\"\nradius_m = 12.5\nthrows = [\"A\", \"B\"]\n",
        );
        assert_eq!(c.port, 4000);
        assert_eq!(c.output, "DP-3");
        assert_eq!(c.screen, "XG27AQDMG");
        assert_eq!(c.radius_m, 12.5);
        assert_eq!(c.throws, ["A", "B"]);
        assert_eq!(Config::parse("").port, 3714);
    }
}
