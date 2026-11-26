//! MCP elicitation, which Claude forwards as an `elicitation` control request and Codex as
//! `mcpServer/elicitation/request`: a form ([`UiPrompt::Form`]) or a step in the browser
//! ([`UiPrompt::Link`]). Only the MCP 2025-11-25 form schema is shown (strings with their formats
//! and lengths, numbers, booleans, single and multiple choices). A form that uses anything else, or
//! would not fit on screen, is refused before display, never half shown. These are user input:
//! Full auto never answers them.

use ghost_protocol::forms::{UiField, UiFieldKind, UiTextFormat, UiValue, check_form};
use ghost_protocol::{UiAnswer, UiPrompt};
use serde_json::{Map, Value, json};

const MAX_FIELDS: usize = 32;
const MAX_OPTIONS: usize = 64;
const MAX_VALUE_CHARS: usize = 1024;
const MAX_URL_BYTES: usize = 4096;
/// What the prompt may serialize to, so the session's normalizer never has to cut a field.
const MAX_PROMPT_BYTES: usize = 6000;

/// What an answer maps back to, kept by the driver until its request resolves.
#[derive(Debug, Clone, PartialEq)]
pub struct Elicitation {
    kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    /// The fields as shown, and each choice field's option values in their displayed order.
    Form {
        fields: Vec<UiField>,
        values: Vec<Option<Vec<String>>>,
    },
    Link,
}

/// A form from the server's requested schema. `order` is the server's property order where the
/// agent reports it (Claude does); the schema's own order follows.
pub fn form(
    message: &str,
    source: Option<&str>,
    schema: &Value,
    order: Option<&[String]>,
) -> Result<(UiPrompt, Elicitation), String> {
    let schema = schema
        .as_object()
        .ok_or("The form came without its fields.")?;
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err("The form's fields are not set out as Ghost can show them.".into());
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .filter(|properties| !properties.is_empty())
        .ok_or("The form came without its fields.")?;
    if properties.len() > MAX_FIELDS {
        return Err("The form has more fields than Ghost shows.".into());
    }
    let required: Vec<&str> = match schema.get("required") {
        None | Some(Value::Null) => Vec::new(),
        Some(required) => required
            .as_array()
            .and_then(|names| names.iter().map(Value::as_str).collect::<Option<_>>())
            .ok_or("The form's required fields are not a list of names.")?,
    };
    let mut keys: Vec<&String> = order
        .unwrap_or_default()
        .iter()
        .filter(|key| properties.contains_key(*key))
        .collect();
    for key in properties.keys() {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    let mut fields = Vec::new();
    let mut values = Vec::new();
    for key in keys {
        let property = properties[key]
            .as_object()
            .ok_or("A form field is not set out as Ghost can show it.")?;
        let (field, choices) = field(key, property, required.contains(&key.as_str()))?;
        fields.push(field);
        values.push(choices);
    }
    let prompt = UiPrompt::Form {
        message: message.to_owned(),
        source: source.map(str::to_owned),
        fields: fields.clone(),
    };
    if serde_json::to_vec(&prompt).map_or(true, |bytes| bytes.len() > MAX_PROMPT_BYTES) {
        return Err("The form is larger than Ghost shows.".into());
    }
    Ok((
        prompt,
        Elicitation {
            kind: Kind::Form { fields, values },
        },
    ))
}

/// A step in the browser, for an http or https address only.
pub fn link(
    message: &str,
    source: Option<&str>,
    url: &str,
) -> Result<(UiPrompt, Elicitation), String> {
    let web = url.starts_with("https://") || url.starts_with("http://");
    if !web || url.len() > MAX_URL_BYTES || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("The server asked to open an address Ghost does not open.".into());
    }
    Ok((
        UiPrompt::Link {
            message: message.to_owned(),
            source: source.map(str::to_owned),
            url: url.to_owned(),
        },
        Elicitation { kind: Kind::Link },
    ))
}

