use super::*;
use ghost_protocol::search::SearchResults;

impl SessionManager {
    pub async fn search(
        &self,
        query: String,
        include_archived: bool,
    ) -> Result<SearchResults, SessionError> {
        if query.len() > 512 || query.contains('\0') {
            return Err(SessionError::Organization(
                "Search is limited to 512 bytes and cannot contain a null character.".into(),
            ));
        }
        self.deps()
            .store
            .search(query, include_archived)
            .await
            .map_err(|error| storage_error(&error))
    }
}
