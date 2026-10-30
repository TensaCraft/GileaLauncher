//! Browser-preview backend for skins and capes: two official profiles (one to sign in again),
//! three skins drawn here as plain coloured figures, and two capes.

use launcher_shared::{AppError, AppResult, ErrorCode};
use module_skins::dto::{
    Account, AccountState, Cape, CurrentSkin, ImportArgs, LibrarySkin, MESSAGE, SkinSource, SkinVariant,
    VariantArgs,
};
use serde_json::Value;

use crate::mock_builds::to_value;

const SKIN_TONE: [u8; 3] = [214, 170, 130];
const BOOTS: [u8; 3] = [52, 40, 34];

/// The colours of a drawn figure: shirt, trousers, hair.
type Outfit = ([u8; 3], [u8; 3], [u8; 3]);

const OUTFITS: [Outfit; 6] = [
    ([46, 125, 70], [70, 62, 48], [92, 58, 30]),
    ([220, 120, 40], [48, 58, 92], [30, 24, 20]),
    ([40, 110, 190], [235, 235, 235], [214, 168, 72]),
    ([150, 60, 160], [60, 60, 60], [180, 70, 40]),
    ([200, 50, 60], [40, 40, 50], [240, 220, 160]),
    ([30, 160, 160], [90, 70, 50], [20, 20, 20]),
];

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// An RGBA picture as a PNG, its pixels stored uncompressed.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let row = width as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for line in rgba.chunks(row) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let n = (u32::from(group[0]) << 16)
            | (u32::from(*group.get(1).unwrap_or(&0)) << 8)
            | u32::from(*group.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= group.len() {
                out.push(ABC[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn data_url(png: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64(png))
}

/// An RGBA picture being drawn.
struct Canvas {
    width: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Canvas {
        Canvas { width, pixels: vec![0; (width * height * 4) as usize] }
    }

    fn fill(&mut self, (x0, y0, x1, y1): (u32, u32, u32, u32), rgb: [u8; 3]) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.set(x, y, [rgb[0], rgb[1], rgb[2], 255]);
            }
        }
    }

    fn clear(&mut self, (x0, y0, x1, y1): (u32, u32, u32, u32)) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.set(x, y, [0, 0, 0, 0]);
            }
        }
    }

    fn set(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let at = ((y * self.width + x) * 4) as usize;
        self.pixels[at..at + 4].copy_from_slice(&rgba);
    }

    fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * self.width + x) * 4) as usize;
        [self.pixels[at], self.pixels[at + 1], self.pixels[at + 2], self.pixels[at + 3]]
    }
}

/// A plain figure in `outfit`, with Alex's narrow arms when `slim`.
fn draw_skin((shirt, pants, hair): Outfit, slim: bool) -> Canvas {
    let mut skin = Canvas::new(64, 64);
    skin.fill((0, 0, 32, 16), SKIN_TONE);
    for part in [(8, 0, 16, 8), (8, 8, 16, 10), (0, 8, 8, 11), (16, 8, 24, 11), (24, 8, 32, 16)] {
        skin.fill(part, hair);
    }
    skin.set(9, 12, [245, 245, 245, 255]);
    skin.set(10, 12, [40, 50, 80, 255]);
    skin.set(13, 12, [40, 50, 80, 255]);
    skin.set(14, 12, [245, 245, 245, 255]);
    skin.fill((11, 14, 13, 15), [160, 96, 84]);
    skin.fill((16, 16, 40, 32), shirt);
    // Arms: a sleeve, then the hand.
    skin.fill((40, 16, 56, 32), SKIN_TONE);
    skin.fill((40, 20, 56, 25), shirt);
    skin.fill((44, 16, 48, 20), shirt);
    skin.fill((32, 48, 48, 64), SKIN_TONE);
    skin.fill((32, 52, 48, 57), shirt);
    skin.fill((36, 48, 40, 52), shirt);
    // Legs and boots.
    skin.fill((0, 16, 16, 32), pants);
    skin.fill((0, 29, 16, 32), BOOTS);
    skin.fill((16, 48, 32, 64), pants);
    skin.fill((16, 61, 32, 64), BOOTS);
    if slim {
        skin.clear((54, 20, 56, 32));
        skin.clear((50, 16, 52, 20));
        skin.clear((46, 52, 48, 64));
        skin.clear((42, 48, 44, 52));
    }
    skin
}

