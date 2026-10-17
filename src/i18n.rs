//! Interface translations loaded from JSON files.
//!
//! Each language is a flat JSON object of keys to strings, stored as
//! `assets/lang/<code>.json`. Indonesian and English are built in; more files
//! are read at start-up from the folders [`search_dirs`] lists.

use std::collections::HashMap;
use std::path::PathBuf;

/// The language a key missing from the current language falls back to.
const FALLBACK: &str = "id";

/// Built-in languages, embedded so an installed binary needs no files.
const BUILTIN: [(&str, &str); 2] = [
    ("id", include_str!("../assets/lang/id.json")),
    ("en", include_str!("../assets/lang/en.json")),
];

/// One language: its code (the file stem) and its strings.
#[derive(Clone, Debug)]
struct Locale {
    code: String,
    strings: HashMap<String, String>,
}

impl Locale {
    /// The `"name"` entry, or the code when the file has none.
    fn name(&self) -> &str {
        self.strings.get("name").map_or(&self.code, String::as_str)
    }
}

/// A language file that could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadError {
    pub file: String,
    pub error: String,
}

/// Every available language and the one in use.
#[derive(Clone, Debug)]
pub struct I18n {
    /// Sorted by display name, for the menu.
    locales: Vec<Locale>,
    current: String,
}

fn parse(json: &str) -> Result<HashMap<String, String>, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

impl I18n {
    /// The built-in languages only, with Indonesian selected.
    pub fn builtin() -> Self {
        let mut i18n = I18n {
            locales: Vec::new(),
            current: FALLBACK.to_owned(),
        };
        for (code, json) in BUILTIN {
            i18n.merge(
                code,
                parse(json).expect("built-in language files are valid"),
            );
        }
        i18n
    }

    /// The built-in languages plus every `*.json` file in `dirs`. A file whose
    /// code is already loaded is merged over it key by key. Unreadable or
    /// invalid files are skipped and reported.
    pub fn load(dirs: &[PathBuf]) -> (Self, Vec<LoadError>) {
        let mut i18n = Self::builtin();
        let mut errors = Vec::new();
        let mut seen = Vec::new();
        for dir in dirs {
            // A missing folder is normal: most installs have none.
            let Ok(dir) = dir.canonicalize() else {
                continue;
            };
            if seen.contains(&dir) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect();
            files.sort();
            for path in files {
                let file = path.file_name().unwrap_or_default().to_string_lossy();
                let code = path.file_stem().unwrap_or_default().to_string_lossy();
                let strings = std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|json| parse(&json));
                match strings {
                    Ok(strings) => i18n.merge(&code, strings),
                    Err(error) => errors.push(LoadError {
                        file: file.into_owned(),
                        error,
                    }),
                }
            }
            seen.push(dir);
        }
        (i18n, errors)
    }

    /// Adds `strings` to the language `code`, creating it if needed.
    fn merge(&mut self, code: &str, strings: HashMap<String, String>) {
        match self.locales.iter_mut().find(|locale| locale.code == code) {
            Some(locale) => locale.strings.extend(strings),
            None => self.locales.push(Locale {
                code: code.to_owned(),
                strings,
            }),
        }
        self.locales.sort_by(|a, b| a.name().cmp(b.name()));
    }

    fn locale(&self, code: &str) -> Option<&Locale> {
        self.locales.iter().find(|locale| locale.code == code)
    }

    /// The text for `key` in the current language, else in Indonesian, else
    /// the key itself.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        [self.current.as_str(), FALLBACK]
            .into_iter()
            .filter_map(|code| self.locale(code))
            .find_map(|locale| locale.strings.get(key))
            .map_or(key, String::as_str)
    }

    /// [`Self::t`] with every `{name}` token replaced by its value.
    pub fn tf(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut text = self.t(key).to_owned();
        for (name, value) in args {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text
    }

    /// Switches to the language `code`; false (and no change) if unknown.
    pub fn select(&mut self, code: &str) -> bool {
        let known = self.locale(code).is_some();
        if known {
            self.current = code.to_owned();
        }
        known
    }

    /// Code of the language in use.
    pub fn code(&self) -> &str {
        &self.current
    }

    /// `(code, display name)` of every language, sorted by name.
    pub fn locales(&self) -> impl Iterator<Item = (&str, &str)> {
        self.locales
            .iter()
            .map(|locale| (locale.code.as_str(), locale.name()))
    }

    /// Picks the first available of: the saved choice, the system language,
    /// English, Indonesian.
    pub fn choose_initial(&mut self, saved: Option<&str>, system: Option<&str>) {
        for code in [saved, system, Some("en"), Some(FALLBACK)]
            .into_iter()
            .flatten()
        {
            if self.select(code) {
                return;
            }
        }
    }
}

