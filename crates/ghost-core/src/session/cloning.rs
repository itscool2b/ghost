use super::*;
use crate::cloning::{CloneError, Clones};
use ghost_process::{LeftoverRegistry, Spawner};
use ghost_protocol::setup_actions::{CloneCheck, CloneInput, CloneRecord, CloneState};

impl SessionManager {
    pub async fn clone_check(
        &self,
        service: &Clones,
        owner: &str,
        input: CloneInput,
    ) -> Result<CloneCheck, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        service
            .check(owner, input, self.deps().env.get().await)
            .await
            .map_err(clone_error)
    }
    pub fn clone_start(
        &self,
        service: &Clones,
        owner: &str,
        id: &str,
        registry: Arc<LeftoverRegistry>,
        admission: Box<dyn Send + Sync>,
    ) -> Result<CloneRecord, SessionError> {
        if self.shared().is_shutting_down() {
            return Err(SessionError::ShuttingDown);
        }
        let (prepared, job) = service.take(owner, id).map_err(clone_error)?;
        let view = job.snapshot();
        let manager = self.clone();
        self.deps().rt.spawn(async move {
            // Native profile admission lives through reservation acquisition, execution and
            // cleanup. Starting the task must not leave a gap in which its dataset can switch.
            let _admission = admission;
            let folders = vec![
                prepared.parent.clone(),
                PathBuf::from(&prepared.view.destination),
            ];
            match manager.reserve_git_folders(folders).await {
                Ok(_reservation) => {
                    let spawner = Spawner::new(registry, manager.deps().rt.clone());
                    crate::cloning::execute(prepared, job, spawner).await;
                }
                Err(error) => job.finish(CloneState::Failed, &error.to_string()),
            }
        });
        Ok(view)
    }
}
fn clone_error(error: CloneError) -> SessionError {
    SessionError::Organization(error.to_string())
}
