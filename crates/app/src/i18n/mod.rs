//! Interface languages: the text of the settings window and the tray menu.
//!
//! Each language is one [`Strings`] value in its own file. The app follows the Windows display
//! language unless the user picks another one, which is kept in
//! `HKCU\Software\CoolerCast\Language`. The CLI and the service stay in English.

use std::ptr;
use std::sync::atomic::{AtomicU8, Ordering};

use coolercast_core::win::{from_wide, wide};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

mod de;
mod en;
mod es;
mod fr;
mod it;
mod ja;
mod ko;
mod pl;
mod pt;
mod ru;
mod tr;
mod zh;

/// Declares [`Strings`]: one `&'static str` per field, so a language that misses one does not
/// compile. `{}` marks where [`fill`] puts its arguments.
macro_rules! strings {
    ($($(#[$doc:meta])* $field:ident,)*) => {
        pub struct Strings {
            $($(#[$doc])* pub $field: &'static str,)*
        }

        impl Strings {
            /// Every field with its name, to check the translations.
            #[cfg(test)]
            fn all(&self) -> Vec<(&'static str, &'static str)> {
                vec![$((stringify!($field), self.$field),)*]
            }
        }
    };
}

strings! {
    /// Decimal separator.
    decimal,
    /// Durations: seconds, minutes, and both.
    seconds,
    minutes,
    minutes_seconds,

    // Sidebar pages and the line under each title.
    overview,
    overview_about,
    display,
    display_about,
    custom_value,
    custom_about,
    display_off,
    display_off_about,
    alarm,
    alarm_about,
    general,
    general_about,

    // Display page.
    show,
    show_about,
    device,
    device_about,
    switch_every,
    switch_every_about,
    unit,
    unit_about,
    usage_bar,
    usage_bar_about,
    refresh_every,
    refresh_every_about,
    temperature,
    usage,
    alternate,
    custom,
    smart,

    // Custom value page.
    not_shown,
    not_shown_about,
    show_it,
    number,
    number_about,
    symbol,
    symbol_about,
    bar,
    bar_about,

    // Display off page.
    when_locked,
    when_locked_about,
    when_screen_off,
    when_screen_off_about,
    at_night,
    at_night_about,
    from,
    until,
    local_time,

    // Alarm page.
    blink,
    blink_about,
    threshold,
    threshold_about,

    // General page.
    language,
    language_about,
    windows_language,
    start_with_windows,
    start_with_windows_about,
    version,
    version_about,
    check_updates,
    checking,
    up_to_date,
    download,
    check_failed,

    // Switches.
    on,
    off,

    // Service state in the sidebar.
    service_down,
    service_down_detail,
    no_cooler,
    temperature_unavailable,
    waiting_for_cooler,
    running,

    // What the display shows, under the preview.
    not_connected,
    off_while_locked,
    off_while_screen_off,
    off_for_the_night,
    showing_temperature,
    showing_usage,
    showing_power,
    showing_custom,
    starting,
    too_hot,

    // Values next to the preview.
    cpu_temperature,
    cpu_usage,
    gpu_temperature,
    gpu_usage,
    cpu_power,
    cpu_clock,
    highest_temperature,
    highest_usage,

    // Chart.
    last,
    now,
    collecting_data,
    no_data,

    // Tray menu.
    settings,
    power_ls,
    gpu_while_busy,
    celsius,
    fahrenheit,
    turn_display_off,
    at_night_between,
    alarm_at,
    exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Es,
    Pt,
    Fr,
    De,
    It,
    Pl,
    Tr,
    Ru,
    Zh,
    Ja,
    Ko,
}

/// In the order of the language list.
pub const LANGS: [Lang; 12] = [
    Lang::En,
    Lang::Es,
    Lang::Pt,
    Lang::Fr,
    Lang::De,
    Lang::It,
    Lang::Pl,
    Lang::Tr,
    Lang::Ru,
    Lang::Zh,
    Lang::Ja,
    Lang::Ko,
];

impl Lang {
    /// BCP 47 tag, as stored in the registry.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Es => "es",
            Lang::Pt => "pt-BR",
            Lang::Fr => "fr",
            Lang::De => "de",
            Lang::It => "it",
            Lang::Pl => "pl",
            Lang::Tr => "tr",
            Lang::Ru => "ru",
            Lang::Zh => "zh-CN",
            Lang::Ja => "ja",
            Lang::Ko => "ko",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        LANGS.into_iter().find(|l| l.code() == code)
    }

    /// The name of the language in itself.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Es => "Español",
            Lang::Pt => "Português (Brasil)",
            Lang::Fr => "Français",
            Lang::De => "Deutsch",
            Lang::It => "Italiano",
            Lang::Pl => "Polski",
            Lang::Tr => "Türkçe",
            Lang::Ru => "Русский",
            Lang::Zh => "简体中文",
            Lang::Ja => "日本語",
            Lang::Ko => "한국어",
        }
    }

    pub fn strings(self) -> &'static Strings {
        match self {
            Lang::En => &en::STRINGS,
            Lang::Es => &es::STRINGS,
            Lang::Pt => &pt::STRINGS,
            Lang::Fr => &fr::STRINGS,
            Lang::De => &de::STRINGS,
            Lang::It => &it::STRINGS,
            Lang::Pl => &pl::STRINGS,
            Lang::Tr => &tr::STRINGS,
            Lang::Ru => &ru::STRINGS,
            Lang::Zh => &zh::STRINGS,
            Lang::Ja => &ja::STRINGS,
            Lang::Ko => &ko::STRINGS,
        }
    }

    /// The Windows UI font of languages Segoe UI does not cover.
    pub fn font(self) -> Option<&'static str> {
        match self {
            Lang::Zh => Some("Microsoft YaHei UI"),
            Lang::Ja => Some("Yu Gothic UI"),
            Lang::Ko => Some("Malgun Gothic"),
            _ => None,
        }
    }

    /// The language of a Windows `LANGID`, by its primary language; English if it has none.
    fn from_langid(id: u16) -> Self {
        match id & 0x3FF {
            0x0A => Lang::Es,
            0x16 => Lang::Pt,
            0x0C => Lang::Fr,
            0x07 => Lang::De,
            0x10 => Lang::It,
            0x15 => Lang::Pl,
            0x1F => Lang::Tr,
            0x19 => Lang::Ru,
            0x04 => Lang::Zh,
            0x11 => Lang::Ja,
            0x12 => Lang::Ko,
            _ => Lang::En,
        }
    }

    fn index(self) -> u8 {
        LANGS.iter().position(|&l| l == self).unwrap_or(0) as u8
    }
}

