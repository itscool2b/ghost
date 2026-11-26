//! One browser's session: its hello, its sealed requests and what each starts.

use super::*;
use crate::backend::{Action, Outcome};

/// What a browser's message leads to.
pub(super) enum Received {
    /// Frames to write to the relay.
    Send(Vec<String>),
    /// The session is over: an optional last word, then the connection is dropped.
    Refuse(Option<String>),
}

pub(super) fn refusal(reason: Refusal) -> String {
    serde_json::to_string(&Refused { reason }).unwrap_or_default()
}

pub(super) fn bot_answer(
    rid: &str,
    text: &str,
    error: bool,
    image: Option<&crate::backend::Picture>,
) -> String {
    format!(
        "b{}",
        serde_json::to_string(&BotAnswer {
            rid,
            text,
            error,
            image
        })
        .unwrap_or_default()
    )
}

/// The bot links as the relay checks them: ids and token hashes, never tokens.
pub(super) fn bot_links(shared: &Shared) -> String {
    let registry = lock(&shared.registry);
    let links: Vec<BotLink<'_>> = registry
        .bots()
        .iter()
        .map(|bot| BotLink {
            id: &bot.id,
            hash: &bot.token_hash,
            full: bot.full && !bot.read_only,
            read: bot.read_only,
            v: crate::bot::TOOLS_VERSION,
            hook: bot.hook.is_some(),
        })
        .collect();
    format!(
        "l{}",
        serde_json::to_string(&links).unwrap_or_else(|_| "[]".into())
    )
}

pub(super) fn seal(
    channel: &mut Channel,
    event: &Event<'_>,
) -> Result<String, crypto::CryptoError> {
    let json = serde_json::to_string(event).map_err(|_| crypto::CryptoError::Seal)?;
    channel.seal(json.as_bytes())
}

/// A browser is gone: nothing follows a chat on its behalf any more.
pub(super) fn closed(shared: &Shared, backend: &dyn Backend, cid: &str, connection: Connection) {
    // First the mark, then the release: an open that finishes attaching after this sees the
    // mark and releases what it attached.
    if let Some(session) = &connection.session {
        session.ended.store(true, Ordering::SeqCst);
        // Its staged photos go with it: a message being sent keeps only what it holds.
        lock(&session.uploads).clear();
    }
    backend.release(&owner(cid));
    let Some(session) = connection.session else {
        return;
    };
    let Some(device) = session.device.filter(|_| session.proven) else {
        return;
    };
    let mut connected = lock(&shared.connected);
    if let Some(count) = connected.get_mut(&device) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            connected.remove(&device);
        }
    }
    drop(connected);
    (shared.config.changed)();
}

/// Handles one payload from a browser. Runs on the connection task: it opens the message and
/// starts whatever the request needs, and never waits.
pub(super) fn receive(
    shared: &Arc<Shared>,
    backend: &Arc<dyn Backend>,
    out: &UnboundedSender<Out>,
    cid: &str,
    connection: &mut Connection,
    payload: &str,
) -> Received {
    let Some((kind, body)) = payload.split_at_checked(1) else {
        return Received::Refuse(None);
    };
    match (kind, connection.session.as_mut()) {
        ("H", None) => hello(shared, backend, cid, connection, body),
        ("E", Some(session)) => {
            let Ok(message) = session.channel.open(body) else {
                // Not sealed with this session's key, or out of order: the session is over.
                if let Some(pairing) = &session.pairing {
                    lock(&shared.registry).pairing_failed(pairing);
                }
                return Received::Refuse(None);
            };
            let mut frames = Vec::new();
            if !session.proven {
                session.proven = true;
                frames.push(format!("a{cid}"));
                if let Some(device) = &session.device {
                    lock(&shared.registry).seen(device, now());
                    *lock(&shared.connected).entry(device.clone()).or_default() += 1;
                    (shared.config.changed)();
                }
            }
            let Ok(request) = serde_json::from_slice::<Request>(&message) else {
                return Received::Refuse(None);
            };
            match start(shared, backend, out, cid, session, request) {
                Ok(()) => Received::Send(frames),
                Err(()) => Received::Refuse(None),
            }
        }
        _ => Received::Refuse(None),
    }
}

