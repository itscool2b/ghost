//! Our wire format against the ACP schema (`agent-client-protocol-schema` 1.12.0, a dev-dependency):
//! every request Ghost sends equals the same request built with the schema's builders, messages
//! built with the schema's builders parse with our lenient wire types to the expected events, and
//! every `sessionUpdate` value the schema defines has a row in the mapping table.

#[cfg(test)]
mod conformance {
    use std::collections::BTreeSet;

    use agent_client_protocol_schema::v1 as acp;
    use ghost_drivers::acp::map::{MAPPED_TAGS, Mapped, Mapper};
    use ghost_drivers::acp::options::OptionModel;
    use ghost_drivers::acp::requests;
    use ghost_drivers::acp::wire::{
        AuthMethod, AuthMethodKind, InitializeReply, OpenReply, PermissionParams, PromptReply,
        Update, UpdateNotification, WireUsage,
    };
    use ghost_drivers::driver::{DriverEvent, PermissionOutcome};
    use ghost_protocol::session::{ConfigCategory, ConfigKind, ConfigValue};
    use ghost_protocol::{
        Cost, GhostEvent, NoticeLevel, PermissionOptionKind, PlanEntry, PlanPriority, PlanStatus,
        StopReason, ToolContent, ToolKind, ToolLocation, ToolStatus, UsageInfo,
    };
    use serde_json::{Value, json};
    use tokio::time::Instant;

    fn to_json<T: serde::Serialize>(value: &T) -> Value {
        serde_json::to_value(value).expect("the schema type serializes")
    }

    const SESSION: &str = "sess-1";

