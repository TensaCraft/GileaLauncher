use leptos::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagTone {
    Vanilla,
    Fabric,
    Forge,
    NeoForge,
    Quilt,
    Snapshot,
    Beta,
    Neutral,
    Danger,
    Warn,
    Info,
}

impl TagTone {
    pub fn class(self) -> &'static str {
        match self {
            TagTone::Vanilla => "tag--vanilla",
            TagTone::Fabric => "tag--fabric",
            TagTone::Forge => "tag--forge",
            TagTone::NeoForge => "tag--neoforge",
            TagTone::Quilt => "tag--quilt",
            TagTone::Snapshot => "tag--snapshot",
            TagTone::Beta => "tag--beta",
            TagTone::Neutral => "tag--neutral",
            TagTone::Danger => "tag--danger",
            TagTone::Warn => "tag--warn",
            TagTone::Info => "tag--info",
        }
    }

    /// The `ChipTabs` colour class of this tone (loader chips).
    pub fn chip_class(self) -> &'static str {
        match self {
            TagTone::Vanilla => "chip--vanilla",
            TagTone::Fabric => "chip--fabric",
            TagTone::Forge => "chip--forge",
            TagTone::NeoForge => "chip--neoforge",
            TagTone::Quilt => "chip--quilt",
            TagTone::Snapshot => "chip--snapshot",
            TagTone::Beta => "chip--beta",
            TagTone::Neutral => "chip--neutral",
            TagTone::Danger => "chip--danger",
            TagTone::Warn => "chip--warn",
            TagTone::Info => "chip--info",
        }
    }

    /// Maps a loader id / client name / installed version id to its tag colour.
    pub fn for_loader(loader: &str) -> Self {
        let l = loader.to_lowercase();
        if l.contains("neoforge") {
            TagTone::NeoForge
        } else if l.contains("fabric") {
            TagTone::Fabric
        } else if l.contains("quilt") {
            TagTone::Quilt
        } else if l.contains("forge") && !l.contains("curseforge") {
            TagTone::Forge
        } else if l == "minecraft" || l == "vanilla" {
            TagTone::Vanilla
        } else {
            TagTone::Neutral
        }
    }
}

#[component]
pub fn Tag(tone: TagTone, children: Children) -> impl IntoView {
    view! { <span class=format!("tag {}", tone.class())>{children()}</span> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_tones() {
        assert_eq!(TagTone::for_loader("NeoForge"), TagTone::NeoForge);
        assert_eq!(TagTone::for_loader("fabric-loader-0.16.9-1.21.1"), TagTone::Fabric);
        assert_eq!(TagTone::for_loader("1.20.1-forge-47.3.0"), TagTone::Forge);
        assert_eq!(TagTone::for_loader("quilt"), TagTone::Quilt);
        assert_eq!(TagTone::for_loader("Minecraft"), TagTone::Vanilla);
        assert_eq!(TagTone::for_loader("curseforge"), TagTone::Neutral);
    }

    #[test]
    fn semantic_tones_have_tag_and_chip_colours() {
        let css = include_str!("../../styles/components.css");
        for (tone, name) in [(TagTone::Danger, "danger"), (TagTone::Warn, "warn"), (TagTone::Info, "info")] {
            assert_eq!(tone.class(), format!("tag--{name}"));
            assert_eq!(tone.chip_class(), format!("chip--{name}"));
            assert!(css.contains(&format!(".tag--{name} {{")), "tag {name}");
            assert!(css.contains(&format!(".chip--{name} {{ --chip:")), "chip {name}");
        }
    }
}
