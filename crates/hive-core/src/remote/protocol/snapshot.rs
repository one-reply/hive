//! Exact disk bytes on the wire; recovery keeps its backwards-compatible JSON shape.
use crate::files::Snapshot;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serializer};
use std::path::Path;

pub(super) fn serialize<S: Serializer>(
    snapshot: &Snapshot,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&STANDARD.encode(snapshot.bytes()))
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Snapshot, D::Error> {
    let encoded = String::deserialize(deserializer)?;
    let bytes = STANDARD.decode(encoded).map_err(serde::de::Error::custom)?;
    Snapshot::parse(Path::new("remote snapshot"), &bytes).map_err(serde::de::Error::custom)
}
