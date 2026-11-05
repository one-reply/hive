//! Tolerant deserialization of nested state. `state.json` is hand-editable and may come from
//! a newer Hive, so one value this build cannot read is replaced by its default, or dropped
//! from a list, with a warning, instead of discarding everything around it.

use serde::{Deserialize, Deserializer, de::DeserializeOwned};
use serde_json::Value;

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

    #[test]
    fn readable_values_pass_through() {
        let outer: Outer = serde_json::from_str(r#"{"inner":{"size":2},"items":[{"size":1}]}"#)
            .expect("valid json");
        assert_eq!(outer.inner, Inner { size: 2 });
        assert_eq!(outer.items, vec![Inner { size: 1 }]);
    }
}
