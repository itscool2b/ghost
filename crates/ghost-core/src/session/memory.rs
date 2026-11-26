//! User-facing project memory operations. All reads and changes use the profile writer.
use super::*;
use crate::chatstore::memory::{Command, Reply};
use ghost_protocol::memory::*;
fn failed(error: impl std::fmt::Display) -> SessionError {
    SessionError::OptionInvalid(error.to_string())
}
impl SessionManager {
    pub async fn read_project_memory(
        &self,
        project_id: &str,
    ) -> Result<ProjectMemory, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::Read(project_id.into()))
            .await
            .map_err(failed)?
        {
            Reply::Memory(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    pub async fn save_project_memory(
        &self,
        input: SaveProjectMemory,
    ) -> Result<ProjectMemory, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::Save(input))
            .await
            .map_err(failed)?
        {
            Reply::Memory(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    pub async fn propose_project_memory(
        &self,
        input: ProposeProjectMemory,
    ) -> Result<MemoryProposal, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::Propose(input))
            .await
            .map_err(failed)?
        {
            Reply::Proposal(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    pub async fn accept_memory_proposal(
        &self,
        input: AcceptMemoryProposal,
    ) -> Result<ProjectMemory, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::Accept(input))
            .await
            .map_err(failed)?
        {
            Reply::Memory(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    pub async fn reject_memory_proposal(
        &self,
        project_id: &str,
        proposal_id: &str,
    ) -> Result<Vec<MemoryProposal>, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::Reject {
                project_id: project_id.into(),
                proposal_id: proposal_id.into(),
            })
            .await
            .map_err(failed)?
        {
            Reply::Proposals(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    pub async fn project_memory_history(
        &self,
        project_id: &str,
    ) -> Result<Vec<MemoryRevision>, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::History(project_id.into()))
            .await
            .map_err(failed)?
        {
            Reply::History(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    pub async fn project_memory_proposals(
        &self,
        project_id: &str,
    ) -> Result<Vec<MemoryProposal>, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::Proposals(project_id.into()))
            .await
            .map_err(failed)?
        {
            Reply::Proposals(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    pub async fn export_project_memory(
        &self,
        project_id: &str,
    ) -> Result<ProjectMemoryBundle, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::Export(project_id.into()))
            .await
            .map_err(failed)?
        {
            Reply::Bundle(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    pub async fn import_project_memory(
        &self,
        input: ImportProjectMemory,
    ) -> Result<ProjectMemory, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::Import(input))
            .await
            .map_err(failed)?
        {
            Reply::Memory(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    /// One note of a project's memory, with its text.
    pub async fn read_memory_note(
        &self,
        project_id: &str,
        name: &str,
    ) -> Result<MemoryNote, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::ReadNote {
                project_id: project_id.into(),
                name: name.into(),
            })
            .await
            .map_err(failed)?
        {
            Reply::Note(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    /// The user's save of one note, from the revision they read.
    pub async fn save_memory_note(
        &self,
        input: SaveMemoryNote,
    ) -> Result<MemoryNote, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::SaveNote(input))
            .await
            .map_err(failed)?
        {
            Reply::Note(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    /// The user's removal of one note; answers the memory as it then stands.
    pub async fn delete_memory_note(
        &self,
        input: DeleteMemoryNote,
    ) -> Result<ProjectMemory, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::DeleteNote(input))
            .await
            .map_err(failed)?
        {
            Reply::Memory(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    /// Takes back a run of edits to the brief or to one note (`first` to `last`; the same
    /// revision for one edit), as the user's own save. Refused once the brief or the note has
    /// moved on from `last`.
    pub async fn undo_memory_edit(
        &self,
        project_id: &str,
        first: &str,
        last: &str,
    ) -> Result<ProjectMemory, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::Undo {
                project_id: project_id.into(),
                first: first.into(),
                last: last.into(),
            })
            .await
            .map_err(failed)?
        {
            Reply::Memory(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
    /// A linked bot's save of the brief: saved as a bot's, so the window says so and offers to
    /// undo it, as it does for an agent's.
    pub async fn bot_save_project_memory(
        &self,
        input: SaveProjectMemory,
    ) -> Result<ProjectMemory, SessionError> {
        match self
            .deps()
            .store
            .memory(Command::BotSave(input))
            .await
            .map_err(failed)?
        {
            Reply::Memory(v) => Ok(v),
            _ => Err(failed("Invalid memory reply.")),
        }
    }
}
