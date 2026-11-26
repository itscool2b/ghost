//! Deferred Cursor callbacks and OMP 18.12.6 form elicitation. These are user input, never
//! permissions: Full auto cannot answer them. Unsupported schemas fail explicitly.

use std::collections::HashSet;

use ghost_protocol::{UiAnswer, UiPrompt, UiQuestion, UiQuestionOption};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::dialect::DialectReply;
use crate::jsonrpc::codes;

const MAX_CALLBACK_BYTES: usize = 64 * 1024;
const MAX_QUESTIONS: usize = 32;
const MAX_OPTIONS: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct UiCall {
    pub prompt: UiPrompt,
    /// Full vendor plan text, rendered as transcript deltas before the approval card so the
    /// card's normal size limit cannot turn approval into approval of an unseen truncated plan.
    pub context: Option<String>,
    response: Response,
}

#[derive(Debug, Clone, PartialEq)]
enum Response {
    CursorQuestions(Vec<(String, Vec<String>)>),
    CursorPlan,
    Form(Vec<Field>),
}

#[derive(Debug, Clone, PartialEq)]
struct Field {
    key: String,
    kind: FieldKind,
}

#[derive(Debug, Clone, PartialEq)]
enum FieldKind {
    Text,
    Boolean,
    Choices {
        values: Vec<String>,
        options: Vec<UiQuestionOption>,
        multiple: bool,
        other_key: Option<String>,
    },
}

impl UiCall {
    pub fn cancelled(&self) -> Value {
        match self.response {
            Response::Form(_) => json!({"action": "cancel"}),
            _ => json!({"outcome": {"outcome": "cancelled"}}),
        }
    }

    /// `None` is an invalid answer; retain the request so a stale or malformed answer cannot
    /// silently become approval or consume the real user's pending dialog.
    pub fn answer(&self, answer: &UiAnswer) -> Option<Value> {
        match (&self.response, &self.prompt, answer) {
            (Response::CursorPlan, _, UiAnswer::Confirm { yes }) => Some(json!({
                "outcome": {"outcome": if *yes { "accepted" } else { "rejected" }}
            })),
            (
                Response::CursorQuestions(ids),
                UiPrompt::Questions { questions, .. },
                UiAnswer::Questions { answers },
            ) => {
                if !valid_answers(questions, answers) {
                    return None;
                }
                let answers: Vec<Value> = ids
                    .iter()
                    .map(|(id, options)| {
                        let answer = answers.iter().find(|answer| answer.question_id == *id)?;
                        Some(
                            json!({"questionId": id, "selectedOptionIds": answer.selected.iter()
                        .map(|index| &options[*index as usize]).collect::<Vec<_>>()}),
                        )
                    })
                    .collect::<Option<_>>()?;
                Some(json!({"outcome": {"outcome": "answered", "answers": answers}}))
            }
            (Response::Form(fields), UiPrompt::Input { .. }, UiAnswer::Input { text }) => {
                let content =
                    Map::from_iter([(fields.first()?.key.clone(), Value::String(text.clone()))]);
                Some(json!({"action": "accept", "content": content}))
            }
            (
                Response::Form(fields),
                UiPrompt::Questions { questions, .. },
                UiAnswer::Questions { answers },
            ) => {
                if !valid_answers(questions, answers) {
                    return None;
                }
                let mut content = Map::new();
                for field in fields {
                    let answer = answers
                        .iter()
                        .find(|answer| answer.question_id == field.key)?;
                    let value = match &field.kind {
                        FieldKind::Text => Value::String(answer.text.clone()?),
                        FieldKind::Boolean => Value::Bool(*answer.selected.first()? == 0),
                        FieldKind::Choices {
                            values,
                            multiple,
                            other_key,
                            ..
                        } => {
                            if let Some(text) = &answer.text {
                                content.insert(other_key.clone()?, Value::String(text.clone()));
                            }
                            if *multiple {
                                json!(
                                    answer
                                        .selected
                                        .iter()
                                        .map(|index| &values[*index as usize])
                                        .collect::<Vec<_>>()
                                )
                            } else if let Some(index) = answer.selected.first() {
                                Value::String(values[*index as usize].clone())
                            } else {
                                continue;
                            }
                        }
                    };
                    content.insert(field.key.clone(), value);
                }
                Some(json!({"action": "accept", "content": content}))
            }
            _ => None,
        }
    }
}

