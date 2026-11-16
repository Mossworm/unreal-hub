use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LanguagePreference {
    #[default]
    System,
    Korean,
    English,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Korean,
    English,
}

impl Language {
    pub fn code(self) -> &'static str {
        match self {
            Language::Korean => "ko",
            Language::English => "en",
        }
    }
}

pub fn language_for_locale(locale: Option<&str>) -> Language {
    match locale {
        Some(locale) if locale.to_ascii_lowercase().starts_with("ko") => Language::Korean,
        _ => Language::English,
    }
}

pub fn resolve(preference: LanguagePreference) -> Language {
    match preference {
        LanguagePreference::Korean => Language::Korean,
        LanguagePreference::English => Language::English,
        LanguagePreference::System => language_for_locale(sys_locale::get_locale().as_deref()),
    }
}

pub fn set_language(language: Language) {
    ENGLISH.store(language == Language::English, Ordering::Relaxed);
}

pub fn current() -> Language {
    if is_english() {
        Language::English
    } else {
        Language::Korean
    }
}

pub fn is_english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

macro_rules! tr {
    ($korean:expr, $english:expr $(,)?) => {
        if $crate::i18n::is_english() {
            $english
        } else {
            $korean
        }
    };
}

macro_rules! trf {
    ($korean:literal, $english:literal $(, $($argument:tt)*)?) => {
        if $crate::i18n::is_english() {
            format!($english $(, $($argument)*)?)
        } else {
            format!($korean $(, $($argument)*)?)
        }
    };
}

macro_rules! tr_anyhow {
    ($korean:literal, $english:literal $(, $($argument:tt)*)?) => {
        anyhow::anyhow!("{}", trf!($korean, $english $(, $($argument)*)?))
    };
}

macro_rules! tr_bail {
    ($korean:literal, $english:literal $(, $($argument:tt)*)?) => {
        return Err(tr_anyhow!($korean, $english $(, $($argument)*)?))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_korean_locales() {
        assert_eq!(language_for_locale(Some("ko-KR")), Language::Korean);
        assert_eq!(language_for_locale(Some("KO")), Language::Korean);
        assert_eq!(language_for_locale(Some("en-US")), Language::English);
        assert_eq!(language_for_locale(Some("ja-JP")), Language::English);
        assert_eq!(language_for_locale(None), Language::English);
    }

    #[test]
    fn formats_in_the_selected_language() {
        let count = 3;
        set_language(Language::English);
        assert_eq!(trf!("{count}개", "{count} items"), "3 items");
        assert_eq!(tr!("취소", "Cancel"), "Cancel");
        set_language(Language::Korean);
        assert_eq!(trf!("{count}개", "{count} items"), "3개");
        assert_eq!(tr!("취소", "Cancel"), "취소");
    }
}