/// Language code of the system locale (`LC_ALL`, `LC_MESSAGES`, then `LANG`).
pub fn system_language() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|value| !value.is_empty())
        .and_then(|value| language_code(&value))
}

/// `"id_ID.UTF-8"` → `"id"`. `None` for the `C` / `POSIX` locales.
fn language_code(locale: &str) -> Option<String> {
    let code = locale.split(['_', '.', '@']).next()?.to_ascii_lowercase();
    (!code.is_empty() && code != "c" && code != "posix").then_some(code)
}

/// Folders searched for extra language files: `assets/lang` next to the
/// executable, then in the working directory.
pub fn search_dirs() -> Vec<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()));
    let cwd = std::env::current_dir().ok();
    [exe_dir, cwd]
        .into_iter()
        .flatten()
        .map(|dir| dir.join("assets").join("lang"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use std::path::Path;

    /// A fresh folder `rred_i18n_<name>` in the temp directory holding
    /// `files` as `(file name, contents)`.
    fn lang_dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rred_i18n_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (file, contents) in files {
            fs::write(dir.join(file), contents).unwrap();
        }
        dir
    }

    fn keys(json: &str) -> HashSet<String> {
        parse(json).unwrap().into_keys().collect()
    }

    #[test]
    fn test_builtin_languages_have_the_same_keys() {
        let id = keys(BUILTIN[0].1);
        let en = keys(BUILTIN[1].1);
        assert!(id.contains("name"));
        assert_eq!(
            id.symmetric_difference(&en).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "id.json and en.json must list the same keys"
        );
    }

    #[test]
    fn test_builtin_starts_in_indonesian_and_lists_languages_by_name() {
        let i18n = I18n::builtin();
        assert_eq!(i18n.code(), "id");
        assert_eq!(i18n.t("menu.view"), "Tampilan");
        let locales: Vec<(&str, &str)> = i18n.locales().collect();
        assert_eq!(locales, vec![("id", "Bahasa Indonesia"), ("en", "English")]);
    }

    #[test]
    fn test_select_switches_language_and_rejects_unknown_codes() {
        let mut i18n = I18n::builtin();
        assert!(i18n.select("en"));
        assert_eq!(i18n.t("menu.view"), "View");
        assert!(!i18n.select("zz"));
        assert_eq!(i18n.code(), "en", "an unknown code changes nothing");
    }

    #[test]
    fn test_missing_keys_fall_back_to_indonesian_then_the_key() {
        let dir = lang_dir(
            "fallback",
            &[("xx.json", r#"{"name": "Test", "menu.file": "Fichier"}"#)],
        );
        let (mut i18n, errors) = I18n::load(std::slice::from_ref(&dir));
        assert!(errors.is_empty());
        assert!(i18n.select("xx"));
        assert_eq!(i18n.t("menu.file"), "Fichier");
        assert_eq!(i18n.t("menu.view"), "Tampilan");
        assert_eq!(i18n.t("no.such.key"), "no.such.key");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tf_replaces_placeholders() {
        let i18n = I18n::builtin();
        assert_eq!(
            i18n.tf("msg.open_failed", &[("error", "boom")]),
            "Gagal membuka file: boom"
        );
        assert_eq!(
            i18n.tf("status.document", &[("name", "a.rs"), ("modified", "")]),
            "Dokumen: a.rs"
        );
    }

    #[test]
    fn test_load_adds_and_merges_languages() {
        let dir = lang_dir(
            "merge",
            &[
                ("xx.json", r#"{"name": "Xx"}"#),
                ("en.json", r#"{"menu.view": "Look"}"#),
                ("notes.txt", "not a language"),
            ],
        );
        let (mut i18n, errors) = I18n::load(std::slice::from_ref(&dir));
        assert!(errors.is_empty());
        let codes: Vec<&str> = i18n.locales().map(|(code, _)| code).collect();
        assert_eq!(codes, vec!["id", "en", "xx"]);

        i18n.select("en");
        assert_eq!(i18n.t("menu.view"), "Look", "the file overrides the key");
        assert_eq!(i18n.t("menu.file"), "File", "other built-in keys stay");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_invalid_files_are_reported_once_and_skipped() {
        let dir = lang_dir(
            "invalid",
            &[
                ("bad.json", "{ not json"),
                ("nested.json", r#"{"a": {"b": 1}}"#),
            ],
        );
        // The same folder twice is read once.
        let (i18n, errors) = I18n::load(&[dir.clone(), dir.clone()]);
        let files: Vec<&str> = errors.iter().map(|e| e.file.as_str()).collect();
        assert_eq!(files, vec!["bad.json", "nested.json"]);
        assert!(errors.iter().all(|e| !e.error.is_empty()));
        assert_eq!(i18n.locales().count(), 2, "no language added");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_missing_folders_are_ignored() {
        let (i18n, errors) = I18n::load(&[Path::new("/no/such/rred/folder").to_path_buf()]);
        assert!(errors.is_empty());
        assert_eq!(i18n.locales().count(), 2);
    }

    #[test]
    fn test_choose_initial_precedence() {
        let pick = |saved, system| {
            let mut i18n = I18n::builtin();
            i18n.choose_initial(saved, system);
            i18n.code().to_owned()
        };
        assert_eq!(pick(Some("en"), Some("id")), "en", "saved choice wins");
        assert_eq!(
            pick(Some("zz"), Some("id")),
            "id",
            "unknown saved code is ignored"
        );
        assert_eq!(pick(None, Some("id")), "id", "system language");
        assert_eq!(
            pick(None, Some("fr")),
            "en",
            "English when the system language is missing"
        );
        assert_eq!(pick(None, None), "en");
    }

    #[test]
    fn test_language_code_from_locale() {
        assert_eq!(language_code("id_ID.UTF-8").as_deref(), Some("id"));
        assert_eq!(language_code("en_US").as_deref(), Some("en"));
        assert_eq!(language_code("de").as_deref(), Some("de"));
        assert_eq!(language_code("sr@latin").as_deref(), Some("sr"));
        assert_eq!(language_code("C"), None);
        assert_eq!(language_code("POSIX"), None);
        assert_eq!(language_code(""), None);
    }

    #[test]
    fn test_search_dirs_end_in_assets_lang() {
        let dirs = search_dirs();
        assert_eq!(dirs.len(), 2);
        assert!(dirs.iter().all(|d| d.ends_with("assets/lang")));
    }

    /// Every key passed to `t("…")` or `tf("…"` in the UI code exists in the
    /// Indonesian file, so no label ever shows as a raw key.
    #[test]
    fn test_every_key_used_in_code_exists() {
        let sources = [
            include_str!("app.rs"),
            include_str!("modals.rs"),
            include_str!("file_browser.rs"),
            include_str!("activity_bar.rs"),
        ];
        let known = keys(BUILTIN[0].1);
        let mut used = HashSet::new();
        for source in sources {
            for pattern in ["t(\"", "tf(\""] {
                for (at, _) in source.match_indices(pattern) {
                    let rest = &source[at + pattern.len()..];
                    let key = &rest[..rest.find('"').unwrap()];
                    // Translation keys are dotted; skips e.g. `select("en")`.
                    if key.contains('.') && !key.contains(' ') {
                        used.insert(key.to_owned());
                    }
                }
            }
        }
        assert!(used.len() > 30, "scanner found only {} keys", used.len());
        let missing: Vec<&String> = used.difference(&known).collect();
        assert!(missing.is_empty(), "keys missing from id.json: {missing:?}");
    }
}
