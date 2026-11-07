//! Lossless Unix paths: JSON strings cannot represent non-UTF-8 filenames.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serializer};
use std::{
    ffi::OsString,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

pub(crate) fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&STANDARD.encode(path.as_os_str().as_bytes()))
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
    let encoded = String::deserialize(deserializer)?;
    decode(&encoded).map_err(serde::de::Error::custom)
}

fn decode(encoded: &str) -> Result<PathBuf, String> {
    let bytes = STANDARD.decode(encoded).map_err(|e| e.to_string())?;
    if bytes.contains(&0) {
        return Err("NUL in remote path".into());
    }
    Ok(OsString::from_vec(bytes).into())
}

/// Existing UTF-8 state stays readable; Unix byte paths use an explicit tagged value.
pub(crate) mod persisted {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize, Deserialize)]
    #[serde(untagged)]
    enum StoredPath {
        Text(String),
        Bytes { unix_base64: String },
    }

    impl StoredPath {
        fn from_path(path: &Path) -> Self {
            match path.to_str() {
                Some(path) => Self::Text(path.into()),
                None => Self::Bytes {
                    unix_base64: STANDARD.encode(path.as_os_str().as_bytes()),
                },
            }
        }

        fn into_path(self) -> Result<PathBuf, String> {
            match self {
                Self::Text(path) if !path.contains('\0') => Ok(path.into()),
                Self::Text(_) => Err("NUL in stored path".into()),
                Self::Bytes { unix_base64 } => decode(&unix_base64),
            }
        }
    }

    pub(crate) fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        StoredPath::from_path(path).serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        StoredPath::deserialize(deserializer)?
            .into_path()
            .map_err(serde::de::Error::custom)
    }

    #[cfg(feature = "desktop")]
    pub(crate) mod optional {
        use super::*;
        pub(crate) fn serialize<S: Serializer>(
            path: &Option<PathBuf>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            path.as_deref()
                .map(StoredPath::from_path)
                .serialize(serializer)
        }
        pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<PathBuf>, D::Error> {
            Option::<StoredPath>::deserialize(deserializer)?
                .map(StoredPath::into_path)
                .transpose()
                .map_err(serde::de::Error::custom)
        }
    }
}

pub(crate) mod many {
    use super::*;
    use serde::Serialize;
    pub(crate) fn serialize<S: Serializer>(
        paths: &[PathBuf],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        paths
            .iter()
            .map(|p| STANDARD.encode(p.as_os_str().as_bytes()))
            .collect::<Vec<_>>()
            .serialize(serializer)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<PathBuf>, D::Error> {
        Vec::<String>::deserialize(deserializer)?
            .iter()
            .map(|s| decode(s).map_err(serde::de::Error::custom))
            .collect()
    }
}
