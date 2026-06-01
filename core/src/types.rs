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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestMod {
    pub id: String,
    pub v: String,
    pub src: ModSource,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub r#ref: Option<String>,
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
