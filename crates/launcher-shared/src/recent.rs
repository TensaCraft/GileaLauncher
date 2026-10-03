//! Home's «Продовжити гру»: the builds played last, each with its last activity (a server or a
//! world), a server's status as the game's multiplayer list shows it, and where a launch goes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The builds «Продовжити гру» shows at most, and unless the user chose otherwise.
pub const RECENT_MOST: u8 = 10;
pub const RECENT_DEFAULT: u8 = 0;

pub(crate) fn recent_default() -> u8 {
    RECENT_DEFAULT
}

/// Where a launch takes the player: straight into a server or a world (for this launch only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Join {
    Server {
        host: String,
        port: u16,
    },
    /// A world by its folder in `saves/`.
    World {
        folder: String,
    },
}

/// What a build was last played on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Activity {
    Server {
        host: String,
        port: u16,
    },
    World {
        folder: String,
        name: String,
        /// `survival`, `creative`, `adventure` or `spectator`.
        mode: String,
        /// `peaceful`, `easy`, `normal` or `hard`; empty when the world does not say.
        difficulty: String,
        hardcore: bool,
        /// Its `icon.png` as a `data:` URL.
        icon: Option<String>,
        /// Whether the game can open it straight from the launcher (1.20 and later).
        quick_play: bool,
    },
}

impl Activity {
    /// Where «Грати» takes the player.
    pub fn join(&self) -> Join {
        match self {
            Activity::Server { host, port } => Join::Server { host: host.clone(), port: *port },
            Activity::World { folder, .. } => Join::World { folder: folder.clone() },
        }
    }
}

/// A build on «Продовжити гру».
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentBuild {
    pub key: String,
    /// When it was last played (ms since the epoch).
    pub played_ms: u64,
    pub activity: Option<Activity>,
}

/// A server as its status answer gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerStatus {
    pub motd: Vec<MotdSpan>,
    pub online: u32,
    pub max: u32,
    pub version: String,
    /// Its favicon as a `data:image/png;base64,…` URL.
    pub favicon: Option<String>,
    pub ping_ms: u32,
}

/// A piece of a MOTD with one look.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotdSpan {
    pub text: String,
    /// `#rrggbb`, or `None` for the default colour.
    pub color: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub underlined: bool,
    pub strikethrough: bool,
}

/// The sixteen colours of the game's `§` codes, by code and by name.
const COLORS: [(char, &str, &str); 16] = [
    ('0', "black", "#000000"),
    ('1', "dark_blue", "#0000aa"),
    ('2', "dark_green", "#00aa00"),
    ('3', "dark_aqua", "#00aaaa"),
    ('4', "dark_red", "#aa0000"),
    ('5', "dark_purple", "#aa00aa"),
    ('6', "gold", "#ffaa00"),
    ('7', "gray", "#aaaaaa"),
    ('8', "dark_gray", "#555555"),
    ('9', "blue", "#5555ff"),
    ('a', "green", "#55ff55"),
    ('b', "aqua", "#55ffff"),
    ('c', "red", "#ff5555"),
    ('d', "light_purple", "#ff55ff"),
    ('e', "yellow", "#ffff55"),
    ('f', "white", "#ffffff"),
];

/// The look a span takes on; `§` codes and chat components change it.
#[derive(Clone, Default)]
struct Style {
    color: Option<String>,
    bold: bool,
    italic: bool,
    underlined: bool,
    strikethrough: bool,
}

impl Style {
    fn span(&self, text: String) -> MotdSpan {
        MotdSpan {
            text,
            color: self.color.clone(),
            bold: self.bold,
            italic: self.italic,
            underlined: self.underlined,
            strikethrough: self.strikethrough,
        }
    }
}

/// A colour a chat component names: one of the sixteen names, or `#rrggbb`.
fn named_color(name: &str) -> Option<String> {
    if name.len() == 7 && name.starts_with('#') && name[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(name.to_ascii_lowercase());
    }
    COLORS.iter().find(|(_, n, _)| *n == name).map(|(_, _, hex)| hex.to_string())
}

/// `text` with its `§` codes read, starting from `style`, into `out`.
fn legacy(text: &str, start: &Style, out: &mut Vec<MotdSpan>) {
    let mut style = start.clone();
    let mut piece = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '§' {
            piece.push(c);
            continue;
        }
        let Some(code) = chars.next().map(|c| c.to_ascii_lowercase()) else { break };
        if !piece.is_empty() {
            out.push(style.span(std::mem::take(&mut piece)));
        }
        match code {
            'l' => style.bold = true,
            'o' => style.italic = true,
            'n' => style.underlined = true,
            'm' => style.strikethrough = true,
            'r' => style = start.clone(),
            'k' => {}
            _ => {
                if let Some((_, _, hex)) = COLORS.iter().find(|(c, _, _)| *c == code) {
                    // A colour code resets the formatting, as the game does.
                    style = Style { color: Some(hex.to_string()), ..Style::default() };
                }
            }
        }
    }
    if !piece.is_empty() {
        out.push(style.span(piece));
    }
}

