use std::{
    collections::HashMap,
    env, fs,
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::Arc,
};

use hz_core::{HzError, HzResult};
use serde::{Deserialize, Serialize};

pub use hz_diff::{DiffOptions, DiffScope, DiffSource, PatchSource};
pub use hz_syntax::{
    SyntaxAddResult, SyntaxAvailableFilter, SyntaxCleanResult, SyntaxDoctorReport,
    SyntaxLanguageStatus, SyntaxLimits, SyntaxMode, SyntaxRemoveResult, SyntaxSettings,
    SyntaxThemeConfig, SyntaxThemeSource, SyntaxUpdateResult,
};
pub use hz_worktree::{
    CreateWorktree, CreatedWorktree, FindWorktree, HandoffMode, HandoffWorktree, ListWorktrees,
    LocalWorktree, LocalWorktreeInfo, PathWorktree, RemoveWorktree, WorktreeEntry, WorktreeHandoff,
    WorktreeSource, WorktreeStatus,
};

const HZ_DIR: &str = ".hz";
const CONFIG_FILE: &str = "hz.toml";
const ENVIRONMENT_DIR: &str = "environment";
const SETUP_SCRIPT: &str = "setup";
const CLEANUP_SCRIPT: &str = "cleanup";

pub fn create_worktree(input: CreateWorktree) -> HzResult<CreatedWorktree> {
    hz_worktree::create(create_worktree_with_config_defaults(input)?)
}

pub fn create_worktree_with_lifecycle(
    input: CreateWorktree,
    run_setup: bool,
) -> HzResult<CreatedWorktree> {
    let created = create_worktree(input)?;
    if run_setup {
        let target = created_worktree_target(&created);
        run_lifecycle_for_path(&created.repo, &created.path, &target, LifecycleKind::Setup)?;
    }
    Ok(created)
}

pub fn path_worktree(input: PathWorktree) -> HzResult<hz_core::paths::WorktreeTarget> {
    hz_worktree::path(input)
}

pub fn handoff_worktree(input: HandoffWorktree) -> HzResult<WorktreeHandoff> {
    hz_worktree::handoff(with_configured_handoff_detached_limit(input)?)
}

pub fn list_worktrees(input: ListWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    hz_worktree::list(input)
}

pub fn list_worktree_targets(input: ListWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    hz_worktree::list_targets(input)
}

pub fn local_worktree(input: LocalWorktree) -> HzResult<LocalWorktreeInfo> {
    hz_worktree::local(input)
}

pub fn current_worktree_path(input: ListWorktrees) -> HzResult<PathBuf> {
    hz_worktree::current_path(input)
}

pub fn find_worktree(input: FindWorktree) -> HzResult<WorktreeEntry> {
    hz_worktree::find(input)
}

pub fn remove_worktree(input: RemoveWorktree) -> HzResult<WorktreeEntry> {
    hz_worktree::remove(input)
}

pub fn remove_found_worktree(entry: WorktreeEntry) -> HzResult<WorktreeEntry> {
    hz_worktree::remove_found(entry)
}

