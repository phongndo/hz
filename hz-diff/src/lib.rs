use std::{
    borrow::Cow,
    fs,
    io::{self, BufRead, BufReader, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    sync::Arc,
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult};

const STREAM_BUFFER_BYTES: usize = 1024 * 1024;

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
    Branch {
        base: String,
        head: String,
    },
    Range {
        left: String,
        right: String,
    },
    Patch(PatchSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchSource {
    File(PathBuf),
    Stdin(Arc<[u8]>),
    Text { label: String, patch: Arc<[u8]> },
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
    pub raw_patch: Vec<u8>,
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

pub fn load_review_ref_path(options: &DiffOptions, path: &Path) -> HzResult<Changeset> {
    load_changeset_paths(options, &[path.to_path_buf()], false)
}

pub fn load_review_ref_paths(options: &DiffOptions, paths: &[PathBuf]) -> HzResult<Changeset> {
    load_changeset_paths(options, paths, false)
}

fn load_changeset(options: &DiffOptions, keep_raw_patch: bool) -> HzResult<Changeset> {
    let title = diff_title(options);
    let (repo, patch) = diff_patch_bytes(options)?;
    changeset_from_patch(repo, title, patch, keep_raw_patch)
}

fn load_changeset_paths(
    options: &DiffOptions,
    paths: &[PathBuf],
    keep_raw_patch: bool,
) -> HzResult<Changeset> {
    let title = diff_title(options);
    let (repo, patch) = diff_patch_bytes_paths(options, paths)?;
    changeset_from_patch(repo, title, Cow::Owned(patch), keep_raw_patch)
}

fn changeset_from_patch(
    repo: PathBuf,
    title: String,
    patch: Cow<'_, [u8]>,
    keep_raw_patch: bool,
) -> HzResult<Changeset> {
    let files = {
        // The parsed model is text-only for stats/TUI display. Keep raw_patch
        // as bytes and only decode lossily at this display/parsing boundary.
        let patch_text = String::from_utf8_lossy(patch.as_ref());
        parse_patch(&patch_text)
    };
    let raw_patch = if keep_raw_patch {
        patch.into_owned()
    } else {
        Vec::new()
    };

    Ok(Changeset {
        repo,
        title,
        files,
        raw_patch,
    })
}

fn diff_patch_bytes_paths(
    options: &DiffOptions,
    paths: &[PathBuf],
) -> HzResult<(PathBuf, Vec<u8>)> {
    if matches!(options.source, DiffSource::Patch(_)) {
        return Err(HzError::Usage(
            "path-scoped reload does not apply to patch input".to_owned(),
        ));
    }
    if paths.is_empty() {
        return Err(HzError::Usage(
            "path-scoped reload requires at least one path".to_owned(),
        ));
    }

    let repo = hz_git::repository_root(options.repo.as_deref())?;
    validate_options(options)?;
    let mut args = git_diff_args(options);
    append_pathspecs(&mut args, paths);
    let patch = if should_include_untracked(options) {
        git_diff_bytes_with_untracked_pathspecs(&repo, &args, paths)?
    } else {
        git_diff_bytes(&repo, &args)?
    };

    Ok((repo, patch))
}

fn diff_patch_bytes(options: &DiffOptions) -> HzResult<(PathBuf, Cow<'_, [u8]>)> {
    if let DiffSource::Patch(source) = &options.source {
        validate_options(options)?;
        let repo = options.repo.clone().unwrap_or_default();
        return Ok((repo, patch_source_bytes(source)?));
    }

    let repo = hz_git::repository_root(options.repo.as_deref())?;
    validate_options(options)?;
    let args = git_diff_args(options);
    let patch = if should_include_untracked(options) {
        git_diff_bytes_with_untracked(&repo, &args)?
    } else {
        git_diff_bytes(&repo, &args)?
    };

    Ok((repo, Cow::Owned(patch)))
}

pub fn render(options: DiffOptions) -> HzResult<String> {
    let bytes = render_bytes(options)?;
    String::from_utf8(bytes).map_err(|_| {
        HzError::Usage("diff output is not valid UTF-8; use byte-preserving output".to_owned())
    })
}

pub fn render_bytes(options: DiffOptions) -> HzResult<Vec<u8>> {
    if options.stat {
        return render_stat_bytes(&options);
    }
    let (_, patch) = diff_patch_bytes(&options)?;
    Ok(patch.into_owned())
}

pub fn render_to_writer(options: DiffOptions, writer: impl Write) -> HzResult<()> {
    render_to_writer_ref(&options, writer)
}

pub fn render_to_writer_ref(options: &DiffOptions, mut writer: impl Write) -> HzResult<()> {
    if options.stat {
        writer.write_all(&render_stat_bytes(options)?)?;
        return Ok(());
    }

    if let DiffSource::Patch(source) = &options.source {
        validate_options(options)?;
        write_patch_source(source, writer)?;
        return Ok(());
    }

    let repo = hz_git::repository_root(options.repo.as_deref())?;
    validate_options(options)?;
    let args = git_diff_args(options);
    if should_include_untracked(options) {
        git_diff_to_writer_with_untracked(&repo, &args, writer)
    } else {
        git_diff_to_writer(&repo, &args, writer)
    }
}

fn render_stat_bytes(options: &DiffOptions) -> HzResult<Vec<u8>> {
    let stats = patch_stats(options)?;
    Ok(render_patch_stats(&stats).into_bytes())
}

fn write_patch_source(source: &PatchSource, mut writer: impl Write) -> HzResult<()> {
    match source {
        PatchSource::File(path) => {
            let mut file = fs::File::open(path)?;
            copy_to_writer(&mut file, &mut writer)?;
        }
        PatchSource::Stdin(patch) => writer.write_all(patch.as_ref())?,
        PatchSource::Text { patch, .. } => writer.write_all(patch.as_ref())?,
    }
    Ok(())
}

fn copy_to_writer(mut reader: impl Read, mut writer: impl Write) -> io::Result<u64> {
    let mut total = 0u64;
    let mut buffer = vec![0; STREAM_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        total = total.saturating_add(read as u64);
    }
    Ok(total)
}

fn patch_source_bytes(source: &PatchSource) -> HzResult<Cow<'_, [u8]>> {
    match source {
        PatchSource::File(path) => Ok(Cow::Owned(fs::read(path)?)),
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PatchStats {
    files: Vec<PatchFileStat>,
    totals: DiffStats,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PatchFileStat {
    old_path: Option<String>,
    new_path: Option<String>,
    additions: usize,
    deletions: usize,
    is_binary: bool,
}

impl PatchFileStat {
    fn display_path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or("/dev/null")
    }
}

fn render_patch_stats(stats: &PatchStats) -> String {
    let mut output = String::new();
    for file in &stats.files {
        output.push_str(&format!(
            "{:>6} {:>6} {}\n",
            file.additions,
            file.deletions,
            file.display_path()
        ));
    }
    output.push_str(&format!(
        "\n{} files changed, {} insertions(+), {} deletions(-)",
        stats.totals.files, stats.totals.additions, stats.totals.deletions
    ));
    if stats.totals.binary_files > 0 {
        output.push_str(&format!(", {} binary", stats.totals.binary_files));
    }
    output.push('\n');
    output
}

fn patch_stats(options: &DiffOptions) -> HzResult<PatchStats> {
    if let DiffSource::Patch(source) = &options.source {
        validate_options(options)?;
        return patch_source_stats(source);
    }

    let repo = hz_git::repository_root(options.repo.as_deref())?;
    validate_options(options)?;
    let args = git_diff_numstat_args(options);
    if should_include_untracked(options) {
        git_numstat_stats_with_untracked(&repo, &args)
    } else {
        git_numstat_stats(&repo, &args)
    }
}

fn patch_source_stats(source: &PatchSource) -> HzResult<PatchStats> {
    match source {
        PatchSource::File(path) => {
            let file = fs::File::open(path)?;
            parse_patch_stats(BufReader::new(file)).map_err(HzError::Io)
        }
        PatchSource::Stdin(patch) => {
            parse_patch_stats(BufReader::new(patch.as_ref())).map_err(HzError::Io)
        }
        PatchSource::Text { patch, .. } => {
            parse_patch_stats(BufReader::new(patch.as_ref())).map_err(HzError::Io)
        }
    }
}

fn parse_patch_stats(reader: impl BufRead) -> io::Result<PatchStats> {
    let mut stats = PatchStats::default();
    let mut current: Option<PatchFileStatBuilder> = None;
    let mut current_hunk: Option<PatchHunkStat> = None;

    for line in reader.lines() {
        let line = line?;
        let line = line.trim_end_matches('\r');

        if let Some(hunk) = current_hunk.as_mut() {
            hunk.push_line(line, current.as_mut());
            if hunk.is_complete() {
                current_hunk = None;
            }
            continue;
        }

        if line.starts_with("diff --git ") {
            finish_patch_stat_file(&mut stats, &mut current);
            current = Some(PatchFileStatBuilder::from_diff_git(line));
            continue;
        }

        if line.starts_with("--- ") {
            if current
                .as_ref()
                .is_some_and(PatchFileStatBuilder::has_seen_hunk)
            {
                finish_patch_stat_file(&mut stats, &mut current);
            }
            let file = current.get_or_insert_with(PatchFileStatBuilder::default);
            file.apply_header(line);
            continue;
        }

        if let Some(file) = current.as_mut() {
            if line.starts_with("@@ ") {
                file.seen_hunk = true;
                current_hunk = Some(PatchHunkStat::from_header(line));
                continue;
            }

            file.apply_header(line);
        }
    }

    finish_patch_stat_file(&mut stats, &mut current);
    Ok(stats)
}

fn finish_patch_stat_file(stats: &mut PatchStats, file: &mut Option<PatchFileStatBuilder>) {
    let Some(file) = file.take() else {
        return;
    };
    let file = file.finish();
    stats.totals.files += 1;
    stats.totals.additions += file.additions;
    stats.totals.deletions += file.deletions;
    if file.is_binary {
        stats.totals.binary_files += 1;
    }
    stats.files.push(file);
}

#[derive(Debug, Default)]
struct PatchFileStatBuilder {
    old_path: Option<String>,
    new_path: Option<String>,
    additions: usize,
    deletions: usize,
    is_binary: bool,
    seen_hunk: bool,
}

impl PatchFileStatBuilder {
    fn from_diff_git(line: &str) -> Self {
        let (old_path, new_path) = diff_git_paths(line);
        Self {
            old_path,
            new_path,
            ..Self::default()
        }
    }

    fn has_seen_hunk(&self) -> bool {
        self.seen_hunk
    }

    fn apply_header(&mut self, line: &str) {
        if line.starts_with("Binary files ") || line == "GIT binary patch" {
            self.is_binary = true;
        } else if let Some(path) = line.strip_prefix("--- ") {
            let path = unified_header_path(path);
            if path.as_ref() != "/dev/null" {
                self.old_path = strip_prefix_path(path.as_ref(), "a/");
            } else {
                self.old_path = None;
            }
        } else if let Some(path) = line.strip_prefix("+++ ") {
            let path = unified_header_path(path);
            if path.as_ref() != "/dev/null" {
                self.new_path = strip_prefix_path(path.as_ref(), "b/");
            } else {
                self.new_path = None;
            }
        } else if let Some(path) = line.strip_prefix("rename from ") {
            self.old_path = Some(path.to_owned());
        } else if let Some(path) = line.strip_prefix("rename to ") {
            self.new_path = Some(path.to_owned());
        } else if let Some(path) = line.strip_prefix("copy from ") {
            self.old_path = Some(path.to_owned());
        } else if let Some(path) = line.strip_prefix("copy to ") {
            self.new_path = Some(path.to_owned());
        }
    }

    fn finish(self) -> PatchFileStat {
        PatchFileStat {
            old_path: self.old_path,
            new_path: self.new_path,
            additions: self.additions,
            deletions: self.deletions,
            is_binary: self.is_binary,
        }
    }
}

#[derive(Debug)]
struct PatchHunkStat {
    old_remaining: usize,
    new_remaining: usize,
}

impl PatchHunkStat {
    fn from_header(header: &str) -> Self {
        let (_, old_count, _, new_count) = parse_hunk_header(header);
        Self {
            old_remaining: old_count,
            new_remaining: new_count,
        }
    }

    fn push_line(&mut self, raw: &str, file: Option<&mut PatchFileStatBuilder>) {
        match raw.as_bytes().first().copied() {
            Some(b'+') => {
                self.new_remaining = self.new_remaining.saturating_sub(1);
                if let Some(file) = file {
                    file.additions += 1;
                }
            }
            Some(b'-') => {
                self.old_remaining = self.old_remaining.saturating_sub(1);
                if let Some(file) = file {
                    file.deletions += 1;
                }
            }
            Some(b'\\') => {}
            _ => {
                self.old_remaining = self.old_remaining.saturating_sub(1);
                self.new_remaining = self.new_remaining.saturating_sub(1);
            }
        }
    }

    fn is_complete(&self) -> bool {
        self.old_remaining == 0 && self.new_remaining == 0
    }
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
            DiffScope::All => {
                args.push("--end-of-options".to_owned());
                args.push("HEAD".to_owned());
            }
            DiffScope::Staged => args.push("--cached".to_owned()),
            DiffScope::Unstaged => {}
        },
        DiffSource::Base(base) => {
            args.push("--end-of-options".to_owned());
            args.push(format!("{base}...HEAD"));
        }
        DiffSource::Branch { base, head } => {
            args.push("--end-of-options".to_owned());
            args.push(format!("{base}...{head}"));
        }
        DiffSource::Range { left, right } => {
            args.push("--end-of-options".to_owned());
            args.push(left.clone());
            args.push(right.clone());
        }
        DiffSource::Patch(_) => {}
    }

    args
}

fn append_pathspecs(args: &mut Vec<String>, paths: &[PathBuf]) {
    args.push("--".to_owned());
    args.extend(paths.iter().map(|path| path.to_string_lossy().into_owned()));
}

fn git_diff_numstat_args(options: &DiffOptions) -> Vec<String> {
    let mut args = vec![
        "diff".to_owned(),
        "--numstat".to_owned(),
        "-z".to_owned(),
        "--no-ext-diff".to_owned(),
        "--no-color".to_owned(),
        "--find-renames".to_owned(),
    ];

    match &options.source {
        DiffSource::Worktree => match options.scope {
            DiffScope::All => {
                args.push("--end-of-options".to_owned());
                args.push("HEAD".to_owned());
            }
            DiffScope::Staged => args.push("--cached".to_owned()),
            DiffScope::Unstaged => {}
        },
        DiffSource::Base(base) => {
            args.push("--end-of-options".to_owned());
            args.push(format!("{base}...HEAD"));
        }
        DiffSource::Branch { base, head } => {
            args.push("--end-of-options".to_owned());
            args.push(format!("{base}...{head}"));
        }
        DiffSource::Range { left, right } => {
            args.push("--end-of-options".to_owned());
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
        DiffSource::Branch { base, head } => format!("{base}...{head}"),
        DiffSource::Range { left, right } => format!("{left}..{right}"),
        DiffSource::Patch(PatchSource::File(path)) => format!("patch {}", path.display()),
        DiffSource::Patch(PatchSource::Stdin(_)) => "patch stdin".to_owned(),
        DiffSource::Patch(PatchSource::Text { label, .. }) => label.clone(),
    }
}

fn git_diff_bytes(repo: &Path, args: &[String]) -> HzResult<Vec<u8>> {
    git_diff_bytes_with_index(repo, args, None)
}

fn git_diff_bytes_with_index(
    repo: &Path,
    args: &[String],
    index: Option<&Path>,
) -> HzResult<Vec<u8>> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(args);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }

    let output = command.output()?;
    if !output.status.success() {
        return Err(git_error("failed to render git diff", &output));
    }
    Ok(output.stdout)
}

fn git_diff_bytes_with_untracked(repo: &Path, args: &[String]) -> HzResult<Vec<u8>> {
    let untracked = untracked_paths(repo)?;
    git_diff_bytes_with_untracked_paths(repo, args, untracked)
}

fn git_diff_bytes_with_untracked_pathspecs(
    repo: &Path,
    args: &[String],
    pathspecs: &[PathBuf],
) -> HzResult<Vec<u8>> {
    let untracked = untracked_paths_for(repo, pathspecs)?;
    git_diff_bytes_with_untracked_paths(repo, args, untracked)
}

fn git_diff_bytes_with_untracked_paths(
    repo: &Path,
    args: &[String],
    untracked: Vec<PathBuf>,
) -> HzResult<Vec<u8>> {
    if untracked.is_empty() {
        return git_diff_bytes(repo, args);
    }

    let temp_index = create_temp_index(repo)?;
    add_intent_to_add(repo, temp_index.path(), &untracked)?;
    git_diff_bytes_with_index(repo, args, Some(temp_index.path()))
}

fn git_diff_to_writer(repo: &Path, args: &[String], writer: impl Write) -> HzResult<()> {
    git_diff_to_writer_with_index(repo, args, None, writer)
}

fn git_diff_to_writer_with_index(
    repo: &Path,
    args: &[String],
    index: Option<&Path>,
    mut writer: impl Write,
) -> HzResult<()> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }

    let mut child = command.spawn()?;
    let stderr = drain_child_stderr(child.stderr.take());
    if let Some(mut stdout) = child.stdout.take() {
        if let Err(error) = copy_to_writer(&mut stdout, &mut writer) {
            abort_git_child(child, stderr);
            return Err(error.into());
        }
    }
    wait_for_git_child(child, stderr, "failed to render git diff")
}