impl Elicitation {
    /// The MCP result for an answer: dismissing cancels, a form declines or sends its values, a
    /// browser step accepts once opened. `None` when the answer does not fit, so the request stays
    /// open rather than send something the user did not mean.
    pub fn result(&self, answer: Option<&UiAnswer>) -> Option<Value> {
        let Some(answer) = answer else {
            return Some(json!({"action": "cancel"}));
        };
        match (&self.kind, answer) {
            (Kind::Link, UiAnswer::Confirm { yes }) => {
                Some(json!({"action": if *yes { "accept" } else { "decline" }}))
            }
            (
                Kind::Form { .. },
                UiAnswer::Form {
                    accept: false,
                    values,
                },
            ) if values.is_empty() => Some(json!({"action": "decline"})),
            (
                Kind::Form {
                    fields,
                    values: options,
                },
                UiAnswer::Form {
                    accept: true,
                    values,
                },
            ) => {
                check_form(fields, true, values).ok()?;
                let mut content = Map::new();
                for answer in values {
                    let index = fields.iter().position(|field| field.id == answer.id)?;
                    let value = match (&answer.value, &fields[index].kind) {
                        (UiValue::Text { text }, _) => json!(text),
                        (UiValue::Number { number }, UiFieldKind::Number { integer: true, .. }) => {
                            json!(*number as i64)
                        }
                        (UiValue::Number { number }, _) => json!(number),
                        (UiValue::Boolean { value }, _) => json!(value),
                        (UiValue::Choices { selected }, UiFieldKind::Choice { multiple, .. }) => {
                            let names = options[index].as_ref()?;
                            let picked = selected
                                .iter()
                                .map(|at| names.get(*at as usize))
                                .collect::<Option<Vec<_>>>()?;
                            if *multiple {
                                json!(picked)
                            } else {
                                json!(picked.first()?)
                            }
                        }
                        _ => return None,
                    };
                    content.insert(answer.id.clone(), value);
                }
                Some(json!({"action": "accept", "content": content}))
            }
            _ => None,
        }
    }
}

fn text(property: &Map<String, Value>, key: &str) -> Option<String> {
    property
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn count(property: &Map<String, Value>, key: &str) -> Result<Option<u32>, String> {
    match property.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| format!("A form field's {key} is not a whole number.")),
    }
}

fn number(property: &Map<String, Value>, key: &str) -> Result<Option<f64>, String> {
    match property.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or_else(|| format!("A form field's {key} is not a number.")),
    }
}

/// Option values and their labels: `enum` (labelled by `enumNames` where given) or `oneOf`/`anyOf`
/// of `{const, title}`.
fn choices(source: &Map<String, Value>) -> Result<(Vec<String>, Vec<String>), String> {
    let strings = |list: &Value| -> Option<Vec<String>> {
        list.as_array()?
            .iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect()
    };
    let (values, labels) = if let Some(list) = source.get("enum") {
        let values = strings(list).ok_or("A form choice is not text.")?;
        let labels = match source.get("enumNames") {
            None | Some(Value::Null) => values.clone(),
            Some(names) => strings(names)
                .filter(|names| names.len() == values.len())
                .ok_or("A form choice's names do not match its values.")?,
        };
        (values, labels)
    } else if let Some(list) = source.get("oneOf").or_else(|| source.get("anyOf")) {
        list.as_array()
            .ok_or("A form's choices are not a list.")?
            .iter()
            .map(|option| {
                let value = option["const"].as_str()?.to_owned();
                let label = option["title"].as_str().unwrap_or(&value).to_owned();
                Some((value, label))
            })
            .collect::<Option<Vec<_>>>()
            .ok_or("A form choice is missing its value.")?
            .into_iter()
            .unzip()
    } else {
        return Err("A form choice has no options.".into());
    };
    if values.is_empty()
        || values.len() > MAX_OPTIONS
        || values
            .iter()
            .chain(&labels)
            .any(|text| text.chars().count() > MAX_VALUE_CHARS)
    {
        return Err("A form choice has more options than Ghost shows.".into());
    }
    if values
        .iter()
        .enumerate()
        .any(|(at, value)| values[..at].contains(value))
    {
        return Err("A form choice offers the same value twice.".into());
    }
    Ok((values, labels))
}