pub fn remove_found_worktree_with_force(
    entry: WorktreeEntry,
    force: bool,
) -> HzResult<WorktreeEntry> {
    hz_worktree::remove_found_with_force(entry, force)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitRepo {
    pub repo: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepoInit {
    pub repo: PathBuf,
    pub config_path: PathBuf,
    pub setup_path: PathBuf,
    pub cleanup_path: PathBuf,
    pub config_created: bool,
    pub setup_created: bool,
    pub cleanup_created: bool,
}

pub fn init_repo(input: InitRepo) -> HzResult<RepoInit> {
    let repo = config_repo(input.repo.as_deref())?;
    let config_path = config_path(&repo);
    let lifecycle_path = repo.join(HZ_DIR).join(ENVIRONMENT_DIR);
    let setup_path = lifecycle_path.join(SETUP_SCRIPT);
    let cleanup_path = lifecycle_path.join(CLEANUP_SCRIPT);

    let config_created = write_new_file(&config_path, default_config())?;
    let setup_created = write_new_script(&setup_path, default_setup_script())?;
    let cleanup_created = write_new_script(&cleanup_path, default_cleanup_script())?;

    Ok(RepoInit {
        repo,
        config_path,
        setup_path,
        cleanup_path,
        config_created,
        setup_created,
        cleanup_created,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleKind {
    Setup,
    Cleanup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLifecycle {
    pub target: Option<String>,
    pub repo: Option<PathBuf>,
    pub kind: LifecycleKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LifecycleRun {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub target: String,
    pub kind: LifecycleKind,
    pub configured: bool,
}

pub fn run_lifecycle(input: RunLifecycle) -> HzResult<LifecycleRun> {
    let target = input.target.unwrap_or_else(|| "local".to_owned());
    if target == "local" {
        let local = hz_worktree::local(LocalWorktree { repo: input.repo })?;
        return run_lifecycle_for_path(&local.repo, &local.path, "local", input.kind);
    }

    let worktree = hz_worktree::find(FindWorktree {
        target: target.clone(),
        repo: input.repo,
    })?;

    run_lifecycle_for_entry(&worktree, input.kind)
}

pub fn run_lifecycle_for_entry(
    entry: &WorktreeEntry,
    kind: LifecycleKind,
) -> HzResult<LifecycleRun> {
    run_lifecycle_for_path(&entry.repo, &entry.path, &worktree_target(entry), kind)
}

pub fn diff(input: DiffOptions) -> HzResult<String> {
    hz_diff::render(input)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHubPullRequest {
    owner: String,
    repo: String,
    number: u64,
}

pub fn github_pr_diff_options(
    repo: Option<PathBuf>,
    target: &str,
    stat: bool,
) -> HzResult<DiffOptions> {
    let pull_request = github_pull_request_from_target(repo.as_deref(), target)?;
    let label = github_pull_request_label(&pull_request);
    let patch = fetch_github_pull_request_diff(&pull_request)?;

    Ok(DiffOptions {
        repo,
        source: DiffSource::Patch(PatchSource::Text {
            label,
            patch: Arc::from(patch),
        }),
        scope: DiffScope::All,
        include_untracked: false,
        stat,
    })
}

fn github_pull_request_from_target(
    repo: Option<&Path>,
    target: &str,
) -> HzResult<GitHubPullRequest> {
    if let Ok(number) = target.parse::<u64>() {
        if number == 0 {
            return Err(HzError::Usage(
                "pull request number must be greater than zero".to_owned(),
            ));
        }

        return local_github_pull_request(repo, number);
    }

    github_pull_request_from_url(target).ok_or_else(|| {
        HzError::Usage("expected a pull request number or GitHub pull request URL".to_owned())
    })
}

fn local_github_pull_request(repo: Option<&Path>, number: u64) -> HzResult<GitHubPullRequest> {
    let root = hz_git::repository_root(repo)?;
    let remote_url = hz_git::remote_url(&root, "origin")?;
    let (owner, repo) = github_repo_from_remote_url(&remote_url).ok_or_else(|| {
        let remote_url = redact_url_userinfo(&remote_url);
        HzError::Usage(format!(
            "origin remote is not a GitHub repository URL: {remote_url}"
        ))
    })?;

    Ok(GitHubPullRequest {
        owner,
        repo,
        number,
    })
}

fn github_pull_request_from_url(url: &str) -> Option<GitHubPullRequest> {
    let url = url.trim();
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let path = without_scheme.strip_prefix("github.com/")?;
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut segments = path.split('/');
    let owner = segments.next()?;
    let repo = segments.next()?;
    if segments.next()? != "pull" {
        return None;
    }
    let number = segments.next()?.parse::<u64>().ok()?;
    if number == 0 || !valid_github_path_segment(owner) || !valid_github_path_segment(repo) {
        return None;
    }

    Some(GitHubPullRequest {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        number,
    })
}

fn github_repo_from_remote_url(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let path = if let Some(path) = url.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = url.strip_prefix("ssh://git@github.com/") {
        path
    } else {
        let without_scheme = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))?;
        let (authority, path) = without_scheme.split_once('/')?;
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        if host != "github.com" {
            return None;
        }

        path
    };
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or(path)
        .trim_end_matches('/');
    let mut segments = path.split('/');
    let owner = segments.next()?;
    let repo = segments.next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);

    if segments.next().is_some()
        || !valid_github_path_segment(owner)
        || !valid_github_path_segment(repo)
    {
        return None;
    }

    Some((owner.to_owned(), repo.to_owned()))
}

fn redact_url_userinfo(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_owned();
    };
    let authority_start = scheme_end + "://".len();
    let authority_end = url[authority_start..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |offset| authority_start + offset);
    let Some(at_offset) = url[authority_start..authority_end].rfind('@') else {
        return url.to_owned();
    };
    let at = authority_start + at_offset;
    format!("{}<redacted>@{}", &url[..authority_start], &url[at + 1..])
}

fn valid_github_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn github_pull_request_label(pull_request: &GitHubPullRequest) -> String {
    format!(
        "github pr {}/{}#{}",
        pull_request.owner, pull_request.repo, pull_request.number
    )
}

fn github_pull_request_diff_url(pull_request: &GitHubPullRequest) -> String {
    format!(
        "https://github.com/{}/{}/pull/{}.diff",
        pull_request.owner, pull_request.repo, pull_request.number
    )
}

fn fetch_github_pull_request_diff(pull_request: &GitHubPullRequest) -> HzResult<String> {
    let token = github_token();
    let config = github_curl_config(
        &github_pull_request_diff_url(pull_request),
        token.as_deref(),
    );
    let mut child = ProcessCommand::new("curl")
        .args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                HzError::Usage("curl is required to fetch GitHub pull requests".to_owned())
            } else {
                HzError::Io(error)
            }
        })?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| HzError::Usage("failed to open curl config stdin".to_owned()))?
        .write_all(config.as_bytes())?;
    drop(child.stdin.take());

    let output = child.wait_with_output().map_err(HzError::Io)?;

    if !output.status.success() {
        return Err(github_fetch_error(pull_request, &output, token.is_some()));
    }

    github_diff_from_stdout(output.stdout)
}

fn github_diff_from_stdout(stdout: Vec<u8>) -> HzResult<String> {
    String::from_utf8(stdout).map_err(|_| {
        HzError::Usage("GitHub pull request diff response was not valid UTF-8".to_owned())
    })
}

fn github_curl_config(url: &str, token: Option<&str>) -> String {
    let mut config = String::from("fail\nlocation\nsilent\nshow-error\n");
    push_curl_config_value(&mut config, "connect-timeout", "10");
    push_curl_config_value(&mut config, "max-time", "60");
    push_curl_config_value(&mut config, "header", "User-Agent: hz");
    if let Some(token) = token {
        push_curl_config_value(
            &mut config,
            "header",
            &format!("Authorization: Bearer {token}"),
        );
    }
    push_curl_config_value(&mut config, "url", url);
    config
}

fn push_curl_config_value(config: &mut String, key: &str, value: &str) {
    config.push_str(key);
    config.push_str(" = \"");
    for ch in value.chars() {
        match ch {
            '\\' => config.push_str("\\\\"),
            '"' => config.push_str("\\\""),
            '\n' => config.push_str("\\n"),
            '\r' => config.push_str("\\r"),
            '\t' => config.push_str("\\t"),
            _ => config.push(ch),
        }
    }
    config.push_str("\"\n");
}

fn github_token() -> Option<String> {
    env::var("GH_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .or_else(|| {
            env::var("GITHUB_TOKEN")
                .ok()
                .filter(|token| !token.is_empty())
        })
}

fn github_fetch_error(
    pull_request: &GitHubPullRequest,
    output: &std::process::Output,
    authenticated: bool,
) -> HzError {
    let status = output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| output.status.to_string());
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let mut message = format!(
        "failed to fetch GitHub pull request {}/{}#{}: curl exited with status {status}",
        pull_request.owner, pull_request.repo, pull_request.number
    );
    if !detail.is_empty() {
        message.push_str(&format!(": {detail}"));
    }
    if !authenticated {
        message.push_str(
            "; set GH_TOKEN or GITHUB_TOKEN for private repositories or higher rate limits",
        );
    }

    HzError::Usage(message)
}

