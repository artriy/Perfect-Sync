use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X86,
    X64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Store {
    Steam,
    Epic,
    Itch,
    Msstore,
    Manual,
}

/// How an Among Us install is executed. Among Us is a Windows-only build, so on
/// Linux it runs under Steam Proton and on macOS under CrossOver/Wine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Native,
    Proton,
    Wine,
    Crossover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModTag {
    Role,
    AllClient,
    HostOnly,
    Map,
    Cosmetic,
    Library,
    Loader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModSource {
    Catalog,
    Github,
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Platform {
    pub store: Store,
    pub arch: Arch,
}

/// One mod in a share code. Kept minimal to keep codes short: `id` is
/// `owner/repo` (source is always GitHub, derivable from it), `v` is the tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestMod {
    pub id: String,
    pub v: String,
    /// exact asset file the host installed, so a custom/multi-asset repo
    /// resolves to the same file (omitted when there's nothing special to pin).
    #[serde(rename = "a", skip_serializing_if = "Option::is_none", default)]
    pub asset: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoaderPins {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bepinex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reactor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LobbyManifest {
    pub v: u8,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub platform: Option<Platform>,
    #[serde(rename = "gameBuild", skip_serializing_if = "Option::is_none", default)]
    pub game_build: Option<String>,
    pub mods: Vec<ManifestMod>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub loader: Option<LoaderPins>,
}
