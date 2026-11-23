//! Tolerant deserialization of nested state. `state.json` is hand-editable and may come from
//! a newer Hive, so one value this build cannot read is replaced by its default, or dropped
//! from a list, with a warning, instead of discarding everything around it.

use serde::{Deserialize, Deserializer, de::DeserializeOwned};
use serde_json::{Map, Value};

/// `#[serde(deserialize_with)]`: an unreadable value becomes `T::default()`.
pub(crate) fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_else(|error| {
        tracing::warn!(
            value = std::any::type_name::<T>(),
            %error,
            "unreadable value replaced by its default"
        );
        T::default()
    }))
}

/// `#[serde(deserialize_with)]`: unreadable elements are dropped, the rest are kept in order.
pub(crate) fn elements<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let values = Vec::<Value>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .filter_map(|value| {
            serde_json::from_value(value)
                .inspect_err(|error| {
                    tracing::warn!(
                        element = std::any::type_name::<T>(),
                        %error,
                        "unreadable list element dropped"
                    )
                })
                .ok()
        })
        .collect())
}

/// A section that is a group of fields, read so that a value this build cannot read costs
/// only itself: the field takes its default and its path (`settings.files.font_size`) goes to
/// `lost`, the rest of the section stays. A section that is not a JSON object is lost whole.
///
/// Works for any `T` whose fields have defaults, without a list of them: each entry is tried as
/// the only one present, and an unreadable entry that is itself an object is searched the same
/// way one level down.
pub(crate) fn fields<T>(name: &str, value: Option<Value>, lost: &mut Vec<String>) -> T
where
    T: Default + DeserializeOwned,
{
    let Some(value) = value else {
        return T::default();
    };
    let error = match T::deserialize(&value) {
        // The usual case: everything reads, and nothing below runs.
        Ok(section) if value.is_object() => return section,
        // A struct also reads from an array, by position; a section never is one.
        Ok(_) => None,
        Err(error) => Some(error.to_string()),
    };
    let Value::Object(object) = value else {
        tracing::warn!(
            section = name,
            "state section is not an object; reset to defaults"
        );
        lost.push(name.to_owned());
        return T::default();
    };
    let mut unreadable = Vec::new();
    let kept = readable::<T>(name, object, &|entry| entry, &mut unreadable);
    match serde_json::from_value(Value::Object(kept)) {
        Ok(section) => {
            // Reported in one order whatever `serde_json::Map` is in this build.
            unreadable.sort();
            tracing::warn!(
                section = name,
                ?error,
                ?unreadable,
                "unreadable fields reset to defaults"
            );
            lost.append(&mut unreadable);
            section
        }
        Err(error) => {
            tracing::warn!(section = name, %error, "unreadable state section reset to defaults");
            lost.push(name.to_owned());
            T::default()
        }
    }
}