pub fn syntax_add(languages: &[String]) -> HzResult<SyntaxAddResult> {
    hz_syntax::add_languages(languages)
}

pub fn syntax_update(languages: &[String], all: bool) -> HzResult<SyntaxUpdateResult> {
    hz_syntax::update_languages(languages, all)
}

pub fn syntax_remove(languages: &[String]) -> HzResult<SyntaxRemoveResult> {
    hz_syntax::remove_languages(languages)
}

pub fn syntax_statuses() -> HzResult<Vec<SyntaxLanguageStatus>> {
    hz_syntax::language_statuses()
}

pub fn syntax_available_languages(filter: SyntaxAvailableFilter) -> HzResult<Vec<String>> {
    hz_syntax::available_languages(filter)
}

pub fn syntax_clean_cache() -> HzResult<SyntaxCleanResult> {
    hz_syntax::clean_cache()
}

pub fn syntax_cache_dir() -> HzResult<String> {
    hz_syntax::cache_dir()
}

pub fn syntax_config_path() -> HzResult<PathBuf> {
    hz_syntax::config_path()
}

pub fn syntax_settings_path() -> HzResult<PathBuf> {
    hz_syntax::settings_path()
}

pub fn syntax_colorscheme_dir() -> HzResult<PathBuf> {
    hz_syntax::colorscheme_dir()
}

pub fn syntax_doctor() -> HzResult<SyntaxDoctorReport> {
    hz_syntax::doctor()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRepoConfig {
    pub repo: Option<PathBuf>,
}

pub fn load_repo_config(input: LoadRepoConfig) -> HzResult<HzConfig> {
    let repo = config_repo(input.repo.as_deref())?;
    HzConfig::load(&repo)
}

fn create_worktree_with_config_defaults(mut input: CreateWorktree) -> HzResult<CreateWorktree> {
    let needs_detached_limit =
        input.max_detached_worktrees.is_none() && creates_detached_worktree(&input);
    if input.base.is_none() || needs_detached_limit {
        let repo = config_repo(input.repo.as_deref())?;
        let config = HzConfig::load(&repo)?;
        if input.base.is_none()
            && let Some(base) = config.default_base()
        {
            input.base = Some(base.to_owned());
        }
        if needs_detached_limit {
            input.max_detached_worktrees = Some(config.max_detached_worktrees());
        }
    }

    Ok(input)
}

fn config_repo(repo: Option<&Path>) -> HzResult<PathBuf> {
    let current = hz_git::repository_root(repo)?;
    hz_git::main_worktree(&current)
}

fn run_lifecycle_for_path(
    repo: &Path,
    path: &Path,
    target: &str,
    kind: LifecycleKind,
) -> HzResult<LifecycleRun> {
    let config = HzConfig::load(path)?;
    let Some(command) = config.lifecycle_command(kind) else {
        return Ok(LifecycleRun {
            repo: repo.to_path_buf(),
            path: path.to_path_buf(),
            target: target.to_owned(),
            kind,
            configured: false,
        });
    };

    let mut stderr = io::stderr();
    run_lifecycle_command(repo, path, target, kind, command, &mut stderr)?;
    Ok(LifecycleRun {
        repo: repo.to_path_buf(),
        path: path.to_path_buf(),
        target: target.to_owned(),
        kind,
        configured: true,
    })
}

fn run_lifecycle_command(
    repo: &Path,
    worktree: &Path,
    target: &str,
    kind: LifecycleKind,
    argv: &[String],
    stdout: &mut impl Write,
) -> HzResult<()> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| HzError::Usage(format!("{} command cannot be empty", kind.label())))?;
    if program.is_empty() {
        return Err(HzError::Usage(format!(
            "{} command program cannot be empty",
            kind.label()
        )));
    }

    let program = lifecycle_program(worktree, program)?;
    let mut child = ProcessCommand::new(&program)
        .args(args)
        .current_dir(worktree)
        .env("HZ_REPO", repo)
        .env("HZ_WORKTREE", worktree)
        .env("HZ_TARGET", target)
        .env("HZ_LIFECYCLE", kind.label())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let Some(mut child_stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(HzError::Usage(
            "failed to open lifecycle command stdout".to_owned(),
        ));
    };
    if let Err(error) = io::copy(&mut child_stdout, stdout) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let status = child.wait()?;

    if !status.success() {
        return Err(HzError::Usage(format!(
            "{} command failed with status {}",
            kind.label(),
            status
        )));
    }

    Ok(())
}

fn lifecycle_program(worktree: &Path, program: &str) -> HzResult<PathBuf> {
    if looks_like_path(program) {
        let path = worktree.join(program);
        if !path.exists() {
            return Err(HzError::Usage(format!(
                "lifecycle command not found in worktree: {}",
                path.display()
            )));
        }
        return Ok(path);
    }

    Ok(PathBuf::from(program))
}

fn looks_like_path(program: &str) -> bool {
    program.contains('/') || program.contains('\\') || program == "." || program == ".."
}

fn worktree_target(entry: &WorktreeEntry) -> String {
    branch_or_handle(entry.branch.as_deref(), &entry.handle)
}

fn created_worktree_target(created: &CreatedWorktree) -> String {
    branch_or_handle(created.branch.as_deref(), &created.handle)
}

