use crate::types::{LobbyManifest, ManifestValidationError};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Read, Write};

const PREFIX: &str = "PERFECT-";
const MAX_CODE_LEN: usize = 64 * 1024;
const MAX_DECOMPRESSED: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodecError {
    #[error("missing PERFECT- prefix")]
    BadPrefix,
    #[error("malformed code")]
    Malformed,
    #[error("malformed lobby manifest: {0}")]
    MalformedManifest(&'static str),
    #[error("checksum mismatch")]
    BadChecksum,
    #[error("unsupported lobby schema version {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported lobby feature: {0}")]
    UnsupportedFeature(&'static str),
    #[error("{field} exceeds the limit of {limit} bytes/items")]
    ExcessiveInput { field: &'static str, limit: usize },
}

impl From<ManifestValidationError> for CodecError {
    fn from(error: ManifestValidationError) -> Self {
        match error {
            ManifestValidationError::Malformed(reason) => Self::MalformedManifest(reason),
            ManifestValidationError::UnsupportedVersion(version) => {
                Self::UnsupportedVersion(version)
            }
            ManifestValidationError::UnsupportedFeature(feature) => {
                Self::UnsupportedFeature(feature)
            }
            ManifestValidationError::ExcessiveInput { field, limit } => {
                Self::ExcessiveInput { field, limit }
            }
        }
    }
}

pub fn encode(m: &LobbyManifest) -> Result<String, CodecError> {
    m.validate()?;
    Ok(encode_unchecked(m))
}

fn encode_unchecked(m: &LobbyManifest) -> String {
    let json = serde_json::to_vec(m).expect("manifest serializes");
    let mut enc = GzEncoder::new(Vec::new(), Compression::best());
    enc.write_all(&json).expect("gzip write");
    let gz = enc.finish().expect("gzip finish");
    let body = URL_SAFE_NO_PAD.encode(gz);
    let crc = crc32fast::hash(body.as_bytes()) & 0xffff;
    format!("{PREFIX}{body}.{crc:04x}")
}

pub fn decode(code: &str) -> Result<LobbyManifest, CodecError> {
    if code.len() > MAX_CODE_LEN {
        return Err(CodecError::ExcessiveInput {
            field: "lobby code",
            limit: MAX_CODE_LEN,
        });
    }
    let rest = code.strip_prefix(PREFIX).ok_or(CodecError::BadPrefix)?;
    let (body, crc_str) = rest.rsplit_once('.').ok_or(CodecError::Malformed)?;
    let want = u32::from_str_radix(crc_str, 16).map_err(|_| CodecError::Malformed)?;
    if crc32fast::hash(body.as_bytes()) & 0xffff != want {
        return Err(CodecError::BadChecksum);
    }
    let gz = URL_SAFE_NO_PAD
        .decode(body.as_bytes())
        .map_err(|_| CodecError::Malformed)?;
    let mut buf = Vec::new();
    GzDecoder::new(&gz[..])
        .take((MAX_DECOMPRESSED + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|_| CodecError::Malformed)?;
    if buf.len() > MAX_DECOMPRESSED {
        return Err(CodecError::ExcessiveInput {
            field: "decompressed lobby manifest",
            limit: MAX_DECOMPRESSED,
        });
    }
    let manifest: LobbyManifest =
        serde_json::from_slice(&buf).map_err(|_| CodecError::Malformed)?;
    manifest.validate()?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::{fetch_release_by_tag, Http, ResolveError};
    use crate::types::{
        Arch, LoaderPins, ManifestMod, Platform, Store, MAX_ASSET_NAME_LEN, MAX_MANIFEST_MODS,
        MAX_MANIFEST_NAME_LEN, MAX_RELEASE_TAG_LEN, MAX_REPO_ID_LEN, MAX_VERSION_LEN,
    };
    use std::cell::RefCell;

    fn sample() -> LobbyManifest {
        LobbyManifest {
            v: 1,
            name: Some("TownOfUs Night".into()),
            platform: None,
            game_build: Some("17.0.1".into()),
            mods: vec![ManifestMod {
                id: "AU-Avengers/TOU-Mira".into(),
                v: "1.6.3".into(),
                asset: None,
            }],
            loader: None,
        }
    }

    #[test]
    fn encode_rejects_invalid_manifest_before_serializing() {
        let mut invalid = sample();
        invalid.v = 99;
        assert_eq!(encode(&invalid), Err(CodecError::UnsupportedVersion(99)));
    }

    #[test]
    fn round_trip() {
        let code = encode_unchecked(&sample());
        assert!(code.starts_with("PERFECT-"));
        assert_eq!(decode(&code).unwrap(), sample());
    }

    #[test]
    fn rejects_bad_prefix() {
        assert_eq!(decode("NOPE-abc.0000"), Err(CodecError::BadPrefix));
    }

    #[test]
    fn rejects_tampered_body() {
        let mut code = encode_unchecked(&sample());
        // flip a character in the body (before the '.')
        let dot = code.rfind('.').unwrap();
        let bytes = unsafe { code.as_bytes_mut() };
        bytes[dot - 1] = if bytes[dot - 1] == b'A' { b'B' } else { b'A' };
        assert_eq!(decode(&code), Err(CodecError::BadChecksum));
    }

    #[test]
    fn rejects_decompression_bomb_as_excessive() {
        let mut m = sample();
        m.name = Some("a".repeat(2 * 1024 * 1024));
        let code = encode_unchecked(&m);
        assert_eq!(
            decode(&code),
            Err(CodecError::ExcessiveInput {
                field: "decompressed lobby manifest",
                limit: MAX_DECOMPRESSED,
            })
        );
    }

    #[test]
    fn rejects_overlong_code_as_excessive() {
        let code = "x".repeat(MAX_CODE_LEN + 1);
        assert_eq!(
            decode(&code),
            Err(CodecError::ExcessiveInput {
                field: "lobby code",
                limit: MAX_CODE_LEN,
            })
        );
    }

    #[test]
    fn rejects_unsupported_schema_versions() {
        for version in [0, 2] {
            let mut m = sample();
            m.v = version;
            assert_eq!(
                decode(&encode_unchecked(&m)),
                Err(CodecError::UnsupportedVersion(version))
            );
        }
    }

    #[test]
    fn rejects_unhonored_platform_and_loader_pins() {
        let mut platform = sample();
        platform.platform = Some(Platform {
            store: Store::Steam,
            arch: Arch::X64,
        });
        assert_eq!(
            decode(&encode_unchecked(&platform)),
            Err(CodecError::UnsupportedFeature("platform pin"))
        );

        for loader in [
            LoaderPins {
                bepinex: Some("1".repeat(MAX_VERSION_LEN)),
                reactor: None,
            },
            LoaderPins {
                bepinex: None,
                reactor: Some("1".repeat(MAX_VERSION_LEN)),
            },
        ] {
            let mut m = sample();
            m.loader = Some(loader);
            assert_eq!(
                decode(&encode_unchecked(&m)),
                Err(CodecError::UnsupportedFeature("loader pins"))
            );
        }
    }

    #[test]
    fn game_and_loader_versions_keep_restricted_grammar() {
        let mut game = sample();
        game.game_build = Some("release/1.0".into());
        assert!(matches!(
            decode(&encode_unchecked(&game)),
            Err(CodecError::MalformedManifest("invalid version"))
        ));

        for loader in [
            LoaderPins {
                bepinex: Some("release/1.0".into()),
                reactor: None,
            },
            LoaderPins {
                bepinex: None,
                reactor: Some("release/1.0".into()),
            },
        ] {
            let mut manifest = sample();
            manifest.loader = Some(loader);
            assert!(matches!(
                decode(&encode_unchecked(&manifest)),
                Err(CodecError::MalformedManifest("invalid version"))
            ));
        }
    }

    #[test]
    fn rejects_every_excessive_semantic_field() {
        let cases = [
            (
                {
                    let mut m = sample();
                    m.name = Some("n".repeat(MAX_MANIFEST_NAME_LEN + 1));
                    m
                },
                "manifest name",
                MAX_MANIFEST_NAME_LEN,
            ),
            (
                {
                    let mut m = sample();
                    m.game_build = Some("1".repeat(MAX_VERSION_LEN + 1));
                    m
                },
                "game build",
                MAX_VERSION_LEN,
            ),
            (
                {
                    let mut m = sample();
                    m.mods[0].id = format!("a/{}", "r".repeat(MAX_REPO_ID_LEN));
                    m
                },
                "mod repository identity",
                MAX_REPO_ID_LEN,
            ),
            (
                {
                    let mut m = sample();
                    m.mods[0].v = "1".repeat(MAX_RELEASE_TAG_LEN + 1);
                    m
                },
                "mod release tag",
                MAX_RELEASE_TAG_LEN,
            ),
            (
                {
                    let mut m = sample();
                    m.mods[0].asset = Some("a".repeat(MAX_ASSET_NAME_LEN + 1));
                    m
                },
                "asset name",
                MAX_ASSET_NAME_LEN,
            ),
            (
                {
                    let mut m = sample();
                    m.loader = Some(LoaderPins {
                        bepinex: Some("1".repeat(MAX_VERSION_LEN + 1)),
                        reactor: None,
                    });
                    m
                },
                "BepInEx version",
                MAX_VERSION_LEN,
            ),
            (
                {
                    let mut m = sample();
                    m.loader = Some(LoaderPins {
                        bepinex: None,
                        reactor: Some("1".repeat(MAX_VERSION_LEN + 1)),
                    });
                    m
                },
                "Reactor version",
                MAX_VERSION_LEN,
            ),
        ];
        for (manifest, field, limit) in cases {
            assert_eq!(
                decode(&encode_unchecked(&manifest)),
                Err(CodecError::ExcessiveInput { field, limit })
            );
        }

        let mut too_many = sample();
        too_many.mods = (0..=MAX_MANIFEST_MODS)
            .map(|i| ManifestMod {
                id: format!("Owner/Repo{i}"),
                v: "1".into(),
                asset: None,
            })
            .collect();
        assert_eq!(
            decode(&encode_unchecked(&too_many)),
            Err(CodecError::ExcessiveInput {
                field: "manifest mod count",
                limit: MAX_MANIFEST_MODS,
            })
        );
    }

    #[test]
    fn rejects_malformed_repositories_versions_assets_and_duplicates() {
        for id in [
            "",
            "owner",
            "owner/repo/extra",
            "../repo",
            "https://github.com/owner/repo",
            "-owner/repo",
            "owner/.",
            "owner/..",
            "owner/.git",
            "owner/.GIT",
            "owner/repo.git",
            "owner/repo.GIT",
            "owner/repo@tag",
            "owner/repo\n",
        ] {
            let mut m = sample();
            m.mods[0].id = id.into();
            assert!(
                matches!(
                    decode(&encode_unchecked(&m)),
                    Err(CodecError::MalformedManifest(_))
                ),
                "{id:?} must be rejected"
            );
        }

        for tag in ["", "release/\n1.0", "release/\u{7f}1.0"] {
            let mut m = sample();
            m.mods[0].v = tag.into();
            assert!(
                matches!(
                    decode(&encode_unchecked(&m)),
                    Err(CodecError::MalformedManifest(_))
                ),
                "{tag:?} must be rejected"
            );
        }

        for asset in [
            "",
            ".",
            "..",
            "../Mod.dll",
            r"folder\Mod.dll",
            "C:Mod.dll",
            "CON.dll",
            "Mod.dll.",
        ] {
            let mut m = sample();
            m.mods[0].asset = Some(asset.into());
            assert!(
                matches!(
                    decode(&encode_unchecked(&m)),
                    Err(CodecError::MalformedManifest(_))
                ),
                "{asset:?} must be rejected"
            );
        }

        let mut duplicate = sample();
        duplicate.mods.push(ManifestMod {
            id: "au-avengers/tou-mira".into(),
            v: "1.6.4".into(),
            asset: None,
        });
        assert!(matches!(
            decode(&encode_unchecked(&duplicate)),
            Err(CodecError::MalformedManifest(
                "duplicate mod repository identity"
            ))
        ));
    }

    #[test]
    fn slash_tag_and_leading_dot_repository_round_trip_and_resolve() {
        struct RecordingHttp {
            urls: RefCell<Vec<String>>,
        }

        impl Http for RecordingHttp {
            fn get_text(&self, url: &str) -> Result<String, ResolveError> {
                self.urls.borrow_mut().push(url.into());
                Ok(r#"{"tag_name":"release/1.0","assets":[]}"#.into())
            }

            fn get_bytes(&self, _url: &str) -> Result<Vec<u8>, ResolveError> {
                Ok(Vec::new())
            }
        }

        let mut manifest = sample();
        manifest.mods[0].id = "Owner/.github".into();
        manifest.mods[0].v = "release/1.0".into();
        manifest.mods[0].asset = Some("Mod.dll".into());

        let decoded = decode(&encode_unchecked(&manifest)).unwrap();
        assert_eq!(decoded, manifest);

        let http = RecordingHttp {
            urls: RefCell::new(Vec::new()),
        };
        let release = fetch_release_by_tag(&http, &decoded.mods[0].id, &decoded.mods[0].v).unwrap();
        assert_eq!(release.tag, "release/1.0");
        assert_eq!(
            http.urls.into_inner(),
            ["https://api.github.com/repos/Owner/.github/releases/tags/release%2F1%2E0"]
        );
    }

    #[test]
    fn rejects_all_windows_device_asset_names() {
        for stem in [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
            "COM¹", "COM²", "COM³", "LPT¹", "LPT²", "LPT³",
        ] {
            for asset in [stem.to_ascii_lowercase(), format!("{stem}.dll")] {
                let mut manifest = sample();
                manifest.mods[0].asset = Some(asset.clone());
                assert!(
                    matches!(
                        decode(&encode_unchecked(&manifest)),
                        Err(CodecError::MalformedManifest(_))
                    ),
                    "{asset:?} must be rejected"
                );
            }
        }
    }

    #[test]
    fn maximum_valid_manifest_round_trips() {
        let owner = "o".repeat(39);
        let release_tag = "1".repeat(MAX_RELEASE_TAG_LEN);
        let version = "1".repeat(MAX_VERSION_LEN);
        let asset = format!("{}.dll", "a".repeat(MAX_ASSET_NAME_LEN - 4));
        let mods = (0..MAX_MANIFEST_MODS)
            .map(|i| {
                let suffix = i.to_string();
                let repo = format!("R{}{}", "r".repeat(99 - suffix.len()), suffix);
                ManifestMod {
                    id: format!("{owner}/{repo}"),
                    v: release_tag.clone(),
                    asset: Some(asset.clone()),
                }
            })
            .collect();
        let manifest = LobbyManifest {
            v: 1,
            name: Some("n".repeat(MAX_MANIFEST_NAME_LEN)),
            platform: None,
            game_build: Some(version),
            mods,
            loader: None,
        };
        assert_eq!(decode(&encode(&manifest).unwrap()).unwrap(), manifest);
    }
}