/// A browser's hello: find the key it claims to hold and answer with this side's nonce and a
/// first sealed message, which only the holder of that key can open.
fn hello(
    shared: &Arc<Shared>,
    backend: &Arc<dyn Backend>,
    cid: &str,
    connection: &mut Connection,
    body: &str,
) -> Received {
    let refuse = |reason| Received::Refuse(Some(format!("m{cid} R{}", refusal(reason))));
    let Ok(hello) = serde_json::from_str::<Hello>(body) else {
        return Received::Refuse(None);
    };
    if hello.v != wire::VERSION {
        return refuse(Refusal::Version);
    }
    let Some(client_nonce) = crypto::decode::<NONCE_BYTES>(&hello.nonce) else {
        return Received::Refuse(None);
    };
    // The key, the workspace it is good for and what the device is, read together.
    let (key, scope, app) = {
        let mut registry = lock(&shared.registry);
        let (key, app) = match hello.kind {
            CredentialKind::Pair => (registry.pairing_key(&hello.id, now()), false),
            CredentialKind::Device => (
                registry.device_key(&hello.id),
                registry.device_kind(&hello.id) == Some(DeviceKind::Ios),
            ),
        };
        (key, registry.scope().to_owned(), app)
    };
    let Some(key) = key else {
        let elsewhere = hello.kind == CredentialKind::Device
            && lock(&shared.registry).device_elsewhere(&hello.id);
        return refuse(if elsewhere {
            Refusal::Elsewhere
        } else {
            Refusal::Unknown
        });
    };
    let Ok(host_nonce) = crypto::random::<NONCE_BYTES>() else {
        return Received::Refuse(None);
    };
    let context = format!("{}|{}", hello.kind.as_str(), hello.id);
    let Ok(mut channel) = Channel::new(Side::Host, &key, &client_nonce, &host_nonce, &context)
    else {
        return Received::Refuse(None);
    };
    let paired = hello.kind == CredentialKind::Device;
    // The app's features need the app's path, which the relay marks and no page can open, and
    // either a pairing link or a device recorded as the app. A browser's word that it is the app
    // is not enough; neither is the app's path with a browser's key.
    let native = connection.native;
    let features = if native && (!paired || app) {
        wire::APP_FEATURES
    } else {
        wire::FEATURES
    };
    let look = backend.look();
    let Ok(ready) = seal(
        &mut channel,
        &Event::Ready {
            host_name: &shared.config.host_name,
            paired,
            features,
            look: look.as_ref(),
            // The app keeps the site it pairs through, and follows it when it moves.
            site: native.then_some(shared.config.site.as_str()),
            notifications: native.then(|| super::notify::relays(shared)),
        },
    ) else {
        return Received::Refuse(None);
    };
    let welcome = serde_json::to_string(&Welcome {
        v: wire::VERSION,
        nonce: crypto::encode(&host_nonce),
    })
    .unwrap_or_default();
    connection.session = Some(Session {
        channel,
        pairing: (!paired).then(|| hello.id.clone()),
        device: paired.then_some(hello.id),
        scope,
        proven: false,
        features,
        native,
        calls: Arc::new(AtomicUsize::new(0)),
        backlog: Arc::new(AtomicUsize::new(0)),
        follow: Arc::new(AtomicU64::new(0)),
        following: Arc::new(tokio::sync::Mutex::new(())),
        ended: Arc::new(AtomicBool::new(false)),
        uploads: Arc::new(Mutex::new(super::uploads::Uploads::new(
            shared.staged.clone(),
        ))),
        picturing: Arc::new(AtomicBool::new(false)),
    });
    Received::Send(vec![
        format!("m{cid} W{welcome}"),
        format!("m{cid} E{ready}"),
    ])
}

/// Sends the answer to request `id`, and says whether it went as it was. One larger than a sealed
/// message carries ([`MAX_PLAINTEXT`]) is answered `too_large` instead: sealed, it could pass the
/// relay's limit for one frame, and the relay ends this computer's whole connection for that.
fn reply(
    out: &UnboundedSender<Out>,
    cid: &str,
    id: u32,
    result: Result<Data, BackendError>,
) -> bool {
    let failure = |error: &BackendError| {
        serde_json::to_string(&Reply {
            re: id,
            ok: false,
            data: None,
            error: Some(&ErrorBody {
                code: error.code,
                message: error.message.clone(),
            }),
        })
    };
    let json = match &result {
        Ok(data) => serde_json::to_string(&Reply {
            re: id,
            ok: true,
            data: Some(data),
            error: None,
        }),
        Err(error) => failure(error),
    };
    let whole = json.as_ref().is_ok_and(|json| json.len() <= MAX_PLAINTEXT);
    let json = match json {
        Ok(json) if whole => Ok(json),
        Ok(_) => failure(&BackendError::new(
            "too_large",
            "The answer is too large to send to this device.",
        )),
        Err(error) => Err(error),
    };
    // A value that will not serialize is a fault here, never the browser's doing: say so.
    let json = json.unwrap_or_else(|_| {
        serde_json::json!({
            "re": id,
            "ok": false,
            "error": { "code": "internal", "message": "The answer could not be sent." },
        })
        .to_string()
    });
    let _ = out.send(Out::Sealed {
        cid: cid.to_owned(),
        json,
        backlog: 0,
    });
    whole
}

