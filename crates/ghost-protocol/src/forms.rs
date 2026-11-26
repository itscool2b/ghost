//! Forms an MCP server asks for through an agent (MCP elicitation, form mode): the fields as the
//! UI shows them, and the check every answer passes before it reaches the agent. The actor and the
//! drivers run the same check, and the UI mirrors it (`lib/chat/forms.ts`).

use serde::{Deserialize, Serialize};
use specta::Type;

/// One field of a form. `id` is the schema property's name, which answers use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct UiField {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub required: bool,
    pub kind: UiFieldKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiFieldKind {
    Text {
        format: Option<UiTextFormat>,
        min_length: Option<u32>,
        max_length: Option<u32>,
        default: Option<String>,
    },
    Number {
        integer: bool,
        minimum: Option<f64>,
        maximum: Option<f64>,
        default: Option<f64>,
    },
    Boolean {
        default: Option<bool>,
    },
    /// One choice or, with `multiple`, any number within the bounds. Answers name choices by
    /// position, so shortening a label for display never changes what is sent.
    Choice {
        options: Vec<String>,
        multiple: bool,
        min_items: Option<u32>,
        max_items: Option<u32>,
        default: Vec<u32>,
    },
}

/// The string formats MCP forms can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum UiTextFormat {
    Email,
    Uri,
    Date,
    DateTime,
}

/// One answered field. A field left empty has no value at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct UiFieldValue {
    pub id: String,
    pub value: UiValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiValue {
    Text {
        text: String,
    },
    Number {
        number: f64,
    },
    Boolean {
        value: bool,
    },
    /// Positions in the field's options, each at most once.
    Choices {
        selected: Vec<u32>,
    },
}

/// Whether an answer fits the form: a declined form carries no values; an accepted one answers
/// each field at most once, with a value its field takes, and every required field.
pub fn check_form(fields: &[UiField], accept: bool, values: &[UiFieldValue]) -> Result<(), String> {
    if !accept {
        return if values.is_empty() {
            Ok(())
        } else {
            Err("A declined form carries no answers.".into())
        };
    }
    for (index, value) in values.iter().enumerate() {
        if values[..index].iter().any(|other| other.id == value.id) {
            return Err(format!("{} is answered twice.", value.id));
        }
        let field = fields
            .iter()
            .find(|field| field.id == value.id)
            .ok_or_else(|| format!("{} is not part of this form.", value.id))?;
        field.check(&value.value)?;
    }
    if let Some(missing) = fields
        .iter()
        .find(|field| field.required && !values.iter().any(|value| value.id == field.id))
    {
        return Err(format!("{} is required.", missing.label));
    }
    Ok(())
}

impl UiField {
    /// Whether `value` fits this field: its kind, bounds, format and choices.
    pub fn check(&self, value: &UiValue) -> Result<(), String> {
        let name = &self.label;
        match (&self.kind, value) {
            (
                UiFieldKind::Text {
                    format,
                    min_length,
                    max_length,
                    ..
                },
                UiValue::Text { text },
            ) => {
                let length = text.chars().count();
                if let Some(min) = min_length
                    && length < *min as usize
                {
                    return Err(format!("{name} needs at least {min} characters."));
                }
                if let Some(max) = max_length
                    && length > *max as usize
                {
                    return Err(format!("{name} takes at most {max} characters."));
                }
                if let Some(format) = format
                    && !format.accepts(text)
                {
                    return Err(format!("{name} must be {}.", format.describe()));
                }
                Ok(())
            }
            (
                UiFieldKind::Number {
                    integer,
                    minimum,
                    maximum,
                    ..
                },
                UiValue::Number { number },
            ) => {
                if !number.is_finite() {
                    return Err(format!("{name} must be a number."));
                }
                if *integer && number.fract() != 0.0 {
                    return Err(format!("{name} must be a whole number."));
                }
                if let Some(min) = minimum
                    && number < min
                {
                    return Err(format!("{name} must be at least {min}."));
                }
                if let Some(max) = maximum
                    && number > max
                {
                    return Err(format!("{name} must be at most {max}."));
                }
                Ok(())
            }
            (UiFieldKind::Boolean { .. }, UiValue::Boolean { .. }) => Ok(()),
            (
                UiFieldKind::Choice {
                    options,
                    multiple,
                    min_items,
                    max_items,
                    ..
                },
                UiValue::Choices { selected },
            ) => {
                let distinct = selected
                    .iter()
                    .enumerate()
                    .all(|(at, index)| !selected[..at].contains(index));
                if !distinct
                    || selected
                        .iter()
                        .any(|index| *index as usize >= options.len())
                {
                    return Err(format!("{name} has a choice it does not offer."));
                }
                if !multiple {
                    return if selected.len() == 1 {
                        Ok(())
                    } else {
                        Err(format!("Choose one option for {name}."))
                    };
                }
                if let Some(min) = min_items
                    && selected.len() < *min as usize
                {
                    return Err(format!("Choose at least {min} for {name}."));
                }
                if let Some(max) = max_items
                    && selected.len() > *max as usize
                {
                    return Err(format!("Choose at most {max} for {name}."));
                }
                Ok(())
            }
            _ => Err(format!("{name} has the wrong kind of answer.")),
        }
    }
}

