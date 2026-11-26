//! `lenient_enum!`: unit enums that read a name they do not know as a fallback variant.
//!
//! Stored rows outlive the build that wrote them, and agents send values Ghost has never seen, so
//! the enums that reach storage or come from agents read an unknown name as their fallback (`Other`
//! where the enum has one) instead of failing the whole event or history row. `#[serde(other)]` would
//! do the same, but through tauri-specta's export it widens the TypeScript union to `string` and drops
//! the fallback's own name, which defeats the UI's exhaustive switches. So `Serialize` stays derived,
//! which is also what specta reads, and this macro writes the `Deserialize` by hand.
//!
//! Each variant names its wire string next to it. The derived `Serialize` uses the enum's own serde
//! attributes, so the two must agree; the tests check every variant of every lenient enum.

/// Declares a unit enum with a lenient `Deserialize`, `ALL`, `FALLBACK`, `as_str` and `from_wire`.
///
/// ```text
/// lenient_enum! {
///     #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
///     #[serde(rename_all = "snake_case")]
///     pub enum Level {
///         Info = "info",
///         Warning = "warning",
///     }
///     fallback = Info;
/// }
/// ```
///
/// A non-string value (a number, `null`, an object) is still an error.
macro_rules! lenient_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident = $wire:literal,
            )+
        }
        fallback = $fallback:ident;
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $(
                $(#[$variant_meta])*
                $variant,
            )+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// What a name this build does not know reads as.
            pub const FALLBACK: $name = $name::$fallback;

            /// The variant's wire name, as serde writes it.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $wire,)+
                }
            }

            /// The variant named `wire`, or [`Self::FALLBACK`] for a name this build does not know.
            pub fn from_wire(wire: &str) -> $name {
                match wire {
                    $($wire => $name::$variant,)+
                    _ => $name::$fallback,
                }
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<$name, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct Visitor;

                impl serde::de::Visitor<'_> for Visitor {
                    type Value = $name;

                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(concat!("the name of a ", stringify!($name)))
                    }

                    fn visit_str<E>(self, value: &str) -> Result<$name, E>
                    where
                        E: serde::de::Error,
                    {
                        Ok($name::from_wire(value))
                    }
                }

                deserializer.deserialize_str(Visitor)
            }
        }
    };
}

pub(crate) use lenient_enum;