fn git_diff_to_writer_with_untracked(
    repo: &Path,
    args: &[String],
    writer: impl Write,
) -> HzResult<()> {
    let untracked = untracked_paths(repo)?;
    if untracked.is_empty() {
        return git_diff_to_writer(repo, args, writer);
    }

    let temp_index = create_temp_index(repo)?;
    add_intent_to_add(repo, temp_index.path(), &untracked)?;
    git_diff_to_writer_with_index(repo, args, Some(temp_index.path()), writer)
}

fn git_numstat_stats(repo: &Path, args: &[String]) -> HzResult<PatchStats> {
    git_numstat_stats_with_index(repo, args, None)
}

fn git_numstat_stats_with_index(
    repo: &Path,
    args: &[String],
    index: Option<&Path>,
) -> HzResult<PatchStats> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }

    let mut child = command.spawn()?;
    let stderr = drain_child_stderr(child.stderr.take());
    let stats = match if let Some(stdout) = child.stdout.take() {
        parse_numstat(stdout)
    } else {
        Ok(PatchStats::default())
    } {
        Ok(stats) => stats,
        Err(error) => {
            abort_git_child(child, stderr);
            return Err(error.into());
        }
    };
    wait_for_git_child(child, stderr, "failed to render git diff")?;
    Ok(stats)
}

