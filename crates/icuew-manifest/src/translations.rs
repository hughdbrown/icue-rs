//! `translation.json`.
//!
//! The documented shape is `{ "<lang>": { "translation": { "Key": "Value" } } }`. Some shipped
//! widgets (e.g. Corsair's Doodle Pad sample) use a flat `{ "Key": "Value" }` table instead; that
//! is read as the `en` table.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::WidgetError;

#[derive(Debug, Deserialize)]
struct LangEntry {
    #[serde(default)]
    translation: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawTranslations {
    Flat(BTreeMap<String, String>),
    PerLanguage(BTreeMap<String, LangEntry>),
}

/// Translation tables by language code (`en`, `de`, `zh_CN`, …).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Translations(BTreeMap<String, BTreeMap<String, String>>);

impl Translations {
    /// Parse `translation.json` text.
    ///
    /// # Errors
    ///
    /// [`WidgetError::Json`] if the text is neither the documented shape nor a flat table.
    pub fn from_json(text: &str) -> Result<Self, WidgetError> {
        let raw: RawTranslations =
            serde_json::from_str(text).map_err(|source| WidgetError::Json {
                file: "translation.json",
                source,
            })?;
        Ok(Self(match raw {
            RawTranslations::Flat(table) if table.is_empty() => BTreeMap::new(),
            RawTranslations::Flat(table) => BTreeMap::from([("en".to_owned(), table)]),
            RawTranslations::PerLanguage(langs) => langs
                .into_iter()
                .map(|(lang, e)| (lang, e.translation))
                .collect(),
        }))
    }

    /// The table for `lang`, with `en` entries filling any gaps (iCUE's documented fallback).
    ///
    /// ```
    /// use icuew_manifest::Translations;
    /// let t = Translations::from_json(r#"{"en":{"translation":{"Hi":"Hi","Bye":"Bye"}},
    ///                                     "de":{"translation":{"Hi":"Hallo"}}}"#).unwrap();
    /// let de = t.table("de");
    /// assert_eq!(de["Hi"], "Hallo");
    /// assert_eq!(de["Bye"], "Bye");
    /// ```
    #[must_use]
    pub fn table(&self, lang: &str) -> BTreeMap<String, String> {
        let mut out = self.0.get("en").cloned().unwrap_or_default();
        if lang != "en" {
            if let Some(t) = self.0.get(lang) {
                out.extend(t.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
        }
        out
    }

    /// Language codes present.
    pub fn languages(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_table_is_english() {
        let t = Translations::from_json(r#"{"Doodle pad": "Doodle pad", "Color 1": "Color 1"}"#)
            .unwrap();
        assert_eq!(t.languages().collect::<Vec<_>>(), ["en"]);
        assert_eq!(t.table("de")["Color 1"], "Color 1");
    }

    #[test]
    fn rejects_other_shapes() {
        assert!(Translations::from_json(r#"{"en": 5}"#).is_err());
        assert!(Translations::from_json("[]").is_err());
        assert_eq!(
            Translations::from_json("{}").unwrap(),
            Translations::default()
        );
    }
}