    #[test]
    fn conformance_requests() {
        let capabilities = acp::ClientCapabilities::new()
            .fs(acp::FileSystemCapabilities::new())
            .terminal(false)
            .session(
                acp::ClientSessionCapabilities::new()
                    .notices(acp::NoticeCapabilities::new())
                    .config_options(
                        acp::SessionConfigOptionsCapabilities::new()
                            .boolean(acp::BooleanConfigOptionCapabilities::new()),
                    ),
            )
            .auth(acp::AuthCapabilities::new().terminal(true))
            // Display-only terminals, which codex-acp streams once a client says so.
            .meta(serde_json::Map::from_iter([(
                "terminal_output".to_owned(),
                serde_json::json!(true),
            )]));
        let initialize =
            acp::InitializeRequest::new(agent_client_protocol_schema::ProtocolVersion::V1)
                .client_capabilities(capabilities)
                .client_info(acp::Implementation::new("ghost", "0.1.0").title("Ghost ADE"));
        assert_eq!(requests::initialize("0.1.0"), to_json(&initialize));

        assert_eq!(
            requests::new_session("/work"),
            to_json(&acp::NewSessionRequest::new("/work"))
        );
        assert_eq!(
            requests::load_session(SESSION, "/work"),
            to_json(&acp::LoadSessionRequest::new(SESSION, "/work"))
        );
        // The schema type skips an empty `mcpServers` on resume; Ghost always sends it.
        let mut resume = requests::resume_session(SESSION, "/work");
        assert_eq!(resume["mcpServers"], json!([]));
        resume
            .as_object_mut()
            .expect("an object")
            .remove("mcpServers");
        assert_eq!(
            resume,
            to_json(&acp::ResumeSessionRequest::new(SESSION, "/work"))
        );

        let prompt = acp::PromptRequest::new(
            SESSION,
            vec![
                acp::ContentBlock::Text(acp::TextContent::new("one")),
                acp::ContentBlock::Text(acp::TextContent::new("two")),
            ],
        );
        assert_eq!(requests::prompt(SESSION, &["one", "two"]), to_json(&prompt));
        assert_eq!(
            requests::cancel(SESSION),
            to_json(&acp::CancelNotification::new(SESSION))
        );
        assert_eq!(
            requests::set_config_option(
                SESSION,
                "model",
                &ConfigValue::Choice {
                    value: "opus".into()
                }
            ),
            to_json(&acp::SetSessionConfigOptionRequest::new(
                SESSION, "model", "opus"
            ))
        );
        assert_eq!(
            requests::set_config_option(SESSION, "fast", &ConfigValue::Toggle { value: true }),
            to_json(&acp::SetSessionConfigOptionRequest::new(
                SESSION, "fast", true
            ))
        );
        assert_eq!(
            requests::set_mode(SESSION, "plan"),
            to_json(&acp::SetSessionModeRequest::new(SESSION, "plan"))
        );
        // `session/set_model` left the schema; Hermes still takes the legacy shape.
        assert_eq!(
            requests::set_model(SESSION, "p:m"),
            json!({"sessionId": SESSION, "modelId": "p:m"})
        );
        assert_eq!(
            requests::permission_reply(&PermissionOutcome::Selected("allow".into())),
            to_json(&acp::RequestPermissionResponse::new(
                acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                    "allow"
                ))
            ))
        );
        assert_eq!(
            requests::permission_reply(&PermissionOutcome::Cancelled),
            to_json(&acp::RequestPermissionResponse::new(
                acp::RequestPermissionOutcome::Cancelled
            ))
        );
        // And the schema reads every request back.
        serde_json::from_value::<acp::InitializeRequest>(requests::initialize("0.1.0"))
            .expect("valid");
        serde_json::from_value::<acp::ResumeSessionRequest>(requests::resume_session(
            SESSION, "/w",
        ))
        .expect("valid");
    }

    /// Maps one notification built with the schema's builders.
    fn map(mapper: &mut Mapper, update: acp::SessionUpdate) -> Mapped {
        let notification = to_json(&acp::SessionNotification::new(SESSION, update));
        let parsed = UpdateNotification::parse(&notification).expect("a notification");
        assert_eq!(parsed.session_id, Some(SESSION));
        let update = Update::parse(parsed.update).expect("our wire types read it");
        mapper.map(update, Instant::now())
    }

    fn events(mapper: &mut Mapper, update: acp::SessionUpdate) -> Vec<GhostEvent> {
        match map(mapper, update) {
            Mapped::Events(events) => events
                .into_iter()
                .map(|event| match event {
                    DriverEvent::Event { turn, event } => {
                        assert_eq!(turn.as_deref(), Some("T"));
                        event
                    }
                    DriverEvent::Title(title) => GhostEvent::Notice {
                        level: NoticeLevel::Info,
                        message: format!("title: {title:?}"),
                    },
                    other => panic!("unexpected {other:?}"),
                })
                .collect(),
            other => panic!("expected events, got {other:?}"),
        }
    }

    fn text(content: &str) -> acp::ContentBlock {
        acp::ContentBlock::Text(acp::TextContent::new(content))
    }

    #[test]
    fn conformance_updates() {
        let mut mapper = Mapper::new("spawn");
        mapper.begin_turn("T", Instant::now());

        assert!(matches!(
            map(
                &mut mapper,
                acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(text("hi")))
            ),
            Mapped::Nothing(_)
        ));
        assert_eq!(
            events(
                &mut mapper,
                acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(text("Thinking")))
            ),
            [GhostEvent::ThoughtDelta {
                thought_id: "T.t1".into(),
                text: "Thinking".into()
            }]
        );
        let chunk = acp::ContentChunk::new(text("Hello")).message_id(acp::MessageId::new("msg_1"));
        let mapped = events(&mut mapper, acp::SessionUpdate::AgentMessageChunk(chunk));
        assert!(
            matches!(&mapped[0], GhostEvent::ThoughtCompleted { thought_id, .. } if thought_id == "T.t1")
        );
        assert_eq!(
            mapped[1],
            GhostEvent::MessageDelta {
                message_id: "T.m1".into(),
                text: "Hello".into()
            }
        );

        let call = acp::ToolCall::new("call_1", "Read lib.rs")
            .name("Read")
            .kind(acp::ToolKind::Read)
            .status(acp::ToolCallStatus::InProgress)
            .content(vec![acp::ToolCallContent::from(text("pub fn x() {}"))])
            .locations(vec![acp::ToolCallLocation::new("/w/lib.rs").line(3)])
            .raw_input(json!({"path": "/w/lib.rs"}));
        let mapped = events(&mut mapper, acp::SessionUpdate::ToolCall(call));
        assert_eq!(
            mapped[0],
            GhostEvent::MessageCompleted {
                message_id: "T.m1".into()
            }
        );
        let GhostEvent::ToolCall(call) = &mapped[1] else {
            panic!("a tool call");
        };
        assert_eq!(call.tool_call_id, "T.call_1");
        assert_eq!(
            (call.kind, call.status),
            (ToolKind::Read, ToolStatus::InProgress)
        );
        assert_eq!(call.tool_name.as_deref(), Some("Read"));
        assert_eq!(
            call.locations,
            [ToolLocation {
                path: "/w/lib.rs".into(),
                line: Some(3)
            }]
        );
        assert_eq!(
            call.content,
            [ToolContent::Text {
                text: "pub fn x() {}".into(),
                truncated: None
            }]
        );

        // The schema skips a default kind (`other`) and status (`pending`).
        let bare = events(
            &mut mapper,
            acp::SessionUpdate::ToolCall(acp::ToolCall::new("call_2", "Think")),
        );
        let GhostEvent::ToolCall(bare) = &bare[0] else {
            panic!("a tool call");
        };
        assert_eq!(
            (bare.kind, bare.status),
            (ToolKind::Other, ToolStatus::Pending)
        );

        let diff = acp::Diff::new("/w/lib.rs", "new").old_text("old");
        let update = acp::ToolCallUpdate::new(
            "call_1",
            acp::ToolCallUpdateFields::new()
                .status(acp::ToolCallStatus::Completed)
                .content(vec![acp::ToolCallContent::Diff(diff)]),
        );
        let mapped = events(&mut mapper, acp::SessionUpdate::ToolCallUpdate(update));
        let GhostEvent::ToolUpdate(update) = &mapped[0] else {
            panic!("a tool update");
        };
        assert_eq!(update.status, Some(ToolStatus::Completed));
        assert_eq!(
            update.content,
            Some(vec![ToolContent::Diff {
                path: "/w/lib.rs".into(),
                old_text: Some("old".into()),
                new_text: "new".into(),
                truncated: None
            }])
        );
        assert_eq!((update.title.clone(), update.kind), (None, None));

        let plan = acp::Plan::new(vec![
            acp::PlanEntry::new(
                "Read",
                acp::PlanEntryPriority::High,
                acp::PlanEntryStatus::Completed,
            ),
            acp::PlanEntry::new(
                "Write",
                acp::PlanEntryPriority::Low,
                acp::PlanEntryStatus::InProgress,
            ),
        ]);
        assert_eq!(
            events(&mut mapper, acp::SessionUpdate::Plan(plan)),
            [GhostEvent::PlanUpdated {
                entries: vec![
                    PlanEntry {
                        content: "Read".into(),
                        status: PlanStatus::Completed,
                        priority: Some(PlanPriority::High)
                    },
                    PlanEntry {
                        content: "Write".into(),
                        status: PlanStatus::InProgress,
                        priority: Some(PlanPriority::Low)
                    },
                ]
            }]
        );

        let commands = acp::AvailableCommandsUpdate::new(vec![
            acp::AvailableCommand::new("plan", "Plan first").input(
                acp::AvailableCommandInput::Unstructured(acp::UnstructuredCommandInput::new(
                    "goal",
                )),
            ),
        ]);
        let Mapped::Commands(commands) = map(
            &mut mapper,
            acp::SessionUpdate::AvailableCommandsUpdate(commands),
        ) else {
            panic!("commands");
        };
        assert_eq!(
            (commands[0].name.as_str(), commands[0].input_hint.as_deref()),
            ("plan", Some("goal"))
        );

        assert_eq!(
            map(
                &mut mapper,
                acp::SessionUpdate::CurrentModeUpdate(acp::CurrentModeUpdate::new("plan"))
            ),
            Mapped::Mode("plan".into())
        );
        let option = acp::SessionConfigOption::select(
            "model",
            "Model",
            "a",
            vec![
                acp::SessionConfigSelectOption::new("a", "A"),
                acp::SessionConfigSelectOption::new("b", "B"),
            ],
        )
        .category(acp::SessionConfigOptionCategory::Model);
        let Mapped::ConfigOptions(raw) = map(
            &mut mapper,
            acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(vec![option])),
        ) else {
            panic!("config options");
        };
        let mut model = OptionModel::default();
        model.replace_real(&raw);
        let options = model.options();
        assert_eq!(
            (options[0].id.as_str(), options[0].category),
            ("model", ConfigCategory::Model)
        );
        assert!(
            matches!(&options[0].kind, ConfigKind::Select { current, choices } if current == "a" && choices.len() == 2)
        );

        assert_eq!(
            events(
                &mut mapper,
                acp::SessionUpdate::SessionInfoUpdate(
                    acp::SessionInfoUpdate::new().title("Fix it")
                )
            ),
            [GhostEvent::Notice {
                level: NoticeLevel::Info,
                message: "title: Some(\"Fix it\")".into()
            }]
        );
        assert_eq!(
            events(
                &mut mapper,
                acp::SessionUpdate::UsageUpdate(
                    acp::UsageUpdate::new(10, 100).cost(acp::Cost::new(0.5, "USD"))
                )
            ),
            [GhostEvent::UsageUpdated(UsageInfo {
                context_used: Some(10),
                context_size: Some(100),
                cost: Some(Cost {
                    amount: 0.5,
                    currency: "USD".into()
                }),
            })]
        );
        assert_eq!(
            events(
                &mut mapper,
                acp::SessionUpdate::Notice(
                    acp::Notice::new(acp::NoticeSeverity::Warning, "Rate limited")
                        .description("retrying")
                )
            ),
            [GhostEvent::Notice {
                level: NoticeLevel::Warning,
                message: "Rate limited: retrying".into()
            }]
        );
        assert!(matches!(
            map(
                &mut mapper,
                acp::SessionUpdate::CompactionUpdate(acp::CompactionUpdate::new(
                    "c1",
                    acp::CompactionStatus::InProgress
                ))
            ),
            Mapped::Nothing(_)
        ));
    }

    #[test]
    fn conformance_replies_and_requests_from_the_agent() {
        let init = acp::InitializeResponse::new(agent_client_protocol_schema::ProtocolVersion::V1)
            .agent_capabilities(
                acp::AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(acp::PromptCapabilities::new().image(true))
                    .session_capabilities(
                        acp::SessionCapabilities::new()
                            .resume(acp::SessionResumeCapabilities::new()),
                    ),
            )
            .auth_methods(vec![
                acp::AuthMethod::Terminal(
                    acp::AuthMethodTerminal::new("setup", "Set up").args(vec!["--setup".into()]),
                ),
                acp::AuthMethod::Agent(acp::AuthMethodAgent::new("api-key", "API key")),
            ])
            .agent_info(acp::Implementation::new("fake", "1.0.0").title("Fake"));
        let reply = InitializeReply::parse(&to_json(&init));
        assert!(reply.speaks_version_1() && reply.load_session && reply.resume && reply.image);
        assert_eq!(reply.agent_title.as_deref(), Some("Fake"));
        assert_eq!(
            reply
                .auth_methods
                .iter()
                .map(|method| method.kind.clone())
                .collect::<Vec<_>>(),
            [AuthMethodKind::Terminal, AuthMethodKind::Agent]
        );
        let terminal = AuthMethod::parse(&to_json(&acp::AuthMethod::Terminal(
            acp::AuthMethodTerminal::new("t", "T").args(vec!["a".into()]),
        )))
        .expect("an auth method");
        assert_eq!(terminal.args, ["a"]);

        let new = acp::NewSessionResponse::new(SESSION)
            .modes(acp::SessionModeState::new(
                "default",
                vec![acp::SessionMode::new("default", "Default")],
            ))
            .config_options(vec![acp::SessionConfigOption::boolean(
                "fast", "Fast", true,
            )]);
        let open = OpenReply::parse(&to_json(&new));
        assert_eq!(open.session_id.as_deref(), Some(SESSION));
        let options = OptionModel::from_open(&open).options();
        assert_eq!(
            options.len(),
            2,
            "the boolean option and the synthesized mode"
        );
        assert_eq!(options[0].kind, ConfigKind::Toggle { current: true });

        let prompt = acp::PromptResponse::new(acp::StopReason::MaxTokens).usage(
            acp::Usage::new(30, 20, 10)
                .cached_read_tokens(5)
                .cached_write_tokens(2)
                .thought_tokens(4),
        );
        let reply = PromptReply::parse(&to_json(&prompt));
        assert_eq!(reply.stop_reason, StopReason::MaxTokens);
        assert_eq!(
            reply.usage,
            Some(WireUsage {
                input: 20,
                output: 10,
                cached_read: Some(5),
                cached_write: Some(2),
                thought: Some(4)
            })
        );

        let request = acp::RequestPermissionRequest::new(
            SESSION,
            acp::ToolCallUpdate::new(
                "call_1",
                acp::ToolCallUpdateFields::new()
                    .title("Run ls")
                    .kind(acp::ToolKind::Execute),
            ),
            vec![
                acp::PermissionOption::new("allow", "Allow", acp::PermissionOptionKind::AllowOnce),
                acp::PermissionOption::new(
                    "always",
                    "Always",
                    acp::PermissionOptionKind::AllowAlways,
                ),
                acp::PermissionOption::new(
                    "reject",
                    "Reject",
                    acp::PermissionOptionKind::RejectOnce,
                ),
                acp::PermissionOption::new(
                    "never",
                    "Never",
                    acp::PermissionOptionKind::RejectAlways,
                ),
            ],
        );
        let parsed = PermissionParams::parse(&to_json(&request)).expect("parses");
        assert_eq!(parsed.tool.tool_call_id, "call_1");
        assert_eq!(parsed.tool.kind, Some(ToolKind::Execute));
        assert_eq!(
            parsed
                .options
                .iter()
                .map(|option| option.kind)
                .collect::<Vec<_>>(),
            [
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::AllowAlways,
                PermissionOptionKind::RejectOnce,
                PermissionOptionKind::RejectAlways
            ]
        );
    }

    /// Every `sessionUpdate` value in the schema of a session notification.
    fn schema_update_tags() -> BTreeSet<String> {
        let schema =
            serde_json::to_value(schemars::schema_for!(acp::SessionNotification)).expect("json");
        let mut tags = BTreeSet::new();
        let mut stack = vec![&schema];
        while let Some(value) = stack.pop() {
            match value {
                Value::Object(object) => {
                    if let Some(tag) = object
                        .get("properties")
                        .and_then(|properties| properties.get("sessionUpdate"))
                    {
                        if let Some(constant) = tag.get("const").and_then(Value::as_str) {
                            tags.insert(constant.to_owned());
                        }
                        for value in tag
                            .get("enum")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            if let Some(value) = value.as_str() {
                                tags.insert(value.to_owned());
                            }
                        }
                    }
                    stack.extend(object.values());
                }
                Value::Array(items) => stack.extend(items),
                _ => {}
            }
        }
        tags
    }

    #[test]
    fn every_update_tag_is_mapped() {
        let tags = schema_update_tags();
        assert_eq!(tags.len(), 19, "{tags:?}");
        for tag in &tags {
            assert!(
                MAPPED_TAGS.contains(&tag.as_str()),
                "{tag} has no row in the mapping table"
            );
        }
        let mapped: BTreeSet<String> = MAPPED_TAGS.iter().map(|tag| (*tag).to_owned()).collect();
        assert_eq!(mapped, tags, "the table names no tag the schema lacks");
    }
}
