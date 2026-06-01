use crate::types::LobbyManifest;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Read, Write};

const PREFIX: &str = "PERFECT-";

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CodecError {
    #[error("missing PERFECT- prefix")]
    BadPrefix,
    #[error("malformed code")]
    Malformed,
    #[error("checksum mismatch")]
    BadChecksum,
}

pub fn encode(m: &LobbyManifest) -> String {
    let json = serde_json::to_vec(m).expect("manifest serializes");
    let mut enc = GzEncoder::new(Vec::new(), Compression::best());
    enc.write_all(&json).expect("gzip write");
    let gz = enc.finish().expect("gzip finish");
    let body = URL_SAFE_NO_PAD.encode(gz);
    let crc = crc32fast::hash(body.as_bytes()) & 0xffff;
    format!("{PREFIX}{body}.{crc:04x}")
}

pub fn decode(code: &str) -> Result<LobbyManifest, CodecError> {
    let rest = code.strip_prefix(PREFIX).ok_or(CodecError::BadPrefix)?;
    let (body, crc_str) = rest.rsplit_once('.').ok_or(CodecError::Malformed)?;
    let want = u32::from_str_radix(crc_str, 16).map_err(|_| CodecError::Malformed)?;
    if crc32fast::hash(body.as_bytes()) & 0xffff != want {
        return Err(CodecError::BadChecksum);
    }
    let gz = URL_SAFE_NO_PAD
        .decode(body.as_bytes())
        .map_err(|_| CodecError::Malformed)?;
    let mut s = String::new();
    GzDecoder::new(&gz[..])
        .read_to_string(&mut s)
        .map_err(|_| CodecError::Malformed)?;
    serde_json::from_str(&s).map_err(|_| CodecError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ManifestMod, ModSource};

    fn sample() -> LobbyManifest {
        LobbyManifest {
            v: 1,
            name: Some("TownOfUs Night".into()),
            platform: None,
            game_build: Some("17.0.1".into()),
            mods: vec![ManifestMod {
                id: "AU-Avengers/TOU-Mira".into(),
                v: "1.6.3".into(),
                src: ModSource::Github,
                r#ref: None,
            }],
            loader: None,
        }
    }

    #[test]
    fn round_trip() {
        let code = encode(&sample());
        assert!(code.starts_with("PERFECT-"));
        assert_eq!(decode(&code).unwrap(), sample());
    }

    #[test]
    fn rejects_bad_prefix() {
        assert_eq!(decode("NOPE-abc.0000"), Err(CodecError::BadPrefix));
    }

    #[test]
    fn rejects_tampered_body() {
        let mut code = encode(&sample());
        // flip a character in the body (before the '.')
        let dot = code.rfind('.').unwrap();
        let bytes = unsafe { code.as_bytes_mut() };
        bytes[dot - 1] = if bytes[dot - 1] == b'A' { b'B' } else { b'A' };
        assert_eq!(decode(&code), Err(CodecError::BadChecksum));
    }
}