/// Halves a page from its oldest end until it fits one message; the rest is the next page.
pub(super) fn fit(page: &mut HistoryPage) {
    while page.items.len() > 1
        && serde_json::to_vec(&*page).map_or(0, |bytes| bytes.len()) > MAX_PLAINTEXT
    {
        let keep = page.items.len() / 2;
        page.items.drain(..page.items.len() - keep);
        if let Some(first) = page.items.first() {
            page.has_more = true;
            page.before_seq = Some(first.seq);
            page.cut_turn_id.clone_from(&first.turn_id);
        }
    }
}

pub(super) fn text_ok(text: &str) -> Result<(), BackendError> {
    if text.trim().is_empty() {
        Err(BackendError::new("empty", "The message is empty."))
    } else if text.len() > MAX_TEXT {
        Err(BackendError::new("too_large", "The message is too long."))
    } else {
        Ok(())
    }
}

/// The most a model, thinking level or agent id may be: they are short names, never text.
const MAX_NAME: usize = 256;

/// A value picked from a list the desktop sent: short, and free of control characters.
pub(super) fn name_ok(name: &str) -> Result<(), BackendError> {
    if name.is_empty() || name.len() > MAX_NAME || name.chars().any(char::is_control) {
        Err(BackendError::new(
            "invalid",
            "That is not one of the choices.",
        ))
    } else {
        Ok(())
    }
}

fn choices(list: Vec<crate::backend::Choice>) -> Vec<wire::Choice> {
    list.into_iter()
        .map(|choice| wire::Choice {
            value: choice.value,
            name: choice.name,
        })
        .collect()
}

/// The app answered an action with something other than what that action returns.
pub(super) fn unexpected() -> BackendError {
    BackendError::new("failed", "This computer's Ghost answered unexpectedly.")
}