fn valid_answers(questions: &[UiQuestion], answers: &[ghost_protocol::UiQuestionAnswer]) -> bool {
    questions.len() == answers.len()
        && questions.iter().all(|question| {
            let mut matches = answers
                .iter()
                .filter(|answer| answer.question_id == question.id);
            let Some(answer) = matches.next() else {
                return false;
            };
            let choices = answer.selected.len() + usize::from(answer.text.is_some());
            matches.next().is_none()
                && choices > 0
                && (question.multi_select || choices == 1)
                && answer
                    .text
                    .as_ref()
                    .is_none_or(|text| question.allow_free_text && !text.trim().is_empty())
                && answer.selected.iter().enumerate().all(|(at, index)| {
                    (*index as usize) < question.options.len()
                        && !answer.selected[..at].contains(index)
                })
        })
}

pub fn request(method: &str, params: &Value, cursor: bool) -> DialectReply {
    let parsed = if params.to_string().len() > MAX_CALLBACK_BYTES {
        Err("This user input request exceeds Ghost's 64 KiB limit.")
    } else if cursor {
        match method {
            "cursor/ask_question" => cursor_questions(params),
            "cursor/create_plan" => cursor_plan(params),
            _ => return DialectReply::NotMine,
        }
    } else if method == "elicitation/create" {
        form(params)
    } else {
        return DialectReply::NotMine;
    };
    match parsed {
        Ok(call) if fits_display(&call.prompt) => DialectReply::Deferred(Box::new(call)),
        Ok(_) => DialectReply::Error {
            code: codes::INVALID_PARAMS,
            message: "This dialog exceeds the display limits; no answer was submitted.".into(),
        },
        Err(message) => DialectReply::Error {
            code: codes::INVALID_PARAMS,
            message: message.into(),
        },
    }
}

