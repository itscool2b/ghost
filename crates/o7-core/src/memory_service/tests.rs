use super::*;
use crate::{
    chatstore::{NewChat, rows},
    owner::ProfileOwner,
    paths::DataRoot,
    profile::Profiles,
    store::Store,
};
use tokio::io::AsyncBufReadExt;
fn fixture() -> (tempfile::TempDir, Arc<ChatStore>, String) {
    let temp = tempfile::tempdir().unwrap();
    let profile = Profiles::new(DataRoot::at(temp.path()))
        .open_active()
        .unwrap();
    let owner = ProfileOwner::acquire(&profile).unwrap();
    let (store, _) = Store::open(&profile).unwrap();
    let chat = rows::create_chat(
        store.conn(),
        &NewChat {
            folder: "/service-memory".into(),
            agent_id: "mock".into(),
            agent_host: Default::default(),
            device_id: "device".into(),
            now: 1,
        },
    )
    .unwrap();
    let store = ChatStore::start(
        store,
        owner,
        &profile,
        "device".into(),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    (temp, store, chat.id)
}
async fn call(endpoint: &str, token: &str, mut value: serde_json::Value) -> serde_json::Value {
    let mut stream = tokio::net::TcpStream::connect(endpoint).await.unwrap();
    value["token"] = serde_json::json!(token);
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    stream.write_all(&bytes).await.unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}
#[test]
fn token_comparison_rejects_prefix_and_wrong_bytes() {
    assert!(same_token("abc", "abc"));
    assert!(!same_token("abc", "ab"));
    assert!(!same_token("abc", "abd"));
    assert!(!same_token("abc", "abcd"));
}
fn grant(store: &Arc<ChatStore>, chat: &str, direct: &Arc<AtomicBool>) -> MemoryGrant {
    MemoryGrant::start(
        store.clone(),
        chat.into(),
        direct.clone(),
        &tokio::runtime::Handle::current(),
    )
    .unwrap()
}
#[tokio::test]
async fn read_only_memory_service_reads_refuses_changes_and_drop_revokes_credential() {
    let (_dir, store, chat) = fixture();
    let grant = grant(&store, &chat, &Arc::new(AtomicBool::new(false)));
    let endpoint = grant.endpoint.clone();
    let token = grant.token.clone();
    assert!(!format!("{grant:?}").contains(&token));
    let denied = call(&endpoint, "wrong", serde_json::json!({"operation":"read"})).await;
    assert!(denied.get("error").is_some());
    let read = call(&endpoint, &token, serde_json::json!({"operation":"read"})).await;
    assert_eq!(read["result"]["markdown"], "");
    // A session from before proposals went may still ask for one: it is told they are gone.
    let proposed=call(&endpoint,&token,serde_json::json!({"operation":"propose","base_revision":null,"markdown":"Proposed durable fact","reason":"Useful across chats","idempotency_key":"service-test"})).await;
    assert_eq!(proposed["error"], NO_PROPOSALS);
    let read = call(&endpoint, &token, serde_json::json!({"operation":"read"})).await;
    assert_eq!(read["result"]["markdown"], "");
    let write = call(
        &endpoint,
        &token,
        serde_json::json!({"operation":"write","base_revision":null,"markdown":"Direct","reason":"Not allowed"}),
    )
    .await;
    assert_eq!(write["error"], READ_ONLY);
    let read = call(&endpoint, &token, serde_json::json!({"operation":"read"})).await;
    assert_eq!(read["result"]["markdown"], "");
    let accept = call(&endpoint, &token, serde_json::json!({"operation":"accept"})).await;
    assert_eq!(
        accept["error"],
        "Unsupported memory operation. Use read, write, note_write or note_delete."
    );
    let cross = call(
        &endpoint,
        &token,
        serde_json::json!({"operation":"read","project_id":"foreign"}),
    )
    .await;
    assert!(cross.get("error").is_some());
    drop(grant);
    // The endpoint closes once its task sees the grant gone, which takes more than one turn of the
    // runtime while other tests keep it busy.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while tokio::net::TcpStream::connect(&endpoint).await.is_ok() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the dropped grant's endpoint closes");
    store.shutdown(std::time::Duration::from_secs(5)).await;
}
#[tokio::test]
async fn direct_writes_follow_the_profile_switch_mid_grant_and_still_need_the_token() {
    let (_dir, store, chat) = fixture();
    let direct = Arc::new(AtomicBool::new(true));
    let grant = grant(&store, &chat, &direct);
    let (endpoint, token) = (grant.endpoint.clone(), grant.token.clone());
    let mut changes = store.memory_changes();
    let write = |base: serde_json::Value, markdown: &str| serde_json::json!({"operation":"write","base_revision":base,"markdown":markdown,"reason":"Learned the test command"});
    let stranger = call(&endpoint, "wrong", write(serde_json::Value::Null, "Forged")).await;
    assert_eq!(
        stranger["error"],
        "Memory access expired. Restart the o7 session."
    );
    let first = call(
        &endpoint,
        &token,
        write(serde_json::Value::Null, "Run cargo test."),
    )
    .await;
    assert_eq!(first["result"]["markdown"], "Run cargo test.");
    let revision = first["result"]["revision"].clone();
    assert!(revision.is_string());
    let project = first["result"]["project_id"].as_str().unwrap().to_owned();
    // The window hears about the agent's edit, with what its notice needs to undo it.
    assert_eq!(
        changes.recv().await.unwrap(),
        o7_protocol::memory::ProjectMemoryChanged {
            project_id: Some(project.clone()),
            agent_edit: Some(o7_protocol::memory::AgentMemoryEdit {
                revision: revision.as_str().unwrap().to_owned(),
                parent_revision: None,
                chat_id: chat.clone(),
                note: None,
                deleted: false,
            }),
        }
    );
    let Reply::History(history) = store
        .memory(Command::History(project.clone()))
        .await
        .unwrap()
    else {
        panic!("history")
    };
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].source, format!("agent:{chat}"));
    // A lost reply retried with the old base answers the revision that landed.
    let retried = call(
        &endpoint,
        &token,
        write(serde_json::Value::Null, "Run cargo test."),
    )
    .await;
    assert_eq!(retried["result"]["revision"], revision);
    // The retry added nothing, so it names no edit for a notice.
    assert_eq!(changes.recv().await.unwrap().agent_edit, None);
    // A different edit from a stale base is refused with the way forward.
    let stale = call(
        &endpoint,
        &token,
        write(serde_json::Value::Null, "Lost update"),
    )
    .await;
    assert!(
        stale["error"]
            .as_str()
            .unwrap()
            .contains("Read it again, apply your edit")
    );
    // Writes must say what they replace memory with; absence is not an empty document.
    let partial = call(
        &endpoint,
        &token,
        serde_json::json!({"operation":"write","base_revision":revision,"reason":"No Markdown"}),
    )
    .await;
    assert!(partial.get("error").is_some());
    let unknown = call(
        &endpoint,
        &token,
        serde_json::json!({"operation":"write","base_revision":revision,"markdown":"x","reason":"x","project_id":"foreign"}),
    )
    .await;
    assert!(unknown.get("error").is_some());
    // Turning the switch off applies to this running grant at once; reading stays.
    direct.store(false, std::sync::atomic::Ordering::Release);
    let refused = call(&endpoint, &token, write(revision.clone(), "While off")).await;
    assert_eq!(refused["error"], READ_ONLY);
    let proposed = call(
        &endpoint,
        &token,
        serde_json::json!({"operation":"propose","base_revision":revision,"markdown":"While off","reason":"Review this","idempotency_key":"off"}),
    )
    .await;
    assert_eq!(proposed["error"], NO_PROPOSALS);
    let read = call(&endpoint, &token, serde_json::json!({"operation":"read"})).await;
    assert_eq!(read["result"]["markdown"], "Run cargo test.");
    direct.store(true, std::sync::atomic::Ordering::Release);
    let again = call(&endpoint, &token, write(revision.clone(), "Back on")).await;
    assert_eq!(again["result"]["markdown"], "Back on");
    assert_ne!(again["result"]["revision"], revision);
    drop(grant);
    store.shutdown(std::time::Duration::from_secs(5)).await;
}
/// Notes through a chat's grant: an agent adds, reads, replaces and removes one note of its own
/// chat's project, each from the revision it read; the window hears of each edit; and every write
/// follows the profile's switch.
#[tokio::test]
async fn an_agent_keeps_notes_through_its_grant_and_every_write_follows_the_switch() {
    let (_dir, store, chat) = fixture();
    let direct = Arc::new(AtomicBool::new(true));
    let grant = grant(&store, &chat, &direct);
    let (endpoint, token) = (grant.endpoint.clone(), grant.token.clone());
    let mut changes = store.memory_changes();
    let write = |base: serde_json::Value, markdown: &str| serde_json::json!({"operation":"note_write","note":"windows-build","base_revision":base,"description":"Read before building on Windows","markdown":markdown,"reason":"Learned how the build works"});
    // A stranger, a name that could leave the list, and a write that names no note are refused.
    let stranger = call(&endpoint, "wrong", write(serde_json::Value::Null, "Forged")).await;
    assert_eq!(
        stranger["error"],
        "Memory access expired. Restart the o7 session."
    );
    let outside = call(&endpoint, &token, serde_json::json!({"operation":"note_write","note":"../other","base_revision":null,"description":"d","markdown":"m","reason":"r"})).await;
    assert!(
        outside["error"]
            .as_str()
            .unwrap()
            .starts_with("Name a note like windows-build")
    );
    let unnamed = call(&endpoint, &token, serde_json::json!({"operation":"note_write","base_revision":null,"markdown":"m","reason":"r"})).await;
    assert!(
        unnamed["error"]
            .as_str()
            .unwrap()
            .starts_with("A note write needs the note's name")
    );
    // A project named by the caller is not a field the service takes.
    let elsewhere = call(&endpoint, &token, serde_json::json!({"operation":"read","project_id":"another-project"})).await;
    assert_eq!(elsewhere["error"], "Invalid memory request.");

    let added = call(&endpoint, &token, write(serde_json::Value::Null, "Use tb2.")).await;
    assert_eq!(added["result"]["name"], "windows-build");
    assert_eq!(added["result"]["by"], "agent");
    let first = added["result"]["revision"].as_str().unwrap().to_owned();
    let project = added["result"]["project_id"].as_str().unwrap().to_owned();
    assert_eq!(
        changes.recv().await.unwrap(),
        o7_protocol::memory::ProjectMemoryChanged {
            project_id: Some(project.clone()),
            agent_edit: Some(o7_protocol::memory::AgentMemoryEdit {
                revision: first.clone(),
                parent_revision: None,
                chat_id: chat.clone(),
                note: Some("windows-build".into()),
                deleted: false,
            }),
        }
    );
    // The brief and the list: the note is listed with its line, not its text.
    let listed = call(&endpoint, &token, serde_json::json!({"operation":"read"})).await;
    assert_eq!(listed["result"]["markdown"], "");
    assert_eq!(listed["result"]["revision"], serde_json::Value::Null);
    assert_eq!(listed["result"]["head"], first.as_str());
    assert_eq!(
        listed["result"]["notes"][0]["description"],
        "Read before building on Windows"
    );
    assert!(listed["result"]["notes"][0].get("markdown").is_none());
    let read = call(&endpoint, &token, serde_json::json!({"operation":"read","note":"windows-build"})).await;
    assert_eq!(read["result"]["markdown"], "Use tb2.");
    // Replacing it needs the revision that was read.
    let stale = call(&endpoint, &token, write(serde_json::Value::Null, "Other")).await;
    assert!(
        stale["error"]
            .as_str()
            .unwrap()
            .starts_with("There is already a note windows-build")
    );
    let replaced = call(&endpoint, &token, write(serde_json::json!(first), "Use tb2. Never cargo.")).await;
    let second = replaced["result"]["revision"].as_str().unwrap().to_owned();
    assert_ne!(second, first);
    let _ = changes.recv().await.unwrap();

    // With the switch off, no write of any kind lands, and reads still answer.
    direct.store(false, Ordering::Release);
    for refused in [
        write(serde_json::json!(second), "Changed while off"),
        serde_json::json!({"operation":"note_delete","note":"windows-build","base_revision":second,"reason":"Out of date"}),
        serde_json::json!({"operation":"write","base_revision":null,"markdown":"Brief","reason":"r"}),
    ] {
        assert_eq!(call(&endpoint, &token, refused).await["error"], READ_ONLY);
    }
    let still = call(&endpoint, &token, serde_json::json!({"operation":"read","note":"windows-build"})).await;
    assert_eq!(still["result"]["markdown"], "Use tb2. Never cargo.");
    direct.store(true, Ordering::Release);

    // Removing it, from the revision that was read; the window hears that too.
    let early = call(&endpoint, &token, serde_json::json!({"operation":"note_delete","note":"windows-build","base_revision":first,"reason":"Out of date"})).await;
    assert!(early["error"].as_str().unwrap().contains("Read it again"));
    let removed = call(&endpoint, &token, serde_json::json!({"operation":"note_delete","note":"windows-build","base_revision":second,"reason":"Out of date"})).await;
    assert_eq!(removed["result"]["notes"], serde_json::json!([]));
    let heard = changes.recv().await.unwrap();
    let edit = heard.agent_edit.expect("the removal is an agent's edit");
    assert_eq!(
        (edit.note.as_deref(), edit.deleted, edit.parent_revision),
        (Some("windows-build"), true, Some(second))
    );
    let gone = call(&endpoint, &token, serde_json::json!({"operation":"read","note":"windows-build"})).await;
    assert_eq!(
        gone["error"],
        "There is no note windows-build. o7_memory_read lists the notes there are."
    );
}