/// `CHOICE` value while the app follows the Windows language.
const SYSTEM: u8 = u8::MAX;

static CURRENT: AtomicU8 = AtomicU8::new(0);
static CHOICE: AtomicU8 = AtomicU8::new(SYSTEM);

const KEY: &str = r"Software\CoolerCast";
const VALUE: &str = "Language";

/// Reads the chosen language; called once at startup.
pub fn load() {
    let chosen = read_choice().as_deref().and_then(Lang::from_code);
    apply(chosen);
}

/// The language in use.
pub fn current() -> Lang {
    LANGS[usize::from(CURRENT.load(Ordering::Relaxed)).min(LANGS.len() - 1)]
}

/// The text of the language in use.
pub fn text() -> &'static Strings {
    current().strings()
}

/// The language picked by the user, or `None` to follow Windows.
pub fn choice() -> Option<Lang> {
    match CHOICE.load(Ordering::Relaxed) {
        SYSTEM => None,
        i => LANGS.get(usize::from(i)).copied(),
    }
}

/// The language Windows shows its own interface in.
pub fn system() -> Lang {
    Lang::from_langid(unsafe { GetUserDefaultUILanguage() })
}

/// Switches the language and remembers the choice.
pub fn choose(choice: Option<Lang>) {
    apply(choice);
    let (key, name) = (wide(KEY), wide(VALUE));
    match choice {
        Some(lang) => {
            let code = wide(lang.code());
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    name.as_ptr(),
                    REG_SZ,
                    code.as_ptr().cast(),
                    (code.len() * 2) as u32,
                )
            };
        }
        None => unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
        },
    }
}

fn apply(choice: Option<Lang>) {
    let lang = choice.unwrap_or_else(system);
    CURRENT.store(lang.index(), Ordering::Relaxed);
    CHOICE.store(choice.map_or(SYSTEM, Lang::index), Ordering::Relaxed);
}

fn read_choice() -> Option<String> {
    let (key, name) = (wide(KEY), wide(VALUE));
    let mut buf = [0u16; 32];
    let mut size = size_of_val(&buf) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then(|| from_wide(&buf))
}

/// `template` with each `{}` replaced by the next argument.
pub fn fill(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut args = args.iter();
    let mut parts = template.split("{}");
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    for part in parts {
        out.push_str(args.next().copied().unwrap_or_default());
        out.push_str(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_every_string() {
        let english = Lang::En.strings().all();
        for lang in LANGS {
            for ((field, text), (_, en)) in lang.strings().all().into_iter().zip(&english) {
                assert!(!text.trim().is_empty(), "{lang:?}.{field} is empty");
                assert_eq!(
                    text.matches("{}").count(),
                    en.matches("{}").count(),
                    "{lang:?}.{field} has other placeholders than English"
                );
            }
        }
    }

    #[test]
    fn decimal_separator_is_one_character() {
        for lang in LANGS {
            assert!(matches!(lang.strings().decimal, "." | ","), "{lang:?}");
        }
    }

    #[test]
    fn fill_replaces_in_order() {
        assert_eq!(
            fill("At night ({}–{})", &["23:00", "07:00"]),
            "At night (23:00–07:00)"
        );
        assert_eq!(fill("{} s", &["5"]), "5 s");
        assert_eq!(fill("no placeholder", &["x"]), "no placeholder");
        assert_eq!(fill("{} and {}", &["one"]), "one and ");
    }

    #[test]
    fn windows_languages_map_by_primary_language() {
        assert_eq!(Lang::from_langid(0x0C0A), Lang::Es); // es-ES
        assert_eq!(Lang::from_langid(0x080A), Lang::Es); // es-MX
        assert_eq!(Lang::from_langid(0x0816), Lang::Pt); // pt-PT
        assert_eq!(Lang::from_langid(0x0404), Lang::Zh); // zh-TW
        assert_eq!(Lang::from_langid(0x0411), Lang::Ja);
        assert_eq!(Lang::from_langid(0x0413), Lang::En); // nl-NL: not translated
    }

    #[test]
    fn codes_round_trip() {
        for lang in LANGS {
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
            assert_eq!(LANGS[usize::from(lang.index())], lang);
        }
        assert_eq!(Lang::from_code("xx"), None);
    }
}