/// The entries of `object` that `T` reads; `place` puts an object where `object` sits in `T`.
/// The paths of the entries left out are added to `unreadable`.
fn readable<T: DeserializeOwned>(
    path: &str,
    object: Map<String, Value>,
    place: &dyn Fn(Value) -> Value,
    unreadable: &mut Vec<String>,
) -> Map<String, Value> {
    let mut kept = Map::new();
    for (key, value) in object {
        let path = format!("{path}.{key}");
        let alone = |value: Value| place(Value::Object(Map::from_iter([(key.clone(), value)])));
        let reads = |value: &Value| T::deserialize(&alone(value.clone())).is_ok();
        if reads(&value) {
            kept.insert(key, value);
            continue;
        }
        let Value::Object(inner) = value else {
            unreadable.push(path);
            continue;
        };
        let mut inside = Vec::new();
        let inner = Value::Object(readable::<T>(&path, inner, &alone, &mut inside));
        if reads(&inner) {
            kept.insert(key, inner);
            unreadable.append(&mut inside);
        } else {
            // Not a group of independent fields (a tagged choice, say): it goes as one value.
            unreadable.push(path);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize, Default, PartialEq, Debug)]
    #[serde(default)]
    struct Inner {
        size: u32,
    }

    #[derive(Deserialize, Default, PartialEq, Debug)]
    #[serde(default)]
    struct Outer {
        #[serde(deserialize_with = "or_default")]
        inner: Inner,
        #[serde(deserialize_with = "elements")]
        items: Vec<Inner>,
    }

    #[test]
    fn unreadable_value_becomes_default_and_unreadable_elements_are_dropped() {
        let outer: Outer = serde_json::from_str(
            r#"{"inner":{"size":"big"},"items":[{"size":1},{"size":null},"x",{"size":3}]}"#,
        )
        .expect("valid json");
        assert_eq!(outer.inner, Inner::default());
        assert_eq!(outer.items, vec![Inner { size: 1 }, Inner { size: 3 }]);
    }

    #[derive(Deserialize, PartialEq, Debug)]
    #[serde(default)]
    struct Section {
        width: u32,
        open: bool,
        inner: Inner,
        names: std::collections::BTreeMap<String, String>,
        choice: Option<Choice>,
    }

    impl Default for Section {
        fn default() -> Self {
            Self {
                width: 260,
                open: true,
                inner: Inner { size: 14 },
                names: Default::default(),
                choice: None,
            }
        }
    }

    #[derive(Deserialize, PartialEq, Debug)]
    #[serde(tag = "kind", rename_all = "lowercase")]
    enum Choice {
        Cli { agent: String },
    }

    fn read(json: &str) -> (Section, Vec<String>) {
        let mut lost = Vec::new();
        let value = serde_json::from_str(json).expect("valid json");
        (fields("section", Some(value), &mut lost), lost)
    }

    #[test]
    fn an_unreadable_field_takes_its_own_default_and_the_rest_is_kept() {
        let (section, lost) = read(
            r#"{"width":"wide","open":false,"inner":{"size":"big"},
                "names":{"a":"x","b":7},"future":{"anything":1}}"#,
        );
        assert_eq!(
            section,
            Section {
                open: false,
                names: [("a".to_owned(), "x".to_owned())].into(),
                // The fields that were unreadable are as in `Section::default()`, not zeroed.
                width: 260,
                inner: Inner::default(),
                ..Section::default()
            }
        );
        assert_eq!(
            lost,
            ["section.inner.size", "section.names.b", "section.width"]
        );
    }

    #[test]
    fn a_value_that_reads_only_whole_is_lost_whole() {
        let (section, lost) = read(r#"{"open":false,"choice":{"kind":"cli","agent":7}}"#);
        assert_eq!(section.choice, None);
        assert!(!section.open);
        assert_eq!(lost, ["section.choice"]);
    }

    #[test]
    fn a_readable_absent_or_misshapen_section_is_whole_default_or_lost() {
        let (section, lost) = read(r#"{"width":300,"choice":{"kind":"cli","agent":"codex"}}"#);
        assert_eq!(section.width, 300);
        assert!(section.choice.is_some() && lost.is_empty());

        let mut lost = Vec::new();
        assert_eq!(
            fields::<Section>("section", None, &mut lost),
            Section::default()
        );
        assert!(lost.is_empty());
        // A struct would read from an array by position; a section never is one.
        for misshapen in ["[]", "[300,false]", "7", r#""x""#] {
            let (section, lost) = read(misshapen);
            assert_eq!(section, Section::default(), "{misshapen}");
            assert_eq!(lost, ["section"], "{misshapen}");
        }
    }

    #[test]
    fn readable_values_pass_through() {
        let outer: Outer = serde_json::from_str(r#"{"inner":{"size":2},"items":[{"size":1}]}"#)
            .expect("valid json");
        assert_eq!(outer.inner, Inner { size: 2 });
        assert_eq!(outer.items, vec![Inner { size: 1 }]);
    }
}
