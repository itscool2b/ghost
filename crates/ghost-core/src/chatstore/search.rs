//! Literal, bounded search of metadata and the profile's completed-message index.
use ghost_protocol::search::{ChatSearchHit, ProjectSearchHit, SearchResults};
use rusqlite::{Connection, params};

pub(crate) fn execute(
    conn: &Connection,
    query: &str,
    include_archived: bool,
) -> rusqlite::Result<SearchResults> {
    let literal = query.trim();
    // Quoting individual alphanumeric tokens makes FTS operators ordinary input. Prefix matching
    // keeps incremental search useful without passing caller syntax to the FTS parser.
    let terms = literal
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(8)
        .map(|word| format!("\"{}\"*", word.chars().take(64).collect::<String>()))
        .collect::<Vec<_>>()
        .join(" AND ");
    let mut projects = conn.prepare_cached(
        "SELECT id,substr(name,1,160) FROM projects WHERE deleted=0
         AND (?1='' OR instr(lower(name),lower(?1))>0) ORDER BY updated_at DESC,id LIMIT 20",
    )?;
    let projects = projects
        .query_map([literal], |row| {
            Ok(ProjectSearchHit {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut statement = conn.prepare_cached(
        "SELECT c.id,substr(COALESCE(c.title,'New chat'),1,180),substr(p.name,1,160),c.archived
         FROM chats c JOIN projects p ON p.id=c.project_id
         WHERE c.deleted=0 AND p.deleted=0 AND (?2 OR c.archived=0)
         AND (?1='' OR instr(lower(COALESCE(c.title,'')),lower(?1))>0
              OR instr(lower(p.name),lower(?1))>0 OR instr(lower(COALESCE(c.active_agent,'')),lower(?1))>0)
         ORDER BY c.updated_at DESC,c.id LIMIT 40",
    )?;
    let mut chats = statement
        .query_map(params![literal, include_archived], |row| {
            Ok(ChatSearchHit {
                id: row.get(0)?,
                title: row.get(1)?,
                project: row.get(2)?,
                archived: row.get(3)?,
                snippet: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !terms.is_empty() {
        let mut statement = conn.prepare_cached(
            "SELECT c.id,substr(COALESCE(c.title,'New chat'),1,180),substr(p.name,1,160),c.archived,
             snippet(message_fts,2,'','','…',24)
             FROM message_fts JOIN chats c ON c.id=message_fts.chat_id
             JOIN projects p ON p.id=c.project_id WHERE message_fts MATCH ?1
             AND c.deleted=0 AND p.deleted=0 AND (?2 OR c.archived=0)
             ORDER BY rank LIMIT 80",
        )?;
        let rows = statement.query_map(params![terms, include_archived], |row| {
            Ok(ChatSearchHit {
                id: row.get(0)?,
                title: row.get(1)?,
                project: row.get(2)?,
                archived: row.get(3)?,
                snippet: Some(row.get::<_, String>(4)?.chars().take(240).collect()),
            })
        })?;
        for row in rows {
            let hit = row?;
            if let Some(existing) = chats.iter_mut().find(|chat| chat.id == hit.id) {
                if existing.snippet.is_none() {
                    existing.snippet = hit.snippet;
                }
            } else if chats.len() < 40 {
                chats.push(hit);
            }
        }
    }
    Ok(SearchResults { chats, projects })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chatstore::test_support::TestDb;

    #[test]
    fn literal_fts_search_folds_accents_and_hides_deleted_and_archived_chats() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        db.conn().execute("INSERT INTO message_fts(chat_id,message_id,text) VALUES(?1,'m','Crème brûlée render pipeline')",[&chat]).expect("message");
        let result = execute(db.conn(), "creme", false).expect("search");
        assert_eq!(result.chats[0].id, chat);
        assert!(
            result.chats[0]
                .snippet
                .as_ref()
                .expect("snippet")
                .contains("Crème")
        );
        assert!(
            execute(db.conn(), "\" OR * : (", false)
                .expect("literal")
                .chats
                .is_empty()
        );
        db.conn()
            .execute("UPDATE chats SET archived=1 WHERE id=?1", [&chat])
            .expect("archive");
        assert!(
            execute(db.conn(), "render", false)
                .expect("hidden")
                .chats
                .is_empty()
        );
        assert_eq!(
            execute(db.conn(), "render", true)
                .expect("archive search")
                .chats
                .len(),
            1
        );
        db.conn()
            .execute("UPDATE chats SET deleted=1 WHERE id=?1", [&chat])
            .expect("delete");
        assert!(
            execute(db.conn(), "render", true)
                .expect("deleted")
                .chats
                .is_empty()
        );
    }

    fn index(db: &TestDb, chat: &str, message: &str, text: &str) {
        db.conn()
            .execute(
                "INSERT INTO message_fts(chat_id,message_id,text) VALUES(?1,?2,?3)",
                params![chat, message, text],
            )
            .expect("index");
    }

    fn project_of(db: &TestDb, chat: &str) -> String {
        db.conn()
            .query_row("SELECT project_id FROM chats WHERE id=?1", [chat], |row| {
                row.get(0)
            })
            .expect("project")
    }

    #[test]
    fn metadata_matches_title_project_and_agent_in_any_case() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        db.conn()
            .execute(
                "UPDATE chats SET title='Fix the Sidebar', active_agent='claude' WHERE id=?1",
                [&chat],
            )
            .expect("title");
        db.conn()
            .execute(
                "UPDATE projects SET name='Aurora App' WHERE id=?1",
                [project_of(&db, &chat)],
            )
            .expect("project name");
        for query in ["sidebar", "AURORA", "Claude"] {
            let result = execute(db.conn(), query, false).expect("search");
            assert_eq!(result.chats.len(), 1, "{query}");
            assert_eq!(result.chats[0].id, chat, "{query}");
            assert!(result.chats[0].snippet.is_none(), "{query}");
        }
        let projects = execute(db.conn(), "aurora", false)
            .expect("projects")
            .projects;
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "Aurora App");
    }

    #[test]
    fn an_empty_query_lists_recent_chats_newest_first() {
        let db = TestDb::new();
        let older = db.chat("/older");
        let newer = db.chat("/newer");
        for (chat, at) in [(&older, 10), (&newer, 20)] {
            db.conn()
                .execute(
                    "UPDATE chats SET updated_at=?2 WHERE id=?1",
                    params![chat, at],
                )
                .expect("updated");
        }
        let result = execute(db.conn(), "   ", false).expect("empty");
        let ids: Vec<&str> = result.chats.iter().map(|chat| chat.id.as_str()).collect();
        assert_eq!(ids, [newer.as_str(), older.as_str()]);
        assert!(!result.projects.is_empty());
    }

    #[test]
    fn message_search_matches_prefixes_and_requires_every_word() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        index(&db, &chat, "m1", "The render pipeline stalls on resize");
        for query in ["rend", "render resize", "PIPE stall"] {
            let result = execute(db.conn(), query, false).expect("search");
            assert_eq!(result.chats.len(), 1, "{query}");
            assert!(result.chats[0].snippet.is_some(), "{query}");
        }
        assert!(
            execute(db.conn(), "render banana", false)
                .expect("no match")
                .chats
                .is_empty()
        );
    }

    #[test]
    fn a_chat_matching_both_ways_appears_once_with_its_snippet() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        db.conn()
            .execute("UPDATE chats SET title='Render fixes' WHERE id=?1", [&chat])
            .expect("title");
        index(&db, &chat, "m1", "the render pipeline");
        index(&db, &chat, "m2", "render again");
        let result = execute(db.conn(), "render", false).expect("search");
        assert_eq!(result.chats.len(), 1);
        assert!(
            result.chats[0]
                .snippet
                .as_deref()
                .is_some_and(|snippet| snippet.contains("render"))
        );
    }

    #[test]
    fn chats_of_a_deleted_project_are_hidden_everywhere() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        db.conn()
            .execute("UPDATE chats SET title='Render fixes' WHERE id=?1", [&chat])
            .expect("title");
        index(&db, &chat, "m1", "render pipeline");
        db.conn()
            .execute(
                "UPDATE projects SET deleted=1 WHERE id=?1",
                [project_of(&db, &chat)],
            )
            .expect("delete project");
        let result = execute(db.conn(), "render", true).expect("search");
        assert!(result.chats.is_empty());
        assert!(
            execute(db.conn(), "", true)
                .expect("all")
                .projects
                .is_empty()
        );
    }

    #[test]
    fn huge_and_operator_heavy_queries_stay_literal_and_bounded() {
        let db = TestDb::new();
        let chat = db.chat("/work");
        index(&db, &chat, "m1", "render pipeline");
        let long = "r".repeat(10_000);
        assert!(
            execute(db.conn(), &long, false)
                .expect("long")
                .chats
                .is_empty()
        );
        let many = (0..50)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            execute(db.conn(), &many, false)
                .expect("many")
                .chats
                .is_empty()
        );
        // FTS keywords and column filters are matched as words, never as syntax.
        for query in ["NEAR(render pipeline)", "text:render", "render OR banana"] {
            let result = execute(db.conn(), query, false).expect(query);
            assert!(result.chats.is_empty(), "{query}");
        }
    }
}