fn fits_display(prompt: &UiPrompt) -> bool {
    let escaped_fits =
        |text: &str| serde_json::to_string(text).is_ok_and(|value| value.len() <= 2048);
    if !serde_json::to_vec(prompt).is_ok_and(|value| value.len() <= 6000) {
        return false;
    }
    match prompt {
        UiPrompt::Confirm { message } => escaped_fits(message),
        UiPrompt::Input {
            message,
            placeholder,
        } => {
            escaped_fits(message)
                && placeholder
                    .as_ref()
                    .is_none_or(|text| text.chars().count() <= 256)
        }
        UiPrompt::Questions { message, questions } => {
            escaped_fits(message)
                && questions.iter().all(|question| {
                    escaped_fits(&question.question)
                        && question.header.chars().count() <= 64
                        && question.options.iter().all(|option| {
                            option.label.chars().count() <= 64 && escaped_fits(&option.description)
                        })
                })
        }
        _ => false,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorQuestions {
    tool_call_id: String,
    title: Option<String>,
    questions: Vec<CursorQuestion>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorQuestion {
    id: String,
    prompt: String,
    options: Vec<CursorOption>,
    #[serde(default)]
    allow_multiple: bool,
}
#[derive(Deserialize)]
struct CursorOption {
    id: String,
    label: String,
}

fn cursor_questions(params: &Value) -> Result<UiCall, &'static str> {
    let raw: CursorQuestions =
        serde_json::from_value(params.clone()).map_err(|_| "Invalid Cursor question request.")?;
    if raw.tool_call_id.is_empty()
        || raw.questions.is_empty()
        || raw.questions.len() > MAX_QUESTIONS
    {
        return Err("Invalid Cursor question count or tool call id.");
    }
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    let mut questions = Vec::new();
    for question in raw.questions {
        let mut choices = HashSet::new();
        if question.id.is_empty()
            || question.id.len() > 256
            || !seen.insert(question.id.clone())
            || question.options.is_empty()
            || question.options.len() > MAX_OPTIONS
            || question
                .options
                .iter()
                .any(|option| option.id.is_empty() || !choices.insert(option.id.clone()))
        {
            return Err("Invalid or duplicate Cursor question or option ids.");
        }
        ids.push((
            question.id.clone(),
            question
                .options
                .iter()
                .map(|option| option.id.clone())
                .collect(),
        ));
        questions.push(UiQuestion {
            id: question.id,
            header: String::new(),
            question: question.prompt,
            options: question
                .options
                .into_iter()
                .map(|option| UiQuestionOption {
                    label: option.label,
                    description: String::new(),
                })
                .collect(),
            multi_select: question.allow_multiple,
            allow_free_text: false,
        });
    }
    Ok(UiCall {
        prompt: UiPrompt::Questions {
            message: raw
                .title
                .unwrap_or_else(|| "Cursor needs your input".into()),
            questions,
        },
        context: None,
        response: Response::CursorQuestions(ids),
    })
}

fn cursor_plan(params: &Value) -> Result<UiCall, &'static str> {
    let plan = params
        .get("plan")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or("Cursor did not provide a plan to approve.")?;
    if params
        .get("toolCallId")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("Cursor did not provide a plan tool call id.");
    }
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Cursor plan");
    let overview = params.get("overview").and_then(Value::as_str).unwrap_or("");
    let mut context = format!("{name}\n\n{overview}\n\n{plan}");
    if let Some(todos) = params.get("todos") {
        append_plan_todos(&mut context, todos)?;
    }
    if let Some(phases) = params.get("phases") {
        for phase in phases.as_array().ok_or("Invalid plan phases.")? {
            let name = phase
                .get("name")
                .and_then(Value::as_str)
                .ok_or("Invalid plan phase name.")?;
            context.push_str(&format!("\n\nPhase: {name}"));
            append_plan_todos(
                &mut context,
                phase.get("todos").ok_or("Missing phase tasks.")?,
            )?;
        }
    }
    Ok(UiCall {
        prompt: UiPrompt::Confirm {
            message: format!(
                "Approve {name} and start implementation? Review the full plan above before approving."
            ),
        },
        context: Some(context),
        response: Response::CursorPlan,
    })
}

fn append_plan_todos(context: &mut String, value: &Value) -> Result<(), &'static str> {
    for todo in value.as_array().ok_or("Invalid plan tasks.")? {
        let content = todo
            .get("content")
            .and_then(Value::as_str)
            .ok_or("Invalid plan task text.")?;
        let status = todo
            .get("status")
            .and_then(Value::as_str)
            .ok_or("Invalid plan task status.")?;
        if !matches!(
            status,
            "pending" | "in_progress" | "completed" | "cancelled"
        ) {
            return Err("Unsupported plan task status.");
        }
        context.push_str(&format!("\n\n- {content} ({status})"));
    }
    Ok(())
}