/// Starts a request. `Err` ends the session: the request broke the protocol.
fn start(
    shared: &Arc<Shared>,
    backend: &Arc<dyn Backend>,
    out: &UnboundedSender<Out>,
    cid: &str,
    session: &mut Session,
    request: Request,
) -> Result<(), ()> {
    let id = request.id;
    // A pairing link does one thing: become a device.
    if let Some(pairing) = session.pairing.clone() {
        let Op::Pair { name, client } = request.op else {
            reply(
                out,
                cid,
                id,
                Err(BackendError::new("pair_first", "Pair this browser first.")),
            );
            return Ok(());
        };
        // Recorded as the app only on the app's path: anything can say it is the app.
        let app = session.native && client.as_ref().is_some_and(wire::Client::ios);
        let (kind, version) = if app {
            (DeviceKind::Ios, client.and_then(|client| client.version))
        } else {
            (DeviceKind::Browser, None)
        };
        let result =
            lock(&shared.registry).pair_as(&pairing, &name, kind, version.as_deref(), now());
        match result {
            Ok(Some(device)) => {
                // The session is the device's from here on, offered what its next one will be.
                session.pairing = None;
                session.device = Some(device.id.clone());
                session.features = if app {
                    wire::APP_FEATURES
                } else {
                    wire::FEATURES
                };
                *lock(&shared.connected)
                    .entry(device.id.clone())
                    .or_default() += 1;
                (shared.config.changed)();
                reply(
                    out,
                    cid,
                    id,
                    Ok(Data::Paired(Paired {
                        device_id: device.id,
                        device_key: device.key,
                        features: app.then_some(wire::APP_FEATURES),
                    })),
                );
            }
            Ok(None) => {
                reply(
                    out,
                    cid,
                    id,
                    Err(BackendError::new(
                        "expired",
                        "This pairing link is no longer open. Create a new one in Ghost.",
                    )),
                );
            }
            Err(error) => {
                reply(
                    out,
                    cid,
                    id,
                    Err(BackendError::new("refused", error.to_string())),
                );
            }
        }
        return Ok(());
    }
    let Some(device) = session.device.clone() else {
        return Err(());
    };
    // A device removed in Settings is gone at once, even mid-session.
    if lock(&shared.registry).device_key(&device).is_none() {
        return Err(());
    }
    // A request this build knows but did not offer this session is answered, and the session
    // goes on: a device may be newer than this computer's Ghost, or a browser ask for what only
    // the app is offered.
    if let Some(feature) = request.op.feature()
        && !session.features.contains(&feature)
    {
        reply(
            out,
            cid,
            id,
            Err(BackendError::new(
                "not_offered",
                "This computer does not offer that to this device.",
            )),
        );
        return Ok(());
    }
    // A photo arrives in many small pieces: each is taken here, at once and in order, and holds
    // none of the places for calls.
    if let Some(result) = super::app::upload(&session.uploads, &request.op) {
        reply(out, cid, id, result);
        return Ok(());
    }
    if session.calls.load(Ordering::Relaxed) >= MAX_CALLS {
        reply(
            out,
            cid,
            id,
            Err(BackendError::new("busy", "Too many requests at once.")),
        );
        return Ok(());
    }
    // An open or a close takes the next number: only the newest one decides what is followed.
    let turn = if matches!(request.op, Op::Open { .. } | Op::Close) {
        session.backlog.store(0, Ordering::Relaxed);
        Some(Turn {
            mine: session.follow.fetch_add(1, Ordering::SeqCst) + 1,
            newest: session.follow.clone(),
            order: session.following.clone(),
            ended: session.ended.clone(),
        })
    } else {
        None
    };
    session.calls.fetch_add(1, Ordering::Relaxed);
    let (calls, backlog) = (session.calls.clone(), session.backlog.clone());
    let opening = matches!(request.op, Op::Open { .. });
    let context = Context {
        device,
        scope: session.scope.clone(),
        features: session.features,
        uploads: session.uploads.clone(),
        picturing: session.picturing.clone(),
    };
    let (shared, backend, out, cid) = (
        shared.clone(),
        backend.clone().bound(&session.scope),
        out.clone(),
        cid.to_owned(),
    );
    tokio::spawn(async move {
        let (result, held) = match turn {
            Some(turn) => {
                // In the order they were asked for, and only while still the newest.
                let held = turn.order.clone().lock_owned().await;
                let result = if turn.ended.load(Ordering::SeqCst) {
                    Err(BackendError::new("closed", "This session is over."))
                } else if turn.newest.load(Ordering::SeqCst) == turn.mine {
                    let result =
                        perform(&shared, &backend, &out, &cid, &context, backlog, request.op).await;
                    // The browser left while this ran: nothing may go on following for it.
                    if turn.ended.load(Ordering::SeqCst) {
                        backend.release(&owner(&cid));
                    }
                    result
                } else {
                    Err(BackendError::new(
                        "superseded",
                        "Another chat was opened after this one.",
                    ))
                };
                (result, Some(held))
            }
            None => (
                perform(&shared, &backend, &out, &cid, &context, backlog, request.op).await,
                None,
            ),
        };
        calls.fetch_sub(1, Ordering::Relaxed);
        let forgot = matches!(result, Ok(Performed::Forgot));
        let whole = reply(&out, &cid, id, result.map(Performed::data));
        // An open whose answer was too large to send follows nothing, as the device was told it
        // failed. Still in its turn, so no open asked for after it is undone.
        if opening && !whole {
            backend.release(&owner(&cid));
        }
        drop(held);
        if forgot {
            let _ = out.send(Out::Drop { cid });
        }
    });
    Ok(())
}

/// An open's or a close's place among the ones this browser asked for.
struct Turn {
    mine: u64,
    newest: Arc<AtomicU64>,
    order: Arc<tokio::sync::Mutex<()>>,
    ended: Arc<AtomicBool>,
}

enum Performed {
    Data(Data),
    /// The device removed itself: answer, then end the session.
    Forgot,
}

impl Performed {
    fn data(self) -> Data {
        match self {
            Self::Data(data) => data,
            Self::Forgot => Data::None {},
        }
    }
}