fn drain_child_stderr(stderr: Option<process::ChildStderr>) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        if let Some(mut stderr) = stderr {
            stderr.read_to_end(&mut output)?;
        }
        Ok(output)
    })
}

fn wait_for_git_child(
    mut child: process::Child,
    stderr: JoinHandle<io::Result<Vec<u8>>>,
    message: &str,
) -> HzResult<()> {
    let status = child.wait()?;
    let stderr = stderr
        .join()
        .map_err(|_| io::Error::other("git stderr reader panicked"))??;
    let output = process::Output {
        status,
        stdout: Vec::new(),
        stderr,
    };
    if !output.status.success() {
        return Err(git_error(message, &output));
    }
    Ok(())
}

fn abort_git_child(mut child: process::Child, stderr: JoinHandle<io::Result<Vec<u8>>>) {
    let _ = child.kill();
    let _ = child.wait();
    let _ = stderr.join();
}

fn git_numstat_stats_with_untracked(repo: &Path, args: &[String]) -> HzResult<PatchStats> {
    let untracked = untracked_paths(repo)?;
    if untracked.is_empty() {
        return git_numstat_stats(repo, args);
    }

    let temp_index = create_temp_index(repo)?;
    add_intent_to_add(repo, temp_index.path(), &untracked)?;
    git_numstat_stats_with_index(repo, args, Some(temp_index.path()))
}