/// The figure from the front, four pixels to one (64×128), as the backend's thumbnail.
fn front_view(skin: &Canvas, slim: bool) -> Vec<u8> {
    let arm = if slim { 3 } else { 4 };
    let parts = [
        ((8, 8), (4, 0), (8, 8)),
        ((20, 20), (4, 8), (8, 12)),
        ((44, 20), (4 - arm, 8), (arm, 12)),
        ((36, 52), (12, 8), (arm, 12)),
        ((4, 20), (4, 20), (4, 12)),
        ((20, 52), (8, 20), (4, 12)),
    ];
    let mut front = Canvas::new(64, 128);
    for ((fx, fy), (tx, ty), (w, h)) in parts {
        for y in 0..h * 4 {
            for x in 0..w * 4 {
                front.set((tx * 4) + x, (ty * 4) + y, skin.get(fx + x / 4, fy + y / 4));
            }
        }
    }
    encode_png(64, 128, &front.pixels)
}

/// A cape: its front and back in `rgb`, a stripe down the front.
fn draw_cape(rgb: [u8; 3], stripe: [u8; 3]) -> Vec<u8> {
    let mut cape = Canvas::new(64, 32);
    cape.fill((0, 0, 22, 17), rgb);
    cape.fill((5, 1, 7, 17), stripe);
    cape.fill((12, 1, 22, 17), [rgb[0] / 2, rgb[1] / 2, rgb[2] / 2]);
    encode_png(64, 32, &cape.pixels)
}

struct Skin {
    entry: LibrarySkin,
    png: Vec<u8>,
    canvas: Canvas,
}

fn skin(id: &str, name: &str, source: &str, outfit: Outfit, variant: SkinVariant) -> Skin {
    let canvas = draw_skin(outfit, variant == SkinVariant::Slim);
    let png = encode_png(64, 64, &canvas.pixels);
    let thumb = data_url(&front_view(&canvas, variant == SkinVariant::Slim));
    Skin {
        entry: LibrarySkin {
            id: id.into(),
            name: name.into(),
            variant,
            source: source.into(),
            sha256: format!("mock-{id}"),
            added_ms: 1_791_800_000_000,
            thumb,
        },
        png,
        canvas,
    }
}

fn failed(code: ErrorCode, message: &str) -> AppError {
    AppError::new(code, "mock").with_param(MESSAGE, message)
}

pub struct MockSkins {
    accounts: Vec<Account>,
    skins: Vec<Skin>,
    worn: Option<String>,
    capes: Vec<(String, String, String)>,
    active_cape: Option<String>,
    made: usize,
}

impl MockSkins {
    pub fn new(no_accounts: bool) -> MockSkins {
        let account = |key: &str, is_default: bool, reauth_required: bool| Account {
            key: key.into(),
            name: key.into(),
            is_default,
            reauth_required,
        };
        let accounts = if no_accounts {
            Vec::new()
        } else {
            vec![account("Notch_UA", true, false), account("Builder_Olya", false, true)]
        };
        MockSkins {
            accounts,
            skins: vec![
                skin("s3", "Мандрівник", "player:Wanderer", OUTFITS[2], SkinVariant::Classic),
                skin("s2", "Шахтарка", "file", OUTFITS[1], SkinVariant::Slim),
                skin("s1", "Лісник", "account:Notch_UA", OUTFITS[0], SkinVariant::Classic),
            ],
            worn: Some("s1".into()),
            capes: vec![
                ("c1".into(), "Migrator".into(), data_url(&draw_cape([110, 70, 170], [235, 200, 90]))),
                ("c2".into(), "Pan".into(), data_url(&draw_cape([190, 60, 50], [245, 245, 245]))),
            ],
            active_cape: Some("c1".into()),
            made: 0,
        }
    }

    fn signed_in(&self, args: &Value) -> AppResult<()> {
        let profile = args["profile"].as_str().unwrap_or_default();
        match self.accounts.iter().find(|a| a.key == profile) {
            Some(a) if a.reauth_required => Err(failed(ErrorCode::ReauthRequired, "skins_error_reauth")),
            Some(_) => Ok(()),
            None => Err(AppError::new(ErrorCode::NoProfile, "mock")),
        }
    }

