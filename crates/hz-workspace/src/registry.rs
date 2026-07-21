use std::{
    future::Future,
    panic,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use sqlx::{
    QueryBuilder, Row, Sqlite, SqlitePool,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous,
    },
};
use tokio::runtime::{Builder, Handle, Runtime};

use crate::{CopyMode, Error, Materializer, Result, Workspace, WorkspaceState};

const QUERY_BATCH_SIZE: usize = 500;
const WORKSPACE_COLUMNS: &str = "id, root_id, parent_id, handle, path, original_path, storage_path, state, materializer, strategy, copy_mode, pinned, created_at, updated_at";

struct BlockingRuntime(Option<Runtime>);

impl BlockingRuntime {
    fn new() -> std::io::Result<Self> {
        Ok(Self(Some(
            Builder::new_current_thread().enable_all().build()?,
        )))
    }

    fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future + Send,
        F::Output: Send,
    {
        let runtime = self.0.as_ref().expect("runtime is available");
        if Handle::try_current().is_err() {
            return runtime.block_on(future);
        }

        thread::scope(
            |scope| match scope.spawn(move || runtime.block_on(future)).join() {
                Ok(output) => output,
                Err(payload) => panic::resume_unwind(payload),
            },
        )
    }
}

impl Drop for BlockingRuntime {
    fn drop(&mut self) {
        let Some(runtime) = self.0.take() else {
            return;
        };
        if Handle::try_current().is_ok() {
            // Tokio runtimes cannot perform their blocking shutdown from within
            // another runtime, so finish the shutdown on a plain thread.
            let _ = thread::spawn(move || drop(runtime)).join();
        } else {
            drop(runtime);
        }
    }
}

pub(crate) struct Registry {
    database: SqlitePool,
    runtime: BlockingRuntime,
}

impl Registry {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        const SCHEMA_VERSION: i64 = 2;