fn field(
    id: &str,
    property: &Map<String, Value>,
    required: bool,
) -> Result<(UiField, Option<Vec<String>>), String> {
    if id.is_empty() || id.chars().count() > MAX_VALUE_CHARS {
        return Err("A form field has no usable name.".into());
    }
    let index_of = |values: &[String], value: Option<&Value>| -> Vec<u32> {
        value
            .and_then(Value::as_str)
            .and_then(|value| values.iter().position(|option| option == value))
            .map(|at| vec![at as u32])
            .unwrap_or_default()
    };
    let (kind, values) = match property.get("type").and_then(Value::as_str) {
        Some("string") if property.contains_key("enum") || property.contains_key("oneOf") => {
            let (values, labels) = choices(property)?;
            let default = index_of(&values, property.get("default"));
            (
                UiFieldKind::Choice {
                    options: labels,
                    multiple: false,
                    min_items: None,
                    max_items: None,
                    default,
                },
                Some(values),
            )
        }
        Some("string") => (
            UiFieldKind::Text {
                // A format Ghost does not check is left to the server.
                format: match property.get("format").and_then(Value::as_str) {
                    Some("email") => Some(UiTextFormat::Email),
                    Some("uri") => Some(UiTextFormat::Uri),
                    Some("date") => Some(UiTextFormat::Date),
                    Some("date-time") => Some(UiTextFormat::DateTime),
                    _ => None,
                },
                min_length: count(property, "minLength")?,
                max_length: count(property, "maxLength")?,
                default: text(property, "default"),
            },
            None,
        ),
        Some(kind @ ("number" | "integer")) => (
            UiFieldKind::Number {
                integer: kind == "integer",
                minimum: number(property, "minimum")?,
                maximum: number(property, "maximum")?,
                default: number(property, "default")?,
            },
            None,
        ),
        Some("boolean") => (
            UiFieldKind::Boolean {
                default: property.get("default").and_then(Value::as_bool),
            },
            None,
        ),
        Some("array") => {
            let items = property
                .get("items")
                .and_then(Value::as_object)
                .ok_or("A form's multiple choice has no options.")?;
            let (values, labels) = choices(items)?;
            let default = property
                .get("default")
                .and_then(Value::as_array)
                .map(|picked| {
                    picked
                        .iter()
                        .filter_map(|value| index_of(&values, Some(value)).first().copied())
                        .collect()
                })
                .unwrap_or_default();
            (
                UiFieldKind::Choice {
                    options: labels,
                    multiple: true,
                    min_items: count(property, "minItems")?,
                    max_items: count(property, "maxItems")?,
                    default,
                },
                Some(values),
            )
        }
        _ => return Err("The form has a kind of field Ghost cannot show.".into()),
    };
    Ok((
        UiField {
            id: id.to_owned(),
            label: text(property, "title").unwrap_or_else(|| id.to_owned()),
            description: text(property, "description"),
            required,
            kind,
        },
        values,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghost_protocol::forms::UiFieldValue;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "title": "Project name", "minLength": 2},
                "count": {"type": "integer", "minimum": 1, "maximum": 5, "default": 2},
                "notify": {"type": "boolean", "default": true},
                "region": {"type": "string", "oneOf": [
                    {"const": "eu-west-1", "title": "Ireland"},
                    {"const": "us-east-1", "title": "Virginia"}
                ], "default": "us-east-1"},
                "tags": {"type": "array", "items": {"type": "string", "enum": ["web", "api", "db"]},
                    "maxItems": 2},
                "email": {"type": "string", "format": "email", "description": "For alerts"}
            },
            "required": ["name", "region"]
        })
    }

    fn value(id: &str, value: UiValue) -> UiFieldValue {
        UiFieldValue {
            id: id.into(),
            value,
        }
    }

    #[test]
    fn a_form_shows_every_field_kind_in_the_servers_order_and_maps_answers_back() {
        let order = ["region".to_owned(), "name".to_owned()];
        let (prompt, spec) = form(
            "Set up the project",
            Some("deploy"),
            &schema(),
            Some(&order),
        )
        .unwrap();
        let UiPrompt::Form { fields, source, .. } = &prompt else {
            panic!("form")
        };
        assert_eq!(source.as_deref(), Some("deploy"));
        let ids: Vec<_> = fields.iter().map(|field| field.id.as_str()).collect();
        assert_eq!(ids, ["region", "name", "count", "notify", "tags", "email"]);
        assert_eq!(fields[1].label, "Project name");
        assert!(fields[0].required && fields[1].required && !fields[2].required);
        let UiFieldKind::Choice {
            options, default, ..
        } = &fields[0].kind
        else {
            panic!("choice")
        };
        assert_eq!(options, &["Ireland", "Virginia"]);
        assert_eq!(default, &[1]);
        let answer = UiAnswer::Form {
            accept: true,
            values: vec![
                value("region", UiValue::Choices { selected: vec![0] }),
                value(
                    "name",
                    UiValue::Text {
                        text: "Aura".into(),
                    },
                ),
                value("count", UiValue::Number { number: 3.0 }),
                value(
                    "tags",
                    UiValue::Choices {
                        selected: vec![2, 0],
                    },
                ),
            ],
        };
        assert_eq!(
            spec.result(Some(&answer)),
            Some(json!({"action": "accept", "content": {
                "region": "eu-west-1", "name": "Aura", "count": 3, "tags": ["db", "web"]
            }}))
        );
        // An answer that does not fit keeps the request open; declining and dismissing do not.
        let short = UiAnswer::Form {
            accept: true,
            values: vec![
                value("region", UiValue::Choices { selected: vec![0] }),
                value("name", UiValue::Text { text: "A".into() }),
            ],
        };
        assert_eq!(spec.result(Some(&short)), None);
        let declined = UiAnswer::Form {
            accept: false,
            values: vec![],
        };
        assert_eq!(
            spec.result(Some(&declined)),
            Some(json!({"action": "decline"}))
        );
        assert_eq!(spec.result(None), Some(json!({"action": "cancel"})));
    }

    #[test]
    fn what_cannot_be_shown_is_refused_before_display() {
        for schema in [
            json!({"type": "object", "properties": {}}),
            json!({"type": "object", "properties": {"x": {"type": "object"}}}),
            json!({"type": "object", "properties": {"x": {"type": "string", "enum": ["a", "a"]}}}),
            json!({"type": "object", "properties": {"x": {"type": "string", "enum": ["a"], "enumNames": ["A", "B"]}}}),
            json!({"type": "object", "properties": {"x": {"type": "integer", "minimum": "one"}}}),
            json!({"type": "array"}),
        ] {
            assert!(
                form("Fill this in", None, &schema, None).is_err(),
                "{schema}"
            );
        }
        let many: Map<String, Value> = (0..40)
            .map(|at| (format!("field{at}"), json!({"type": "string"})))
            .collect();
        assert!(
            form(
                "Many",
                None,
                &json!({"type": "object", "properties": many}),
                None
            )
            .is_err()
        );
        let long = "x".repeat(400);
        let wide: Map<String, Value> = (0..20)
            .map(|at| {
                (
                    format!("field{at}"),
                    json!({"type": "string", "description": long}),
                )
            })
            .collect();
        assert!(
            form(
                "Wide",
                None,
                &json!({"type": "object", "properties": wide}),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn a_browser_step_opens_only_web_addresses() {
        let (prompt, spec) = link(
            "Sign in to continue",
            Some("github"),
            "https://example.com/auth?x=1",
        )
        .unwrap();
        assert!(matches!(prompt, UiPrompt::Link { .. }));
        assert_eq!(
            spec.result(Some(&UiAnswer::Confirm { yes: true })),
            Some(json!({"action": "accept"}))
        );
        assert_eq!(
            spec.result(Some(&UiAnswer::Confirm { yes: false })),
            Some(json!({"action": "decline"}))
        );
        assert_eq!(
            spec.result(Some(&UiAnswer::Input { text: "x".into() })),
            None
        );
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://exa mple.com",
            "",
        ] {
            assert!(link("Open", None, url).is_err(), "{url}");
        }
    }
}