/// A chat component (a string, a list or an object with `text`/`extra`) into `out`.
fn component(value: &Value, parent: &Style, out: &mut Vec<MotdSpan>) {
    match value {
        Value::String(text) => legacy(text, parent, out),
        Value::Array(parts) => parts.iter().for_each(|part| component(part, parent, out)),
        Value::Object(map) => {
            let mut style = parent.clone();
            if let Some(color) = map.get("color").and_then(Value::as_str).and_then(named_color) {
                style.color = Some(color);
            }
            let flag = |key: &str, now: bool| map.get(key).and_then(Value::as_bool).unwrap_or(now);
            style.bold = flag("bold", style.bold);
            style.italic = flag("italic", style.italic);
            style.underlined = flag("underlined", style.underlined);
            style.strikethrough = flag("strikethrough", style.strikethrough);
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                legacy(text, &style, out);
            }
            if let Some(extra) = map.get("extra") {
                component(extra, &style, out);
            }
        }
        _ => {}
    }
}

/// The MOTD a status answer's `description` gives (a string with `§` codes or a chat component),
/// as spans with one look each; neighbours with the same look are joined.
pub fn parse_motd(description: &Value) -> Vec<MotdSpan> {
    let mut spans = Vec::new();
    component(description, &Style::default(), &mut spans);
    let mut joined: Vec<MotdSpan> = Vec::new();
    for span in spans {
        match joined.last_mut() {
            Some(last)
                if MotdSpan { text: String::new(), ..last.clone() }
                    == MotdSpan { text: String::new(), ..span.clone() } =>
            {
                last.text.push_str(&span.text)
            }
            _ => joined.push(span),
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn plain(spans: &[MotdSpan]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn legacy_codes_colour_and_format_the_motd() {
        let spans = parse_motd(&json!("§eAeronautics §lсезон 3§r\n§bласкаво просимо"));
        assert_eq!(plain(&spans), "Aeronautics сезон 3\nласкаво просимо");
        assert_eq!(
            spans[0],
            MotdSpan { text: "Aeronautics ".into(), color: Some("#ffff55".into()), ..MotdSpan::default() }
        );
        assert_eq!(
            (spans[1].text.as_str(), spans[1].bold, spans[1].color.as_deref()),
            ("сезон 3", true, Some("#ffff55"))
        );
        assert_eq!(spans.last().unwrap().color.as_deref(), Some("#55ffff"));
    }

    #[test]
    fn chat_components_colour_and_format_the_motd() {
        let description = json!({
            "text": "",
            "extra": [
                {"text": "Block", "color": "aqua", "bold": true},
                {"text": "Craft ", "color": "#12AB34"},
                "§cred",
                {"text": " plain"}
            ]
        });
        let spans = parse_motd(&description);
        assert_eq!(plain(&spans), "BlockCraft red plain");
        assert_eq!((spans[0].color.as_deref(), spans[0].bold), (Some("#55ffff"), true));
        assert_eq!(spans[1].color.as_deref(), Some("#12ab34"));
        assert_eq!(spans[2].color.as_deref(), Some("#ff5555"));
        assert_eq!(spans[3], MotdSpan { text: " plain".into(), ..MotdSpan::default() });
    }

    #[test]
    fn odd_descriptions_give_what_text_there_is() {
        assert!(parse_motd(&json!(null)).is_empty());
        assert_eq!(plain(&parse_motd(&json!("ends with §"))), "ends with ");
        assert_eq!(plain(&parse_motd(&json!([{"text": "a"}, "b"]))), "ab");
        assert_eq!(parse_motd(&json!({"text": "x", "color": "nonsense"}))[0].color, None);
    }

    #[test]
    fn a_launch_goes_where_the_activity_was() {
        let server = Activity::Server { host: "play.example.net".into(), port: 25565 };
        assert_eq!(server.join(), Join::Server { host: "play.example.net".into(), port: 25565 });
        let world = Activity::World {
            folder: "Test".into(),
            name: "Test".into(),
            mode: "survival".into(),
            difficulty: "hard".into(),
            hardcore: false,
            icon: None,
            quick_play: true,
        };
        assert_eq!(world.join(), Join::World { folder: "Test".into() });
        assert_eq!(
            serde_json::to_value(Join::Server { host: "h".into(), port: 1 }).unwrap(),
            json!({"kind": "server", "host": "h", "port": 1})
        );
    }
}