fn parse_numstat(mut reader: impl Read) -> io::Result<PatchStats> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;

    let records = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    let mut stats = PatchStats::default();
    let mut index = 0usize;

    while let Some(record) = records.get(index).copied() {
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let additions = fields.next().unwrap_or_default();
        let deletions = fields.next().unwrap_or_default();
        let path = fields.next().unwrap_or_default();
        let (display_path, next_index) = if path.is_empty() && index + 2 < records.len() {
            (records[index + 2], index + 3)
        } else {
            (path, index + 1)
        };

        let is_binary = additions == b"-" || deletions == b"-";
        let additions = parse_numstat_count(additions).unwrap_or_default();
        let deletions = parse_numstat_count(deletions).unwrap_or_default();
        let file = PatchFileStat {
            old_path: None,
            new_path: Some(String::from_utf8_lossy(display_path).into_owned()),
            additions,
            deletions,
            is_binary,
        };

        stats.totals.files += 1;
        stats.totals.additions += additions;
        stats.totals.deletions += deletions;
        if is_binary {
            stats.totals.binary_files += 1;
        }
        stats.files.push(file);
        index = next_index;
    }

    Ok(stats)
}

fn parse_numstat_count(bytes: &[u8]) -> Option<usize> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
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
        let path = temp_index_path(&source, attempt)?;
        let mut temp = match create_private_temp_file(&path) {
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

fn create_private_temp_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn temp_index_path(index_path: &Path, attempt: u32) -> HzResult<PathBuf> {
    let parent = index_path.parent().ok_or_else(|| {
        HzError::Usage(format!(
            "git index path has no parent: {}",
            index_path.display()
        ))
    })?;
    Ok(parent.join(format!(
        ".hz-diff-index-{}-{}-{}.tmp",
        process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| HzError::Usage(format!("system time before unix epoch: {error}")))?
            .as_nanos(),
        attempt
    )))
}

