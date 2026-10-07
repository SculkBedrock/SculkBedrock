use rust_i18n::i18n;
pub use rust_i18n::{locale, replace_patterns};
pub use rust_i18n_macro::key;
pub use sc_log_macros::colorize_literal;

pub mod color;
pub mod file;
i18n!("locales");

/// `t!`: i18n lookup (**verbatim**, keeps section codes).
///
/// For player-visible text or anything that must keep section codes (chat, command output, error values);
/// performs no color conversion. For log output use [`t_log!`] (pre-colored at build time, zero runtime cost).
#[macro_export]
#[allow(clippy::crate_in_macro_def)]
macro_rules! t {
    // t!("foo")
    ($key:expr) => {
        $crate::_rust_i18n_translate(&$crate::locale(), $key)
    };

    // t!("foo", locale = "en")
    ($key:expr, locale = $locale:expr) => {
        $crate::_rust_i18n_translate($locale, $key)
    };

    // t!("foo", locale = "en", a = 1, b = "Foo")
    ($key:expr, locale = $locale:expr, $($var_name:tt = $var_val:expr),+ $(,)?) => {
        {
            let message = $crate::_rust_i18n_translate($locale, $key);
            let patterns: &[&str] = &[
                $($crate::key!($var_name)),+
            ];
            let values = &[
                $(format!("{}", $var_val)),+
            ];

            let output = $crate::replace_patterns(message.as_ref(), patterns, values);
            std::borrow::Cow::from(output)
        }
    };

    // t!("foo %{a} %{b}", a = "bar", b = "baz")
    ($key:expr, $($var_name:tt = $var_val:expr),+ $(,)?) => {
        {
            $crate::t!($key, locale = &$crate::locale(), $($var_name = $var_val),*)
        }
    };

    // t!("foo %{a} %{b}", locale = "en", "a" => "bar", "b" => "baz")
    ($key:expr, locale = $locale:expr, $($var_name:tt => $var_val:expr),+ $(,)?) => {
        {
            $crate::t!($key, locale = $locale, $($var_name = $var_val),*)
        }
    };

    // t!("foo %{a} %{b}", "a" => "bar", "b" => "baz")
    ($key:expr, $($var_name:tt => $var_val:expr),+ $(,)?) => {
        {
            $crate::t!($key, locale = &$crate::locale(), $($var_name = $var_val),*)
        }
    };
}

/// `t_log!`: i18n lookup for **log output** (color conversion done at build time).
///
/// - ANSI available (terminal): hits the build-time pre-colored `COLORED_LOCALES` table, codes already ANSI;
/// - Otherwise (NO_COLOR / TERM=dumb / file): hits the stripped `PLAIN_LOCALES` table;
/// - Unknown keys fall back to [`t!`] (verbatim codes, converted as fallback by `ColorizingWriter`).
///
/// Difference from [`t!`]: `t!` keeps verbatim codes (player-visible text), `t_log!` emits colored text.
#[macro_export]
#[allow(clippy::crate_in_macro_def)]
macro_rules! t_log {
    // t_log!("foo")
    ($key:expr) => {
        $crate::color::translate(&$crate::locale(), $key)
    };

    // t_log!("foo", locale = "en")
    ($key:expr, locale = $locale:expr) => {
        $crate::color::translate($locale, $key)
    };

    // t_log!("foo", locale = "en", a = 1, b = "Foo")
    ($key:expr, locale = $locale:expr, $($var_name:tt = $var_val:expr),+ $(,)?) => {
        {
            let message = $crate::color::translate($locale, $key);
            let patterns: &[&str] = &[
                $($crate::key!($var_name)),+
            ];
            let values = &[
                $(format!("{}", $var_val)),+
            ];

            let output = $crate::replace_patterns(message.as_ref(), patterns, values);
            std::borrow::Cow::from(output)
        }
    };

    // t_log!("foo %{a} %{b}", a = "bar", b = "baz")
    ($key:expr, $($var_name:tt = $var_val:expr),+ $(,)?) => {
        {
            $crate::t_log!($key, locale = &$crate::locale(), $($var_name = $var_val),*)
        }
    };

    // t_log!("foo %{a} %{b}", locale = "en", "a" => "bar", "b" => "baz")
    ($key:expr, locale = $locale:expr, $($var_name:tt => $var_val:expr),+ $(,)?) => {
        {
            $crate::t_log!($key, locale = $locale, $($var_name = $var_val),*)
        }
    };

    // t_log!("foo %{a} %{b}", "a" => "bar", "b" => "baz")
    ($key:expr, $($var_name:tt => $var_val:expr),+ $(,)?) => {
        {
            $crate::t_log!($key, locale = &$crate::locale(), $($var_name = $var_val),*)
        }
    };
}

pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}