fn form(params: &Value) -> Result<UiCall, &'static str> {
    if params
        .get("sessionId")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("Form session id is missing.");
    }
    if params.get("mode").and_then(Value::as_str) != Some("form") {
        return Err("Only form elicitation is supported; URL elicitation is unavailable.");
    }
    let message = params
        .get("message")
        .and_then(Value::as_str)
        .ok_or("Form message is missing.")?
        .to_owned();
    let schema = params
        .get("requestedSchema")
        .and_then(Value::as_object)
        .ok_or("Form schema is missing.")?;
    if schema.get("type").and_then(Value::as_str) != Some("object")
        || schema
            .keys()
            .any(|key| !["type", "properties", "required", "$schema"].contains(&key.as_str()))
    {
        return Err("Unsupported form object schema.");
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("Form properties are missing.")?;
    if properties.is_empty() || properties.len() > MAX_QUESTIONS * 2 {
        return Err("Unsupported form property count.");
    }
    if let Some(required) = schema.get("required") {
        let required = required
            .as_array()
            .ok_or("Invalid required form properties.")?;
        let mut unique = HashSet::new();
        if required.iter().any(|key| {
            key.as_str()
                .is_none_or(|key| !properties.contains_key(key) || !unique.insert(key))
        }) {
            return Err("Invalid required form properties.");
        }
        if required.iter().filter_map(Value::as_str).any(|key| {
            key.ends_with("__other") || properties.contains_key(&format!("{key}__other"))
        }) {
            return Err("Required paired choices and free text are unsupported.");
        }
    }
    let mut questions = Vec::new();
    let mut fields = Vec::new();
    // OMP askDialog's paired qN__other property becomes the same question's free text option.
    let is_other = |key: &str| {
        key.strip_suffix("__other")
            .is_some_and(|base| properties.contains_key(base))
    };
    for (key, value) in properties {
        if is_other(key) {
            continue;
        }
        if key.is_empty() || key.len() > 256 {
            return Err("Invalid form property id.");
        }
        let property = value.as_object().ok_or("Invalid form property.")?;
        let mut kind = property_kind(property)?;
        if let Some(other) = properties.get(&format!("{key}__other")) {
            if property_kind(other.as_object().ok_or("Invalid other-text property.")?)?
                != FieldKind::Text
            {
                return Err("Unsupported other-text property.");
            }
            match &mut kind {
                FieldKind::Choices { other_key, .. } => *other_key = Some(format!("{key}__other")),
                _ => return Err("Other-text properties must accompany choices."),
            }
        }
        let (options, multi_select, allow_free_text) = match &kind {
            FieldKind::Text => (Vec::new(), false, true),
            FieldKind::Boolean => (
                vec![
                    UiQuestionOption {
                        label: "Yes".into(),
                        description: String::new(),
                    },
                    UiQuestionOption {
                        label: "No".into(),
                        description: String::new(),
                    },
                ],
                false,
                false,
            ),
            FieldKind::Choices {
                options,
                multiple,
                other_key,
                ..
            } => (options.clone(), *multiple, other_key.is_some()),
        };
        questions.push(UiQuestion {
            id: key.clone(),
            header: String::new(),
            question: match property.get("description").and_then(Value::as_str) {
                Some(description) if !description.is_empty() => format!(
                    "{}\n\n{description}",
                    property.get("title").and_then(Value::as_str).unwrap_or(key)
                ),
                _ => property
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or(key)
                    .into(),
            },
            options,
            multi_select,
            allow_free_text,
        });
        fields.push(Field {
            key: key.clone(),
            kind,
        });
    }
    if questions.len() > MAX_QUESTIONS {
        return Err("Too many form questions.");
    }
    let prompt = if fields.len() == 1 && fields[0].kind == FieldKind::Text {
        UiPrompt::Input {
            message: if questions[0].question == message || questions[0].question == fields[0].key {
                message
            } else {
                format!("{message}\n\n{}", questions[0].question)
            },
            placeholder: None,
        }
    } else {
        UiPrompt::Questions { message, questions }
    };
    Ok(UiCall {
        prompt,
        context: None,
        response: Response::Form(fields),
    })
}

