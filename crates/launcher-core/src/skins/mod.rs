//! Players' skins in every form the launcher shows them: the texture, the head, the front of the
//! body. Mojang is asked first; third-party hosts only when it has nothing or cannot be reached.

pub mod render;
pub mod service;

pub use service::{AVATAR_SIZE, Avatar, Player, SkinConfig, SkinEndpoints, SkinService, SkinTexture};
