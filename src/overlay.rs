//! The card on screen: an eww window driven with `eww update` / `open` / `close`.
//! Only changed values are sent, and the window only exists while a card is up, so nothing sits
//! over the game the rest of the time.

use std::collections::BTreeMap;
use std::process::{Command, Stdio};

use crate::config::Config;

pub const WINDOW: &str = "nadecue";

/// What the card shows. Field names are the eww variables (with an `nc_` prefix).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub kind: String,
    pub name: String,
    pub side: String,
    pub img: String,
    pub arrow: String,
    pub line: String,
    /// `walk`, `spot`, `ready` or `blind` (no position): picks the guide row's colour.
    pub state: String,
    pub throw: String,
    pub count: String,
    /// Whole-view picture (where you stand).
    pub view: String,
    pub note: String,
    /// JSON list of `{name, on}` for the strip of other lineups.
    pub list: String,
}

impl Card {
    fn vars(&self) -> [(&'static str, &str); 12] {
        [
            ("nc_kind", &self.kind),
            ("nc_name", &self.name),
            ("nc_side", &self.side),
            ("nc_img", &self.img),
            ("nc_arrow", &self.arrow),
            ("nc_line", &self.line),
            ("nc_state", &self.state),
            ("nc_throw", &self.throw),
            ("nc_count", &self.count),
            ("nc_view", &self.view),
            ("nc_note", &self.note),
            ("nc_list", &self.list),
        ]
    }
}

pub struct Overlay {
    dir: String,
    screen: String,
    open: bool,
    sent: BTreeMap<&'static str, String>,
}

impl Overlay {
    pub fn new(cfg: &Config) -> Overlay {
        Overlay {
            dir: cfg.eww_dir.to_string_lossy().into_owned(),
            screen: cfg.screen.clone(),
            open: false,
            sent: BTreeMap::new(),
        }
    }

    fn eww(&self, args: &[&str]) -> bool {
        Command::new("eww")
            .arg("-c")
            .arg(&self.dir)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Start the eww daemon ahead of time so the first card opens fast.
    pub fn warm_up(&mut self) {
        let _ = Command::new("eww")
            .arg("-c")
            .arg(&self.dir)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // A card left over from a previous run.
        self.eww(&["close", WINDOW]);
    }

    pub fn show(&mut self, card: &Card) {
        let changed: Vec<String> = card
            .vars()
            .into_iter()
            .filter(|(k, v)| self.sent.get(k).is_none_or(|old| old != v))
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        if !changed.is_empty() {
            let mut args = vec!["update"];
            args.extend(changed.iter().map(String::as_str));
            if self.eww(&args) {
                for (k, v) in card.vars() {
                    self.sent.insert(k, v.to_string());
                }
            } else {
                self.sent.clear();
            }
        }
        if !self.open {
            let mut args = vec!["open", WINDOW];
            if !self.screen.is_empty() {
                args.extend(["--screen", &self.screen]);
            }
            self.open = self.eww(&args);
        }
    }

    pub fn hide(&mut self) {
        if self.open {
            self.eww(&["close", WINDOW]);
            self.open = false;
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }
}