fn property_kind(property: &Map<String, Value>) -> Result<FieldKind, &'static str> {
    if property.keys().any(|key| {
        ![
            "type",
            "title",
            "description",
            "default",
            "enum",
            "enumNames",
            "oneOf",
            "items",
        ]
        .contains(&key.as_str())
    }) {
        return Err("Unsupported form constraint; no answer was submitted.");
    }
    match property.get("type").and_then(Value::as_str) {
        Some("boolean")
            if property
                .keys()
                .all(|key| ["type", "title", "description", "default"].contains(&key.as_str())) =>
        {
            Ok(FieldKind::Boolean)
        }
        Some("string") if !property.contains_key("items") => {
            if property.contains_key("enumNames") && !property.contains_key("enum") {
                return Err("enumNames requires enum values.");
            }
            if property.contains_key("enum") || property.contains_key("oneOf") {
                let (values, options) = choices(property)?;
                Ok(FieldKind::Choices {
                    values,
                    options,
                    multiple: false,
                    other_key: None,
                })
            } else {
                Ok(FieldKind::Text)
            }
        }
        Some("array") if !property.contains_key("enum") && !property.contains_key("oneOf") => {
            let items = property
                .get("items")
                .and_then(Value::as_object)
                .ok_or("Array form choices are missing.")?;
            if items
                .keys()
                .any(|key| !["type", "enum", "anyOf"].contains(&key.as_str()))
                || items.get("type").is_some_and(|value| value != "string")
            {
                return Err("Unsupported array item constraints.");
            }
            let (values, options) = choices(items)?;
            Ok(FieldKind::Choices {
                values,
                options,
                multiple: true,
                other_key: None,
            })
        }
        _ => Err("Unsupported form field type; no answer was submitted."),
    }
}

fn choices(
    property: &Map<String, Value>,
) -> Result<(Vec<String>, Vec<UiQuestionOption>), &'static str> {
    if property.contains_key("enum")
        && (property.contains_key("oneOf") || property.contains_key("anyOf"))
    {
        return Err("Ambiguous form choices.");
    }
    let values: Vec<String> =
        if let Some(values) = property.get("enum").and_then(Value::as_array) {
            values
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<_>>()
        } else {
            property
                .get("oneOf")
                .or_else(|| property.get("anyOf"))
                .and_then(Value::as_array)
                .and_then(|values| {
                    values
                        .iter()
                        .map(|value| {
                            let value = value.as_object()?;
                            if value.keys().any(|key| {
                                !["const", "title", "description"].contains(&key.as_str())
                            }) {
                                return None;
                            }
                            value
                                .get("const")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .collect::<Option<_>>()
                })
        }
        .ok_or("Unsupported form choice schema.")?;
    let unique: HashSet<&str> = values.iter().map(String::as_str).collect();
    if values.is_empty() || values.len() > MAX_OPTIONS || unique.len() != values.len() {
        return Err("Invalid or duplicate form choices.");
    }
    let options = if let Some(names) = property.get("enumNames") {
        if !property.contains_key("enum") {
            return Err("enumNames requires enum values.");
        }
        let names = names.as_array().ok_or("Invalid choice labels.")?;
        if names.len() != values.len() {
            return Err("Choice labels do not match their values.");
        }
        names
            .iter()
            .map(|name| {
                Some(UiQuestionOption {
                    label: name.as_str()?.into(),
                    description: String::new(),
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or("Invalid choice labels.")?
    } else if let Some(rows) = property
        .get("oneOf")
        .or_else(|| property.get("anyOf"))
        .and_then(Value::as_array)
    {
        rows.iter()
            .zip(&values)
            .map(|(row, value)| {
                let label = match row.get("title") {
                    None => Some(value.as_str()),
                    Some(title) => title.as_str(),
                }?;
                let description = match row.get("description") {
                    None => Some(""),
                    Some(text) => text.as_str(),
                }?;
                Some(UiQuestionOption {
                    label: label.into(),
                    description: description.into(),
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or("Invalid choice labels.")?
    } else {
        values
            .iter()
            .map(|value| UiQuestionOption {
                label: value.clone(),
                description: String::new(),
            })
            .collect()
    };
    if options.iter().any(|option| option.label.is_empty()) {
        return Err("Empty choice label.");
    }
    Ok((values, options))
}
