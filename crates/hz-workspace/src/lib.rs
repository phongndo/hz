mod filter;
mod lock;
mod marker;
mod registry;
mod strategy;
mod types;

#[cfg(all(test, target_os = "linux"))]
mod test_support;
#[cfg(test)]
mod tests;

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use hz_core::{HzError, path_utils::normalize_lexically};
use lock::MutationLock;
use registry::Registry;
use strategy::{PortableCopyStrategy, Strategy, StrategyInit};
use thiserror::Error;
use ulid::Ulid;

pub use marker::FILE_NAME as MARKER_FILE;
pub use types::*;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Database(#[from] sqlx::Error),
    #[error("{0}")]
    Walk(#[from] walkdir::Error),
    #[error("{0}")]
    Core(#[from] HzError),
    #[error("invalid path: {0}")]
    Path(String),
    #[error("copy-on-write cloning unavailable: {0}")]
    CowUnavailable(String),
    #[error("workspace requires initialization: {0}")]
    InitializationRequired(PathBuf),
    #[error("unsupported filesystem entry: {0}")]
    UnsupportedEntry(PathBuf),
    #[error("workspace contains a mounted descendant: {0}")]
    MountedDescendant(PathBuf),
    #[error("workspace is not initialized: {0}")]
    WorkspaceNotInitialized(PathBuf),
    #[error("workspace marker is missing: {0}")]
    MissingMarker(PathBuf),
    #[error("workspace marker does not match the registry: {0}")]
    MarkerMismatch(PathBuf),
    #[error("workspace marker belongs to an unknown registry entry: {0}")]
    UnknownMarker(PathBuf),
    #[error("invalid workspace marker: {0}")]
    InvalidMarker(PathBuf),
    #[error("unknown workspace: {0}")]
    UnknownWorkspace(String),
    #[error("workspace path already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("cannot create a workspace inside its source: {0}")]
    InsideSource(PathBuf),
    #[error("cannot store a workspace inside another managed workspace: {0}")]
    InsideManagedWorkspace(PathBuf),
    #[error("cannot initialize a workspace containing another managed workspace: {0}")]
    ContainsManagedWorkspace(PathBuf),
    #[error("workspace path is missing: {0}")]
    MissingWorkspace(PathBuf),
    #[error("workspace restore was interrupted at {0}; run `hz doctor --fix`")]
    InterruptedRestore(PathBuf),
    #[error("root workspace removal requires --force: {0}")]
    RootForceRequired(PathBuf),
    #[error("workspace registry invariant failed: {0}")]
    RegistryInvariant(String),
}

impl From<Error> for HzError {
    fn from(error: Error) -> Self {
        match error {
            Error::Io(error) => Self::Io(error),
            Error::Core(error) => error,
            Error::WorkspaceNotInitialized(path) => Self::WorkspaceNotInitialized(path),
            Error::MissingMarker(path) => Self::MissingMarker(path),
            Error::MarkerMismatch(path)
            | Error::UnknownMarker(path)
            | Error::InvalidMarker(path) => Self::MarkerMismatch(path),
            Error::CowUnavailable(message) => Self::CowUnavailable(message),
            Error::UnknownWorkspace(target) => Self::UnknownWorkspace { target },
            error => Self::Usage(error.to_string()),
        }
    }
}

pub struct Manager {
    registry: Registry,
    cow_strategy: Box<dyn Strategy>,
    copy_strategy: Box<dyn Strategy>,
    lock_path: PathBuf,
}

impl Manager {
    pub fn open_default() -> Result<Self> {
        let database = default_database_path()?;
        if let Some(parent) = database.parent() {
            fs::create_dir_all(parent)?;
        }
        Self::open(database)
    }

    pub fn open(database: impl AsRef<Path>) -> Result<Self> {
        Self::with_strategy(database, strategy::default_strategy())
    }

    fn with_strategy(database: impl AsRef<Path>, strategy: Box<dyn Strategy>) -> Result<Self> {
        let database = database.as_ref().to_path_buf();
        let lock_path = database_lock_path(&database)?;
        Ok(Self {
            registry: Registry::open(&database)?,
            cow_strategy: strategy,
            copy_strategy: Box::new(PortableCopyStrategy),
            lock_path,
        })
    }

    pub fn select_init_target(&self, requested: &Path, here: bool) -> Result<PathBuf> {
        let requested = existing_directory(requested)?;
        if here {
            match self.workspace_from_optional(&requested) {
                Ok(Some(existing)) if existing.path != requested => {
                    return Err(Error::Path(format!(
                        "cannot initialize a nested workspace inside {}",
                        existing.path.display()
                    )));
                }
                Err(Error::MissingMarker(path)) | Err(Error::UnknownMarker(path))
                    if path == requested =>
                {
                    self.ensure_no_managed_ancestor(&requested)?;
                }
                Err(error) => return Err(error),
                _ => {}
            }
            return Ok(requested);
        }

        match self.workspace_from_optional(&requested) {
            Ok(Some(workspace)) => return Ok(workspace.path),
            Err(Error::UnknownMarker(path)) => {
                self.ensure_no_managed_ancestor(&path)?;
                return Ok(path);
            }
            Err(error) => return Err(error),
            Ok(None) => {}
        }

        Ok(requested)
    }

    pub fn init(&mut self, input: InitWorkspace) -> Result<InitializedWorkspace> {
        let (initialized, ()) = self.init_with_setup(input, |_| Ok(()))?;
        Ok(initialized)
    }

    pub fn init_with_setup<T>(
        &mut self,
        input: InitWorkspace,
        setup: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<(InitializedWorkspace, T)> {
        self.init_with_progress_and_setup(input, |_| {}, setup)
    }

    pub fn init_with_progress(
        &mut self,
        input: InitWorkspace,
        progress: impl FnMut(InitProgress),
    ) -> Result<InitializedWorkspace> {
        let (initialized, ()) = self.init_with_progress_and_setup(input, progress, |_| Ok(()))?;
        Ok(initialized)
    }

    fn init_with_progress_and_setup<T>(
        &mut self,
        input: InitWorkspace,
        mut progress: impl FnMut(InitProgress),
        setup: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<(InitializedWorkspace, T)> {
        let _lock = self.mutation_lock()?;
        let at = self.select_init_target(&input.at, input.here)?;

        if let Some(descendant) = self.registry.all_records()?.into_iter().find(|workspace| {
            workspace.state == WorkspaceState::Active
                && workspace.path != at
                && workspace.path.starts_with(&at)
        }) {
            return Err(Error::ContainsManagedWorkspace(descendant.path));
        }

        if let Some(record) = self.registry.workspace_path(&at)? {
            if record.state == WorkspaceState::Unregistered {
                return Err(Error::Path(format!(
                    "workspace root is in trash; run `hz restore root` from {}",
                    record.path.display()
                )));
            }
            let marker_restored = marker::read(&at)?.is_none();
            marker::protect_from_source_control(&at)?;
            if marker_restored {
                progress(InitProgress::RestoringMarker);
                marker::write(&at, &record.id)?;
            } else {
                marker::verify(&at, &record.id)?;
            }
            if record.strategy != self.copy_strategy.name() {
                let root = self
                    .registry
                    .workspace_id(&record.root_id)?
                    .ok_or_else(|| {
                        Error::RegistryInvariant(format!("missing root {}", record.root_id))
                    })?;
                let storage = root.storage_path.as_ref().ok_or_else(|| {
                    Error::RegistryInvariant(format!("root {} has no storage path", root.id))
                })?;
                ensure_cow_storage_compatible(&at, storage)?;
            }
            let initialized = self
                .strategy_for(&record.strategy)
                .initialize_directory(&at, &mut progress)?;
            let setup = setup(&at)?;
            let workspace = self
                .registry
                .workspace_id(&record.id)?
                .ok_or_else(|| Error::UnknownWorkspace(record.id.clone()))?;
            return Ok((
                InitializedWorkspace {
                    workspace,
                    outcome: if marker_restored {
                        InitOutcome::MarkerRestored
                    } else if initialized == StrategyInit::Converted {
                        InitOutcome::Converted
                    } else {
                        InitOutcome::AlreadyInitialized
                    },
                },
                setup,
            ));
        }

        match self.workspace_from_optional(&at) {
            Ok(Some(existing)) => {
                return Err(Error::Path(format!(
                    "cannot initialize a nested workspace inside {}",
                    existing.path.display()
                )));
            }
            Err(Error::UnknownMarker(path)) if path == at => {
                self.ensure_no_managed_ancestor(&at)?;
            }
            Err(error) => return Err(error),
            Ok(None) => {}
        }

        let handle = root_handle(&at)?;
        let strategy_name = match input.strategy {
            InitStrategy::CopyOnWrite => self.cow_strategy.name(),
            InitStrategy::Copy => self.copy_strategy.name(),
        };
        let initialized = self
            .strategy_for(strategy_name)
            .initialize_directory(&at, &mut progress)?;

        let marker_id = marker::read(&at)?;
        let had_marker = marker_id.is_some();
        let id = match marker_id {
            Some(id) => {
                if let Some(existing) = self.registry.workspace_id(&id)? {
                    return Err(Error::MarkerMismatch(existing.path));
                }
                id
            }
            None => Ulid::new().to_string(),
        };
        let storage = default_storage(&at, &id)?;
        if strategy_name != self.copy_strategy.name() {
            ensure_cow_storage_compatible(&at, &storage)?;
        }

        // Complete caller-owned initialization while the mutation lock is held
        // and before this root becomes visible to workspace creation.
        let setup = setup(&at)?;
        progress(InitProgress::RegisteringWorkspace);
        let result: Result<()> = (|| {
            marker::protect_from_source_control(&at)?;
            marker::write(&at, &id)?;
            self.registry
                .insert_root(&id, &handle, &at, &storage, strategy_name)?;
            Ok(())
        })();
        if result.is_err() && !had_marker {
            let _ = marker::remove(&at);
        }
        result?;

        let workspace = self
            .registry
            .workspace_id(&id)?
            .ok_or_else(|| Error::UnknownWorkspace(id.clone()))?;
        Ok((
            InitializedWorkspace {
                workspace,
                outcome: if initialized == StrategyInit::Converted {
                    InitOutcome::Converted
                } else {
                    InitOutcome::Registered
                },
            },
            setup,
        ))
    }

    pub fn create(&mut self, input: CreateWorkspace) -> Result<CreatedWorkspace> {
        let (created, ()) = self.create_with_setup(input, |_| Ok(()))?;
        Ok(created)
    }

    pub fn create_with_setup<T>(
        &mut self,
        input: CreateWorkspace,
        setup: impl FnOnce(&Workspace) -> Result<T>,
    ) -> Result<(CreatedWorkspace, T)> {
        let _lock = self.mutation_lock()?;
        let requested = existing_directory(&input.from)?;
        let source = self.workspace_from(&requested)?;
        let setup = setup(&source)?;
        let root = if source.id == source.root_id {
            source.clone()
        } else {
            self.registry
                .workspace_id(&source.root_id)?
                .ok_or_else(|| Error::UnknownWorkspace(source.root_id.clone()))?
        };
        strategy::ensure_no_mounted_descendants(&source.path)?;

        let handle = match input.handle {
            Some(handle) => {
                let handle = validate_handle(handle)?;
                if !self
                    .registry
                    .find_target(&root.id, &handle, true)?
                    .is_empty()
                {
                    return Err(Error::Path(format!(
                        "workspace handle already exists in this family: {handle}"
                    )));
                }
                handle
            }
            // generate_handle has already checked family-wide uniqueness.
            None => self.generate_handle(&root.id)?,
        };

        let destination_parent = match input.into {
            Some(path) => absolute_path(&path)?,
            None => root.storage_path.clone().ok_or_else(|| {
                Error::RegistryInvariant(format!("root {} has no storage path", root.id))
            })?,
        };
        if !destination_parent.try_exists()? {
            let existing_parent = nearest_existing_ancestor(&destination_parent)?;
            if self.workspace_from_optional(&existing_parent)?.is_some() {
                return Err(Error::InsideManagedWorkspace(destination_parent));
            }
        }
        fs::create_dir_all(&destination_parent)?;
        let destination_parent = fs::canonicalize(destination_parent)?;
        if self.workspace_from_optional(&destination_parent)?.is_some() {
            return Err(Error::InsideManagedWorkspace(destination_parent));
        }
        let id = Ulid::new().to_string();
        let destination = destination_parent.join(&id);
        let staging = destination_parent.join(format!(".hz-create-{id}"));
        if destination.starts_with(&source.path) || staging.starts_with(&source.path) {
            return Err(Error::InsideSource(destination));
        }
        if destination.try_exists()? || staging.try_exists()? {
            return Err(Error::AlreadyExists(destination));
        }

        let created_at = self.registry.insert_creating(
            &id,
            &root.id,
            &source.id,
            &handle,
            &destination,
            &staging,
            &root.strategy,
            input.copy_mode,
        )?;

        let result = (|| {
            self.strategy_for(&root.strategy).copy_directory(
                &source.path,
                &staging,
                input.copy_mode,
                &id,
            )?;
            marker::protect_from_source_control(&staging)?;
            // Successful strategies guarantee this marker. Recheck the
            // internal contract in development without adding a release-path
            // read before every activation.
            #[cfg(debug_assertions)]
            marker::verify(&staging, &id)?;
            fs::rename(&staging, &destination)?;
            self.registry.activate(&id)
        })();

        let activated_at = match result {
            Ok(activated_at) => activated_at,
            Err(error) => {
                // Native unfiltered clones initially inherit the source marker. If
                // permission restriction or marker replacement fails, that marker
                // still proves ownership as long as the registered source remains
                // independently present and valid.
                let inherited_marker_id =
                    (input.copy_mode == CopyMode::All).then_some(source.id.as_str());
                let staging_removed = self.cleanup_failed_create_path(
                    &root.strategy,
                    &staging,
                    &id,
                    inherited_marker_id,
                );
                let destination_removed =
                    self.cleanup_failed_create_path(&root.strategy, &destination, &id, None);
                if staging_removed && destination_removed {
                    let _ = self.registry.delete_record(&id);
                }
                return Err(error);
            }
        };

        let workspace = Workspace {
            id,
            root_id: root.id,
            parent_id: Some(source.id.clone()),
            handle,
            path: destination,
            original_path: None,
            storage_path: None,
            state: WorkspaceState::Active,
            materializer: Materializer::Snapshot,
            strategy: root.strategy,
            copy_mode: input.copy_mode,
            pinned: false,
            created_at_unix_ms: created_at,
            updated_at_unix_ms: activated_at,
        };
        Ok((
            CreatedWorkspace {
                workspace,
                source: source.path,
            },
            setup,
        ))
    }

    pub fn current(&self, at: impl AsRef<Path>) -> Result<Workspace> {
        let at = existing_directory(at.as_ref())?;
        self.workspace_from(&at)
    }

    pub fn adopt(&self, at: impl AsRef<Path>) -> Result<Workspace> {
        let _lock = self.mutation_lock()?;
        let at = existing_directory(at.as_ref())?;
        let id = marker::read(&at)?.ok_or_else(|| Error::MissingMarker(at.clone()))?;
        let record = self
            .registry
            .workspace_id(&id)?
            .ok_or_else(|| Error::UnknownMarker(at.clone()))?;
        if record.state != WorkspaceState::Active {
            return Err(Error::Path(format!(
                "only active workspaces can be adopted: {}",
                record.handle
            )));
        }
        if record.path.try_exists()? && record.path != at {
            return Err(Error::MarkerMismatch(at));
        }
        if let Some(other) = self.registry.workspace_path(&at)? {
            if other.id != id {
                return Err(Error::MarkerMismatch(at));
            }
        }
        if let Some(parent) = at.parent() {
            if let Some(ancestor) = self.workspace_from_optional(parent)? {
                if ancestor.id != id {
                    return Err(Error::InsideManagedWorkspace(at));
                }
            }
        }
        if let Some(descendant) = self.registry.all_records()?.into_iter().find(|workspace| {
            workspace.state == WorkspaceState::Active
                && workspace.id != id
                && workspace.path != at
                && workspace.path.starts_with(&at)
        }) {
            return Err(Error::ContainsManagedWorkspace(descendant.path));
        }
        if record.strategy != self.copy_strategy.name() {
            let root = self
                .registry
                .workspace_id(&record.root_id)?
                .ok_or_else(|| {
                    Error::RegistryInvariant(format!("missing root {}", record.root_id))
                })?;
            let storage = root.storage_path.as_ref().ok_or_else(|| {
                Error::RegistryInvariant(format!("root {} has no storage path", root.id))
            })?;
            ensure_cow_storage_compatible(&at, storage)?;
        }
        marker::protect_from_source_control(&at)?;
        self.registry.update_path(&id, &at)?;
        self.registry
            .workspace_id(&id)?
            .ok_or(Error::UnknownWorkspace(id))
    }

    pub fn resolve_target(
        &self,
        at: impl AsRef<Path>,
        target: Option<&str>,
        include_trash: bool,
    ) -> Result<Workspace> {
        let at = existing_directory(at.as_ref())?;
        let context = match self.workspace_from_optional(&at) {
            Ok(Some(workspace)) => workspace,
            Err(Error::MissingMarker(path)) | Err(Error::MarkerMismatch(path)) if include_trash => {
                let workspace = self
                    .registry
                    .workspace_current_path_including_trash(&path)?
                    .ok_or_else(|| Error::WorkspaceNotInitialized(at.clone()))?;
                self.verify_context_including_trash(workspace)?
            }
            Ok(None) if include_trash => {
                let workspace = self
                    .registry
                    .workspace_ancestor_including_trash(&at)?
                    .ok_or_else(|| Error::WorkspaceNotInitialized(at.clone()))?;
                self.verify_context_including_trash(workspace)?
            }
            Err(error) => return Err(error),
            Ok(None) => return Err(Error::WorkspaceNotInitialized(at)),
        };
        let Some(target) = target else {
            return Ok(context);
        };
        if matches!(target, "current" | ".") {
            return Ok(context);
        }
        if matches!(target, "root" | "local") {
            let workspace = self
                .registry
                .workspace_id(&context.root_id)?
                .ok_or_else(|| Error::UnknownWorkspace(target.to_owned()))?;
            return self.verify_resolved_workspace(workspace);
        }

        let candidates = self
            .registry
            .find_target(&context.root_id, target, include_trash)?;
        let exact = candidates
            .iter()
            .filter(|workspace| workspace.id == target || workspace.handle == target)
            .collect::<Vec<_>>();
        match exact.as_slice() {
            [workspace] => {
                return self.verify_resolved_workspace((*workspace).clone());
            }
            [] => {}
            _ => {
                return Err(Error::Path(format!(
                    "ambiguous workspace target {target}; matches {} entries",
                    exact.len()
                )));
            }
        }

        match candidates.as_slice() {
            [workspace] => return self.verify_resolved_workspace(workspace.clone()),
            [] => {}
            candidates => {
                return Err(Error::Path(format!(
                    "ambiguous workspace target {target}; matches {} entries",
                    candidates.len()
                )));
            }
        }

        let path = Path::new(target);
        if path.exists() {
            let path = fs::canonicalize(path)?;
            if include_trash {
                if let Some(workspace) = self.registry.workspace_path_including_trash(&path)? {
                    return self.verify_resolved_workspace(workspace);
                }
            }
            if path.is_dir() {
                return self.workspace_from(&path);
            }
        } else if include_trash {
            let path = normalize_lexically(&absolute_path(path)?);
            if let Some(workspace) = self.registry.workspace_path_including_trash(&path)? {
                return self.verify_resolved_workspace(workspace);
            }
        }

        Err(Error::UnknownWorkspace(target.to_owned()))
    }

    pub fn list(&self, input: ListWorkspaces) -> Result<Vec<Workspace>> {
        match input.scope {
            ListScope::Roots => self.registry.roots(input.pinned),
            ListScope::Family | ListScope::Children => {
                let of = input.of.unwrap_or(std::env::current_dir()?);
                let workspace = self.current(of)?;
                match input.scope {
                    ListScope::Family => self.registry.family(&workspace.root_id, input.pinned),
                    ListScope::Children => self.registry.children(&workspace.id, input.pinned),
                    ListScope::Roots => unreachable!(),
                }
            }
        }
    }

    pub fn ancestors(&self, at: impl AsRef<Path>, target: Option<&str>) -> Result<Vec<Workspace>> {
        let workspace = self.resolve_target(at, target, false)?;
        self.registry
            .ancestors(&workspace)?
            .into_iter()
            .map(|ancestor| self.verify_resolved_workspace(ancestor))
            .collect()
    }

    pub fn parent(&self, workspace: &Workspace) -> Result<Option<Workspace>> {
        let Some(parent_id) = workspace.parent_id.as_deref() else {
            return Ok(None);
        };
        let parent = self.registry.workspace_id(parent_id)?.ok_or_else(|| {
            Error::RegistryInvariant(format!("missing parent {parent_id} for {}", workspace.id))
        })?;
        if parent.state != WorkspaceState::Active {
            return Err(Error::RegistryInvariant(format!(
                "parent {parent_id} for {} is not active",
                workspace.id
            )));
        }
        self.verify_resolved_workspace(parent).map(Some)
    }

    pub fn set_pinned(
        &self,
        at: impl AsRef<Path>,
        targets: &[String],
        pinned: bool,
    ) -> Result<Vec<Workspace>> {
        let _lock = self.mutation_lock()?;
        let mut workspaces = Vec::with_capacity(targets.len());
        for target in targets {
            let workspace = self.resolve_target(at.as_ref(), Some(target), false)?;
            workspaces.push(workspace);
        }
        self.registry.set_pinned(
            &workspaces
                .iter()
                .map(|workspace| workspace.id.clone())
                .collect::<Vec<_>>(),
            pinned,
        )?;
        workspaces
            .into_iter()
            .map(|workspace| {
                self.registry
                    .workspace_id(&workspace.id)?
                    .ok_or_else(|| Error::UnknownWorkspace(workspace.id))
            })
            .collect()
    }

    pub fn remove(
        &mut self,
        at: impl AsRef<Path>,
        target: Option<&str>,
        mode: RemoveMode,
        force_root: bool,
    ) -> Result<RemovedWorkspaces> {
        self.remove_with_marker_removal(at, target, mode, force_root, marker::remove)
    }

    pub fn remove_resolved(
        &mut self,
        selected: &Workspace,
        mode: RemoveMode,
        force_root: bool,
    ) -> Result<RemovedWorkspaces> {
        let _lock = self.mutation_lock()?;
        let selected = self
            .registry
            .workspace_id(&selected.id)?
            .filter(|workspace| workspace.state == WorkspaceState::Active)
            .ok_or_else(|| Error::UnknownWorkspace(selected.id.clone()))?;
        let selected = self.verify_resolved_workspace(selected)?;
        self.remove_selected_with_marker_removal(selected, mode, force_root, marker::remove)
    }

    fn remove_with_marker_removal(
        &mut self,
        at: impl AsRef<Path>,
        target: Option<&str>,
        mode: RemoveMode,
        force_root: bool,
        remove_marker: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<RemovedWorkspaces> {
        let _lock = self.mutation_lock()?;
        let selected = self.resolve_target(at, target, false)?;
        self.remove_selected_with_marker_removal(selected, mode, force_root, remove_marker)
    }

    fn remove_selected_with_marker_removal(
        &mut self,
        selected: Workspace,
        mode: RemoveMode,
        force_root: bool,
        remove_marker: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<RemovedWorkspaces> {
        let is_root = selected.parent_id.is_none();
        if is_root && mode == RemoveMode::Subtree && !force_root {
            return Err(Error::RootForceRequired(selected.path));
        }

        let include_root = mode == RemoveMode::Subtree && !is_root;
        let rows = self.registry.subtree(&selected.id, include_root)?;
        let mut verified_ancestors = HashSet::new();
        for row in &rows {
            if row.id == selected.id {
                continue;
            }
            match marker::verify_with_ancestor_cache(&row.path, &row.id, &mut verified_ancestors) {
                Ok(()) => {}
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(Error::MissingWorkspace(row.path.clone()));
                }
                Err(error) => return Err(error),
            }
        }

        let mut targets = Vec::with_capacity(rows.len());
        for row in &rows {
            let trash = trash_path(&row.path, &row.id)?;
            if trash.exists() {
                return Err(Error::AlreadyExists(trash));
            }
            targets.push((row.id.clone(), row.path.clone(), trash));
        }

        #[cfg(windows)]
        self.ensure_current_directory_outside_removal(&rows)?;

        // Preflight every distinct trash parent before moving anything. In
        // particular, never let rename follow a pre-existing .trash symlink or
        // junction.
        let mut trash_parents = HashSet::new();
        for (_, _, trash) in &targets {
            let parent = trash.parent().ok_or_else(|| {
                Error::Path(format!("trash path has no parent: {}", trash.display()))
            })?;
            if trash_parents.insert(parent) {
                ensure_real_trash_directory(parent)?;
            }
        }

        let mut moved = Vec::new();
        for (id, original, trash) in &targets {
            if let Err(error) = fs::rename(original, trash) {
                for (_, previous_original, previous_trash) in moved.iter().rev() {
                    let _ = fs::rename(previous_trash, previous_original);
                }
                return Err(error.into());
            }
            moved.push((id.clone(), original.clone(), trash.clone()));
        }
        let removal_id = Ulid::new().to_string();
        if let Err(error) = self.registry.mark_trashed(&moved, &removal_id) {
            for (_, original, trash) in moved.iter().rev() {
                let _ = fs::rename(trash, original);
            }
            return Err(error);
        }

        let root_unregistered = is_root && mode == RemoveMode::Subtree;
        if root_unregistered {
            if let Err(error) = self.unregister_root_with(&selected, &removal_id, remove_marker) {
                self.rollback_trashed_workspaces(&moved)?;
                return Err(error);
            }
        }

        let removed = if let [row] = rows.as_slice() {
            vec![
                self.registry
                    .workspace_id(&row.id)?
                    .ok_or_else(|| Error::UnknownWorkspace(row.id.clone()))?,
            ]
        } else {
            let removed_ids = rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
            let mut removed_by_id = self
                .registry
                .workspace_ids(&removed_ids)?
                .into_iter()
                .map(|workspace| (workspace.id.clone(), workspace))
                .collect::<HashMap<_, _>>();
            rows.into_iter()
                .map(|row| {
                    removed_by_id
                        .remove(&row.id)
                        .ok_or(Error::UnknownWorkspace(row.id))
                })
                .collect::<Result<Vec<_>>>()?
        };
        let selected = match removed.iter().find(|workspace| workspace.id == selected.id) {
            Some(selected) => selected.clone(),
            None => self
                .registry
                .workspace_id(&selected.id)?
                .ok_or_else(|| Error::UnknownWorkspace(selected.id.clone()))?,
        };

        Ok(RemovedWorkspaces {
            selected,
            removed,
            root_unregistered,
        })
    }

    pub fn restore(&mut self, at: impl AsRef<Path>, target: &str) -> Result<Vec<Workspace>> {
        let _lock = self.mutation_lock()?;
        let selected = self.resolve_target(at, Some(target), true)?;
        if !matches!(
            selected.state,
            WorkspaceState::Trashed | WorkspaceState::Unregistered
        ) {
            return Err(Error::Path(format!(
                "workspace {} is not in trash",
                selected.handle
            )));
        }
        if let Some(parent_id) = &selected.parent_id {
            let parent = self
                .registry
                .workspace_id(parent_id)?
                .ok_or_else(|| Error::RegistryInvariant(format!("missing parent {parent_id}")))?;
            if parent.state != WorkspaceState::Active {
                return Err(Error::Path(format!(
                    "restore parent workspace {} first",
                    parent.handle
                )));
            }
        }

        let rows = self.registry.trashed_subtree(&selected.id)?;
        for row in &rows {
            match row.state {
                WorkspaceState::Unregistered => {
                    self.ensure_restore_destination_unmanaged(&row.path)?;
                    restore_marker_missing(&row.path, &row.id)?;
                }
                WorkspaceState::Trashed => {
                    let original = row.original_path.as_ref().ok_or_else(|| {
                        Error::RegistryInvariant(format!(
                            "trashed workspace {} has no original path",
                            row.id
                        ))
                    })?;
                    if original.try_exists()? {
                        return Err(Error::AlreadyExists(original.clone()));
                    }
                    marker::verify(&row.path, &row.id)?;
                    self.checked_restore_destination(original)?;
                }
                _ => {}
            }
        }
        let mut moved: Vec<(String, PathBuf, PathBuf)> = Vec::new();
        let mut markers_written = Vec::new();
        let mut restored = Vec::new();
        let operation = (|| -> Result<()> {
            for row in &rows {
                match row.state {
                    WorkspaceState::Unregistered => {
                        // Recheck after preflight so a reused root can never
                        // have a foreign marker silently replaced.
                        restore_marker_missing(&row.path, &row.id)?;
                        marker::protect_from_source_control(&row.path)?;
                        if restore_marker_missing(&row.path, &row.id)? {
                            marker::write(&row.path, &row.id)?;
                            markers_written.push(row.path.clone());
                        }
                        restored.push((row.id.clone(), row.path.clone()));
                    }
                    WorkspaceState::Trashed => {
                        let original = row.original_path.clone().ok_or_else(|| {
                            Error::RegistryInvariant(format!(
                                "trashed workspace {} has no original path",
                                row.id
                            ))
                        })?;
                        if original.try_exists()? {
                            return Err(Error::AlreadyExists(original));
                        }
                        marker::verify(&row.path, &row.id)?;
                        if let Some(parent) = original.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        let destination = self.checked_restore_destination(&original)?;
                        fs::rename(&row.path, &destination)?;
                        moved.push((row.id.clone(), destination.clone(), row.path.clone()));
                        restored.push((row.id.clone(), destination));
                    }
                    _ => {}
                }
            }
            self.registry.mark_restored(&restored)
        })();

        if let Err(error) = operation {
            for (_, original, trash) in moved.iter().rev() {
                let _ = fs::rename(original, trash);
            }
            for path in markers_written.iter().rev() {
                let _ = marker::remove(path);
            }
            return Err(error);
        }
        restored
            .into_iter()
            .map(|(id, _)| {
                self.registry
                    .workspace_id(&id)?
                    .ok_or(Error::UnknownWorkspace(id))
            })
            .collect()
    }

    pub fn gc(&mut self) -> Result<GarbageCollection> {
        let _lock = self.mutation_lock()?;
        let unregistered_roots = self
            .registry
            .all_records()?
            .into_iter()
            .filter(|row| row.state == WorkspaceState::Unregistered)
            .collect::<Vec<_>>();
        for root in &unregistered_roots {
            self.ensure_unregistered_root_not_restored(root)?;
        }

        let mut rows = self.registry.trashed()?;
        let by_id = rows
            .iter()
            .map(|row| (row.id.clone(), row.parent_id.clone()))
            .collect::<HashMap<_, _>>();
        rows.sort_by_key(|row| std::cmp::Reverse(depth_in(&row.id, &by_id)));

        for row in &rows {
            self.verify_trashed_workspace_for_gc(row)?;
        }

        let mut deleted = Vec::new();
        let mut ids = Vec::new();
        for row in rows {
            if self.verify_trashed_workspace_for_gc(&row)? {
                self.remove_trashed_workspace(&row)?;
            }
            deleted.push(row.path);
            ids.push(row.id);
        }
        self.registry.delete_rows(&ids)?;

        let roots = self.registry.unregistered_roots_without_children()?;
        for root in &roots {
            self.ensure_unregistered_root_not_restored(root)?;
        }
        self.registry
            .delete_rows(&roots.iter().map(|root| root.id.clone()).collect::<Vec<_>>())?;
        Ok(GarbageCollection { deleted })
    }

    pub fn doctor(&mut self, fix: bool) -> Result<DoctorReport> {
        let _lock = self.mutation_lock()?;
        let initial_records = self.registry.all_records()?;
        let mut issues = Vec::new();
        let mut blocked_root_restores = HashSet::new();
        for root in initial_records
            .iter()
            .filter(|record| record.state == WorkspaceState::Unregistered)
        {
            if workspace_marker_state(&root.path, &root.id)? != MarkerState::Matches {
                continue;
            }
            match self.reconcile_interrupted_root_restore(root, fix)? {
                InterruptedRootRestore::NotDetected => {}
                InterruptedRootRestore::Detected => {
                    blocked_root_restores.insert(root.id.clone());
                }
                InterruptedRootRestore::Fixed => issues.push(DoctorIssue {
                    workspace_id: root.id.clone(),
                    path: root.path.clone(),
                    message: "root-family workspace restore was interrupted".to_owned(),
                    fixed: true,
                }),
            }
        }

        let records = self.registry.all_records()?;
        for record in records {
            let can_fix = fix && !blocked_root_restores.contains(&record.root_id);
            match record.state {
                WorkspaceState::Creating => {
                    let staging = self.registry.staging_path(&record.id)?;
                    let inherited_marker_id = if record.copy_mode == CopyMode::All {
                        record.parent_id.as_deref()
                    } else {
                        None
                    };
                    let destination_marker = if record.path.try_exists()? {
                        Some(workspace_marker_state(&record.path, &record.id)?)
                    } else {
                        None
                    };
                    let staging_marker = if let Some(staging) = staging.as_ref() {
                        if staging.try_exists()? {
                            Some(self.failed_create_marker_state(
                                staging,
                                &record.id,
                                inherited_marker_id,
                            )?)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let (issue_path, message) = match destination_marker {
                        Some(MarkerState::Missing) => (
                            record.path.clone(),
                            "interrupted-create destination marker is missing",
                        ),
                        Some(MarkerState::Mismatch) => (
                            record.path.clone(),
                            "interrupted-create destination marker belongs to another workspace",
                        ),
                        Some(MarkerState::Invalid) => (
                            record.path.clone(),
                            "interrupted-create destination marker is invalid",
                        ),
                        None | Some(MarkerState::Matches) => match staging_marker {
                            Some(MarkerState::Missing) => (
                                staging.clone().expect("staging marker requires a path"),
                                "interrupted-create staging marker is missing",
                            ),
                            Some(MarkerState::Mismatch) => (
                                staging.clone().expect("staging marker requires a path"),
                                "interrupted-create staging marker belongs to another workspace",
                            ),
                            Some(MarkerState::Invalid) => (
                                staging.clone().expect("staging marker requires a path"),
                                "interrupted-create staging marker is invalid",
                            ),
                            None | Some(MarkerState::Matches) => {
                                (record.path.clone(), "workspace creation was interrupted")
                            }
                        },
                    };
                    let destination_is_owned =
                        matches!(destination_marker, None | Some(MarkerState::Matches));
                    let staging_is_owned =
                        matches!(staging_marker, None | Some(MarkerState::Matches));
                    let mut fixed = false;
                    if can_fix && destination_is_owned && staging_is_owned {
                        if let Some(staging) = staging.as_ref() {
                            if staging.try_exists()? {
                                self.verify_failed_create_path(
                                    staging,
                                    &record.id,
                                    inherited_marker_id,
                                )?;
                                self.strategy_for(&record.strategy)
                                    .remove_directory(staging)?;
                            }
                        }
                        if record.path.try_exists()? {
                            marker::verify(&record.path, &record.id)?;
                            self.strategy_for(&record.strategy)
                                .remove_directory(&record.path)?;
                        }
                        self.registry.delete_record(&record.id)?;
                        fixed = true;
                    }
                    issues.push(DoctorIssue {
                        workspace_id: record.id,
                        path: issue_path,
                        message: message.to_owned(),
                        fixed,
                    });
                }
                WorkspaceState::Active => {
                    if !record.path.try_exists()? {
                        let trash = trash_path(&record.path, &record.id)?;
                        let trash_marker = if trash.try_exists()? {
                            workspace_marker_state(&trash, &record.id)?
                        } else {
                            MarkerState::Missing
                        };
                        let interrupted_removal = trash_marker == MarkerState::Matches;
                        let fixed = if can_fix && interrupted_removal {
                            self.registry.mark_trashed(
                                &[(record.id.clone(), record.path.clone(), trash.clone())],
                                &Ulid::new().to_string(),
                            )?;
                            true
                        } else {
                            false
                        };
                        let (path, message) = match trash_marker {
                            MarkerState::Invalid => {
                                (trash, "trash workspace marker is invalid".to_owned())
                            }
                            MarkerState::Mismatch => (
                                trash,
                                "trash marker belongs to another workspace".to_owned(),
                            ),
                            _ if interrupted_removal => (
                                record.path.clone(),
                                "workspace removal was interrupted".to_owned(),
                            ),
                            _ => (
                                record.path.clone(),
                                "active workspace path is missing".to_owned(),
                            ),
                        };
                        issues.push(DoctorIssue {
                            workspace_id: record.id,
                            path,
                            message,
                            fixed,
                        });
                    } else {
                        let marker_state = workspace_marker_state(&record.path, &record.id)?;
                        if marker_state != MarkerState::Matches {
                            // A directory recreated at a registered path is
                            // indistinguishable from the original workspace.
                            // Never claim ownership by synthesizing its marker.
                            issues.push(DoctorIssue {
                                workspace_id: record.id,
                                path: record.path,
                                message: match marker_state {
                                    MarkerState::Missing => "workspace marker is missing",
                                    MarkerState::Mismatch => {
                                        "workspace marker belongs to another workspace"
                                    }
                                    MarkerState::Invalid => "workspace marker is invalid",
                                    MarkerState::Matches => unreachable!(),
                                }
                                .to_owned(),
                                fixed: false,
                            });
                        }
                    }
                }
                WorkspaceState::Trashed => {
                    if !record.path.try_exists()? {
                        let restored = if let Some(original) = &record.original_path {
                            original.try_exists()?
                                && workspace_marker_state(original, &record.id)?
                                    == MarkerState::Matches
                        } else {
                            false
                        };
                        let root_active = if can_fix && restored {
                            self.registry
                                .workspace_id(&record.root_id)?
                                .ok_or_else(|| {
                                    Error::RegistryInvariant(format!(
                                        "missing root {}",
                                        record.root_id
                                    ))
                                })?
                                .state
                                == WorkspaceState::Active
                        } else {
                            false
                        };
                        let fixed = if root_active {
                            self.registry.mark_restored(&[(
                                record.id.clone(),
                                record.original_path.clone().expect("checked above"),
                            )])?;
                            true
                        } else {
                            false
                        };
                        issues.push(DoctorIssue {
                            workspace_id: record.id,
                            path: record.path,
                            message: if restored {
                                "workspace restore was interrupted".to_owned()
                            } else {
                                "trash path is missing".to_owned()
                            },
                            fixed,
                        });
                    } else {
                        let marker_state = workspace_marker_state(&record.path, &record.id)?;
                        if marker_state != MarkerState::Matches {
                            issues.push(DoctorIssue {
                                workspace_id: record.id,
                                path: record.path,
                                message: match marker_state {
                                    MarkerState::Missing => "trash workspace marker is missing",
                                    MarkerState::Mismatch => {
                                        "trash marker belongs to another workspace"
                                    }
                                    MarkerState::Invalid => "trash workspace marker is invalid",
                                    MarkerState::Matches => unreachable!(),
                                }
                                .to_owned(),
                                fixed: false,
                            });
                        }
                    }
                }
                WorkspaceState::Unregistered => {
                    let marker_state = workspace_marker_state(&record.path, &record.id)?;
                    match marker_state {
                        MarkerState::Matches => {
                            let fixed = if can_fix {
                                marker::remove(&record.path)?;
                                true
                            } else {
                                false
                            };
                            issues.push(DoctorIssue {
                                workspace_id: record.id,
                                path: record.path,
                                message: "unregistered root still has a marker".to_owned(),
                                fixed,
                            });
                        }
                        MarkerState::Mismatch | MarkerState::Invalid => {
                            issues.push(DoctorIssue {
                                workspace_id: record.id,
                                path: record.path,
                                message: if marker_state == MarkerState::Invalid {
                                    "unregistered root marker is invalid"
                                } else {
                                    "unregistered root marker belongs to another workspace"
                                }
                                .to_owned(),
                                fixed: false,
                            });
                        }
                        MarkerState::Missing => {}
                    }
                }
            }
        }
        Ok(DoctorReport { issues })
    }

    pub fn trashed(&self, at: impl AsRef<Path>) -> Result<Vec<Workspace>> {
        let at = existing_directory(at.as_ref())?;
        let context = match self.workspace_from_optional(&at) {
            Ok(Some(workspace)) => workspace,
            Err(Error::MissingMarker(path))
            | Err(Error::UnknownMarker(path))
            | Err(Error::MarkerMismatch(path)) => self
                .registry
                .workspace_current_path_including_trash(&path)?
                .ok_or_else(|| Error::WorkspaceNotInitialized(at.clone()))?,
            Ok(None) => self
                .registry
                .workspace_ancestor_including_trash(&at)?
                .ok_or_else(|| Error::WorkspaceNotInitialized(at.clone()))?,
            Err(error) => return Err(error),
        };
        let context = self.verify_context_including_trash(context)?;
        Ok(self
            .registry
            .restorable()?
            .into_iter()
            .filter(|workspace| workspace.root_id == context.root_id)
            .collect())
    }

    fn reconcile_interrupted_root_restore(
        &mut self,
        root: &Workspace,
        fix: bool,
    ) -> Result<InterruptedRootRestore> {
        let rows = self.registry.trashed_subtree(&root.id)?;
        let mut restored = vec![(root.id.clone(), root.path.clone())];
        let mut already_moved = Vec::new();
        let mut pending = Vec::new();
        let mut detected = false;
        let mut recoverable = true;

        for row in rows {
            if row.id == root.id {
                continue;
            }
            let Some(original) = row.original_path.clone() else {
                recoverable = false;
                continue;
            };
            let trash_exists = row.path.try_exists()?;
            let original_exists = original.try_exists()?;
            match (trash_exists, original_exists) {
                (false, true) => {
                    detected = true;
                    if workspace_marker_state(&original, &row.id)? == MarkerState::Matches {
                        already_moved.push((row.id.clone(), original.clone()));
                    } else {
                        recoverable = false;
                    }
                }
                (true, false) => {
                    if workspace_marker_state(&row.path, &row.id)? == MarkerState::Matches {
                        pending.push((row.id.clone(), original.clone(), row.path.clone()));
                    } else {
                        recoverable = false;
                    }
                }
                (false, false) | (true, true) => recoverable = false,
            }
            restored.push((row.id, original));
        }

        if !detected {
            return Ok(InterruptedRootRestore::NotDetected);
        }
        if !fix || !recoverable {
            return Ok(InterruptedRootRestore::Detected);
        }

        marker::verify(&root.path, &root.id)?;
        for (id, original) in &already_moved {
            marker::verify(original, id)?;
        }
        for (_, original, _) in &pending {
            self.checked_restore_destination(original)?;
        }
        for (_, original) in &already_moved {
            self.checked_restore_destination(original)?;
        }
        let mut moved = Vec::new();
        let operation = (|| -> Result<()> {
            for (id, original, trash) in &pending {
                marker::verify(trash, id)?;
                if let Some(parent) = original.parent() {
                    fs::create_dir_all(parent)?;
                }
                let destination = self.checked_restore_destination(original)?;
                fs::rename(trash, &destination)?;
                moved.push((id.clone(), destination, trash.clone()));
            }
            self.registry.mark_restored(&restored)
        })();
        if let Err(error) = operation {
            for (_, original, trash) in moved.iter().rev() {
                let _ = fs::rename(original, trash);
            }
            return Err(error);
        }
        Ok(InterruptedRootRestore::Fixed)
    }

    fn rollback_trashed_workspaces(&mut self, moved: &[(String, PathBuf, PathBuf)]) -> Result<()> {
        for (_, original, trash) in moved.iter().rev() {
            fs::rename(trash, original)?;
        }
        self.registry.mark_restored(
            &moved
                .iter()
                .map(|(id, original, _)| (id.clone(), original.clone()))
                .collect::<Vec<_>>(),
        )
    }

    fn unregister_root_with(
        &self,
        root: &Workspace,
        removal_id: &str,
        remove_marker: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<()> {
        self.registry.mark_unregistered(&root.id, removal_id)?;
        if let Err(error) = remove_marker(&root.path) {
            self.registry.mark_active(&root.id)?;
            return Err(error);
        }
        Ok(())
    }

    fn verify_trashed_workspace_for_gc(&self, workspace: &Workspace) -> Result<bool> {
        if workspace.path.try_exists()? {
            marker::verify(&workspace.path, &workspace.id)?;
            return Ok(true);
        }
        if let Some(original) = &workspace.original_path {
            if original.try_exists()?
                && workspace_marker_state(original, &workspace.id)? == MarkerState::Matches
            {
                return Err(Error::InterruptedRestore(original.clone()));
            }
        }
        Ok(false)
    }

    fn remove_trashed_workspace(&self, workspace: &Workspace) -> Result<()> {
        let result = self
            .strategy_for(&workspace.strategy)
            .remove_directory(&workspace.path);
        if result.is_err()
            && workspace.path.try_exists()?
            && marker::read(&workspace.path)?.is_none()
        {
            // A strategy must leave the ownership marker in place when a
            // recursive deletion fails. Repair older/custom strategies too so
            // the next GC can safely retry instead of becoming unrecoverable.
            marker::write(&workspace.path, &workspace.id)?;
        }
        result
    }

    fn ensure_unregistered_root_not_restored(&self, root: &Workspace) -> Result<()> {
        if !root.path.try_exists()? {
            return Ok(());
        }
        match workspace_marker_state(&root.path, &root.id)? {
            MarkerState::Missing => Ok(()),
            MarkerState::Matches => Err(Error::InterruptedRestore(root.path.clone())),
            MarkerState::Mismatch => Err(Error::MarkerMismatch(root.path.clone())),
            MarkerState::Invalid => Err(Error::InvalidMarker(root.path.clone())),
        }
    }

    fn failed_create_marker_state(
        &self,
        path: &Path,
        id: &str,
        inherited_marker_id: Option<&str>,
    ) -> Result<MarkerState> {
        let state = workspace_marker_state(path, id)?;
        if state != MarkerState::Mismatch {
            return Ok(state);
        }
        let Some(inherited_marker_id) = inherited_marker_id else {
            return Ok(state);
        };
        if marker::read(path)?.as_deref() != Some(inherited_marker_id) {
            return Ok(state);
        }

        // A source marker is only evidence of an interrupted clone if the
        // source itself still exists at a distinct registered path. This keeps
        // a moved source workspace from being mistaken for disposable staging.
        let Some(source) = self.registry.workspace_id(inherited_marker_id)? else {
            return Ok(state);
        };
        if source.path == path || !source.path.try_exists()? {
            return Ok(state);
        }
        if workspace_marker_state(&source.path, inherited_marker_id)? == MarkerState::Matches {
            Ok(MarkerState::Matches)
        } else {
            Ok(state)
        }
    }

    fn verify_failed_create_path(
        &self,
        path: &Path,
        id: &str,
        inherited_marker_id: Option<&str>,
    ) -> Result<()> {
        match self.failed_create_marker_state(path, id, inherited_marker_id)? {
            MarkerState::Matches => Ok(()),
            MarkerState::Missing => Err(Error::MissingMarker(path.to_path_buf())),
            MarkerState::Mismatch => Err(Error::MarkerMismatch(path.to_path_buf())),
            MarkerState::Invalid => Err(Error::InvalidMarker(path.to_path_buf())),
        }
    }

    fn cleanup_failed_create_path(
        &self,
        strategy: &str,
        path: &Path,
        id: &str,
        inherited_marker_id: Option<&str>,
    ) -> bool {
        match path.try_exists() {
            Ok(false) => true,
            Err(_) => false,
            Ok(true) => {
                if self
                    .verify_failed_create_path(path, id, inherited_marker_id)
                    .is_err()
                {
                    return false;
                }
                let _ = self.strategy_for(strategy).remove_directory(path);
                matches!(path.try_exists(), Ok(false))
            }
        }
    }

    fn strategy_for(&self, name: &str) -> &dyn Strategy {
        if name == self.copy_strategy.name() {
            self.copy_strategy.as_ref()
        } else {
            self.cow_strategy.as_ref()
        }
    }

    fn mutation_lock(&self) -> Result<MutationLock> {
        MutationLock::acquire(&self.lock_path)
    }

    fn ensure_no_managed_ancestor(&self, path: &Path) -> Result<()> {
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        if let Some(existing) = self.workspace_from_optional(parent)? {
            return Err(Error::Path(format!(
                "cannot initialize a nested workspace inside {}",
                existing.path.display()
            )));
        }
        Ok(())
    }

    fn checked_restore_destination(&self, path: &Path) -> Result<PathBuf> {
        // Retain the original path in safety errors while also validating the
        // resolved destination used for the rename.
        self.ensure_restore_destination_unmanaged(path)?;
        let resolved = resolve_from_existing_ancestor(path)?;
        if resolved != path {
            return Err(Error::Path(format!(
                "restore destination ancestry changed: {} resolves to {}",
                path.display(),
                resolved.display()
            )));
        }
        self.ensure_restore_destination_unmanaged(&resolved)?;
        Ok(resolved)
    }

    fn ensure_restore_destination_unmanaged(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            let resolved_parent = nearest_existing_ancestor(parent)?;
            if self.workspace_from_optional(&resolved_parent)?.is_some() {
                return Err(Error::InsideManagedWorkspace(path.to_path_buf()));
            }
        }
        if let Some(descendant) = self.registry.all_records()?.into_iter().find(|workspace| {
            workspace.state == WorkspaceState::Active
                && workspace.path != path
                && workspace.path.starts_with(path)
        }) {
            return Err(Error::ContainsManagedWorkspace(descendant.path));
        }
        Ok(())
    }

    #[cfg(windows)]
    fn ensure_current_directory_outside_removal(&self, rows: &[Workspace]) -> Result<()> {
        let current = fs::canonicalize(std::env::current_dir()?)?;
        let Some(selected) = rows.iter().find(|row| current.starts_with(&row.path)) else {
            return Ok(());
        };
        Err(Error::Path(format!(
            "cannot remove {} while the current directory is inside it on Windows; change to another workspace first",
            selected.path.display()
        )))
    }

    fn verify_context_including_trash(&self, workspace: Workspace) -> Result<Workspace> {
        if workspace.state == WorkspaceState::Trashed {
            marker::verify(&workspace.path, &workspace.id)?;
        }
        self.verify_resolved_workspace(workspace)
    }

    fn verify_resolved_workspace(&self, workspace: Workspace) -> Result<Workspace> {
        if workspace.state != WorkspaceState::Active {
            return Ok(workspace);
        }
        if !workspace.path.exists() {
            return Err(Error::MissingWorkspace(workspace.path));
        }
        marker::ensure_real_workspace_path(&workspace.path)?;
        match marker::read(&workspace.path)? {
            Some(id) if id == workspace.id => Ok(workspace),
            Some(_) => Err(Error::MarkerMismatch(workspace.path)),
            None => Err(Error::MissingMarker(workspace.path)),
        }
    }

    fn workspace_from(&self, path: &Path) -> Result<Workspace> {
        self.workspace_from_optional(path)?
            .ok_or_else(|| Error::WorkspaceNotInitialized(path.to_path_buf()))
    }

    fn workspace_from_optional(&self, path: &Path) -> Result<Option<Workspace>> {
        let directories = path.ancestors().collect::<Vec<_>>();
        let mut registered_paths = None;
        for directory in &directories {
            if let Some(id) = marker::read(directory)? {
                let record = self
                    .registry
                    .workspace_id(&id)?
                    .ok_or_else(|| Error::UnknownMarker((*directory).to_path_buf()))?;
                if record.path != *directory || record.state != WorkspaceState::Active {
                    return Err(Error::MarkerMismatch((*directory).to_path_buf()));
                }
                marker::ensure_real_workspace_path(directory)?;
                return Ok(Some(record));
            }
            if registered_paths.is_none() {
                registered_paths = Some(
                    self.registry
                        .workspaces_at_paths(&directories)?
                        .into_iter()
                        .map(|workspace| (workspace.path.clone(), workspace))
                        .collect::<HashMap<_, _>>(),
                );
            }
            if registered_paths
                .as_ref()
                .and_then(|paths| paths.get(*directory))
                .is_some_and(|record| record.state == WorkspaceState::Active)
            {
                return Err(Error::MissingMarker((*directory).to_path_buf()));
            }
        }
        Ok(None)
    }

    fn generate_handle(&self, root_id: &str) -> Result<String> {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        for _ in 0..64 {
            let mut bytes = [0_u8; 4];
            getrandom::getrandom(&mut bytes).map_err(|error| {
                Error::Io(std::io::Error::other(format!(
                    "failed to generate workspace handle: {error}"
                )))
            })?;
            let handle = bytes
                .iter()
                .map(|byte| ALPHABET[*byte as usize % ALPHABET.len()] as char)
                .collect::<String>();
            if self
                .registry
                .find_target(root_id, &handle, true)?
                .is_empty()
            {
                return Ok(handle);
            }
        }
        Err(Error::Path(
            "failed to generate a unique workspace handle".to_owned(),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InterruptedRootRestore {
    NotDetected,
    Detected,
    Fixed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MarkerState {
    Missing,
    Matches,
    Mismatch,
    Invalid,
}

fn workspace_marker_state(path: &Path, expected_id: &str) -> Result<MarkerState> {
    let marker = match marker::ensure_real_workspace_path(path) {
        Ok(()) => marker::read(path),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MarkerState::Missing);
        }
        Err(error) => Err(error),
    };
    match marker {
        Ok(None) => Ok(MarkerState::Missing),
        Ok(Some(id)) if id == expected_id => Ok(MarkerState::Matches),
        Ok(Some(_)) => Ok(MarkerState::Mismatch),
        Err(Error::InvalidMarker(_)) => Ok(MarkerState::Invalid),
        Err(error) => Err(error),
    }
}

fn restore_marker_missing(path: &Path, expected_id: &str) -> Result<bool> {
    match workspace_marker_state(path, expected_id)? {
        MarkerState::Missing => Ok(true),
        MarkerState::Matches => Ok(false),
        MarkerState::Mismatch => Err(Error::MarkerMismatch(path.to_path_buf())),
        MarkerState::Invalid => Err(Error::InvalidMarker(path.to_path_buf())),
    }
}

fn database_lock_path(database: &Path) -> Result<PathBuf> {
    let name = database.file_name().ok_or_else(|| {
        Error::Path(format!(
            "workspace database has no file name: {}",
            database.display()
        ))
    })?;
    let mut lock_name = name.to_os_string();
    lock_name.push(".lock");
    Ok(database.with_file_name(lock_name))
}

fn default_database_path() -> Result<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| Error::Path("HOME directory is unavailable".to_owned()))?;
    Ok(home.join(".hz").join("state.sqlite"))
}

fn nearest_existing_ancestor(path: &Path) -> Result<PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor.try_exists()? {
            return Ok(fs::canonicalize(ancestor)?);
        }
    }
    Err(Error::Path(format!(
        "path has no existing ancestor: {}",
        path.display()
    )))
}

fn resolve_from_existing_ancestor(path: &Path) -> Result<PathBuf> {
    for ancestor in path.ancestors() {
        if ancestor.try_exists()? {
            let suffix = path
                .strip_prefix(ancestor)
                .map_err(|error| Error::Path(error.to_string()))?;
            return Ok(fs::canonicalize(ancestor)?.join(suffix));
        }
    }
    Err(Error::Path(format!(
        "path has no existing ancestor: {}",
        path.display()
    )))
}

fn existing_directory(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path)?;
    if !path.is_dir() {
        return Err(Error::Path(format!("not a directory: {}", path.display())));
    }
    Ok(path)
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn root_handle(path: &Path) -> Result<String> {
    let handle = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::Path(format!("workspace has no name: {}", path.display())))?;
    validate_handle(handle)
}

fn validate_handle(handle: String) -> Result<String> {
    if handle.is_empty()
        || matches!(handle.as_str(), "." | ".." | "current" | "root" | "local")
        || handle.contains('/')
        || handle.contains('\\')
    {
        return Err(Error::Path(format!("invalid workspace handle: {handle}")));
    }
    validate_printable_handle(&handle)?;
    Ok(handle)
}

fn validate_printable_handle(handle: &str) -> Result<()> {
    if handle
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
    {
        return Err(Error::Path(format!("invalid workspace handle: {handle:?}")));
    }
    Ok(())
}

fn default_storage(root: &Path, id: &str) -> Result<PathBuf> {
    let parent = root
        .parent()
        .ok_or_else(|| Error::Path(format!("workspace has no parent: {}", root.display())))?;
    let name = root
        .file_name()
        .ok_or_else(|| Error::Path(format!("workspace has no name: {}", root.display())))?;
    Ok(parent.join(".hz-workspaces").join(format!(
        "{}-{}",
        name.to_string_lossy(),
        &id[..id.len().min(8)]
    )))
}

#[cfg(unix)]
fn ensure_cow_storage_compatible(root: &Path, storage: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let storage_ancestor = nearest_existing_ancestor(storage)?;
    if fs::metadata(root)?.dev() != fs::metadata(&storage_ancestor)?.dev() {
        return Err(Error::CowUnavailable(format!(
            "workspace storage {} is not on the same filesystem as {}",
            storage.display(),
            root.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_cow_storage_compatible(_root: &Path, _storage: &Path) -> Result<()> {
    Ok(())
}

fn trash_path(path: &Path, id: &str) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Path(format!("workspace has no parent: {}", path.display())))?;
    Ok(parent.join(".trash").join(id))
}

fn ensure_real_trash_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    marker::ensure_real_workspace_path(path)
}

fn depth_in(id: &str, parents: &HashMap<String, Option<String>>) -> usize {
    let mut depth = 0;
    let mut current = parents.get(id).cloned().flatten();
    while let Some(parent) = current {
        depth += 1;
        current = parents.get(&parent).cloned().flatten();
    }
    depth
}