        let runtime = BlockingRuntime::new()?;
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .busy_timeout(Duration::from_secs(5))
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .pragma("temp_store", "MEMORY");
        let database = runtime.block_on(
            SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options),
        )?;

        let schema_version: i64 =
            runtime.block_on(sqlx::query_scalar("PRAGMA user_version").fetch_one(&database))?;
        if schema_version > SCHEMA_VERSION {
            return Err(Error::RegistryInvariant(format!(
                "database schema version {schema_version} is newer than supported version {SCHEMA_VERSION}"
            )));
        }
        if schema_version < SCHEMA_VERSION {
            runtime.block_on(
                sqlx::raw_sql(
                    "CREATE TABLE IF NOT EXISTS schema_migration (
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
                )
                .execute(&database),
            )?;
            runtime.block_on(async {
                let mut transaction = database.begin().await?;
                let has_removal_id: bool = sqlx::query_scalar(
                    "SELECT EXISTS(
                       SELECT 1 FROM pragma_table_info('workspace') WHERE name = 'removal_id'
                     )",
                )
                .fetch_one(&mut *transaction)
                .await?;
                if !has_removal_id {
                    sqlx::query("ALTER TABLE workspace ADD COLUMN removal_id TEXT")
                        .execute(&mut *transaction)
                        .await?;
                }
                sqlx::query(
                    "INSERT OR IGNORE INTO schema_migration(version, applied_at)
                     VALUES (2, CAST(unixepoch('subsec') * 1000 AS INTEGER))",
                )
                .execute(&mut *transaction)
                .await?;
                sqlx::query("PRAGMA user_version = 2")
                    .execute(&mut *transaction)
                    .await?;
                transaction.commit().await
            })?;
        }
        Ok(Self { database, runtime })
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
        self.runtime.block_on(
            sqlx::query(
                "INSERT INTO workspace
                 (id, root_id, parent_id, handle, path, original_path, storage_path, staging_path,
                  state, materializer, strategy, copy_mode, pinned, created_at, updated_at)
                 VALUES (?1, ?1, NULL, ?2, ?3, NULL, ?4, NULL, 'active',
                         'registered_root', ?5, 'all', 1, ?6, ?6)",
            )
            .bind(id)
            .bind(handle)
            .bind(path_text(path)?)
            .bind(path_text(storage_path)?)
            .bind(strategy)
            .bind(now)
            .execute(&self.database),
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
    ) -> Result<u64> {
        let now = timestamp();
        self.runtime.block_on(
            sqlx::query(
                "INSERT INTO workspace
                 (id, root_id, parent_id, handle, path, original_path, storage_path, staging_path,
                  state, materializer, strategy, copy_mode, pinned, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6, 'creating',
                         'snapshot', ?7, ?8, 0, ?9, ?9)",
            )
            .bind(id)
            .bind(root_id)
            .bind(parent_id)
            .bind(handle)
            .bind(path_text(path)?)
            .bind(path_text(staging_path)?)
            .bind(strategy)
            .bind(copy_mode.as_str())
            .bind(now)
            .execute(&self.database),
        )?;
        Ok(now as u64)
    }

    pub(crate) fn activate(&self, id: &str) -> Result<u64> {
        let now = timestamp();
        self.runtime.block_on(
            sqlx::query(
                "UPDATE workspace SET state = 'active', staging_path = NULL, updated_at = ?2
                 WHERE id = ?1 AND state = 'creating'",
            )
            .bind(id)
            .bind(now)
            .execute(&self.database),
        )?;
        Ok(now as u64)
    }

    pub(crate) fn delete_record(&self, id: &str) -> Result<()> {
        self.runtime.block_on(
            sqlx::query("DELETE FROM workspace WHERE id = ?1")
                .bind(id)
                .execute(&self.database),
        )?;
        Ok(())
    }

    pub(crate) fn workspace_id(&self, id: &str) -> Result<Option<Workspace>> {
        let sql = format!("SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE id = ?1");
        let row = self
            .runtime
            .block_on(sqlx::query(&sql).bind(id).fetch_optional(&self.database))?;
        row.map(|row| workspace_from_row(&row))
            .transpose()
            .map_err(Into::into)
    }

    pub(crate) fn workspace_ids(&self, ids: &[String]) -> Result<Vec<Workspace>> {
        let mut workspaces = Vec::with_capacity(ids.len());
        for ids in ids.chunks(QUERY_BATCH_SIZE) {
            let mut query = QueryBuilder::<Sqlite>::new(format!(
                "SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE id IN ("
            ));
            {
                let mut separated = query.separated(", ");
                for id in ids {
                    separated.push_bind(id);
                }
            }
            query.push(")");
            let rows = self
                .runtime
                .block_on(query.build().fetch_all(&self.database))?;
            workspaces.extend(
                rows.iter()
                    .map(workspace_from_row)
                    .collect::<sqlx::Result<Vec<_>>>()?,
            );
        }
        Ok(workspaces)
    }

    pub(crate) fn workspace_path(&self, path: &Path) -> Result<Option<Workspace>> {
        self.optional_workspace(
            &format!(
                "SELECT {WORKSPACE_COLUMNS} FROM workspace
                 WHERE path = ?1 AND state IN ('active', 'unregistered')"
            ),
            path_text(path)?,
        )
    }

    pub(crate) fn workspaces_at_paths(&self, paths: &[&Path]) -> Result<Vec<Workspace>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let paths = paths
            .iter()
            .map(|path| path_text(path))
            .collect::<Result<Vec<_>>>()?;
        let mut query = QueryBuilder::<Sqlite>::new(format!(
            "SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE path IN ("
        ));
        {
            let mut separated = query.separated(", ");
            for path in paths {
                separated.push_bind(path);
            }
        }
        query.push(") AND state IN ('active', 'unregistered')");
        let rows = self
            .runtime
            .block_on(query.build().fetch_all(&self.database))?;
        rows.iter()
            .map(workspace_from_row)
            .collect::<sqlx::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub(crate) fn workspace_current_path_including_trash(
        &self,
        path: &Path,
    ) -> Result<Option<Workspace>> {
        self.optional_workspace(
            &format!(
                "SELECT {WORKSPACE_COLUMNS} FROM workspace
                 WHERE path = ?1 AND state IN ('active', 'trashed', 'unregistered')"
            ),
            path_text(path)?,
        )
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
        self.optional_workspace(
            &format!(
                "SELECT {WORKSPACE_COLUMNS} FROM workspace
                 WHERE (path = ?1 OR original_path = ?1)
                   AND state IN ('active', 'trashed', 'unregistered')"
            ),
            path_text(path)?,
        )
    }

    pub(crate) fn staging_path(&self, id: &str) -> Result<Option<PathBuf>> {
        let path: Option<Option<String>> = self.runtime.block_on(
            sqlx::query_scalar("SELECT staging_path FROM workspace WHERE id = ?1")
                .bind(id)
                .fetch_optional(&self.database),
        )?;
        Ok(path.flatten().map(PathBuf::from))
    }

    pub(crate) fn family(&self, root_id: &str, pinned: Option<bool>) -> Result<Vec<Workspace>> {
        self.by_parent_filter("root_id", root_id, pinned)
    }

    pub(crate) fn children(&self, parent_id: &str, pinned: Option<bool>) -> Result<Vec<Workspace>> {
        self.by_parent_filter("parent_id", parent_id, pinned)
    }

    pub(crate) fn roots(&self, pinned: Option<bool>) -> Result<Vec<Workspace>> {
        let mut sql = format!(
            "SELECT {WORKSPACE_COLUMNS} FROM workspace
             WHERE parent_id IS NULL AND state = 'active'"
        );
        if pinned.is_some() {
            sql.push_str(" AND pinned = ?1");
        }
        sql.push_str(" ORDER BY created_at, id");
        let rows = if let Some(pinned) = pinned {
            self.runtime
                .block_on(sqlx::query(&sql).bind(pinned).fetch_all(&self.database))?
        } else {
            self.runtime
                .block_on(sqlx::query(&sql).fetch_all(&self.database))?
        };
        workspace_rows(&rows)
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
        let minimum_depth = if include_root { 0_i64 } else { 1_i64 };
        let rows = self.runtime.block_on(
            sqlx::query(
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
            )
            .bind(id)
            .bind(minimum_depth)
            .fetch_all(&self.database),
        )?;
        workspace_rows(&rows)
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
            "SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE root_id = ?1 AND state IN {states}
             AND (handle = ?2 OR id = ?2 OR id LIKE (?3 || '%') ESCAPE '\\')
             ORDER BY CASE WHEN handle = ?2 OR id = ?2 THEN 0 ELSE 1 END, created_at"
        );
        let escaped_prefix = target
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let rows = self.runtime.block_on(
            sqlx::query(&sql)
                .bind(root_id)
                .bind(target)
                .bind(escaped_prefix)
                .fetch_all(&self.database),
        )?;
        workspace_rows(&rows)
    }

    pub(crate) fn update_path(&self, id: &str, path: &Path) -> Result<()> {
        self.runtime.block_on(
            sqlx::query("UPDATE workspace SET path = ?2, updated_at = ?3 WHERE id = ?1")
                .bind(id)
                .bind(path_text(path)?)
                .bind(timestamp())
                .execute(&self.database),
        )?;
        Ok(())
    }

    pub(crate) fn set_pinned(&self, ids: &[String], pinned: bool) -> Result<()> {
        for id in ids {
            self.runtime.block_on(
                sqlx::query("UPDATE workspace SET pinned = ?2, updated_at = ?3 WHERE id = ?1")
                    .bind(id)
                    .bind(pinned)
                    .bind(timestamp())
                    .execute(&self.database),
            )?;
        }
        Ok(())
    }

    pub(crate) fn mark_trashed(
        &mut self,
        moved: &[(String, PathBuf, PathBuf)],
        removal_id: &str,
    ) -> Result<()> {
        let updated_at = timestamp();
        self.runtime.block_on(async {
            let mut transaction = self.database.begin().await?;
            for (id, original, trash) in moved {
                sqlx::query(
                    "UPDATE workspace SET state = 'trashed', path = ?2, original_path = ?3,
                                          removal_id = ?4, updated_at = ?5 WHERE id = ?1",
                )
                .bind(id)
                .bind(path_text(trash)?)
                .bind(path_text(original)?)
                .bind(removal_id)
                .bind(updated_at)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await?;
            Ok::<(), Error>(())
        })
    }

    pub(crate) fn mark_unregistered(&self, id: &str, removal_id: &str) -> Result<()> {
        self.runtime.block_on(
            sqlx::query(
                "UPDATE workspace SET state = 'unregistered', removal_id = ?2, updated_at = ?3
                 WHERE id = ?1",
            )
            .bind(id)
            .bind(removal_id)
            .bind(timestamp())
            .execute(&self.database),
        )?;
        Ok(())
    }

    pub(crate) fn mark_active(&self, id: &str) -> Result<()> {
        self.runtime.block_on(
            sqlx::query(
                "UPDATE workspace SET state = 'active', removal_id = NULL, updated_at = ?2
                 WHERE id = ?1 AND state = 'unregistered'",
            )
            .bind(id)
            .bind(timestamp())
            .execute(&self.database),
        )?;
        Ok(())
    }

    pub(crate) fn trashed_subtree(&self, id: &str) -> Result<Vec<Workspace>> {
        let workspace = self
            .workspace_id(id)?
            .ok_or_else(|| Error::UnknownWorkspace(id.into()))?;
        let rows = self.runtime.block_on(
            sqlx::query(
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
            )
            .bind(workspace.id)
            .fetch_all(&self.database),
        )?;
        workspace_rows(&rows)
    }

    pub(crate) fn mark_restored(&mut self, moved: &[(String, PathBuf)]) -> Result<()> {
        let updated_at = timestamp();
        self.runtime.block_on(async {
            let mut transaction = self.database.begin().await?;
            for (id, path) in moved {
                sqlx::query(
                    "UPDATE workspace SET state = 'active', path = ?2, original_path = NULL,
                                          removal_id = NULL, updated_at = ?3 WHERE id = ?1",
                )
                .bind(id)
                .bind(path_text(path)?)
                .bind(updated_at)
                .execute(&mut *transaction)
                .await?;
            }
            transaction.commit().await?;
            Ok::<(), Error>(())
        })
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
        self.runtime.block_on(async {
            let mut transaction = self.database.begin().await?;
            for id in ids {
                sqlx::query("DELETE FROM workspace WHERE id = ?1")
                    .bind(id)
                    .execute(&mut *transaction)
                    .await?;
            }
            transaction.commit().await?;
            Ok::<(), Error>(())
        })
    }

    pub(crate) fn unregistered_roots_without_children(&self) -> Result<Vec<Workspace>> {
        let rows = self.runtime.block_on(
            sqlx::query(
                "SELECT root.id, root.root_id, root.parent_id, root.handle, root.path,
                        root.original_path, root.storage_path, root.state, root.materializer,
                        root.strategy, root.copy_mode, root.pinned, root.created_at, root.updated_at
                 FROM workspace root
                 WHERE root.state = 'unregistered'
                   AND NOT EXISTS (SELECT 1 FROM workspace child WHERE child.parent_id = root.id)",
            )
            .fetch_all(&self.database),
        )?;
        workspace_rows(&rows)
    }

    fn optional_workspace(&self, sql: &str, value: String) -> Result<Option<Workspace>> {
        let row = self
            .runtime
            .block_on(sqlx::query(sql).bind(value).fetch_optional(&self.database))?;
        row.map(|row| workspace_from_row(&row))
            .transpose()
            .map_err(Into::into)
    }

    fn by_parent_filter(
        &self,
        column: &str,
        id: &str,
        pinned: Option<bool>,
    ) -> Result<Vec<Workspace>> {
        let mut sql = format!(
            "SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE {column} = ?1 AND state = 'active'"
        );
        if pinned.is_some() {
            sql.push_str(" AND pinned = ?2");
        }
        sql.push_str(" ORDER BY created_at, id");
        let rows = if let Some(pinned) = pinned {
            self.runtime.block_on(
                sqlx::query(&sql)
                    .bind(id)
                    .bind(pinned)
                    .fetch_all(&self.database),
            )?
        } else {
            self.runtime
                .block_on(sqlx::query(&sql).bind(id).fetch_all(&self.database))?
        };
        workspace_rows(&rows)
    }

    fn by_states(&self, states: &str, order: &str) -> Result<Vec<Workspace>> {
        let sql = format!(
            "SELECT {WORKSPACE_COLUMNS} FROM workspace WHERE state IN {states} ORDER BY {order}"
        );
        let rows = self
            .runtime
            .block_on(sqlx::query(&sql).fetch_all(&self.database))?;
        workspace_rows(&rows)
    }

    #[cfg(test)]
    pub(crate) fn execute_batch(&self, sql: &str) -> Result<()> {
        self.runtime
            .block_on(sqlx::raw_sql(sql).execute(&self.database))?;
        Ok(())
    }

    #[cfg(test)]
    fn user_version(&self) -> Result<i64> {
        Ok(self
            .runtime
            .block_on(sqlx::query_scalar("PRAGMA user_version").fetch_one(&self.database))?)
    }
}

