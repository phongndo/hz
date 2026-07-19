use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{CopyMode, Error, Materializer, Result, Workspace, WorkspaceState};

pub(crate) struct Registry {
    database: Connection,
}

impl Registry {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut database = Connection::open(path)?;
        database.execute_batch(
            "PRAGMA busy_timeout = 5000;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS schema_migration (
               version INTEGER PRIMARY KEY,
               applied_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS workspace (
               id TEXT PRIMARY KEY,
               root_id TEXT NOT NULL,
               parent_id TEXT REFERENCES workspace(id) ON DELETE RESTRICT,
               handle TEXT NOT NULL,
               path TEXT NOT NULL UNIQUE,
               original_path TEXT,
               removal_id TEXT,
               storage_path TEXT,
               staging_path TEXT,
               state TEXT NOT NULL,
               materializer TEXT NOT NULL,
               strategy TEXT NOT NULL,
               copy_mode TEXT NOT NULL,
               pinned INTEGER NOT NULL DEFAULT 0,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL,
               UNIQUE(root_id, handle)
             );
             CREATE INDEX IF NOT EXISTS workspace_parent_idx ON workspace(parent_id);
             CREATE INDEX IF NOT EXISTS workspace_root_state_idx ON workspace(root_id, state);
             INSERT OR IGNORE INTO schema_migration(version, applied_at)
             VALUES (1, CAST(unixepoch('subsec') * 1000 AS INTEGER));",
        )?;
        {
            let transaction = database.transaction()?;
            let has_removal_id = transaction.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pragma_table_info('workspace') WHERE name = 'removal_id'
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )?;
            if !has_removal_id {
                transaction.execute("ALTER TABLE workspace ADD COLUMN removal_id TEXT", [])?;
            }
            transaction.execute(
                "INSERT OR IGNORE INTO schema_migration(version, applied_at)
                 VALUES (2, CAST(unixepoch('subsec') * 1000 AS INTEGER))",
                [],
            )?;
            transaction.commit()?;
        }
        Ok(Self { database })
    }

    pub(crate) fn insert_root(
        &mut self,
        id: &str,
        handle: &str,
        path: &Path,
        storage_path: &Path,
        strategy: &str,
    ) -> Result<()> {
        let now = timestamp();
        self.database.execute(
            "INSERT INTO workspace
             (id, root_id, parent_id, handle, path, original_path, storage_path, staging_path,
              state, materializer, strategy, copy_mode, pinned, created_at, updated_at)
             VALUES (?1, ?1, NULL, ?2, ?3, NULL, ?4, NULL, 'active',
                     'registered_root', ?5, 'all', 1, ?6, ?6)",
            params![
                id,
                handle,
                path_text(path)?,
                path_text(storage_path)?,
                strategy,
                now
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert_creating(
        &self,
        id: &str,
        root_id: &str,
        parent_id: &str,
        handle: &str,
        path: &Path,
        staging_path: &Path,
        strategy: &str,
        copy_mode: CopyMode,
    ) -> Result<()> {
        let now = timestamp();
        self.database.execute(
            "INSERT INTO workspace
             (id, root_id, parent_id, handle, path, original_path, storage_path, staging_path,
              state, materializer, strategy, copy_mode, pinned, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6, 'creating',
                     'snapshot', ?7, ?8, 0, ?9, ?9)",
            params![
                id,
                root_id,
                parent_id,
                handle,
                path_text(path)?,
                path_text(staging_path)?,
                strategy,
                copy_mode.as_str(),
                now
            ],
        )?;
        Ok(())
    }

    pub(crate) fn activate(&self, id: &str) -> Result<()> {
        self.database.execute(
            "UPDATE workspace SET state = 'active', staging_path = NULL, updated_at = ?2
             WHERE id = ?1 AND state = 'creating'",
            params![id, timestamp()],
        )?;
        Ok(())
    }

    pub(crate) fn delete_record(&self, id: &str) -> Result<()> {
        self.database
            .execute("DELETE FROM workspace WHERE id = ?1", [id])?;
        Ok(())
    }

    pub(crate) fn workspace_id(&self, id: &str) -> Result<Option<Workspace>> {
        let workspace = self
            .database
            .query_row(
                "SELECT id, root_id, parent_id, handle, path, original_path, storage_path,
                        state, materializer, strategy, copy_mode, pinned, created_at, updated_at
                 FROM workspace WHERE id = ?1",
                [id],
                workspace_from_row,
            )
            .optional()?;
        Ok(workspace)
    }

    pub(crate) fn workspace_path(&self, path: &Path) -> Result<Option<Workspace>> {
        let workspace = self
            .database
            .query_row(
                "SELECT id, root_id, parent_id, handle, path, original_path, storage_path,
                        state, materializer, strategy, copy_mode, pinned, created_at, updated_at
                 FROM workspace WHERE path = ?1 AND state IN ('active', 'unregistered')",
                [path_text(path)?],
                workspace_from_row,
            )
            .optional()?;
        Ok(workspace)
    }

    pub(crate) fn workspace_current_path_including_trash(
        &self,
        path: &Path,
    ) -> Result<Option<Workspace>> {
        let workspace = self
            .database
            .query_row(
                "SELECT id, root_id, parent_id, handle, path, original_path, storage_path,
                        state, materializer, strategy, copy_mode, pinned, created_at, updated_at
                 FROM workspace
                 WHERE path = ?1 AND state IN ('active', 'trashed', 'unregistered')",
                [path_text(path)?],
                workspace_from_row,
            )
            .optional()?;
        Ok(workspace)
    }

    pub(crate) fn workspace_ancestor_including_trash(
        &self,
        path: &Path,
    ) -> Result<Option<Workspace>> {
        for ancestor in path.ancestors() {
            if let Some(workspace) = self.workspace_current_path_including_trash(ancestor)? {
                return Ok(Some(workspace));
            }
        }
        Ok(None)
    }

    pub(crate) fn workspace_path_including_trash(&self, path: &Path) -> Result<Option<Workspace>> {
        let workspace = self
            .database
            .query_row(
                "SELECT id, root_id, parent_id, handle, path, original_path, storage_path,
                        state, materializer, strategy, copy_mode, pinned, created_at, updated_at
                 FROM workspace
                 WHERE (path = ?1 OR original_path = ?1)
                   AND state IN ('active', 'trashed', 'unregistered')",
                [path_text(path)?],
                workspace_from_row,
            )
            .optional()?;
        Ok(workspace)
    }

    pub(crate) fn staging_path(&self, id: &str) -> Result<Option<PathBuf>> {
        self.database
            .query_row(
                "SELECT staging_path FROM workspace WHERE id = ?1",
                [id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|path| path.flatten().map(PathBuf::from))
            .map_err(Error::from)
    }

    pub(crate) fn family(&self, root_id: &str, pinned: Option<bool>) -> Result<Vec<Workspace>> {
        let mut sql = String::from(
            "SELECT id, root_id, parent_id, handle, path, original_path, storage_path,
                    state, materializer, strategy, copy_mode, pinned, created_at, updated_at
             FROM workspace WHERE root_id = ?1 AND state = 'active'",
        );
        if pinned.is_some() {
            sql.push_str(" AND pinned = ?2");
        }
        sql.push_str(" ORDER BY created_at, id");
        let mut statement = self.database.prepare(&sql)?;
        let rows = if let Some(pinned) = pinned {
            statement
                .query_map(params![root_id, pinned], workspace_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            statement
                .query_map([root_id], workspace_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    pub(crate) fn children(&self, parent_id: &str, pinned: Option<bool>) -> Result<Vec<Workspace>> {
        let mut sql = String::from(
            "SELECT id, root_id, parent_id, handle, path, original_path, storage_path,
                    state, materializer, strategy, copy_mode, pinned, created_at, updated_at
             FROM workspace WHERE parent_id = ?1 AND state = 'active'",
        );
        if pinned.is_some() {
            sql.push_str(" AND pinned = ?2");
        }
        sql.push_str(" ORDER BY created_at, id");
        let mut statement = self.database.prepare(&sql)?;
        let rows = if let Some(pinned) = pinned {
            statement
                .query_map(params![parent_id, pinned], workspace_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            statement
                .query_map([parent_id], workspace_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    pub(crate) fn roots(&self, pinned: Option<bool>) -> Result<Vec<Workspace>> {
        let mut sql = String::from(
            "SELECT id, root_id, parent_id, handle, path, original_path, storage_path,
                    state, materializer, strategy, copy_mode, pinned, created_at, updated_at
             FROM workspace WHERE parent_id IS NULL AND state = 'active'",
        );
        if pinned.is_some() {
            sql.push_str(" AND pinned = ?1");
        }
        sql.push_str(" ORDER BY created_at, id");
        let mut statement = self.database.prepare(&sql)?;
        let rows = if let Some(pinned) = pinned {
            statement
                .query_map([pinned], workspace_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            statement
                .query_map([], workspace_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    pub(crate) fn ancestors(&self, workspace: &Workspace) -> Result<Vec<Workspace>> {
        let mut result = Vec::new();
        let mut parent_id = workspace.parent_id.clone();
        while let Some(id) = parent_id {
            let parent = self.workspace_id(&id)?.ok_or_else(|| {
                Error::RegistryInvariant(format!("missing parent {id} for {}", workspace.id))
            })?;
            parent_id = parent.parent_id.clone();
            result.push(parent);
        }
        Ok(result)
    }

    pub(crate) fn subtree(&self, id: &str, include_root: bool) -> Result<Vec<Workspace>> {
        let minimum_depth = if include_root { 0 } else { 1 };
        let mut statement = self.database.prepare(
            "WITH RECURSIVE subtree(id, depth) AS (
               SELECT id, 0 FROM workspace WHERE id = ?1 AND state = 'active'
               UNION ALL
               SELECT workspace.id, subtree.depth + 1
               FROM workspace JOIN subtree ON workspace.parent_id = subtree.id
               WHERE workspace.state = 'active'
             )
             SELECT workspace.id, workspace.root_id, workspace.parent_id, workspace.handle,
                    workspace.path, workspace.original_path, workspace.storage_path,
                    workspace.state, workspace.materializer, workspace.strategy,
                    workspace.copy_mode, workspace.pinned, workspace.created_at,
                    workspace.updated_at
             FROM workspace JOIN subtree ON workspace.id = subtree.id
             WHERE subtree.depth >= ?2 ORDER BY subtree.depth DESC, workspace.id",
        )?;
        let rows = statement
            .query_map(params![id, minimum_depth], workspace_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub(crate) fn find_target(
        &self,
        root_id: &str,
        target: &str,
        include_trash: bool,
    ) -> Result<Vec<Workspace>> {
        let states = if include_trash {
            "('active', 'trashed', 'unregistered')"
        } else {
            "('active')"
        };
        let sql = format!(
            "SELECT id, root_id, parent_id, handle, path, original_path, storage_path,
                    state, materializer, strategy, copy_mode, pinned, created_at, updated_at
             FROM workspace WHERE root_id = ?1 AND state IN {states}
             AND (handle = ?2 OR id = ?2 OR id LIKE (?3 || '%') ESCAPE '\\')
             ORDER BY CASE WHEN handle = ?2 OR id = ?2 THEN 0 ELSE 1 END, created_at"
        );
        let escaped_prefix = target
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let mut statement = self.database.prepare(&sql)?;
        let rows = statement
            .query_map(params![root_id, target, escaped_prefix], workspace_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub(crate) fn update_path(&self, id: &str, path: &Path) -> Result<()> {
        self.database.execute(
            "UPDATE workspace SET path = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, path_text(path)?, timestamp()],
        )?;
        Ok(())
    }

    pub(crate) fn set_pinned(&self, ids: &[String], pinned: bool) -> Result<()> {
        for id in ids {
            self.database.execute(
                "UPDATE workspace SET pinned = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, pinned, timestamp()],
            )?;
        }
        Ok(())
    }

    pub(crate) fn mark_trashed(
        &mut self,
        moved: &[(String, PathBuf, PathBuf)],
        removal_id: &str,
    ) -> Result<()> {
        let transaction = self.database.transaction()?;
        for (id, original, trash) in moved {
            transaction.execute(
                "UPDATE workspace SET state = 'trashed', path = ?2, original_path = ?3,
                                      removal_id = ?4, updated_at = ?5 WHERE id = ?1",
                params![
                    id,
                    path_text(trash)?,
                    path_text(original)?,
                    removal_id,
                    timestamp()
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn mark_unregistered(&self, id: &str, removal_id: &str) -> Result<()> {
        self.database.execute(
            "UPDATE workspace SET state = 'unregistered', removal_id = ?2, updated_at = ?3
             WHERE id = ?1",
            params![id, removal_id, timestamp()],
        )?;
        Ok(())
    }

    pub(crate) fn mark_active(&self, id: &str) -> Result<()> {
        self.database.execute(
            "UPDATE workspace SET state = 'active', removal_id = NULL, updated_at = ?2
             WHERE id = ?1 AND state = 'unregistered'",
            params![id, timestamp()],
        )?;
        Ok(())
    }

    pub(crate) fn trashed_subtree(&self, id: &str) -> Result<Vec<Workspace>> {
        let workspace = self
            .workspace_id(id)?
            .ok_or_else(|| Error::UnknownWorkspace(id.into()))?;
        let mut statement = self.database.prepare(
            "WITH RECURSIVE subtree(id, depth, removal_id) AS (
               SELECT id, 0, removal_id FROM workspace WHERE id = ?1
               UNION ALL
               SELECT workspace.id, subtree.depth + 1, subtree.removal_id
               FROM workspace JOIN subtree ON workspace.parent_id = subtree.id
               WHERE workspace.state = 'trashed'
                 AND workspace.removal_id IS subtree.removal_id
             )
             SELECT workspace.id, workspace.root_id, workspace.parent_id, workspace.handle,
                    workspace.path, workspace.original_path, workspace.storage_path,
                    workspace.state, workspace.materializer, workspace.strategy,
                    workspace.copy_mode, workspace.pinned, workspace.created_at,
                    workspace.updated_at
             FROM workspace JOIN subtree ON workspace.id = subtree.id
             WHERE workspace.state IN ('trashed', 'unregistered')
             ORDER BY subtree.depth, workspace.id",
        )?;
        let rows = statement
            .query_map([workspace.id], workspace_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub(crate) fn mark_restored(&mut self, moved: &[(String, PathBuf)]) -> Result<()> {
        let transaction = self.database.transaction()?;
        for (id, path) in moved {
            transaction.execute(
                "UPDATE workspace SET state = 'active', path = ?2, original_path = NULL,
                                      removal_id = NULL, updated_at = ?3 WHERE id = ?1",
                params![id, path_text(path)?, timestamp()],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn trashed(&self) -> Result<Vec<Workspace>> {
        self.by_states("('trashed')", "created_at, id")
    }

    pub(crate) fn restorable(&self) -> Result<Vec<Workspace>> {
        self.by_states("('trashed', 'unregistered')", "created_at, id")
    }

    pub(crate) fn all_records(&self) -> Result<Vec<Workspace>> {
        self.by_states(
            "('creating', 'active', 'trashed', 'unregistered')",
            "created_at, id",
        )
    }

    pub(crate) fn delete_rows(&mut self, ids: &[String]) -> Result<()> {
        let transaction = self.database.transaction()?;
        for id in ids {
            transaction.execute("DELETE FROM workspace WHERE id = ?1", [id])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn unregistered_roots_without_children(&self) -> Result<Vec<Workspace>> {
        let mut statement = self.database.prepare(
            "SELECT root.id, root.root_id, root.parent_id, root.handle, root.path,
                    root.original_path, root.storage_path, root.state, root.materializer,
                    root.strategy, root.copy_mode, root.pinned, root.created_at, root.updated_at
             FROM workspace root
             WHERE root.state = 'unregistered'
               AND NOT EXISTS (SELECT 1 FROM workspace child WHERE child.parent_id = root.id)",
        )?;
        let rows = statement
            .query_map([], workspace_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn by_states(&self, states: &str, order: &str) -> Result<Vec<Workspace>> {
        let sql = format!(
            "SELECT id, root_id, parent_id, handle, path, original_path, storage_path,
                    state, materializer, strategy, copy_mode, pinned, created_at, updated_at
             FROM workspace WHERE state IN {states} ORDER BY {order}"
        );
        let mut statement = self.database.prepare(&sql)?;
        let rows = statement
            .query_map([], workspace_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn workspace_from_row(row: &Row<'_>) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: row.get(0)?,
        root_id: row.get(1)?,
        parent_id: row.get(2)?,
        handle: row.get(3)?,
        path: PathBuf::from(row.get::<_, String>(4)?),
        original_path: row.get::<_, Option<String>>(5)?.map(PathBuf::from),
        storage_path: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
        state: WorkspaceState::from_stored(&row.get::<_, String>(7)?),
        materializer: Materializer::from_stored(&row.get::<_, String>(8)?),
        strategy: row.get(9)?,
        copy_mode: CopyMode::from_stored(&row.get::<_, String>(10)?),
        pinned: row.get(11)?,
        created_at_unix_ms: row.get::<_, i64>(12)? as u64,
        updated_at_unix_ms: row.get::<_, i64>(13)? as u64,
    })
}

fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::Path(format!("path is not valid UTF-8: {}", path.display())))
}

fn timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
