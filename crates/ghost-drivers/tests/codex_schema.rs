//! Stable fixture frames conform to the installed CLI schema. The documented synthetic
//! collaboration-list extension is checked exactly, since the stable bundle excludes that method.
#[cfg(test)]
mod conformance {
    use serde_json::{Value, json};
    use std::{collections::BTreeMap, path::Path};

    fn validator(name: &str) -> jsonschema::Validator {
        let mut schema: Value =
            serde_json::from_str(include_str!("schemas/codex-0.162.1/protocol.json"))
                .expect("generated schema JSON");
        schema["$ref"] = json!(format!("#/definitions/{name}"));
        jsonschema::validator_for(&schema).expect("generated schema is valid")
    }

    fn response_name(method: &str) -> Option<&'static str> {
        Some(match method {
            "initialize" => "InitializeResponse",
            "thread/start" => "v2/ThreadStartResponse",
            "thread/resume" => "v2/ThreadResumeResponse",
            "thread/fork" => "v2/ThreadForkResponse",
            "turn/start" => "v2/TurnStartResponse",
            "turn/steer" => "v2/TurnSteerResponse",
            "turn/interrupt" => "v2/TurnInterruptResponse",
            "model/list" => "v2/ModelListResponse",
            "account/rateLimits/read" => "v2/GetAccountRateLimitsResponse",
            "item/commandExecution/requestApproval" => "CommandExecutionRequestApprovalResponse",
            "item/fileChange/requestApproval" => "FileChangeRequestApprovalResponse",
            "item/permissions/requestApproval" => "PermissionsRequestApprovalResponse",
            "item/tool/requestUserInput" => "ToolRequestUserInputResponse",
            _ => return None,
        })
    }

    #[test]
    fn native_fixtures_conform_to_codex_01621() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex");
        let mut validators = BTreeMap::new();
        let mut frames = 0;
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path
                .extension()
                .is_none_or(|extension| extension != "jsonl")
            {
                continue;
            }
            let mut requests: BTreeMap<(String, String), String> = BTreeMap::new();
            for (line_index, line) in std::fs::read_to_string(&path).unwrap().lines().enumerate() {
                let record: Value = serde_json::from_str(line).unwrap();
                let Some(msg) = record.get("msg") else {
                    continue;
                };
                let direction = record["d"].as_str().unwrap();
                if msg["id"] == "aura-replay-collaboration" {
                    let expected = match direction {
                        "out" => {
                            json!({"id":"aura-replay-collaboration","method":"collaborationMode/list","params":{}})
                        }
                        "in" => json!({"id":"aura-replay-collaboration","result":{"data":[]}}),
                        _ => panic!("invalid synthetic collaboration direction"),
                    };
                    assert_eq!(*msg, expected, "documented derived handshake only");
                    continue;
                }
                let (schema_name, payload) = if let Some(method) = msg["method"].as_str() {
                    let request = msg.get("id").is_some();
                    if request {
                        requests.insert((direction.into(), msg["id"].to_string()), method.into());
                    }
                    (
                        match (direction, request) {
                            ("out", true) => "ClientRequest",
                            ("out", false) => "ClientNotification",
                            ("in", true) => "ServerRequest",
                            _ => "ServerNotification",
                        },
                        msg,
                    )
                } else if msg.get("result").is_some() {
                    let key = (
                        if direction == "out" { "in" } else { "out" }.into(),
                        msg["id"].to_string(),
                    );
                    let Some(method) = requests.get(&key) else {
                        continue;
                    };
                    let Some(name) = response_name(method) else {
                        panic!("missing response schema for {method}");
                    };
                    (name, &msg["result"])
                } else {
                    continue;
                };
                let validator = validators
                    .entry(schema_name.to_owned())
                    .or_insert_with(|| validator(schema_name));
                let errors: Vec<_> = validator
                    .iter_errors(payload)
                    .map(|error| error.to_string())
                    .collect();
                assert!(
                    errors.is_empty(),
                    "{}:{} {schema_name}: {}",
                    path.display(),
                    line_index + 1,
                    errors.join("; ")
                );
                frames += 1;
            }
        }
        assert!(
            frames >= 70,
            "the native fixture suite must cover its wire surface"
        );
    }

    #[test]
    fn schema_catches_missing_turn_preconditions_and_bad_approval_decisions() {
        let requests = validator("ClientRequest");
        assert!(
            !requests.is_valid(
                &json!({"id":1,"method":"turn/steer","params":{"threadId":"t","input":[]}})
            )
        );
        let approvals = validator("CommandExecutionRequestApprovalResponse");
        assert!(!approvals.is_valid(&json!({"decision":"allow"})));
        assert!(approvals.is_valid(&json!({"decision":"accept"})));
    }
    #[test]
    fn native_plan_collaboration_uses_schema_settings_and_builtin_instructions() {
        let collaboration = validator("v2/CollaborationMode");
        for mode in ["plan", "default"] {
            assert!(collaboration.is_valid(&json!({"mode":mode,"settings":{"model":"gpt-5","reasoning_effort":"high","developer_instructions":null}})));
        }
        assert!(!collaboration.is_valid(&json!({"mode":"unknown","settings":{"model":"gpt-5"}})));
    }
}
