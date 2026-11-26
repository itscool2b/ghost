//! Bounded, display-only Cursor extensions. Records pass through the standard mapper so message
//! boundaries, tool IDs, turn tags and redaction are shared with ordinary ACP updates.

use ghost_protocol::{NoticeLevel, PlanEntry, PlanStatus};
use serde::Deserialize;
use serde_json::{Value, json};

use super::wire::{ToolFields, Update};

const MAX_TODOS: usize = 256;

#[derive(Default)]
pub(super) struct CursorUpdates {
    todos: Vec<Todo>,
}

#[derive(Clone, Deserialize)]
struct Todo {
    id: String,
    content: String,
    status: String,
}

impl CursorUpdates {
    pub(super) fn notification(&mut self, method: &str, params: &Value) -> Option<Update> {
        if !matches!(
            method,
            "cursor/update_todos" | "cursor/task" | "cursor/generate_image"
        ) {
            return None;
        }
        Some(
            self.parse(method, params)
                .unwrap_or_else(|| Update::Notice {
                    level: NoticeLevel::Warning,
                    message: "Cursor sent an unsupported or oversized activity update.".into(),
                }),
        )
    }

    fn parse(&mut self, method: &str, params: &Value) -> Option<Update> {
        if super::json_bytes(params) > 64 * 1024 {
            return None;
        }
        let tool_id = params.get("toolCallId")?.as_str()?;
        if tool_id.is_empty() {
            return None;
        }
        if method == "cursor/update_todos" {
            let merge = params.get("merge")?.as_bool()?;
            let incoming: Vec<Todo> = serde_json::from_value(params.get("todos")?.clone()).ok()?;
            let mut ids = std::collections::HashSet::new();
            if incoming.len() > MAX_TODOS
                || incoming.iter().any(|todo| {
                    todo.id.is_empty()
                        || todo.id.len() > 256
                        || todo.content.len() > 4096
                        || !ids.insert(todo.id.as_str())
                        || !matches!(
                            todo.status.as_str(),
                            "pending" | "in_progress" | "completed" | "cancelled"
                        )
                })
            {
                return None;
            }
            let mut next = if merge {
                self.todos.clone()
            } else {
                Vec::new()
            };
            for todo in incoming {
                if let Some(existing) = next.iter_mut().find(|item| item.id == todo.id) {
                    *existing = todo;
                } else {
                    next.push(todo);
                }
            }
            if next.len() > MAX_TODOS {
                return None;
            }
            let entries = next
                .iter()
                .map(|todo| PlanEntry {
                    content: if todo.status == "cancelled" {
                        format!("Cancelled: {}", todo.content)
                    } else {
                        todo.content.clone()
                    },
                    status: match todo.status.as_str() {
                        "in_progress" => PlanStatus::InProgress,
                        "completed" | "cancelled" => PlanStatus::Completed,
                        _ => PlanStatus::Pending,
                    },
                    priority: None,
                })
                .collect();
            self.todos = next;
            return Some(Update::Plan(entries));
        }
        let description = params.get("description")?.as_str()?;
        let (title, content) = if method == "cursor/task" {
            let prompt = params.get("prompt")?.as_str()?;
            (
                format!("Cursor task: {description}"),
                format!("Task prompt:\n{prompt}"),
            )
        } else {
            let path = params.get("filePath").and_then(Value::as_str);
            (
                format!("Cursor image: {description}"),
                path.map_or_else(
                    || "Cursor reported image generation without a file path.".into(),
                    |path| format!("Reported output path: {path}"),
                ),
            )
        };
        ToolFields::parse(&json!({
            "toolCallId": tool_id, "title": title, "status": "completed", "kind": "other",
            "name": method, "rawInput": params,
            "content": [{"type":"content", "content":{"type":"text", "text":content}}]
        }))
        .map(Update::ToolCall)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_merge_retains_order_updates_by_native_id_and_marks_cancellation() {
        let mut state = CursorUpdates::default();
        let _ = state.notification("cursor/update_todos", &json!({"toolCallId":"a", "merge":false,
            "todos":[{"id":"a","content":"One","status":"pending"},{"id":"b","content":"Two","status":"pending"}]}));
        let Some(Update::Plan(entries)) = state.notification("cursor/update_todos", &json!({"toolCallId":"b", "merge":true,
            "todos":[{"id":"b","content":"Two","status":"cancelled"},{"id":"c","content":"Three","status":"in_progress"}]})) else { panic!("plan"); };
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].content, "One");
        assert_eq!(entries[1].content, "Cancelled: Two");
        assert_eq!(entries[2].status, PlanStatus::InProgress);
        assert!(matches!(
            state.notification(
                "cursor/update_todos",
                &json!({"toolCallId":"c","merge":true,
            "todos":[{"id":"d","content":"Bad","status":"future"}]})
            ),
            Some(Update::Notice { .. })
        ));
        assert_eq!(state.todos.len(), 3, "bad updates are atomic");
        assert!(
            matches!(state.notification("cursor/update_todos", &json!({"toolCallId":"d","merge":false,"todos":[]})), Some(Update::Plan(entries)) if entries.is_empty())
        );
    }

    #[test]
    fn task_and_image_updates_preserve_context_without_reading_files() {
        let mut state = CursorUpdates::default();
        let task = json!({"toolCallId":"t", "description":"Explore", "prompt":"Visible task prompt", "subagentType":"explore"});
        let Some(Update::ToolCall(fields)) = state.notification("cursor/task", &task) else {
            panic!("tool");
        };
        assert_eq!(fields.raw_input, Some(task));
        let image =
            json!({"toolCallId":"i", "description":"Logo", "filePath":"/not-read/logo.png"});
        assert!(matches!(
            state.notification("cursor/generate_image", &image),
            Some(Update::ToolCall(_))
        ));
        assert!(
            state
                .notification("future/notification", &Value::Null)
                .is_none()
        );
    }
}