    fn state(&self, args: &Value) -> AppResult<AccountState> {
        self.signed_in(args)?;
        let skin = self.worn.as_ref().and_then(|id| self.skins.iter().find(|s| &s.entry.id == id));
        Ok(AccountState {
            profile: args["profile"].as_str().unwrap_or_default().into(),
            skin: skin.map(|s| CurrentSkin {
                sha256: s.entry.sha256.clone(),
                variant: s.entry.variant,
                data: data_url(&s.png),
                library_id: Some(s.entry.id.clone()),
                default: false,
            }),
            capes: self
                .capes
                .iter()
                .map(|(id, alias, data)| Cape {
                    id: id.clone(),
                    alias: alias.clone(),
                    active: self.active_cape.as_ref() == Some(id),
                    data: data.clone(),
                })
                .collect(),
            skin_unavailable: false,
        })
    }

    fn find(&mut self, args: &Value) -> AppResult<&mut Skin> {
        let id = args["id"].as_str().unwrap_or_default();
        self.skins
            .iter_mut()
            .find(|s| s.entry.id == id)
            .ok_or_else(|| failed(ErrorCode::NotFound, "skins_error_gone"))
    }

    fn add(&mut self, name: &str, source: String, variant: SkinVariant) -> LibrarySkin {
        self.made += 1;
        let added = skin(
            &format!("new{}", self.made),
            name,
            &source,
            OUTFITS[(2 + self.made) % OUTFITS.len()],
            variant,
        );
        let entry = added.entry.clone();
        self.skins.insert(0, added);
        entry
    }

