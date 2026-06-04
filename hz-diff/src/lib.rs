use std::{
    borrow::Cow,
    env, fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    process::{self, Command},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiffScope {
    #[default]
    All,
    Staged,
    Unstaged,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DiffSource {
    #[default]
    Worktree,
    Base(String),
    Range {
        left: String,
        right: String,
    },
    Patch(PatchSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchSource {
    File(PathBuf),
    Stdin(Arc<str>),
    Text { label: String, patch: Arc<str> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffOptions {
    pub repo: Option<PathBuf>,
    pub source: DiffSource,
    pub scope: DiffScope,
    pub include_untracked: bool,
    pub stat: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            repo: None,
            source: DiffSource::Worktree,
            scope: DiffScope::All,
            include_untracked: true,
            stat: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changeset {
    pub repo: PathBuf,
    pub title: String,
    pub files: Vec<DiffFile>,
    pub raw_patch: String,
}

impl Changeset {
    pub fn stats(&self) -> DiffStats {
        let mut stats = DiffStats {
            files: self.files.len(),
            ..DiffStats::default()
        };
        for file in &self.files {
            stats.additions += file.additions;
            stats.deletions += file.deletions;
            if file.is_binary {
                stats.binary_files += 1;
            }
        }
        stats
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub files: usize,
    pub additions: usize,
    pub deletions: usize,
    pub binary_files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: FileStatus,
    pub hunks: Vec<DiffHunk>,
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
}

impl DiffFile {
    pub fn display_path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or("/dev/null")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unknown,
}

impl FileStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Added => "added",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::TypeChanged => "type-changed",
            Self::Unknown => "changed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
    Meta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRowRef {
    FileHeader(usize),
    BinaryFile(usize),
    HunkHeader {
        file: usize,
        hunk: usize,
    },
    Line {
        file: usize,
        hunk: usize,
        line: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffViewModel {
    rows: Vec<DiffRowRef>,
    file_start_rows: Vec<usize>,
    hunk_start_rows: Vec<usize>,
}

impl DiffViewModel {
    pub fn new(changeset: &Changeset) -> Self {
        let mut rows = Vec::new();
        let mut file_start_rows = Vec::with_capacity(changeset.files.len());
        let mut hunk_start_rows = Vec::new();

        for (file_index, file) in changeset.files.iter().enumerate() {
            file_start_rows.push(rows.len());
            rows.push(DiffRowRef::FileHeader(file_index));

            if file.is_binary || file.hunks.is_empty() {
                rows.push(DiffRowRef::BinaryFile(file_index));
                continue;
            }

            for (hunk_index, hunk) in file.hunks.iter().enumerate() {
                hunk_start_rows.push(rows.len());
                rows.push(DiffRowRef::HunkHeader {
                    file: file_index,
                    hunk: hunk_index,
                });
                for line_index in 0..hunk.lines.len() {
                    rows.push(DiffRowRef::Line {
                        file: file_index,
                        hunk: hunk_index,
                        line: line_index,
                    });
                }
            }
        }

        Self {
            rows,
            file_start_rows,
            hunk_start_rows,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn row(&self, index: usize) -> Option<DiffRowRef> {
        self.rows.get(index).copied()
    }

    pub fn file_start_row(&self, file: usize) -> Option<usize> {
        self.file_start_rows.get(file).copied()
    }

    pub fn file_at_row(&self, row: usize) -> Option<usize> {
        if self.file_start_rows.is_empty() {
            return None;
        }
        match self.file_start_rows.binary_search(&row) {
            Ok(index) => Some(index),
            Err(0) => Some(0),
            Err(index) => Some(index - 1),
        }
    }

    pub fn next_hunk_row(&self, row: usize) -> Option<usize> {
        self.hunk_start_rows
            .iter()
            .copied()
            .find(|start| *start > row)
    }

    pub fn previous_hunk_row(&self, row: usize) -> Option<usize> {
        self.hunk_start_rows
            .iter()
            .rev()
            .copied()
            .find(|start| *start < row)
    }
}

pub fn load(options: DiffOptions) -> HzResult<Changeset> {
    load_changeset(&options, true)
}

pub fn load_review(options: DiffOptions) -> HzResult<Changeset> {
    load_review_ref(&options)
}

pub fn load_review_ref(options: &DiffOptions) -> HzResult<Changeset> {
    load_changeset(options, false)
}

fn load_changeset(options: &DiffOptions, keep_raw_patch: bool) -> HzResult<Changeset> {
    let title = diff_title(options);
    let (repo, patch) = diff_patch(options)?;
    let files = parse_patch(&patch);
    let raw_patch = if keep_raw_patch {
        patch.into_owned()
    } else {
        String::new()
    };

    Ok(Changeset {
        repo,
        title,
        files,
        raw_patch,
    })
}

fn diff_patch(options: &DiffOptions) -> HzResult<(PathBuf, Cow<'_, str>)> {
    if let DiffSource::Patch(source) = &options.source {
        validate_options(options)?;
        let repo = options.repo.clone().unwrap_or_default();
        return Ok((repo, patch_source_text(source)?));
    }

    let repo = hz_git::repository_root(options.repo.as_deref())?;
    validate_options(options)?;
    let args = git_diff_args(options);
    let patch = if should_include_untracked(options) {
        git_diff_text_with_untracked(&repo, &args)?
    } else {
        git_diff_text(&repo, &args)?
    };

    Ok((repo, Cow::Owned(patch)))
}

pub fn render(options: DiffOptions) -> HzResult<String> {
    if options.stat {
        let changeset = load_review_ref(&options)?;
        return Ok(render_stat(&changeset));
    }
    let (_, patch) = diff_patch(&options)?;
    Ok(patch.into_owned())
}

fn patch_source_text(source: &PatchSource) -> HzResult<Cow<'_, str>> {
    match source {
        PatchSource::File(path) => Ok(Cow::Owned(fs::read_to_string(path)?)),
        PatchSource::Stdin(patch) => Ok(Cow::Borrowed(patch.as_ref())),
        PatchSource::Text { patch, .. } => Ok(Cow::Borrowed(patch.as_ref())),
    }
}

pub fn render_stat(changeset: &Changeset) -> String {
    let mut output = String::new();
    for file in &changeset.files {
        output.push_str(&format!(
            "{:>6} {:>6} {}\n",
            file.additions,
            file.deletions,
            file.display_path()
        ));
    }
    let stats = changeset.stats();
    output.push_str(&format!(
        "\n{} files changed, {} insertions(+), {} deletions(-)",
        stats.files, stats.additions, stats.deletions
    ));
    if stats.binary_files > 0 {
        output.push_str(&format!(", {} binary", stats.binary_files));
    }
    output.push('\n');
    output
}

fn validate_options(options: &DiffOptions) -> HzResult<()> {
    if matches!(options.source, DiffSource::Patch(_)) {
        if options.scope != DiffScope::All {
            return Err(HzError::Usage(
                "--staged and --unstaged do not apply to patch input".to_owned(),
            ));
        }
        return Ok(());
    }

    if !matches!(options.source, DiffSource::Worktree) && options.scope != DiffScope::All {
        return Err(HzError::Usage(
            "--staged and --unstaged only apply to working tree diffs".to_owned(),
        ));
    }
    Ok(())
}

fn git_diff_args(options: &DiffOptions) -> Vec<String> {
    let mut args = vec![
        "diff".to_owned(),
        "--binary".to_owned(),
        "--no-ext-diff".to_owned(),
        "--no-color".to_owned(),
        "--find-renames".to_owned(),
    ];

    match &options.source {
        DiffSource::Worktree => match options.scope {
            DiffScope::All => args.push("HEAD".to_owned()),
            DiffScope::Staged => args.push("--cached".to_owned()),
            DiffScope::Unstaged => {}
        },
        DiffSource::Base(base) => args.push(format!("{base}...HEAD")),
        DiffSource::Range { left, right } => {
            args.push(left.clone());
            args.push(right.clone());
        }
        DiffSource::Patch(_) => {}
    }

    args
}

fn should_include_untracked(options: &DiffOptions) -> bool {
    options.include_untracked
        && matches!(options.source, DiffSource::Worktree)
        && matches!(options.scope, DiffScope::All | DiffScope::Unstaged)
}

fn diff_title(options: &DiffOptions) -> String {
    match &options.source {
        DiffSource::Worktree => match options.scope {
            DiffScope::All => "working tree vs HEAD".to_owned(),
            DiffScope::Staged => "staged changes".to_owned(),
            DiffScope::Unstaged => "unstaged changes".to_owned(),
        },
        DiffSource::Base(base) => format!("{base}...HEAD"),
        DiffSource::Range { left, right } => format!("{left}..{right}"),
        DiffSource::Patch(PatchSource::File(path)) => format!("patch {}", path.display()),
        DiffSource::Patch(PatchSource::Stdin(_)) => "patch stdin".to_owned(),
        DiffSource::Patch(PatchSource::Text { label, .. }) => label.clone(),
    }
}

fn git_diff_text(repo: &Path, args: &[String]) -> HzResult<String> {
    git_diff_text_with_index(repo, args, None)
}

fn git_diff_text_with_index(
    repo: &Path,
    args: &[String],
    index: Option<&Path>,
) -> HzResult<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(args);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }

    let output = command.output()?;
    if !output.status.success() {
        return Err(git_error("failed to render git diff", &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_diff_text_with_untracked(repo: &Path, args: &[String]) -> HzResult<String> {
    let untracked = untracked_paths(repo)?;
    if untracked.is_empty() {
        return git_diff_text(repo, args);
    }

    let temp_index = create_temp_index(repo)?;
    add_intent_to_add(repo, temp_index.path(), &untracked)?;
    git_diff_text_with_index(repo, args, Some(temp_index.path()))
}

fn add_intent_to_add(repo: &Path, index: &Path, paths: &[PathBuf]) -> HzResult<()> {
    for chunk in paths.chunks(128) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .env("GIT_INDEX_FILE", index)
            .args(["add", "-N", "--"])
            .args(chunk)
            .output()?;
        if !output.status.success() {
            return Err(git_error(
                "failed to prepare untracked files for diff",
                &output,
            ));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct TempIndex {
    path: PathBuf,
}

impl TempIndex {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn create_temp_index(repo: &Path) -> HzResult<TempIndex> {
    let source = git_path(repo, "index")?;
    for attempt in 0..16 {
        let path = temp_index_path(attempt)?;
        let mut temp = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };

        let copy_result = (|| -> HzResult<()> {
            if source.exists() {
                let mut source_file = fs::File::open(&source)?;
                std::io::copy(&mut source_file, &mut temp)?;
            }
            temp.flush()?;
            Ok(())
        })();

        if let Err(error) = copy_result {
            let _ = fs::remove_file(&path);
            return Err(error);
        }

        return Ok(TempIndex { path });
    }

    Err(HzError::Usage(
        "failed to create a unique temporary git index".to_owned(),
    ))
}

fn git_path(repo: &Path, path: &str) -> HzResult<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--git-path", path])
        .output()?;
    if !output.status.success() {
        return Err(git_error("failed to resolve git path", &output));
    }

    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if path.is_empty() {
        return Err(HzError::Usage("git path was empty".to_owned()));
    }

    let path = PathBuf::from(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(repo.join(path))
    }
}

fn temp_index_path(attempt: u32) -> HzResult<PathBuf> {
    Ok(env::temp_dir().join(format!(
        "hz-diff-index-{}-{}-{}.tmp",
        process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| HzError::Usage(format!("system time before unix epoch: {error}")))?
            .as_nanos(),
        attempt
    )))
}

fn untracked_paths(repo: &Path) -> HzResult<Vec<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()?;

    if !output.status.success() {
        return Err(git_error("failed to list untracked files", &output));
    }

    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(path_from_git_bytes)
        .collect())
}

#[cfg(unix)]
fn path_from_git_bytes(path: &[u8]) -> PathBuf {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    PathBuf::from(OsString::from_vec(path.to_vec()))
}

#[cfg(not(unix))]
fn path_from_git_bytes(path: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(path).into_owned())
}

fn git_error(message: &str, output: &std::process::Output) -> HzError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        HzError::Usage(message.to_owned())
    } else {
        HzError::Usage(format!("{message}: {stderr}"))
    }
}

pub fn parse_patch(patch: &str) -> Vec<DiffFile> {
    let mut files = Vec::new();
    let mut current: Option<DiffFileBuilder> = None;
    let mut current_hunk: Option<DiffHunkBuilder> = None;

    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            finish_hunk(&mut current, &mut current_hunk);
            finish_file(&mut files, &mut current);
            current = Some(DiffFileBuilder::from_diff_git(line));
            continue;
        }

        let Some(file) = current.as_mut() else {
            continue;
        };

        if line.starts_with("@@ ") {
            finish_hunk(&mut current, &mut current_hunk);
            current_hunk = Some(DiffHunkBuilder::from_header(line));
            continue;
        }

        if let Some(hunk) = current_hunk.as_mut() {
            hunk.push_line(line);
            continue;
        }

        file.apply_header(line);
    }

    finish_hunk(&mut current, &mut current_hunk);
    finish_file(&mut files, &mut current);
    files
}

fn finish_hunk(file: &mut Option<DiffFileBuilder>, hunk: &mut Option<DiffHunkBuilder>) {
    if let (Some(file), Some(hunk)) = (file.as_mut(), hunk.take()) {
        file.additions += hunk.additions;
        file.deletions += hunk.deletions;
        file.hunks.push(hunk.finish());
    }
}

fn finish_file(files: &mut Vec<DiffFile>, file: &mut Option<DiffFileBuilder>) {
    if let Some(file) = file.take() {
        files.push(file.finish());
    }
}

#[derive(Debug)]
struct DiffFileBuilder {
    old_path: Option<String>,
    new_path: Option<String>,
    status: FileStatus,
    hunks: Vec<DiffHunk>,
    additions: usize,
    deletions: usize,
    is_binary: bool,
}

impl DiffFileBuilder {
    fn from_diff_git(line: &str) -> Self {
        let (old_path, new_path) = diff_git_paths(line);

        Self {
            old_path,
            new_path,
            status: FileStatus::Modified,
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
            is_binary: false,
        }
    }

    fn apply_header(&mut self, line: &str) {
        if line.starts_with("new file mode ") {
            self.status = FileStatus::Added;
        } else if line.starts_with("deleted file mode ") {
            self.status = FileStatus::Deleted;
        } else if line.starts_with("rename from ") {
            self.status = FileStatus::Renamed;
            self.old_path = Some(line.trim_start_matches("rename from ").to_owned());
        } else if line.starts_with("rename to ") {
            self.status = FileStatus::Renamed;
            self.new_path = Some(line.trim_start_matches("rename to ").to_owned());
        } else if line.starts_with("copy from ") {
            self.status = FileStatus::Copied;
            self.old_path = Some(line.trim_start_matches("copy from ").to_owned());
        } else if line.starts_with("copy to ") {
            self.status = FileStatus::Copied;
            self.new_path = Some(line.trim_start_matches("copy to ").to_owned());
        } else if line.starts_with("old mode ") || line.starts_with("new mode ") {
            if !matches!(self.status, FileStatus::Renamed | FileStatus::Copied) {
                self.status = FileStatus::TypeChanged;
            }
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            self.is_binary = true;
        } else if let Some(path) = line.strip_prefix("--- ") {
            if path != "/dev/null" {
                self.old_path = strip_prefix_path(path, "a/");
            } else {
                self.status = FileStatus::Added;
                self.old_path = None;
            }
        } else if let Some(path) = line.strip_prefix("+++ ") {
            if path != "/dev/null" {
                self.new_path = strip_prefix_path(path, "b/");
            } else {
                self.status = FileStatus::Deleted;
                self.new_path = None;
            }
        }
    }

    fn finish(self) -> DiffFile {
        DiffFile {
            old_path: self.old_path,
            new_path: self.new_path,
            status: self.status,
            hunks: self.hunks,
            additions: self.additions,
            deletions: self.deletions,
            is_binary: self.is_binary,
        }
    }
}

fn diff_git_paths(line: &str) -> (Option<String>, Option<String>) {
    let Some(paths) = line.strip_prefix("diff --git ") else {
        return (None, None);
    };

    split_diff_git_paths(paths)
        .map(|(old, new)| (strip_prefix_path(old, "a/"), strip_prefix_path(new, "b/")))
        .unwrap_or((None, None))
}

fn split_diff_git_paths(paths: &str) -> Option<(&str, &str)> {
    let mut fallback = None;
    for (separator, _) in paths.match_indices(" b/") {
        let old = &paths[..separator];
        let new = &paths[separator + 1..];
        if !old.starts_with("a/") || !new.starts_with("b/") {
            continue;
        }

        let old_path = old.strip_prefix("a/").unwrap_or(old);
        let new_path = new.strip_prefix("b/").unwrap_or(new);
        if old_path == new_path {
            return Some((old, new));
        }

        fallback = Some((old, new));
    }

    fallback
}

fn strip_prefix_path(path: &str, prefix: &str) -> Option<String> {
    Some(path.strip_prefix(prefix).unwrap_or(path).to_owned())
}

#[derive(Debug)]
struct DiffHunkBuilder {
    header: String,
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    old_line: usize,
    new_line: usize,
    additions: usize,
    deletions: usize,
    lines: Vec<DiffLine>,
}

impl DiffHunkBuilder {
    fn from_header(header: &str) -> Self {
        let (old_start, old_count, new_start, new_count) = parse_hunk_header(header);
        Self {
            header: header.to_owned(),
            old_start,
            old_count,
            new_start,
            new_count,
            old_line: old_start,
            new_line: new_start,
            additions: 0,
            deletions: 0,
            lines: Vec::new(),
        }
    }

    fn push_line(&mut self, raw: &str) {
        let Some(prefix) = raw.as_bytes().first().copied() else {
            self.push_context("");
            return;
        };

        let text = raw.get(1..).unwrap_or_default().to_owned();
        match prefix {
            b'+' => {
                let new_line = self.new_line;
                self.new_line += 1;
                self.additions += 1;
                self.lines.push(DiffLine {
                    kind: DiffLineKind::Addition,
                    old_line: None,
                    new_line: Some(new_line),
                    text,
                });
            }
            b'-' => {
                let old_line = self.old_line;
                self.old_line += 1;
                self.deletions += 1;
                self.lines.push(DiffLine {
                    kind: DiffLineKind::Deletion,
                    old_line: Some(old_line),
                    new_line: None,
                    text,
                });
            }
            b' ' => self.push_context(&text),
            b'\\' => self.lines.push(DiffLine {
                kind: DiffLineKind::Meta,
                old_line: None,
                new_line: None,
                text: raw.to_owned(),
            }),
            _ => self.push_context(raw),
        }
    }

    fn push_context(&mut self, text: &str) {
        let old_line = self.old_line;
        let new_line = self.new_line;
        self.old_line += 1;
        self.new_line += 1;
        self.lines.push(DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(old_line),
            new_line: Some(new_line),
            text: text.to_owned(),
        });
    }

    fn finish(self) -> DiffHunk {
        DiffHunk {
            header: self.header,
            old_start: self.old_start,
            old_count: self.old_count,
            new_start: self.new_start,
            new_count: self.new_count,
            lines: self.lines,
        }
    }
}

fn parse_hunk_header(header: &str) -> (usize, usize, usize, usize) {
    let mut parts = header.split_whitespace();
    let _ = parts.next();
    let old = parts.next().unwrap_or("-0,0");
    let new = parts.next().unwrap_or("+0,0");
    let (old_start, old_count) = parse_hunk_range(old.trim_start_matches('-'));
    let (new_start, new_count) = parse_hunk_range(new.trim_start_matches('+'));
    (old_start, old_count, new_start, new_count)
}

fn parse_hunk_range(range: &str) -> (usize, usize) {
    let mut parts = range.splitn(2, ',');
    let start = parts.next().unwrap_or("0").parse().unwrap_or(0);
    let count = parts.next().map_or(1, |count| count.parse().unwrap_or(1));
    (start, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, process::Stdio};

    #[test]
    fn parse_patch_reads_file_hunks_and_line_numbers() {
        let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,3 @@\n one\n-two\n+two changed\n+three\n";

        let files = parse_patch(patch);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_path(), "a.txt");
        assert_eq!(files[0].additions, 2);
        assert_eq!(files[0].deletions, 1);
        assert_eq!(files[0].hunks[0].lines[0].old_line, Some(1));
        assert_eq!(files[0].hunks[0].lines[0].new_line, Some(1));
        assert_eq!(files[0].hunks[0].lines[1].old_line, Some(2));
        assert_eq!(files[0].hunks[0].lines[1].new_line, None);
        assert_eq!(files[0].hunks[0].lines[2].old_line, None);
        assert_eq!(files[0].hunks[0].lines[2].new_line, Some(2));
    }

    #[test]
    fn parse_patch_preserves_binary_paths_with_spaces() {
        let patch = "diff --git a/my file.bin b/my file.bin\nindex 1111111..2222222 100644\nGIT binary patch\nliteral 1\nKcmZQz1ONa4\n\n";

        let files = parse_patch(patch);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].old_path.as_deref(), Some("my file.bin"));
        assert_eq!(files[0].new_path.as_deref(), Some("my file.bin"));
        assert_eq!(files[0].display_path(), "my file.bin");
        assert!(files[0].is_binary);
    }

    #[test]
    fn rename_or_copy_status_wins_over_later_mode_headers() {
        let renamed = parse_patch(
            "diff --git a/old.txt b/new.txt\nrename from old.txt\nrename to new.txt\nold mode 100644\nnew mode 100755\n",
        );
        assert_eq!(renamed[0].status, FileStatus::Renamed);

        let copied = parse_patch(
            "diff --git a/source.txt b/copy.txt\ncopy from source.txt\ncopy to copy.txt\nold mode 100644\nnew mode 100755\n",
        );
        assert_eq!(copied[0].status, FileStatus::Copied);
    }

    #[test]
    fn view_model_indexes_file_and_hunk_rows() {
        let changeset = Changeset {
            repo: PathBuf::from("/repo"),
            title: "test".to_owned(),
            files: parse_patch(
                "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n",
            ),
            raw_patch: String::new(),
        };
        let model = DiffViewModel::new(&changeset);

        assert_eq!(model.file_start_row(0), Some(0));
        assert_eq!(model.file_at_row(3), Some(0));
        assert_eq!(model.next_hunk_row(0), Some(1));
        assert_eq!(model.previous_hunk_row(4), Some(1));
    }

    #[test]
    fn patch_file_source_renders_without_git_repo() {
        let test_dir = temp_test_dir("patch-file-source");
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        let patch_path = test_dir.join("change.diff");
        let patch =
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";
        fs::write(&patch_path, patch).expect("patch file should be written");

        let output = render(DiffOptions {
            source: DiffSource::Patch(PatchSource::File(patch_path)),
            ..DiffOptions::default()
        })
        .expect("patch source should render");

        assert_eq!(output, patch);
        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn patch_stdin_source_parses_stats_without_raw_patch_retention() {
        let patch = Arc::<str>::from(
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1,2 @@\n-old\n+new\n+again\n",
        );
        let options = DiffOptions {
            source: DiffSource::Patch(PatchSource::Stdin(patch)),
            stat: true,
            ..DiffOptions::default()
        };

        let changeset = load_review_ref(&options).expect("patch source should parse");

        assert_eq!(changeset.files.len(), 1);
        assert_eq!(changeset.files[0].additions, 2);
        assert_eq!(changeset.files[0].deletions, 1);
        assert!(changeset.raw_patch.is_empty());
    }

    #[test]
    fn patch_text_source_uses_label_title() {
        let patch = Arc::<str>::from(
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n",
        );
        let options = DiffOptions {
            source: DiffSource::Patch(PatchSource::Text {
                label: "github pr owner/repo#1".to_owned(),
                patch,
            }),
            ..DiffOptions::default()
        };

        let changeset = load_review_ref(&options).expect("patch source should parse");

        assert_eq!(changeset.title, "github pr owner/repo#1");
        assert_eq!(changeset.files.len(), 1);
    }

    #[test]
    fn render_untracked_empty_and_noeol_files_as_applyable_patch() {
        let test_dir = temp_test_dir("untracked-exact");
        let repo = test_dir.join("repo");
        let destination = test_dir.join("destination");
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        init_repo(&repo);

        fs::write(repo.join("empty.txt"), "").expect("empty file should be written");
        fs::write(repo.join("noeol.txt"), "no newline").expect("noeol file should be written");

        git(
            [
                "clone",
                "-q",
                repo.to_str().unwrap(),
                destination.to_str().unwrap(),
            ],
            &test_dir,
        );
        let patch = render(DiffOptions {
            repo: Some(repo.clone()),
            ..DiffOptions::default()
        })
        .expect("diff should render");

        git_apply(&destination, patch.as_bytes());
        assert_eq!(fs::read(destination.join("empty.txt")).unwrap(), b"");
        assert_eq!(
            fs::read(destination.join("noeol.txt")).unwrap(),
            b"no newline"
        );

        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn render_untracked_symlink_as_symlink_without_reading_target() {
        let test_dir = temp_test_dir("untracked-symlink");
        let repo = test_dir.join("repo");
        let destination = test_dir.join("destination");
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        init_repo(&repo);

        fs::write(test_dir.join("secret.txt"), "outside secret\n")
            .expect("target file should be written");
        std::os::unix::fs::symlink("../secret.txt", repo.join("link.txt"))
            .expect("symlink should be created");

        git(
            [
                "clone",
                "-q",
                repo.to_str().unwrap(),
                destination.to_str().unwrap(),
            ],
            &test_dir,
        );
        let patch = render(DiffOptions {
            repo: Some(repo.clone()),
            ..DiffOptions::default()
        })
        .expect("diff should render");

        assert!(patch.contains("new file mode 120000"));
        assert!(patch.contains("+../secret.txt"));
        assert!(!patch.contains("outside secret"));

        git_apply(&destination, patch.as_bytes());
        let target = fs::read_link(destination.join("link.txt")).unwrap();
        assert_eq!(target, PathBuf::from("../secret.txt"));

        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "hz-diff-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ))
    }

    fn init_repo(repo: &Path) {
        fs::create_dir_all(repo).expect("repo directory should be created");
        git(["init", "-q"], repo);
        git(["config", "user.email", "test@example.com"], repo);
        git(["config", "user.name", "Test"], repo);
        fs::write(repo.join("base.txt"), "base\n").expect("base file should be written");
        git(["add", "base.txt"], repo);
        git(["commit", "-q", "-m", "init"], repo);
    }

    fn git<const N: usize>(args: [&str; N], cwd: &Path) {
        let output = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_apply(repo: &Path, patch: &[u8]) {
        let mut child = Command::new("git")
            .current_dir(repo)
            .args(["apply", "--binary"])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("git apply should start");
        child
            .stdin
            .as_mut()
            .expect("stdin should be open")
            .write_all(patch)
            .expect("patch should be written");
        let output = child.wait_with_output().expect("git apply should finish");
        assert!(
            output.status.success(),
            "git apply failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
