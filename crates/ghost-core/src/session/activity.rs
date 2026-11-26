use super::*;
use ghost_protocol::activity::{LineEditBucket, LineEdits};

impl SessionManager {
    /// The Dashboard's line edits since `since` (Unix milliseconds): every agent's, by quarter hour,
    /// counting the turns that ended since the last call first. An agent reached two ways (Claude
    /// Code natively and through ACP) is one agent here, named as the catalog names it.
    pub async fn line_edits(&self, since: Millis) -> Result<LineEdits, SessionError> {
        let edits = self
            .deps()
            .store
            .line_edits(since)
            .await
            .map_err(|error| storage_error(&error))?;
        Ok(LineEdits {
            buckets: by_agent(edits.buckets),
            complete: edits.complete,
        })
    }
}

/// Buckets with each agent's connection folded into the agent, oldest first.
fn by_agent(buckets: Vec<LineEditBucket>) -> Vec<LineEditBucket> {
    let mut merged = std::collections::BTreeMap::<(Millis, String), u32>::new();
    for bucket in buckets {
        let agent = ghost_agents::agent_of(&bucket.agent).to_owned();
        let lines = merged.entry((bucket.at, agent)).or_insert(0);
        *lines = lines.saturating_add(bucket.lines);
    }
    merged
        .into_iter()
        .map(|((at, agent), lines)| LineEditBucket { agent, at, lines })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agents_connections_count_as_the_agent() {
        let bucket = |agent: &str, at: Millis, lines: u32| LineEditBucket {
            agent: agent.into(),
            at,
            lines,
        };
        assert_eq!(
            by_agent(vec![
                bucket("claude", 1, 2),
                bucket("claude-acp", 1, 3),
                bucket("codex-acp", 2, 4),
                bucket("someone-new", 2, 1),
            ]),
            vec![
                bucket("claude", 1, 5),
                bucket("codex", 2, 4),
                bucket("someone-new", 2, 1),
            ]
        );
    }
}