/// What a request may use of its session, beside the call itself.
pub(super) struct Context {
    /// The device the session is.
    pub(super) device: String,
    /// The workspace its key was accepted under ([`Session::scope`]): what it reads and keeps
    /// of tasks and the queue is that workspace's, or nothing.
    pub(super) scope: String,
    /// What the session was offered ([`Session::features`]).
    pub(super) features: &'static [&'static str],
    /// Its staged photos.
    pub(super) uploads: Arc<Mutex<super::uploads::Uploads>>,
    /// A picture is being taken for it.
    pub(super) picturing: Arc<AtomicBool>,
}

impl Context {
    pub(super) fn offered(&self, feature: &str) -> bool {
        self.features.contains(&feature)
    }
}

async fn perform(
    shared: &Arc<Shared>,
    backend: &Arc<dyn Backend>,
    out: &UnboundedSender<Out>,
    cid: &str,
    context: &Context,
    backlog: Arc<AtomicUsize>,
    op: Op,
) -> Result<Performed, BackendError> {
    use super::app;
    let done = |data| Ok(Performed::Data(data));
    match op {
        Op::Pair { .. } => Err(BackendError::new(
            "paired",
            "This browser is already paired.",
        )),
        Op::Overview => done(Data::Overview(Box::new(backend.overview().await?))),
        Op::Open { chat_id } => {
            let (sender, to, following) = (out.clone(), cid.to_owned(), chat_id.clone());
            let frames: Frames = Arc::new(move |batch| {
                let Ok(json) = serde_json::to_string(&Event::Frame { batch }) else {
                    return true;
                };
                let size = json.len();
                if backlog.fetch_add(size, Ordering::Relaxed) + size > MAX_BACKLOG {
                    // The browser is not keeping up: stop here and have it open the chat again.
                    let lost = serde_json::to_string(&Event::Lost {
                        chat_id: &following,
                    })
                    .unwrap_or_default();
                    let _ = sender.send(Out::Sealed {
                        cid: to.clone(),
                        json: lost,
                        backlog: size,
                    });
                    return false;
                }
                sender
                    .send(Out::Frame {
                        cid: to.clone(),
                        batch: batch.clone(),
                        json: Some(json),
                        backlog: size,
                    })
                    .is_ok()
            });
            let mut attach = backend.attach(&chat_id, &owner(cid), frames).await?;
            fit(&mut attach.history);
            done(Data::Attach(Box::new(attach)))
        }
        Op::Close => {
            backend.release(&owner(cid));
            done(Data::None {})
        }
        Op::History {
            chat_id,
            before_seq,
        } => {
            let mut page = backend.history(&chat_id, before_seq).await?;
            fit(&mut page);
            done(Data::History(Box::new(page)))
        }
        Op::Send {
            chat_id,
            text,
            attachments,
        } => {
            let sent = app::send(backend, context, &chat_id, text, &attachments).await?;
            app::acted(shared, context, &chat_id, false);
            done(sent)
        }
        Op::Interrupt { chat_id } => {
            backend.interrupt(&chat_id).await?;
            done(Data::None {})
        }
        Op::Permission {
            chat_id,
            request_id,
            option_id,
        } => {
            backend.permission(&chat_id, &request_id, option_id).await?;
            app::acted(shared, context, &chat_id, true);
            done(Data::None {})
        }
        Op::Answer {
            chat_id,
            request_id,
            answer,
        } => {
            backend.answer(&chat_id, &request_id, answer).await?;
            app::acted(shared, context, &chat_id, true);
            done(Data::None {})
        }
        Op::Create {
            folder_id,
            agent_id,
            text,
            attachments,
        } => {
            let created = app::create(
                backend,
                context,
                &folder_id,
                agent_id.as_deref(),
                text,
                &attachments,
            )
            .await?;
            if let Data::Created { chat, .. } | Data::Unsent { chat, .. } = &created {
                app::acted(shared, context, &chat.chat.id, false);
            }
            done(created)
        }
        // A paired browser reaches four of the app's actions and no others: what a chat offers,
        // its model, its thinking level, and handing it to another agent. Ghost for iPhone reaches
        // a few more by feature (`host/app.rs`). The rest of `Action` (projects from any folder,
        // a bot's memory edits) stays with a full-control bot link.
        Op::Options { chat_id } => match backend.act(Action::Options { chat_id }).await? {
            Outcome::Options(options) => done(Data::Options(wire::Choices {
                models: choices(options.models),
                model: options.model,
                thinking: choices(options.thinking),
                thinking_level: options.thinking_level,
                // A browser's answer is as it was: it is offered no access level.
                access: context.offered(wire::CONTROL).then(|| wire::AccessChoices {
                    access: choices(options.access),
                    access_mode: options.access_mode,
                }),
            })),
            _ => Err(unexpected()),
        },
        Op::SetModel { chat_id, model } => {
            name_ok(&model)?;
            backend.act(Action::SetModel { chat_id, model }).await?;
            done(Data::None {})
        }
        Op::SetThinking { chat_id, level } => {
            name_ok(&level)?;
            backend.act(Action::SetThinking { chat_id, level }).await?;
            done(Data::None {})
        }
        Op::SwitchAgent { chat_id, agent_id } => {
            name_ok(&agent_id)?;
            match backend
                .act(Action::SwitchAgent { chat_id, agent_id })
                .await?
            {
                Outcome::Chat { id } => done(Data::Switched { chat_id: id }),
                _ => Err(unexpected()),
            }
        }
        Op::Forget => {
            lock(&shared.registry)
                .remove_device(&context.device)
                .map_err(|error| BackendError::new("refused", error.to_string()))?;
            (shared.config.changed)();
            Ok(Performed::Forgot)
        }
        // What Ghost for iPhone is offered beyond a browser (`host/app.rs`).
        Op::Inbox => done(app::inbox(shared, backend, context).await?),
        Op::Search { query, limit } => done(app::search(backend, &query, limit).await?),
        Op::Archived { before, limit } => done(app::archived(backend, before, limit).await?),
        Op::SetAccess { chat_id, access } => done(app::set_access(backend, chat_id, access).await?),
        Op::Rename { chat_id, title } => done(app::rename(backend, chat_id, &title).await?),
        Op::Archive { chat_id, archived } => {
            backend.act(Action::Archive { chat_id, archived }).await?;
            done(Data::None {})
        }
        Op::StartAgent { chat_id } => {
            backend.act(Action::StartAgent { chat_id }).await?;
            done(Data::None {})
        }
        Op::Changes { chat_id, scope } => {
            done(app::changes(shared, backend, context, &chat_id, scope.as_deref()).await?)
        }
        Op::Diff {
            chat_id,
            path,
            scope,
        } => done(app::diff(shared, backend, context, &chat_id, &path, scope.as_deref()).await?),
        Op::Facts { chat_id } => done(app::facts(backend, &chat_id).await?),
        Op::Memory { project_id } => done(app::memory(backend, project_id).await?),
        Op::SetMemory {
            project_id,
            markdown,
            base_revision,
        } => done(
            app::set_memory(
                shared,
                backend,
                context,
                project_id,
                markdown,
                base_revision,
            )
            .await?,
        ),
        Op::Picture { chat_id, of } => {
            done(app::picture(backend, &context.picturing, &chat_id, &of).await?)
        }
        Op::Task {
            goal,
            project_id,
            agent_id,
            review,
            when,
            access,
            attachments,
        } => {
            let task = app::task(
                shared,
                backend,
                context,
                app::TaskAsked {
                    goal,
                    project_id,
                    agent_id,
                    review,
                    when,
                    access,
                    attachments,
                },
            )
            .await?;
            if let Data::Started(started) = &task {
                app::acted(shared, context, &started.chat_id, false);
            }
            done(task)
        }
        Op::Tasks => done(app::tasks(shared, context)?),
        Op::TaskCancel { queued_id } => done(app::task_cancel(shared, context, &queued_id)?),
        Op::Image {
            chat_id,
            source,
            full,
        } => done(app::image(backend, &chat_id, source, full).await?),
        Op::PushRegister { token, env, prefs } => {
            done(app::push_register(shared, context, &token, &env, prefs)?)
        }
        Op::PushUnregister => done(app::push_unregister(shared, context)?),
        Op::PushTest => done(app::push_test(shared, context)?),
        Op::Activity { token, env } => done(app::activity(shared, context, &token, &env)?),
        // Taken at once, in order, as they arrive (`app::upload`): never here.
        Op::UploadBegin { .. } | Op::UploadChunk { .. } | Op::UploadAbort { .. } => {
            Err(unexpected())
        }
    }
}
