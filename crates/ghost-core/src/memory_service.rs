//! App-owned memory service. A grant owns one generation's listener and revokes on drop.
//! No database path crosses the process boundary.
use crate::chatstore::{
    ChatStore,
    memory::{Command, Reply},
};
use ghost_protocol::{
    mcp::{McpCredentials, McpDefinition, McpTransport, ResolvedMcpServer},
    memory::{DeleteMemoryNote, SaveMemoryNote, SaveProjectMemory},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    task::AbortHandle,
};

pub(crate) struct MemoryGrant {
    endpoint: String,
    token: String,
    abort: AbortHandle,
}
impl std::fmt::Debug for MemoryGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryGrant").finish_non_exhaustive()
    }
}
impl Drop for MemoryGrant {
    fn drop(&mut self) {
        self.abort.abort();
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    token: String,
    operation: String,
    #[serde(default)]
    base_revision: Option<String>,
    /// Absent is not empty: a write must name its whole replacement, even an empty one.
    #[serde(default)]
    markdown: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    /// The note a read, a note write or a note delete names. A read without one answers the
    /// brief and the list of notes.
    #[serde(default)]
    note: Option<String>,
    /// A note's one line that says when to read it.
    #[serde(default)]
    description: Option<String>,
    /// Proposals carried one. A helper from before they were removed still sends it, and unknown
    /// fields are refused, so it is accepted and unused.
    #[serde(default, rename = "idempotency_key")]
    _idempotency_key: String,
}
/// What an agent reads when the person has turned agents' edits off.
pub(crate) const READ_ONLY: &str = "The user has turned off agents' edits to project memory, so nothing was saved. Do not try again: tell the user what you would have saved, so they can add it in Aura's Project memory page.";
/// What an agent reads when it asks for a review that no longer exists (owner, 10 November 2026).
pub(crate) const NO_PROPOSALS: &str = "Project memory has no proposals any more. Save the change with ghost_memory_write, or tell the user what to add.";
impl MemoryGrant {
    /// `direct_writes` is the profile's switch, read on every request, so turning it off or on
    /// applies to running chats at once.
    pub(crate) fn start(
        store: Arc<ChatStore>,
        chat_id: String,
        direct_writes: Arc<AtomicBool>,
        rt: &tokio::runtime::Handle,
    ) -> std::io::Result<Self> {
        let socket = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        socket.set_nonblocking(true)?;
        let endpoint = socket.local_addr()?.to_string();
        let token = format!("{}{}", crate::ids::new_id(), crate::ids::new_id());
        let credential = token.clone();
        let task=rt.spawn(async move {
            let Ok(listener)=TcpListener::from_std(socket) else {return;};
            let project_id = match store.memory(Command::ReadChat(chat_id.clone())).await { Ok(Reply::Memory(memory)) => memory.project_id, _ => return };
            let mut changes = store.workspace_changes();
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    changed = changes.changed() => { if changed.is_err() || !store.workspace_available(&chat_id) { return; } continue; }
                };
                let Ok((mut stream,_))=accepted else {return;};
                let mut revoked = false;
                // Bound both bytes and wall time, including unauthenticated local callers.
                let result=tokio::time::timeout(std::time::Duration::from_secs(3),async {
                    use tokio::io::AsyncReadExt;
                    let mut input=Vec::new();
                    let mut reader=BufReader::new((&mut stream).take(96*1024));
                    reader.read_until(b'\n',&mut input).await?;
                    if input.len()>=96*1024 || input.last()!=Some(&b'\n'){return Ok::<_,std::io::Error>(serde_json::json!({"error":"Invalid bounded memory request."}));}
                    let request:Request=match serde_json::from_slice(&input){Ok(v)=>v,Err(_)=>return Ok(serde_json::json!({"error":"Invalid memory request."}))};
                    if !same_token(&credential,&request.token){return Ok(serde_json::json!({"error":"Memory access expired. Restart the Ghost session."}));}
                    let project=match store.memory(Command::ReadChat(chat_id.clone())).await {Ok(Reply::Memory(memory)) if memory.project_id == project_id =>Some(memory.project_id),_=>None};
                    let Some(project)=project else {revoked=true;return Ok(serde_json::json!({"error":"The project is unavailable."}));};
                    // Scope is derived from the app-owned chat, never a client supplied project ID.
                    let command=match request.operation.as_str(){
                        "read"=>match request.note {
                            Some(name)=>Command::ReadNote{project_id:project,name},
                            None=>Command::Read(project),
                        },
                        "note_write"=>{
                            if !direct_writes.load(Ordering::Acquire){return Ok(serde_json::json!({"error":READ_ONLY}));}
                            let (Some(name),Some(description),Some(markdown),Some(reason))=(request.note,request.description,request.markdown,request.reason) else {return Ok(serde_json::json!({"error":"A note write needs the note's name, its one-line description, its complete Markdown, its base revision and a reason."}));};
                            Command::AgentSaveNote{input:SaveMemoryNote{project_id:project,name,expected_revision:request.base_revision,description,markdown,reason},chat_id:chat_id.clone()}
                        }
                        "note_delete"=>{
                            if !direct_writes.load(Ordering::Acquire){return Ok(serde_json::json!({"error":READ_ONLY}));}
                            let (Some(name),Some(reason))=(request.note,request.reason) else {return Ok(serde_json::json!({"error":"A note delete needs the note's name, its base revision and a reason."}));};
                            Command::AgentDeleteNote{input:DeleteMemoryNote{project_id:project,name,expected_revision:request.base_revision,reason},chat_id:chat_id.clone()}
                        }
                        "write"=>{
                            if !direct_writes.load(Ordering::Acquire){return Ok(serde_json::json!({"error":READ_ONLY}));}
                            let (Some(markdown),Some(reason))=(request.markdown,request.reason) else {return Ok(serde_json::json!({"error":"A memory write needs the complete replacement Markdown, its base revision and a reason."}));};
                            Command::AgentSave{input:SaveProjectMemory{project_id:project,expected_revision:request.base_revision,markdown,reason},chat_id:chat_id.clone()}
                        }
                        // A session started before proposals went still has the tool: it is told so.
                        "propose"=>return Ok(serde_json::json!({"error":NO_PROPOSALS})),
                        _=>return Ok(serde_json::json!({"error":"Unsupported memory operation. Use read, write, note_write or note_delete."}))
                    };
                    Ok(match store.memory(command).await {Ok(Reply::Memory(v))=>serde_json::json!({"result":v}),Ok(Reply::Note(v))=>serde_json::json!({"result":v}),Ok(Reply::Proposal(v))=>serde_json::json!({"result":v}),Ok(_)=>serde_json::json!({"error":"Unexpected memory response."}),Err(error)=>serde_json::json!({"error":error.to_string()})})
                }).await;
                let response=match result{Ok(Ok(v))=>v,_=>serde_json::json!({"error":"The Ghost memory service is unavailable."})};
                if let Ok(mut bytes)=serde_json::to_vec(&response){bytes.push(b'\n');let _=tokio::time::timeout(std::time::Duration::from_secs(1),stream.write_all(&bytes)).await;}
                if revoked { return; }
            }
        });
        Ok(Self {
            endpoint,
            token,
            abort: task.abort_handle(),
        })
    }
    pub(crate) fn mcp(&self, project: Option<String>) -> Option<ResolvedMcpServer> {
        let path = crate::paths::helper_program()?;
        Some(ResolvedMcpServer {
            definition: McpDefinition {
                id: "ghost-project-memory".into(),
                name: "ghost_project_memory".into(),
                project_id: project,
                enabled: true,
                transport: McpTransport::Stdio {
                    command: path.to_string_lossy().into_owned(),
                    args: vec!["--ghost-memory-helper".into(), "--mcp".into()],
                },
                env_keys: vec!["GHOST_MEMORY_ENDPOINT".into(), "GHOST_MEMORY_TOKEN".into()],
                header_keys: vec![],
                agents: None,
            },
            revision: 1,
            credentials: McpCredentials {
                env: BTreeMap::from([
                    ("GHOST_MEMORY_ENDPOINT".into(), self.endpoint.clone()),
                    ("GHOST_MEMORY_TOKEN".into(), self.token.clone()),
                ]),
                headers: BTreeMap::new(),
            },
        })
    }
}
fn same_token(a: &str, b: &str) -> bool {
    let mut diff = a.len() ^ b.len();
    for (i, x) in a.bytes().enumerate() {
        diff |= usize::from(x ^ b.as_bytes().get(i).copied().unwrap_or(0));
    }
    diff == 0
}
#[cfg(test)]
mod tests;

mod helper;
pub use helper::run_helper;