fn workspace_rows(rows: &[SqliteRow]) -> Result<Vec<Workspace>> {
    rows.iter()
        .map(workspace_from_row)
        .collect::<sqlx::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn workspace_from_row(row: &SqliteRow) -> sqlx::Result<Workspace> {
    Ok(Workspace {
        id: row.try_get(0)?,
        root_id: row.try_get(1)?,
        parent_id: row.try_get(2)?,
        handle: row.try_get(3)?,
        path: PathBuf::from(row.try_get::<String, _>(4)?),
        original_path: row.try_get::<Option<String>, _>(5)?.map(PathBuf::from),
        storage_path: row.try_get::<Option<String>, _>(6)?.map(PathBuf::from),
        state: WorkspaceState::from_stored(&row.try_get::<String, _>(7)?),
        materializer: Materializer::from_stored(&row.try_get::<String, _>(8)?),
        strategy: row.try_get(9)?,
        copy_mode: CopyMode::from_stored(&row.try_get::<String, _>(10)?),
        pinned: row.try_get(11)?,
        created_at_unix_ms: row.try_get::<i64, _>(12)? as u64,
        updated_at_unix_ms: row.try_get::<i64, _>(13)? as u64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn schema_migration_records_the_sqlite_user_version() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("workspaces.sqlite");

        let registry = Registry::open(&path).unwrap();
        assert_eq!(registry.user_version().unwrap(), 2);
        drop(registry);

        Registry::open(path).unwrap();
    }

    #[test]
    fn synchronous_registry_api_works_inside_a_tokio_runtime() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("workspaces.sqlite");
        let host_runtime = Builder::new_current_thread().enable_all().build().unwrap();

        host_runtime.block_on(async {
            let registry = Registry::open(path).unwrap();
            assert!(registry.all_records().unwrap().is_empty());
        });
    }

    #[test]
    fn newer_database_schemas_are_rejected() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("workspaces.sqlite");
        let registry = Registry::open(&path).unwrap();
        registry.execute_batch("PRAGMA user_version = 3").unwrap();
        drop(registry);

        assert!(matches!(
            Registry::open(path),
            Err(Error::RegistryInvariant(message)) if message.contains("newer than supported")
        ));
    }
}