fn branch_or_handle(branch: Option<&str>, handle: &str) -> String {
    branch.unwrap_or(handle).to_owned()
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HzConfig {
    pub lifecycle: Option<LifecycleConfig>,
    pub worktree: Option<WorktreeConfig>,
    pub list: Option<ListConfig>,
    pub color: Option<ColorConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LifecycleConfig {
    pub setup: Option<Vec<String>>,
    pub cleanup: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorktreeConfig {
    pub default_base: Option<String>,
    pub max_detached: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListConfig {
    pub headers: Option<ListHeaders>,
    pub columns: Option<Vec<ListColumn>>,
    pub compact_columns: Option<Vec<ListColumn>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListHeaders {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListColumn {
    Marker,
    Target,
    Branch,
    Handle,
    Status,
    Base,
    Modified,
    Path,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ColorConfig {
    pub mode: Option<ColorMode>,
    pub scheme: Option<String>,
    #[serde(default)]
    pub schemes: HashMap<String, ColorSchemeConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ColorSchemeConfig {
    pub header: Option<String>,
    pub target: Option<String>,
    pub branch: Option<String>,
    pub handle: Option<String>,
    pub base: Option<String>,
    pub modified: Option<String>,
    pub path: Option<String>,
    pub clean: Option<String>,
    pub dirty: Option<String>,
    pub unknown: Option<String>,
    pub current: Option<String>,
    pub local: Option<String>,
}

impl HzConfig {
    pub fn default_base(&self) -> Option<&str> {
        self.worktree
            .as_ref()
            .and_then(|worktree| worktree.default_base.as_deref())
            .filter(|base| !base.is_empty())
    }

    fn load(worktree: &Path) -> HzResult<Self> {
        let path = config_path(worktree);
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path)?;
        toml::from_str(&contents)
            .map_err(|error| HzError::Usage(format!("failed to parse {}: {error}", path.display())))
    }

    fn lifecycle_command(&self, kind: LifecycleKind) -> Option<&[String]> {
        let lifecycle = self.lifecycle.as_ref()?;
        match kind {
            LifecycleKind::Setup => lifecycle.setup.as_deref(),
            LifecycleKind::Cleanup => lifecycle.cleanup.as_deref(),
        }
        .filter(|command| !command.is_empty())
    }

    fn max_detached_worktrees(&self) -> usize {
        self.worktree
            .as_ref()
            .and_then(|worktree| worktree.max_detached)
            .unwrap_or(hz_worktree::DEFAULT_MAX_DETACHED_WORKTREES)
    }
}

fn with_configured_handoff_detached_limit(mut input: HandoffWorktree) -> HzResult<HandoffWorktree> {
    if input.max_detached_worktrees.is_none()
        && input.mode == HandoffMode::Patch
        && input.create
        && input.target.is_none()
    {
        input.max_detached_worktrees = Some(configured_detached_limit(input.repo.as_deref())?);
    }
    Ok(input)
}

fn creates_detached_worktree(input: &CreateWorktree) -> bool {
    input.name.is_none() && input.branch.is_none()
}

fn configured_detached_limit(repo: Option<&Path>) -> HzResult<usize> {
    let repo = config_repo(repo)?;
    Ok(HzConfig::load(&repo)?.max_detached_worktrees())
}

fn config_path(repo: &Path) -> PathBuf {
    repo.join(HZ_DIR).join(CONFIG_FILE)
}

impl LifecycleKind {
    fn label(self) -> &'static str {
        match self {
            LifecycleKind::Setup => "setup",
            LifecycleKind::Cleanup => "cleanup",
        }
    }
}

fn write_new_file(path: &Path, contents: &str) -> HzResult<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(contents.as_bytes())?;
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn write_new_script(path: &Path, contents: &str) -> HzResult<bool> {
    let created = write_new_file(path, contents)?;
    if created {
        make_executable(path)?;
    }
    Ok(created)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> HzResult<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> HzResult<()> {
    Ok(())
}

fn default_config() -> &'static str {
    "[worktree]\nmax_detached = 15\n# default_base = \"dev\"\n\n[list]\nheaders = \"auto\"\ncolumns = [\"marker\", \"target\", \"status\", \"modified\", \"path\"]\n\n[color]\nmode = \"auto\"\nscheme = \"terminal\"\n\n[lifecycle]\nsetup = [\".hz/environment/setup\"]\ncleanup = [\".hz/environment/cleanup\"]\n"
}

fn default_setup_script() -> &'static str {
    "#!/usr/bin/env sh\nset -eu\n\n# Add repo setup commands here.\n"
}

fn default_cleanup_script() -> &'static str {
    "#!/usr/bin/env sh\nset -eu\n\n# Add repo cleanup commands here.\n"
}

pub fn shell_init_line(shell: Shell) -> &'static str {
    match shell {
        Shell::Zsh => r#"eval "$(hz shell zsh)""#,
        Shell::Bash => r#"eval "$(hz shell bash)""#,
        Shell::Fish => "hz shell fish | source",
    }
}

pub fn shell_init_comment() -> &'static str {
    "# hz shell integration"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellInit {
    pub path: PathBuf,
    pub line: &'static str,
    pub changed: bool,
}

pub fn install_shell_integration(shell: Shell) -> HzResult<ShellInit> {
    let path = shell_rc_path(shell)?;
    let line = shell_init_line(shell);
    let changed = install_line(&path, line)?;

    Ok(ShellInit {
        path,
        line,
        changed,
    })
}

pub fn shell_integration(shell: Shell) -> &'static str {
    match shell {
        Shell::Zsh => include_str!("shell/hz.zsh"),
        Shell::Bash => include_str!("shell/hz.bash"),
        Shell::Fish => include_str!("shell/hz.fish"),
    }
}

fn shell_rc_path(shell: Shell) -> HzResult<PathBuf> {
    shell_rc_path_from_env(
        shell,
        env_path("HOME"),
        env::var_os("ZDOTDIR").map(PathBuf::from),
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
    )
}

fn shell_rc_path_from_env(
    shell: Shell,
    home: Option<PathBuf>,
    zdotdir: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
) -> HzResult<PathBuf> {
    let home = non_empty_path(home);
    let zdotdir = non_empty_path(zdotdir);
    let xdg_config_home = non_empty_path(xdg_config_home);
    match shell {
        Shell::Zsh => {
            let dotdir = match zdotdir {
                Some(path) => path,
                None => require_home(home)?,
            };
            Ok(dotdir.join(".zshrc"))
        }
        Shell::Bash => Ok(require_home(home)?.join(".bashrc")),
        Shell::Fish => {
            let config_home = match xdg_config_home {
                Some(path) => path,
                None => require_home(home)?.join(".config"),
            };
            Ok(config_home.join("fish").join("config.fish"))
        }
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

fn non_empty_path(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|path| !path.as_os_str().is_empty())
}

fn require_home(home: Option<PathBuf>) -> HzResult<PathBuf> {
    home.ok_or_else(|| HzError::Usage("HOME is not set or empty".to_owned()))
}

fn install_line(path: &Path, line: &'static str) -> HzResult<bool> {
    let existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    if existing.lines().any(|existing_line| existing_line == line) {
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(shell_init_comment());
    next.push('\n');
    next.push_str(line);
    next.push('\n');

    fs::write(path, next)?;
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_repo_creates_config_and_lifecycle_scripts_once() {
        let test_dir = test_repo("hz-repo-init-test");

        let init = init_repo(InitRepo {
            repo: Some(test_dir.clone()),
        })
        .unwrap();

        assert!(init.config_created);
        assert!(init.setup_created);
        assert!(init.cleanup_created);
        assert_eq!(
            fs::read_to_string(&init.config_path).unwrap(),
            default_config()
        );
        assert!(
            fs::read_to_string(&init.setup_path)
                .unwrap()
                .contains("Add repo setup commands here.")
        );
        assert!(
            fs::read_to_string(&init.cleanup_path)
                .unwrap()
                .contains("Add repo cleanup commands here.")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                fs::metadata(&init.setup_path).unwrap().permissions().mode() & 0o111,
                0
            );
            assert_ne!(
                fs::metadata(&init.cleanup_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
        }

        let second = init_repo(InitRepo {
            repo: Some(test_dir.clone()),
        })
        .unwrap();
        assert!(!second.config_created);
        assert!(!second.setup_created);
        assert!(!second.cleanup_created);

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn init_repo_uses_main_worktree_for_linked_worktree() {
        let test_dir = test_repo("hz-repo-init-linked-test");
        commit_initial(&test_dir);
        let linked_dir = test_dir.with_file_name(format!(
            "{}-linked",
            test_dir.file_name().unwrap().to_string_lossy()
        ));
        let linked_arg = linked_dir.to_str().unwrap();
        git(
            &["worktree", "add", "-q", "--detach", linked_arg, "HEAD"],
            &test_dir,
        );

        let init = init_repo(InitRepo {
            repo: Some(linked_dir.clone()),
        })
        .unwrap();

        assert_eq!(
            fs::canonicalize(&init.repo).unwrap(),
            fs::canonicalize(&test_dir).unwrap()
        );
        assert_eq!(
            fs::canonicalize(init.config_path.parent().unwrap()).unwrap(),
            fs::canonicalize(test_dir.join(".hz")).unwrap()
        );
        assert!(init.config_created);
        assert!(!linked_dir.join(".hz").join("hz.toml").exists());

        git(&["worktree", "remove", "-f", linked_arg], &test_dir);
        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn init_repo_creates_hz_config_when_root_hz_toml_exists() {
        let test_dir = test_repo("hz-repo-init-root-config-test");
        fs::write(
            test_dir.join("hz.toml"),
            "[worktree]\ndefault_base = \"dev\"\n",
        )
        .unwrap();

        let init = init_repo(InitRepo {
            repo: Some(test_dir.clone()),
        })
        .unwrap();

        assert!(init.config_created);
        assert!(init.config_path.exists());
        assert!(init.setup_created);
        assert!(init.cleanup_created);

        let config = load_repo_config(LoadRepoConfig {
            repo: Some(test_dir.clone()),
        })
        .unwrap();
        assert_eq!(config.default_base(), None);

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn lifecycle_setup_runs_configured_script_in_worktree() {
        let test_dir = test_repo("hz-lifecycle-test");
        fs::create_dir_all(test_dir.join(".hz").join("environment")).unwrap();
        fs::write(
            test_dir.join(".hz").join(CONFIG_FILE),
            "[lifecycle]\nsetup = [\".hz/environment/setup\"]\n",
        )
        .unwrap();
        fs::write(
            test_dir.join(".hz").join("environment").join("setup"),
            "#!/usr/bin/env sh\nset -eu\nprintf '%s' \"$HZ_TARGET:$HZ_LIFECYCLE\" > lifecycle.out\n",
        )
        .unwrap();
        make_executable(&test_dir.join(".hz").join("environment").join("setup")).unwrap();

        let run = run_lifecycle(RunLifecycle {
            target: None,
            repo: Some(test_dir.clone()),
            kind: LifecycleKind::Setup,
        })
        .unwrap();

        assert!(run.configured);
        assert_eq!(run.target, "local");
        assert_eq!(
            fs::read_to_string(test_dir.join("lifecycle.out")).unwrap(),
            "local:setup"
        );

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn lifecycle_command_streams_stdout_to_sink() {
        let test_dir = test_repo("hz-lifecycle-stdout-test");
        fs::create_dir_all(test_dir.join(".hz").join("environment")).unwrap();
        let script = test_dir.join(".hz").join("environment").join("setup");
        fs::write(&script, "#!/usr/bin/env sh\nprintf 'hello stdout'\n").unwrap();
        make_executable(&script).unwrap();
        let mut stdout = Vec::new();

        run_lifecycle_command(
            &test_dir,
            &test_dir,
            "local",
            LifecycleKind::Setup,
            &[".hz/environment/setup".to_owned()],
            &mut stdout,
        )
        .unwrap();

        assert_eq!(String::from_utf8(stdout).unwrap(), "hello stdout");
        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn repo_config_loads_hz_directory_config() {
        let test_dir = test_repo("hz-config-test");
        fs::create_dir_all(test_dir.join(".hz")).unwrap();
        fs::write(
            test_dir.join(".hz").join("hz.toml"),
            "[worktree]\ndefault_base = \"dev\"\n",
        )
        .unwrap();

        let config = load_repo_config(LoadRepoConfig {
            repo: Some(test_dir.clone()),
        })
        .unwrap();

        assert_eq!(config.default_base(), Some("dev"));

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn repo_config_ignores_root_hz_toml() {
        let test_dir = test_repo("hz-config-root-test");
        fs::write(
            test_dir.join("hz.toml"),
            "[worktree]\ndefault_base = \"dev\"\n",
        )
        .unwrap();

        let config = load_repo_config(LoadRepoConfig {
            repo: Some(test_dir.clone()),
        })
        .unwrap();

        assert_eq!(config.default_base(), None);

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn create_worktree_defaults_base_from_repo_config() {
        let test_dir = test_repo("hz-create-default-base-test");
        fs::create_dir_all(test_dir.join(".hz")).unwrap();
        fs::write(
            test_dir.join(".hz").join("hz.toml"),
            "[worktree]\ndefault_base = \"dev\"\n",
        )
        .unwrap();

        let input = create_worktree_with_config_defaults(CreateWorktree {
            name: Some("feature/ui".to_owned()),
            repo: Some(test_dir.clone()),
            path: None,
            base: None,
            branch: None,
            max_detached_worktrees: None,
        })
        .unwrap();

        assert_eq!(input.base.as_deref(), Some("dev"));

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn create_worktree_keeps_explicit_base_over_repo_config() {
        let test_dir = test_repo("hz-create-explicit-base-test");
        fs::create_dir_all(test_dir.join(".hz")).unwrap();
        fs::write(
            test_dir.join(".hz").join("hz.toml"),
            "[worktree]\ndefault_base = \"dev\"\n",
        )
        .unwrap();

        let input = create_worktree_with_config_defaults(CreateWorktree {
            name: Some("feature/ui".to_owned()),
            repo: Some(test_dir.clone()),
            path: None,
            base: Some("main".to_owned()),
            branch: None,
            max_detached_worktrees: None,
        })
        .unwrap();

        assert_eq!(input.base.as_deref(), Some("main"));

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn lifecycle_is_noop_without_configured_command() {
        let test_dir = test_repo("hz-lifecycle-noop-test");

        let run = run_lifecycle(RunLifecycle {
            target: None,
            repo: Some(test_dir.clone()),
            kind: LifecycleKind::Cleanup,
        })
        .unwrap();

        assert!(!run.configured);
        assert_eq!(run.target, "local");

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn lifecycle_target_is_consistent_for_created_and_found_worktrees() {
        let created = CreatedWorktree {
            id: "id".to_owned(),
            name: "handle".to_owned(),
            handle: "handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/repo/../worktrees/handle"),
            branch: Some("feature/login".to_owned()),
            base: None,
            source: WorktreeSource::Managed,
            warnings: Vec::new(),
        };
        let found = WorktreeEntry {
            id: "id".to_owned(),
            handle: "handle".to_owned(),
            repo: PathBuf::from("/repo"),
            path: PathBuf::from("/repo/../worktrees/handle"),
            branch: Some("feature/login".to_owned()),
            base: None,
            source: WorktreeSource::Managed,
            created_at_unix: 0,
            modified_at_unix: 0,
            status: WorktreeStatus::Unknown,
        };

        assert_eq!(created_worktree_target(&created), "feature/login");
        assert_eq!(worktree_target(&found), "feature/login");

        let detached = CreatedWorktree {
            branch: None,
            ..created
        };
        assert_eq!(created_worktree_target(&detached), "handle");
    }

    #[test]
    fn github_pull_request_url_parses() {
        assert_eq!(
            github_pull_request_from_url("https://github.com/owner/repo/pull/123/files?plain=1"),
            Some(GitHubPullRequest {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
                number: 123,
            })
        );
        assert_eq!(
            github_pull_request_from_url("github.com/owner/repo/pull/456"),
            Some(GitHubPullRequest {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
                number: 456,
            })
        );

        assert_eq!(
            github_pull_request_from_url("https://example.com/owner/repo/pull/1"),
            None
        );
        assert_eq!(
            github_pull_request_from_url("https://github.com/owner/repo/issues/1"),
            None
        );
        assert_eq!(
            github_pull_request_from_url("https://github.com/owner/repo/pull/0"),
            None
        );
    }

    #[test]
    fn github_remote_url_parses_common_git_url_forms() {
        for remote in [
            "git@github.com:owner/repo.git",
            "ssh://git@github.com/owner/repo.git",
            "https://github.com/owner/repo.git",
            "https://github.com/owner/repo",
            "https://x-access-token:secret@github.com/owner/repo.git",
            "https://user:password@github.com/owner/repo",
        ] {
            assert_eq!(
                github_repo_from_remote_url(remote),
                Some(("owner".to_owned(), "repo".to_owned()))
            );
        }

        assert_eq!(
            github_repo_from_remote_url("https://example.com/owner/repo.git"),
            None
        );
        assert_eq!(
            github_repo_from_remote_url("https://github.com/owner"),
            None
        );
        assert_eq!(
            github_repo_from_remote_url("https://token@example.com/owner/repo.git"),
            None
        );
        assert_eq!(
            github_repo_from_remote_url("https://example.com/path@github.com/owner/repo.git"),
            None
        );
    }

    #[test]
    fn github_remote_error_redacts_url_userinfo() {
        let repo = test_repo("hz-github-pr-redact-origin-test");
        git(
            &[
                "remote",
                "add",
                "origin",
                "https://user:secret-token@example.com/owner/repo.git",
            ],
            &repo,
        );

        let error = github_pull_request_from_target(Some(&repo), "42")
            .expect_err("non-GitHub origin should fail");
        let message = error.to_string();

        assert!(message.contains("https://<redacted>@example.com/owner/repo.git"));
        assert!(!message.contains("secret-token"));
        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn github_curl_config_includes_timeouts_and_escapes_values() {
        let config = github_curl_config(
            "https://github.com/owner/repo/pull/1.diff",
            Some("tok\"en\n"),
        );

        assert!(config.contains("connect-timeout = \"10\"\n"));
        assert!(config.contains("max-time = \"60\"\n"));
        assert!(config.contains("header = \"User-Agent: hz\"\n"));
        assert!(config.contains("header = \"Authorization: Bearer tok\\\"en\\n\"\n"));
        assert!(config.contains("url = \"https://github.com/owner/repo/pull/1.diff\"\n"));
    }

    #[test]
    fn github_diff_stdout_rejects_invalid_utf8() {
        assert_eq!(github_diff_from_stdout(b"diff".to_vec()).unwrap(), "diff");

        let error = github_diff_from_stdout(vec![0xff]).unwrap_err();

        assert!(error.to_string().contains("valid UTF-8"));
    }

    #[test]
    fn github_pull_request_number_uses_origin_remote() {
        let repo = test_repo("hz-github-pr-origin-test");
        git(
            &["remote", "add", "origin", "git@github.com:owner/repo.git"],
            &repo,
        );

        let pull_request = github_pull_request_from_target(Some(&repo), "42")
            .expect("pull request should be inferred from origin");

        assert_eq!(
            pull_request,
            GitHubPullRequest {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
                number: 42,
            }
        );

        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn zsh_init_line_is_rc_file_friendly() {
        assert_eq!(shell_init_line(Shell::Zsh), r#"eval "$(hz shell zsh)""#);
    }

    #[test]
    fn shell_rc_paths_respect_zdotdir_and_ignore_empty_xdg_config_home() {
        let home = Some(PathBuf::from("/home/user"));

        assert_eq!(
            shell_rc_path_from_env(
                Shell::Zsh,
                home.clone(),
                Some(PathBuf::from("/tmp/zdotdir")),
                None,
            )
            .unwrap(),
            PathBuf::from("/tmp/zdotdir/.zshrc")
        );
        assert_eq!(
            shell_rc_path_from_env(Shell::Fish, home, None, Some(PathBuf::new())).unwrap(),
            PathBuf::from("/home/user/.config/fish/config.fish")
        );
    }

    #[test]
    fn shell_rc_paths_do_not_fall_back_to_empty_home() {
        assert_eq!(
            shell_rc_path_from_env(
                Shell::Zsh,
                Some(PathBuf::new()),
                Some(PathBuf::from("/tmp/zdotdir")),
                None,
            )
            .unwrap(),
            PathBuf::from("/tmp/zdotdir/.zshrc")
        );
        assert_eq!(
            shell_rc_path_from_env(
                Shell::Fish,
                Some(PathBuf::new()),
                None,
                Some(PathBuf::from("/tmp/config")),
            )
            .unwrap(),
            PathBuf::from("/tmp/config/fish/config.fish")
        );
        assert!(shell_rc_path_from_env(Shell::Bash, Some(PathBuf::new()), None, None).is_err());
    }

    #[test]
    fn zsh_integration_wraps_new_and_cd() {
        let script = shell_integration(Shell::Zsh);
        let hzlocal_completion = script
            .split("_hzlocal_completion() {")
            .nth(1)
            .and_then(|completion| completion.split("\n}").next())
            .expect("hzlocal completion function should exist");

        assert!(script.contains("command hz \"$@\" --path-only"));
        assert!(script.contains("alias hz='noglob _hz'"));
        assert!(script.contains("_hz() {"));
        assert!(script.contains("alias hzcd='noglob _hzcd'"));
        assert!(script.contains("_hzcd() {"));
        assert!(script.contains("alias hzlocal='noglob _hzlocal'"));
        assert!(script.contains("_hzlocal() {"));
        assert!(script.contains("handoff)"));
        assert!(script.contains("--json|--path-only|--help|-h|-j"));
        assert!(script.contains("builtin cd \"$hz_target_path\" || return"));
        assert!(script.contains("command hz __complete worktree-targets \"${complete_args[@]}\""));
        assert!(
            script.contains("command hz __complete removable-worktrees \"${complete_args[@]}\"")
        );
        assert!(script.contains("complete_args=(-r \"$repo\")"));
        assert!(script.contains("compdef _hz_completion hz _hz"));
        assert!(script.contains("compdef _hzcd_completion hzcd _hzcd"));
        assert!(script.contains("compdef _hzlocal_completion hzlocal _hzlocal"));
        assert!(script.contains("compadd -- -h --help -V --version"));
        assert!(script.contains("if [[ \"$PREFIX\" == -* ]]; then"));
        assert!(script.contains("_hz_complete_command_options \"$cmd\""));
        assert!(script.contains("_hz_complete_command_positionals \"$cmd\""));
        assert!(script.contains("_hz_complete_option_value \"$cmd\""));
        assert!(script.contains("_hz_git_refs"));
        assert!(script.contains("compinit -C"));
        assert!(script.contains("shift words"));
        assert!(script.contains("shift 2 words"));
        assert!(script.contains("'rm:remove one or more worktrees'"));
        assert!(script.contains("'install:install shell integration'"));
        assert!(script.contains("'update:update hz from GitHub releases'"));
        assert!(script.contains("'diff:review a git diff'"));
        assert!(script.contains("'ts:manage diff syntax highlighting languages'"));
        assert!(script.contains("'add:install and enable syntax highlighting languages'"));
        assert!(script.contains("'update:update cached syntax highlighting parsers'"));
        assert!(script.contains("--installed --enabled"));
        assert!(script.contains("--all -h --help"));
        assert!(!script.contains("tui:open the terminal UI"));
        assert!(script.contains("--no-setup"));
        assert!(script.contains("--no-cleanup"));
        assert!(script.contains("--max-detached"));
        assert!(script.contains("--pr"));
        assert!(script.contains("--patch"));
        assert!(script.contains("--staged"));
        assert!(script.contains("--unstaged"));
        assert!(script.contains("--no-untracked"));
        assert!(script.contains("--no-watch"));
        assert!(script.contains("--no-syntax"));
        assert!(hzlocal_completion.contains("_hz_complete_command_options cd"));
        assert!(!hzlocal_completion.contains("_hz_complete_command_positionals cd"));
    }

    #[test]
    fn fish_integration_passes_json_short_flag_through() {
        let script = shell_integration(Shell::Fish);

        assert!(script.contains("case --json --path-only --help -h -j"));
        assert!(script.contains("or return"));
        assert!(script.contains("command hz __complete worktree-targets -r \"$repo\""));
        assert!(script.contains("command hz __complete removable-worktrees -r \"$repo\""));
        assert!(script.contains("__hz_command_is"));
        assert!(script.contains("__hz_top_command_is update"));
        assert!(script.contains("__hz_diff_position_is_revision"));
        assert!(script.contains("__hz_complete_git_refs"));
        assert!(script.contains("complete -c hz -n \"__hz_command_is remove rm\""));
        assert!(script.contains("init install setup cleanup shell update"));
        assert!(script.contains("ts tree-sitter"));
        assert!(script.contains("__hz_needs_ts_subcommand"));
        assert!(script.contains("add update rm remove list available clean path doctor"));
        assert!(script.contains("-l installed"));
        assert!(script.contains("-l enabled"));
        assert!(script.contains("-l all"));
        assert!(script.contains("-l no-setup"));
        assert!(script.contains("-l no-cleanup"));
        assert!(script.contains("-l max-detached"));
        assert!(script.contains("-l target-version"));
        assert!(script.contains("-l pr"));
        assert!(script.contains("-l patch"));
        assert!(script.contains("-l staged"));
        assert!(script.contains("-l unstaged"));
        assert!(script.contains("-l no-untracked"));
        assert!(script.contains("-l no-watch"));
        assert!(script.contains("-l no-syntax"));
        assert!(!script.contains("tui"));
    }

    #[test]
    fn bash_integration_registers_completion() {
        let script = shell_integration(Shell::Bash);
        let worktree_completion = script
            .split("if [[ \"$cmd\" == \"worktree\" || \"$cmd\" == \"wt\" ]]; then")
            .nth(1)
            .and_then(|completion| completion.split("if [[ \"$cmd\" == \"ts\"").next())
            .expect("worktree completion branch should exist");
        let ts_completion = script
            .split("if [[ \"$cmd\" == \"ts\" || \"$cmd\" == \"tree-sitter\" ]]; then")
            .nth(1)
            .and_then(|completion| {
                completion
                    .split("_hz_complete_command_args \"$cmd\"")
                    .next()
            })
            .expect("tree-sitter completion branch should exist");

        assert!(script.contains("complete -F _hz_completion hz"));
        assert!(script.contains("_hz_dynamic_reply worktree-targets"));
        assert!(script.contains("_hz_dynamic_reply removable-worktrees"));
        assert!(script.contains("command hz __complete \"$command\" -r \"$repo\""));
        assert!(script.contains("for ((index = 1; index < COMP_CWORD; index++))"));
        assert!(script.contains("_hz_complete_option_value"));
        assert!(script.contains("_hz_git_ref_reply"));
        assert!(script.contains("-b|--branch"));
        assert!(script.contains("init install setup cleanup shell update"));
        assert!(script.contains("ts tree-sitter"));
        assert!(script.contains("_hz_complete_ts_args"));
        assert!(script.contains("add update rm remove list available clean path doctor"));
        assert!(script.contains("--installed --enabled"));
        assert!(script.contains("--all -h --help"));
        assert!(script.contains("--no-setup"));
        assert!(script.contains("--no-cleanup"));
        assert!(script.contains("--max-detached"));
        assert!(script.contains("--target-version"));
        assert!(script.contains("--pr"));
        assert!(script.contains("--patch"));
        assert!(script.contains("--staged"));
        assert!(script.contains("--unstaged"));
        assert!(script.contains("--no-untracked"));
        assert!(script.contains("--no-watch"));
        assert!(script.contains("--no-syntax"));
        assert!(
            worktree_completion
                .contains("_hz_complete_command_args \"${COMP_WORDS[2]}\" \"$current\"")
        );
        assert!(ts_completion.contains("_hz_complete_ts_args \"${COMP_WORDS[2]}\" \"$current\""));
        assert!(!script.contains("tui"));
    }

    #[test]
    fn installs_line_once() {
        let test_dir = env::temp_dir().join(format!(
            "hz-init-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        let rc_file = test_dir.join(".zshrc");

        assert!(install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());
        assert!(!install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());

        let contents = fs::read_to_string(&rc_file).unwrap();
        assert_eq!(contents.matches(shell_init_line(Shell::Zsh)).count(), 1);
        assert_eq!(contents.matches(shell_init_comment()).count(), 1);

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn does_not_duplicate_existing_bare_line() {
        let test_dir = env::temp_dir().join(format!(
            "hz-init-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        let rc_file = test_dir.join(".zshrc");
        fs::create_dir_all(&test_dir).unwrap();
        fs::write(&rc_file, format!("{}\n", shell_init_line(Shell::Zsh))).unwrap();

        assert!(!install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());

        let contents = fs::read_to_string(&rc_file).unwrap();
        assert_eq!(contents.matches(shell_init_line(Shell::Zsh)).count(), 1);
        assert_eq!(contents.matches(shell_init_comment()).count(), 0);

        fs::remove_dir_all(test_dir).unwrap();
    }

    fn test_repo(prefix: &str) -> PathBuf {
        let test_dir = env::temp_dir().join(format!(
            "{prefix}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&test_dir).unwrap();
        let status = ProcessCommand::new("git")
            .arg("init")
            .arg("-q")
            .arg(&test_dir)
            .status()
            .unwrap();
        assert!(status.success());
        test_dir
    }

    fn commit_initial(repo: &Path) {
        git(&["config", "user.email", "test@example.com"], repo);
        git(&["config", "user.name", "Test"], repo);
        fs::write(repo.join("file.txt"), "base\n").unwrap();
        git(&["add", "file.txt"], repo);
        git(&["commit", "-q", "-m", "init"], repo);
    }

    fn git(args: &[&str], cwd: &Path) {
        let output = ProcessCommand::new("git")
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
}