impl UiTextFormat {
    /// RFC 3339 dates and date-times (with seconds and an offset, as a browser's ISO time has),
    /// a URI with a scheme, and an address with a single @ and a dotted domain.
    pub fn accepts(self, text: &str) -> bool {
        if text.is_empty() || text.chars().any(char::is_whitespace) {
            return false;
        }
        match self {
            Self::Email => text.split_once('@').is_some_and(|(local, domain)| {
                !local.is_empty()
                    && !domain.contains('@')
                    && domain
                        .split_once('.')
                        .is_some_and(|(name, rest)| !name.is_empty() && !rest.is_empty())
            }),
            Self::Uri => text.split_once(':').is_some_and(|(scheme, rest)| {
                !rest.is_empty()
                    && scheme
                        .chars()
                        .next()
                        .is_some_and(|first| first.is_ascii_alphabetic())
                    && scheme
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
            }),
            Self::Date => date(text),
            Self::DateTime => text
                .split_once(['T', 't'])
                .is_some_and(|(day, time)| date(day) && time_of_day(time)),
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Email => "an email address",
            Self::Uri => "a web address or other URI",
            Self::Date => "a date",
            Self::DateTime => "a date and time",
        }
    }
}

fn digits(text: &str, count: usize) -> Option<u32> {
    (text.len() == count && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

fn date(text: &str) -> bool {
    let mut parts = text.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    digits(year, 4).is_some()
        && digits(month, 2).is_some_and(|month| (1..=12).contains(&month))
        && digits(day, 2).is_some_and(|day| (1..=31).contains(&day))
}

/// `HH:MM:SS`, optional fractional seconds, then `Z` or `±HH:MM`.
fn time_of_day(text: &str) -> bool {
    let (clock, offset) = if let Some(clock) = text.strip_suffix(['Z', 'z']) {
        (clock, None)
    } else if let Some(at) = text.rfind(['+', '-']) {
        (&text[..at], Some(&text[at + 1..]))
    } else {
        return false;
    };
    let clock = clock.split_once('.').map_or(clock, |(whole, fraction)| {
        if !fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit()) {
            whole
        } else {
            ""
        }
    });
    let hours_minutes = |text: &str, seconds: bool| {
        let parts: Vec<&str> = text.split(':').collect();
        parts.len() == if seconds { 3 } else { 2 }
            && digits(parts[0], 2).is_some_and(|hour| hour < 24)
            && digits(parts[1], 2).is_some_and(|minute| minute < 60)
            && (!seconds || digits(parts[2], 2).is_some_and(|second| second <= 60))
    };
    hours_minutes(clock, true) && offset.is_none_or(|offset| hours_minutes(offset, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(id: &str, required: bool, kind: UiFieldKind) -> UiField {
        UiField {
            id: id.into(),
            label: id.into(),
            description: None,
            required,
            kind,
        }
    }

    fn text(id: &str, value: &str) -> UiFieldValue {
        UiFieldValue {
            id: id.into(),
            value: UiValue::Text { text: value.into() },
        }
    }

    #[test]
    fn a_form_answers_each_field_once_and_every_required_one() {
        let fields = [
            field(
                "name",
                true,
                UiFieldKind::Text {
                    format: None,
                    min_length: Some(2),
                    max_length: Some(5),
                    default: None,
                },
            ),
            field("note", false, UiFieldKind::Boolean { default: None }),
        ];
        assert!(check_form(&fields, true, &[text("name", "Ada")]).is_ok());
        assert!(check_form(&fields, true, &[]).is_err(), "required");
        assert!(
            check_form(&fields, true, &[text("name", "A")]).is_err(),
            "too short"
        );
        assert!(check_form(&fields, true, &[text("name", "Adaline")]).is_err());
        assert!(check_form(&fields, true, &[text("name", "Ada"), text("name", "Bo")]).is_err());
        assert!(check_form(&fields, true, &[text("name", "Ada"), text("other", "x")]).is_err());
        assert!(
            check_form(
                &fields,
                true,
                &[UiFieldValue {
                    id: "name".into(),
                    value: UiValue::Boolean { value: true },
                }]
            )
            .is_err(),
            "wrong kind"
        );
        // Declining sends nothing back.
        assert!(check_form(&fields, false, &[]).is_ok());
        assert!(check_form(&fields, false, &[text("name", "Ada")]).is_err());
    }

    #[test]
    fn numbers_choices_and_formats_keep_their_bounds() {
        let count = field(
            "count",
            true,
            UiFieldKind::Number {
                integer: true,
                minimum: Some(1.0),
                maximum: Some(10.0),
                default: None,
            },
        );
        let number = |number: f64| UiValue::Number { number };
        assert!(count.check(&number(3.0)).is_ok());
        assert!(count.check(&number(3.5)).is_err());
        assert!(count.check(&number(0.0)).is_err());
        assert!(count.check(&number(11.0)).is_err());
        assert!(count.check(&number(f64::NAN)).is_err());
        let tags = field(
            "tags",
            false,
            UiFieldKind::Choice {
                options: vec!["a".into(), "b".into(), "c".into()],
                multiple: true,
                min_items: Some(1),
                max_items: Some(2),
                default: vec![],
            },
        );
        let choices = |selected: &[u32]| UiValue::Choices {
            selected: selected.to_vec(),
        };
        assert!(tags.check(&choices(&[0, 2])).is_ok());
        assert!(tags.check(&choices(&[])).is_err());
        assert!(tags.check(&choices(&[0, 1, 2])).is_err());
        assert!(tags.check(&choices(&[0, 0])).is_err());
        assert!(tags.check(&choices(&[3])).is_err());
        let one = field(
            "one",
            true,
            UiFieldKind::Choice {
                options: vec!["a".into(), "b".into()],
                multiple: false,
                min_items: None,
                max_items: None,
                default: vec![],
            },
        );
        assert!(one.check(&choices(&[1])).is_ok());
        assert!(one.check(&choices(&[0, 1])).is_err());

        use UiTextFormat::*;
        for (format, good, bad) in [
            (Email, "ada@example.com", "ada@example"),
            (Email, "a.b+c@mail.example.org", "@example.com"),
            (Uri, "https://example.com/a?b", "example.com"),
            (Uri, "mailto:ada@example.com", "1http://x"),
            (Date, "2026-11-05", "2026-13-05"),
            (Date, "2026-02-28", "26-02-28"),
            (DateTime, "2026-11-05T08:30:00.000Z", "2026-11-05T08:30Z"),
            (
                DateTime,
                "2026-11-05T08:30:00+01:00",
                "2026-11-05 08:30:00Z",
            ),
        ] {
            assert!(format.accepts(good), "{format:?} {good}");
            assert!(!format.accepts(bad), "{format:?} {bad}");
        }
    }
}
