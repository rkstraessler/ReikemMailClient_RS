use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A wire value that can be omitted, explicitly cleared, or supplied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Nullable<T> {
    /// The field is omitted and can inherit a batch default.
    #[default]
    Missing,
    /// The field contains JSON `null`.
    Null,
    /// The field has a value.
    Value(T),
}

impl<T> Nullable<T> {
    /// Returns whether the field was omitted.
    #[must_use]
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

impl<T> From<T> for Nullable<T> {
    fn from(value: T) -> Self {
        Self::Value(value)
    }
}

impl<T: Serialize> Serialize for Nullable<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Value(value) => value.serialize(serializer),
            Self::Missing | Self::Null => serializer.serialize_none(),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Nullable<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Option::deserialize(deserializer)?.map_or(Self::Null, Self::Value))
    }
}

pub(crate) fn php_trim(text: &str) -> &str {
    text.trim_matches([' ', '\t', '\n', '\r', '\0', '\u{b}'])
}

pub(crate) fn js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\t'..='\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
            | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}