    pub fn handle(&mut self, command: &str, args: &Value) -> Result<Value, AppError> {
        match command {
            "accounts" => to_value(&self.accounts),
            "state" => to_value(self.state(args)?),
            "library" => to_value(self.skins.iter().map(|s| s.entry.clone()).collect::<Vec<_>>()),
            "skin" => {
                let png = self.find(args)?.png.clone();
                to_value(data_url(&png))
            }
            "import" => {
                let args: ImportArgs = serde_json::from_value(args.clone())
                    .map_err(|e| AppError::new(ErrorCode::InvalidInput, e.to_string()))?;
                let (name, source) = match &args.source {
                    SkinSource::File { path } => {
                        let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
                        (file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_string(), "file".into())
                    }
                    SkinSource::Player { name } => {
                        let name = name.trim().to_string();
                        let fits = (1..=16).contains(&name.len())
                            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                        if !fits || name.eq_ignore_ascii_case("Nobody") {
                            return Err(
                                failed(ErrorCode::NotFound, "skins_error_no_player").with_param("name", name)
                            );
                        }
                        let source = format!("player:{name}");
                        (name, source)
                    }
                    // `…/page…` is a web page, not a skin; anything without a dot is no address.
                    SkinSource::Link { url } => {
                        let url = url.trim();
                        if !url.contains('.') || url.contains(' ') {
                            return Err(failed(ErrorCode::InvalidInput, "skins_error_bad_link"));
                        }
                        if url.contains("page") {
                            return Err(failed(ErrorCode::InvalidInput, "skins_error_bad_png"));
                        }
                        let file = url.trim_end_matches('/').rsplit('/').next().unwrap_or(url);
                        (
                            file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_string(),
                            format!("link:{url}"),
                        )
                    }
                };
                let name = args.name.filter(|n| !n.trim().is_empty()).unwrap_or(name);
                to_value(self.add(name.trim(), source, args.variant.unwrap_or_default()))
            }
            "rename" => {
                let name = args["name"].as_str().unwrap_or_default().trim().to_string();
                if name.is_empty() || name.chars().count() > 48 {
                    return Err(failed(ErrorCode::InvalidInput, "skins_error_name").with_param("max", "48"));
                }
                let skin = self.find(args)?;
                skin.entry.name = name;
                to_value(skin.entry.clone())
            }
            "set_variant" => {
                let VariantArgs { variant, .. } =
                    serde_json::from_value(args.clone()).map_err(|e| AppError::internal(e.to_string()))?;
                let skin = self.find(args)?;
                skin.entry.variant = variant;
                skin.entry.thumb = data_url(&front_view(&skin.canvas, variant == SkinVariant::Slim));
                to_value(skin.entry.clone())
            }
            "delete" => {
                let id = self.find(args)?.entry.id.clone();
                self.skins.retain(|s| s.entry.id != id);
                Ok(Value::Null)
            }
            "apply" => {
                self.signed_in(args)?;
                let id = self.find(args)?.entry.id.clone();
                self.worn = Some(id);
                to_value(self.state(args)?)
            }
            "set_cape" => {
                self.signed_in(args)?;
                self.active_cape = args["cape"].as_str().map(str::to_string);
                to_value(self.state(args)?)
            }
            other => Err(AppError::internal(format!("mock: no skins command {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use module_skins::dto::{Account, AccountState, LibrarySkin, SkinVariant};
    use serde_json::json;

    use super::*;

    fn parse<T: serde::de::DeserializeOwned>(value: Result<Value, AppError>) -> T {
        serde_json::from_value(value.unwrap()).unwrap()
    }

    #[test]
    fn the_mock_has_two_official_accounts_one_to_sign_in_again() {
        let accounts: Vec<Account> = parse(MockSkins::new(false).handle("accounts", &json!(null)));
        assert_eq!(accounts.len(), 2);
        assert!(accounts[0].is_default && !accounts[0].reauth_required);
        assert!(accounts[1].reauth_required);
        let none: Vec<Account> = parse(MockSkins::new(true).handle("accounts", &json!(null)));
        assert!(none.is_empty());
    }

    #[test]
    fn the_mock_library_and_state_are_drawn_skins() {
        let mut mock = MockSkins::new(false);
        let library: Vec<LibrarySkin> = parse(mock.handle("library", &json!(null)));
        assert_eq!(library.len(), 3);
        assert!(library.iter().all(|s| s.thumb.starts_with("data:image/png;base64,iVBORw0KGgo")));
        assert!(library.iter().any(|s| s.variant == SkinVariant::Slim));
        let state: AccountState = parse(mock.handle("state", &json!({"profile": "Notch_UA"})));
        assert_eq!(state.capes.len(), 2);
        assert_eq!(state.capes.iter().filter(|c| c.active).count(), 1);
        let worn = state.skin.unwrap().library_id.unwrap();
        assert!(library.iter().any(|s| s.id == worn), "the worn skin is a library card");
        let skin: String = parse(mock.handle("skin", &json!({"id": worn})));
        assert!(skin.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn the_mock_puts_skins_and_capes_on() {
        let mut mock = MockSkins::new(false);
        let library: Vec<LibrarySkin> = parse(mock.handle("library", &json!(null)));
        let other = library.iter().find(|s| s.variant == SkinVariant::Slim).unwrap().id.clone();
        let state: AccountState = parse(mock.handle("apply", &json!({"profile": "Notch_UA", "id": other})));
        assert_eq!(state.skin.unwrap().library_id.as_deref(), Some(other.as_str()));
        let state: AccountState =
            parse(mock.handle("set_cape", &json!({"profile": "Notch_UA", "cape": null})));
        assert!(state.capes.iter().all(|c| !c.active));
        let file = json!({"source": {"kind": "file", "path": "C:/mock/skin.png"}});
        let added: LibrarySkin = parse(mock.handle("import", &file));
        assert_eq!(added.name, "skin");
        let link = json!({"source": {"kind": "link", "url": "https://example.com/a.png"}, "name": "Лицар", "variant": "slim"});
        let linked: LibrarySkin = parse(mock.handle("import", &link));
        assert_eq!((linked.name.as_str(), linked.variant), ("Лицар", SkinVariant::Slim));
        let renamed: LibrarySkin = parse(mock.handle("rename", &json!({"id": added.id, "name": "Новий"})));
        assert_eq!(renamed.name, "Новий");
        mock.handle("delete", &json!({"id": renamed.id})).unwrap();
        assert!(mock.handle("import", &json!({"source": {"kind": "player", "name": "Nobody"}})).is_err());
        assert!(
            mock.handle("apply", &json!({"profile": "Builder_Olya", "id": other})).is_err(),
            "sign in first"
        );
    }

    #[test]
    fn the_mock_png_is_a_png() {
        let png = encode_png(2, 1, &[255, 0, 0, 255, 0, 0, 255, 128]);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(&png[12..16], b"IHDR");
        assert!(png.ends_with(&[0xAE, 0x42, 0x60, 0x82]), "IEND's CRC");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
    }
}