fn untracked_paths(repo: &Path) -> HzResult<Vec<PathBuf>> {
    untracked_paths_for(repo, &[])
}

fn untracked_paths_for(repo: &Path, pathspecs: &[PathBuf]) -> HzResult<Vec<PathBuf>> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "--others", "--exclude-standard", "-z"]);
    if !pathspecs.is_empty() {
        command.arg("--").args(pathspecs);
    }

    let output = command.output()?;

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
    let mut lines = patch.lines().peekable();

    while let Some(line) = lines.next() {
        if line.starts_with("diff --git ") {
            finish_hunk(&mut current, &mut current_hunk);
            finish_file(&mut files, &mut current);
            current = Some(DiffFileBuilder::from_diff_git(line));
            continue;
        }

        if line.starts_with("--- ")
            && (current.is_none()
                || current_hunk
                    .as_ref()
                    .is_some_and(DiffHunkBuilder::is_complete))
            && let Some(new_header) = lines
                .peek()
                .copied()
                .filter(|line| line.starts_with("+++ "))
        {
            finish_hunk(&mut current, &mut current_hunk);
            finish_file(&mut files, &mut current);
            let new_header = new_header.to_owned();
            let _ = lines.next();
            current = Some(DiffFileBuilder::from_unified_headers(line, &new_header));
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

    fn from_unified_headers(old_header: &str, new_header: &str) -> Self {
        let mut builder = Self {
            old_path: None,
            new_path: None,
            status: FileStatus::Modified,
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
            is_binary: false,
        };
        builder.apply_header(old_header);
        builder.apply_header(new_header);
        builder
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
            let path = unified_header_path(path);
            if path.as_ref() != "/dev/null" {
                self.old_path = strip_prefix_path(path.as_ref(), "a/");
            } else {
                self.status = FileStatus::Added;
                self.old_path = None;
            }
        } else if let Some(path) = line.strip_prefix("+++ ") {
            let path = unified_header_path(path);
            if path.as_ref() != "/dev/null" {
                self.new_path = strip_prefix_path(path.as_ref(), "b/");
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

    if paths.starts_with('"')
        && let Some((old, rest)) = parse_quoted_git_path_token(paths)
        && let Some((new, trailing)) = parse_quoted_git_path_token(rest.trim_start())
        && trailing.trim().is_empty()
    {
        return (strip_prefix_path(&old, "a/"), strip_prefix_path(&new, "b/"));
    }

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

fn unified_header_path(path: &str) -> Cow<'_, str> {
    if path.starts_with('"')
        && let Some((path, _)) = parse_quoted_git_path_token(path)
    {
        return Cow::Owned(path);
    }

    Cow::Borrowed(path.split_once('\t').map_or(path, |(path, _)| path))
}

fn parse_quoted_git_path_token(input: &str) -> Option<(String, &str)> {
    let input = input.strip_prefix('"')?;
    let mut output = Vec::new();
    let mut index = 0;
    let bytes = input.as_bytes();
    while let Some(byte) = bytes.get(index).copied() {
        match byte {
            b'"' => {
                return Some((
                    String::from_utf8_lossy(&output).into_owned(),
                    &input[index + 1..],
                ));
            }
            b'\\' => {
                index += 1;
                parse_git_path_escape(input, &mut index, &mut output)?;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    None
}

fn parse_git_path_escape(input: &str, index: &mut usize, output: &mut Vec<u8>) -> Option<()> {
    let bytes = input.as_bytes();
    let escaped = *bytes.get(*index)?;
    match escaped {
        b'a' => push_escaped_byte(index, output, b'\x07'),
        b'b' => push_escaped_byte(index, output, b'\x08'),
        b'f' => push_escaped_byte(index, output, b'\x0c'),
        b'n' => push_escaped_byte(index, output, b'\n'),
        b'r' => push_escaped_byte(index, output, b'\r'),
        b't' => push_escaped_byte(index, output, b'\t'),
        b'v' => push_escaped_byte(index, output, b'\x0b'),
        b'\\' => push_escaped_byte(index, output, b'\\'),
        b'"' => push_escaped_byte(index, output, b'"'),
        b'0'..=b'7' => push_octal_escape(bytes, index, output),
        byte if byte.is_ascii() => push_escaped_byte(index, output, byte),
        _ => {
            let character = input[*index..].chars().next()?;
            let mut buffer = [0; 4];
            output.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            *index += character.len_utf8();
        }
    }
    Some(())
}

fn push_escaped_byte(index: &mut usize, output: &mut Vec<u8>, byte: u8) {
    output.push(byte);
    *index += 1;
}

fn push_octal_escape(bytes: &[u8], index: &mut usize, output: &mut Vec<u8>) {
    let mut value = 0u32;
    for _ in 0..3 {
        let Some(byte) = bytes.get(*index).copied() else {
            break;
        };
        if !(b'0'..=b'7').contains(&byte) {
            break;
        }
        value = value * 8 + u32::from(byte - b'0');
        *index += 1;
    }
    if let Ok(byte) = u8::try_from(value) {
        output.push(byte);
    } else {
        output.extend_from_slice("\u{FFFD}".as_bytes());
    }
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
            lines: Vec::with_capacity(old_count.saturating_add(new_count)),
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

    fn is_complete(&self) -> bool {
        self.old_line.saturating_sub(self.old_start) >= self.old_count
            && self.new_line.saturating_sub(self.new_start) >= self.new_count
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
    use std::{env, io::Write, process::Stdio};

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
    fn parse_patch_stats_counts_without_storing_hunk_lines() {
        let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,3 @@\n one\n-two\n+two changed\n+three\ndiff --git a/blob.bin b/blob.bin\nBinary files a/blob.bin and b/blob.bin differ\n";

        let stats = parse_patch_stats(BufReader::new(patch.as_bytes())).unwrap();

        assert_eq!(stats.files.len(), 2);
        assert_eq!(stats.files[0].display_path(), "a.txt");
        assert_eq!(stats.files[0].additions, 2);
        assert_eq!(stats.files[0].deletions, 1);
        assert_eq!(stats.files[1].display_path(), "blob.bin");
        assert!(stats.files[1].is_binary);
        assert_eq!(stats.totals.files, 2);
        assert_eq!(stats.totals.additions, 2);
        assert_eq!(stats.totals.deletions, 1);
        assert_eq!(stats.totals.binary_files, 1);
    }

    #[test]
    fn render_bytes_stat_matches_full_changeset_stat_for_patch() {
        let patch = Arc::<[u8]>::from(
            b"--- a/a.txt\n+++ b/a.txt\n@@ -1 +1,2 @@\n-old\n+new\n+next\n--- a/b.txt\n+++ b/b.txt\n@@ -2 +2 @@\n-left\n+right\n"
                .as_slice(),
        );
        let options = DiffOptions {
            source: DiffSource::Patch(PatchSource::Stdin(patch)),
            stat: true,
            include_untracked: false,
            ..DiffOptions::default()
        };

        let streamed = String::from_utf8(render_bytes(options.clone()).unwrap()).unwrap();
        let full = render_stat(&load_review_ref(&options).unwrap());

        assert_eq!(streamed, full);
    }

    #[test]
    fn render_bytes_stat_matches_full_changeset_stat_for_repo_source() {
        let test_dir = temp_test_dir("repo-stat-equivalence");
        let repo = test_dir.join("repo");
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        init_repo(&repo);
        fs::write(repo.join("rename.txt"), "same\n").expect("renamed file should be written");
        fs::write(repo.join("binary.bin"), b"\0base\n").expect("binary file should be written");
        git(["add", "rename.txt", "binary.bin"], &repo);
        git(["commit", "-q", "-m", "fixtures"], &repo);

        fs::write(repo.join("base.txt"), "base\nnext\n").expect("tracked file should change");
        fs::write(repo.join("binary.bin"), b"\0changed\n").expect("binary file should change");
        fs::write(repo.join("untracked.txt"), "new\n").expect("untracked file should be written");
        git(["mv", "rename.txt", "renamed.txt"], &repo);
        let options = DiffOptions {
            repo: Some(repo.clone()),
            stat: true,
            ..DiffOptions::default()
        };

        let streamed = String::from_utf8(render_bytes(options.clone()).unwrap()).unwrap();
        let full = render_stat(&load_review_ref(&options).unwrap());

        assert_eq!(streamed, full);
        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn load_review_ref_path_limits_tracked_and_untracked_files() {
        let test_dir = temp_test_dir("path-scoped-review");
        let repo = test_dir.join("repo");
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        init_repo(&repo);
        fs::write(repo.join("other.txt"), "other\n").expect("other file should be written");
        git(["add", "other.txt"], &repo);
        git(["commit", "-q", "-m", "other"], &repo);

        fs::write(repo.join("base.txt"), "base changed\n").expect("base file should change");
        fs::write(repo.join("other.txt"), "other changed\n").expect("other file should change");
        fs::write(repo.join("new.txt"), "new\n").expect("untracked file should be written");
        let options = DiffOptions {
            repo: Some(repo.clone()),
            ..DiffOptions::default()
        };

        let tracked = load_review_ref_path(&options, Path::new("base.txt")).unwrap();
        assert_eq!(tracked.files.len(), 1);
        assert_eq!(tracked.files[0].display_path(), "base.txt");

        let untracked = load_review_ref_path(&options, Path::new("new.txt")).unwrap();
        assert_eq!(untracked.files.len(), 1);
        assert_eq!(untracked.files[0].display_path(), "new.txt");

        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn load_review_ref_paths_preserves_scoped_rename_metadata() {
        let test_dir = temp_test_dir("path-scoped-rename");
        let repo = test_dir.join("repo");
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        init_repo(&repo);
        let base = (1..=20)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        fs::write(repo.join("old.txt"), base).expect("old file should be written");
        git(["add", "old.txt"], &repo);
        git(["commit", "-q", "-m", "old"], &repo);

        git(["mv", "old.txt", "new.txt"], &repo);
        let changed = (1..=20)
            .map(|line| {
                if line == 20 {
                    "line changed\n".to_owned()
                } else {
                    format!("line {line}\n")
                }
            })
            .collect::<String>();
        fs::write(repo.join("new.txt"), changed).expect("new file should be changed");
        let options = DiffOptions {
            repo: Some(repo.clone()),
            ..DiffOptions::default()
        };

        let new_only = load_review_ref_path(&options, Path::new("new.txt")).unwrap();
        assert_eq!(new_only.files[0].status, FileStatus::Added);

        let paired = load_review_ref_paths(
            &options,
            &[PathBuf::from("old.txt"), PathBuf::from("new.txt")],
        )
        .unwrap();

        assert_eq!(paired.files.len(), 1);
        assert_eq!(paired.files[0].status, FileStatus::Renamed);
        assert_eq!(paired.files[0].old_path.as_deref(), Some("old.txt"));
        assert_eq!(paired.files[0].new_path.as_deref(), Some("new.txt"));
        assert_eq!(paired.files[0].additions, 1);
        assert_eq!(paired.files[0].deletions, 1);

        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn parse_numstat_reads_regular_renamed_and_binary_files() {
        let numstat =
            b"2\t1\tsrc/lib.rs\x00-\t-\timage.bin\x000\t0\t\x00old/name.rs\x00new/name.rs\x00";

        let stats = parse_numstat(numstat.as_slice()).unwrap();

        assert_eq!(stats.files.len(), 3);
        assert_eq!(stats.files[0].display_path(), "src/lib.rs");
        assert_eq!(stats.files[1].display_path(), "image.bin");
        assert!(stats.files[1].is_binary);
        assert_eq!(stats.files[2].display_path(), "new/name.rs");
        assert_eq!(stats.totals.files, 3);
        assert_eq!(stats.totals.additions, 2);
        assert_eq!(stats.totals.deletions, 1);
        assert_eq!(stats.totals.binary_files, 1);
    }

    #[test]
    fn parse_patch_reads_plain_unified_diff_without_git_header() {
        let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";

        let files = parse_patch(patch);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_path(), "a.txt");
        assert_eq!(files[0].additions, 1);
        assert_eq!(files[0].deletions, 1);
    }

    #[test]
    fn plain_unified_file_headers_wait_for_completed_hunks() {
        let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n--- old marker\n+++ new marker\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-old\n+new\n";

        let files = parse_patch(patch);

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].display_path(), "a.txt");
        assert_eq!(files[0].hunks[0].lines[0].text, "-- old marker");
        assert_eq!(files[0].hunks[0].lines[1].text, "++ new marker");
        assert_eq!(files[1].display_path(), "b.txt");
    }

    #[test]
    fn parse_patch_dequotes_git_c_style_paths() {
        let patch = "diff --git \"a/name\\twith\\\"quote\\\\.txt\" \"b/name\\twith\\\"quote\\\\.txt\"\n--- \"a/name\\twith\\\"quote\\\\.txt\"\n+++ \"b/name\\twith\\\"quote\\\\.txt\"\n@@ -1 +1 @@\n-old\n+new\n";

        let files = parse_patch(patch);

        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].old_path.as_deref(),
            Some("name\twith\"quote\\.txt")
        );
        assert_eq!(
            files[0].new_path.as_deref(),
            Some("name\twith\"quote\\.txt")
        );
        assert_eq!(files[0].display_path(), "name\twith\"quote\\.txt");
    }

    #[test]
    fn parse_patch_dequotes_git_octal_utf8_paths() {
        let patch = "diff --git \"a/\\303\\251.txt\" \"b/\\303\\251.txt\"\n--- \"a/\\303\\251.txt\"\n+++ \"b/\\303\\251.txt\"\n@@ -1 +1 @@\n-old\n+new\n";

        let files = parse_patch(patch);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].old_path.as_deref(), Some("é.txt"));
        assert_eq!(files[0].new_path.as_deref(), Some("é.txt"));
        assert_eq!(files[0].display_path(), "é.txt");
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
            raw_patch: Vec::new(),
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
    fn render_bytes_preserves_non_utf8_git_diff_output() {
        let test_dir = temp_test_dir("non-utf8-diff");
        let repo = test_dir.join("repo");
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        init_repo(&repo);

        fs::write(repo.join("bytes.txt"), b"same\n\xff\n")
            .expect("non-UTF-8 base file should be written");
        git(["add", "bytes.txt"], &repo);
        git(["commit", "-q", "-m", "bytes"], &repo);
        fs::write(repo.join("bytes.txt"), b"same\n\xfe\n")
            .expect("non-UTF-8 worktree file should be written");

        let expected = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args([
                "diff",
                "--binary",
                "--no-ext-diff",
                "--no-color",
                "--find-renames",
                "--end-of-options",
                "HEAD",
            ])
            .output()
            .expect("git diff should run");
        assert!(
            expected.status.success(),
            "git diff failed: {}",
            String::from_utf8_lossy(&expected.stderr)
        );
        assert!(expected.stdout.contains(&0xff));
        assert!(expected.stdout.contains(&0xfe));

        let actual = render_bytes(DiffOptions {
            repo: Some(repo.clone()),
            include_untracked: false,
            ..DiffOptions::default()
        })
        .expect("diff bytes should render");

        assert_eq!(actual, expected.stdout);
        let error = render(DiffOptions {
            repo: Some(repo),
            include_untracked: false,
            ..DiffOptions::default()
        })
        .expect_err("text rendering should reject non-UTF-8 output");
        assert!(error.to_string().contains("not valid UTF-8"));
        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn patch_stdin_source_parses_stats_without_raw_patch_retention() {
        let patch = Arc::<[u8]>::from(
            b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1,2 @@\n-old\n+new\n+again\n".as_slice(),
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
        let patch = Arc::<[u8]>::from(
            b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n"
                .as_slice(),
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

    #[test]
    fn revision_operands_cannot_be_reinterpreted_as_git_diff_options() {
        let test_dir = temp_test_dir("revision-option-boundary");
        let repo = test_dir.join("repo");
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        init_repo(&repo);
        let output_path = test_dir.join("poc.diff");

        let result = render(DiffOptions {
            repo: Some(repo),
            source: DiffSource::Range {
                left: format!("--output={}", output_path.display()),
                right: "HEAD".to_owned(),
            },
            ..DiffOptions::default()
        });

        assert!(result.is_err());
        assert!(!output_path.exists());
        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    #[test]
    fn temp_index_paths_are_adjacent_to_source_index() {
        let index = PathBuf::from("/repo/.git/worktrees/feature/index");
        let temp = temp_index_path(&index, 0).expect("temp index path should resolve");

        assert_eq!(temp.parent(), index.parent());
        assert!(
            temp.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".hz-diff-index-")
        );
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
