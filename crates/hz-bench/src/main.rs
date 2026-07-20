use std::{
    env,
    error::Error,
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use clap::{Parser, Subcommand};
use serde::Serialize;

type BenchResult<T> = Result<T, BenchError>;

#[derive(Debug)]
enum BenchError {
    Io(io::Error),
    Json(serde_json::Error),
    Command {
        command: String,
        status: Option<i32>,
        stderr: String,
    },
    Usage(String),
}

impl fmt::Display for BenchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
            Self::Command {
                command,
                status,
                stderr,
            } => {
                let status = status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "terminated by signal".to_owned());
                if stderr.trim().is_empty() {
                    write!(formatter, "command failed with status {status}: {command}")
                } else {
                    write!(
                        formatter,
                        "command failed with status {status}: {command}: {}",
                        stderr.trim()
                    )
                }
            }
            Self::Usage(message) => write!(formatter, "{message}"),
        }
    }
}

impl Error for BenchError {}

impl From<io::Error> for BenchError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for BenchError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Parser)]
#[command(name = "hz-bench", about = "hz headless benchmark utilities")]
struct Cli {
    #[command(subcommand)]
    command: BenchCommand,
}

#[derive(Debug, Subcommand)]
enum BenchCommand {
    #[command(about = "Benchmark end-to-end hz CLI commands against a synthetic repo")]
    Cmd(CmdArgs),
}

#[derive(Debug, Parser)]
struct CmdArgs {
    /// hz binary to benchmark. Defaults to target/debug/hz from the current repo.
    #[arg(long, value_name = "PATH", default_value = "target/debug/hz")]
    hz: PathBuf,
    /// Synthetic workspaces to create before measuring read-only commands.
    #[arg(long, default_value_t = 12)]
    workspaces: usize,
    /// Warmup runs per measured command.
    #[arg(long, default_value_t = 3)]
    warmup: usize,
    /// Measured iterations per command.
    #[arg(long, default_value_t = 10)]
    iterations: usize,
    /// Also measure create/remove command latency.
    #[arg(long)]
    mutating: bool,
    /// Use explicit byte-copy materialization instead of benchmarking native COW.
    #[arg(long)]
    portable: bool,
    /// Included synthetic repository files, in addition to the default README.
    #[arg(long, default_value_t = 0)]
    repo_files: usize,
    /// Synthetic files under target/ for evaluating filtered creation.
    #[arg(long, default_value_t = 0)]
    artifact_files: usize,
    /// Bytes written to each synthetic repository and artifact file.
    #[arg(long, default_value_t = 1024)]
    file_bytes: usize,
    /// Create benchmark workspaces with `hz new --filtered`.
    #[arg(long)]
    filtered: bool,
    /// Logical chain depth for an additional subtree-removal measurement.
    #[arg(long, default_value_t = 0, requires = "mutating")]
    remove_depth: usize,
    /// Keep the fixture directory at this path instead of using a temporary directory.
    #[arg(long, value_name = "DIR")]
    keep: Option<PathBuf>,
    /// Emit JSON instead of a human table.
    #[arg(long)]
    json: bool,
}

#[derive(Debug)]
struct Fixture {
    root: PathBuf,
    home: PathBuf,
    config_home: PathBuf,
    repo: PathBuf,
    targets: Vec<String>,
    keep: bool,
}

#[derive(Debug)]
struct CommandSpec {
    name: &'static str,
    args: Vec<OsString>,
}

#[derive(Debug, Serialize)]
struct CmdBenchReport {
    version: u8,
    hz: String,
    fixture_root: String,
    repo: String,
    workspaces: usize,
    repo_files: usize,
    artifact_files: usize,
    file_bytes: usize,
    remove_depth: usize,
    warmup: usize,
    iterations: usize,
    materialization: String,
    copy_mode: String,
    runs: Vec<CommandReport>,
}

#[derive(Debug, Serialize)]
struct CommandReport {
    name: String,
    iterations: usize,
    min_micros: u128,
    median_micros: u128,
    avg_micros: u128,
    p95_micros: u128,
    max_micros: u128,
    stdout_bytes: usize,
    stderr_bytes: usize,
    samples_micros: Vec<u128>,
}

#[derive(Debug, serde::Deserialize)]
struct CreatedWorkspaceJson {
    workspace: CreatedWorkspaceRecord,
}

#[derive(Debug, serde::Deserialize)]
struct CreatedWorkspaceRecord {
    handle: String,
    path: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match cli.command {
        BenchCommand::Cmd(args) => bench_cmd(args)?,
    }
    Ok(())
}

fn bench_cmd(args: CmdArgs) -> BenchResult<()> {
    if args.iterations == 0 {
        return Err(BenchError::Usage(
            "--iterations must be greater than zero".to_owned(),
        ));
    }

    let hz = resolve_hz_binary(&args.hz)?;
    let fixture = create_fixture(&hz, &args)?;
    let mut runs = Vec::new();

    for spec in read_only_command_specs(&fixture) {
        runs.push(measure_command(
            &hz,
            &fixture,
            spec,
            args.warmup,
            args.iterations,
        )?);
    }
    if args.mutating {
        runs.extend(measure_create_remove(
            &hz,
            &fixture,
            args.warmup,
            args.iterations,
            args.filtered,
        )?);
        if args.remove_depth > 0 {
            runs.push(measure_subtree_remove(
                &hz,
                &fixture,
                args.remove_depth,
                args.warmup,
                args.iterations,
                args.filtered,
            )?);
        }
    }

    let report = CmdBenchReport {
        version: 2,
        hz: hz.display().to_string(),
        fixture_root: fixture.root.display().to_string(),
        repo: fixture.repo.display().to_string(),
        workspaces: fixture.targets.len(),
        repo_files: args.repo_files,
        artifact_files: args.artifact_files,
        file_bytes: args.file_bytes,
        remove_depth: args.remove_depth,
        warmup: args.warmup,
        iterations: args.iterations,
        materialization: if args.portable { "copy" } else { "native_cow" }.to_owned(),
        copy_mode: if args.filtered { "filtered" } else { "all" }.to_owned(),
        runs,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_cmd_report(&report);
    }

    if !fixture.keep {
        cleanup_fixture(&hz, &fixture)?;
    }
    Ok(())
}

fn cleanup_fixture(hz: &Path, fixture: &Fixture) -> BenchResult<()> {
    let context = RunContext::new(hz, &fixture.home, &fixture.config_home, &fixture.repo);
    run_hz(
        &context,
        &[
            "remove".into(),
            "--at".into(),
            fixture.repo.as_os_str().to_owned(),
            "--force".into(),
            "--no-hooks".into(),
            "--json".into(),
        ],
    )?;
    run_hz(&context, &["gc".into(), "--json".into()])?;
    fs::remove_dir_all(&fixture.root)?;
    Ok(())
}

fn resolve_hz_binary(path: &Path) -> BenchResult<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    if !path.is_file() {
        return Err(BenchError::Usage(format!(
            "hz binary not found: {} (run `cargo build -p hz-cli` or pass --hz)",
            path.display()
        )));
    }
    Ok(path)
}

fn create_fixture(hz: &Path, args: &CmdArgs) -> BenchResult<Fixture> {
    let (root, keep) = match &args.keep {
        Some(path) => {
            if path.exists() {
                return Err(BenchError::Usage(format!(
                    "fixture directory already exists: {}",
                    path.display()
                )));
            }
            (path.clone(), true)
        }
        None => (unique_temp_dir("hz-bench-cmd")?, false),
    };
    let home = root.join("home");
    let config_home = root.join("config");
    let repo = root.join("repo");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&config_home)?;
    initialize_repo(&repo)?;

    write_file(&repo.join("README.md"), b"# hz bench\n")?;
    write_synthetic_files(&repo.join("src/hz-bench"), args.repo_files, args.file_bytes)?;
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-m", "initial"])?;
    write_synthetic_files(
        &repo.join("target/hz-bench"),
        args.artifact_files,
        args.file_bytes,
    )?;
    let mut init_args = vec!["init".into()];
    if args.portable {
        init_args.push("--copy".into());
    }
    run_hz(&RunContext::new(hz, &home, &config_home, &repo), &init_args)?;
    git(&repo, &["add", ".hz"])?;
    git(&repo, &["commit", "-m", "add hz lifecycle config"])?;

    let mut targets = Vec::with_capacity(args.workspaces);
    let context = RunContext::new(hz, &home, &config_home, &repo);
    for index in 0..args.workspaces {
        let target = format!("bench-{index:04}");
        let output = run_hz(&context, &new_workspace_args(&target, &repo, args.filtered))?;
        let created: CreatedWorkspaceJson = serde_json::from_slice(&output.stdout)?;
        if index == 0 {
            write_file(&created.workspace.path.join("dirty.txt"), b"dirty\n")?;
        }
        targets.push(target);
    }

    Ok(Fixture {
        root,
        home,
        config_home,
        repo,
        targets,
        keep,
    })
}

fn read_only_command_specs(fixture: &Fixture) -> Vec<CommandSpec> {
    let repo = fixture.repo.as_os_str().to_owned();
    let sample_target = fixture
        .targets
        .first()
        .cloned()
        .unwrap_or_else(|| "local".to_owned());

    vec![
        CommandSpec {
            name: "help",
            args: vec!["--help".into()],
        },
        CommandSpec {
            name: "shell-zsh",
            args: vec!["shell".into(), "zsh".into()],
        },
        CommandSpec {
            name: "list-human",
            args: vec!["list".into(), "--at".into(), repo.clone()],
        },
        CommandSpec {
            name: "list-json",
            args: vec!["list".into(), "--at".into(), repo.clone(), "--json".into()],
        },
        CommandSpec {
            name: "path-root",
            args: vec!["path".into(), "root".into(), "--at".into(), repo.clone()],
        },
        CommandSpec {
            name: "path-workspace",
            args: vec![
                "path".into(),
                sample_target.into(),
                "--at".into(),
                repo.clone(),
            ],
        },
        CommandSpec {
            name: "complete-targets",
            args: vec![
                "__complete".into(),
                "workspace-targets".into(),
                "--at".into(),
                repo.clone(),
            ],
        },
        CommandSpec {
            name: "complete-trash",
            args: vec![
                "__complete".into(),
                "trash-targets".into(),
                "--at".into(),
                repo,
            ],
        },
    ]
}

fn new_workspace_args(target: &str, repo: &Path, filtered: bool) -> Vec<OsString> {
    let mut args = vec![
        "new".into(),
        target.into(),
        "--from".into(),
        repo.as_os_str().to_owned(),
        "--no-hooks".into(),
        "--json".into(),
    ];
    if filtered {
        args.push("--filtered".into());
    }
    args
}

fn generated_workspace_args(repo: &Path, filtered: bool) -> Vec<OsString> {
    let mut args = vec![
        "new".into(),
        "--from".into(),
        repo.as_os_str().to_owned(),
        "--no-hooks".into(),
        "--json".into(),
    ];
    if filtered {
        args.push("--filtered".into());
    }
    args
}

fn measure_command(
    hz: &Path,
    fixture: &Fixture,
    spec: CommandSpec,
    warmup: usize,
    iterations: usize,
) -> BenchResult<CommandReport> {
    let context = RunContext::new(hz, &fixture.home, &fixture.config_home, &fixture.repo);
    for _ in 0..warmup {
        run_hz_os(&context, &spec.args)?;
    }

    let mut samples = Vec::with_capacity(iterations);
    let mut stdout_bytes = 0usize;
    let mut stderr_bytes = 0usize;
    for _ in 0..iterations {
        let start = Instant::now();
        let output = run_hz_os(&context, &spec.args)?;
        let elapsed = start.elapsed().as_micros();
        stdout_bytes = stdout_bytes.saturating_add(output.stdout.len());
        stderr_bytes = stderr_bytes.saturating_add(output.stderr.len());
        samples.push(elapsed);
    }

    Ok(command_report(
        spec.name,
        samples,
        stdout_bytes,
        stderr_bytes,
    ))
}

fn measure_create_remove(
    hz: &Path,
    fixture: &Fixture,
    warmup: usize,
    iterations: usize,
    filtered: bool,
) -> BenchResult<Vec<CommandReport>> {
    let context = RunContext::new(hz, &fixture.home, &fixture.config_home, &fixture.repo);
    for index in 0..warmup {
        create_and_remove(
            &context,
            &fixture.repo,
            &format!("bench-warmup-{index:04}"),
            filtered,
        )?;
        create_and_remove_path(
            &context,
            &fixture.repo,
            &format!("bench-path-warmup-{index:04}"),
            filtered,
        )?;
        create_generated_and_remove(&context, &fixture.repo, filtered)?;
    }

    let mut create_samples = Vec::with_capacity(iterations);
    let mut create_generated_samples = Vec::with_capacity(iterations);
    let mut remove_samples = Vec::with_capacity(iterations);
    let mut remove_path_samples = Vec::with_capacity(iterations);
    let mut create_stdout_bytes = 0usize;
    let mut create_stderr_bytes = 0usize;
    let mut create_generated_stdout_bytes = 0usize;
    let mut create_generated_stderr_bytes = 0usize;
    let mut remove_stdout_bytes = 0usize;
    let mut remove_stderr_bytes = 0usize;
    let mut remove_path_stdout_bytes = 0usize;
    let mut remove_path_stderr_bytes = 0usize;
    for index in 0..iterations {
        let target = format!("bench-mutate-{index:04}");
        let (create, create_elapsed, remove, remove_elapsed) =
            create_and_remove(&context, &fixture.repo, &target, filtered)?;
        create_stdout_bytes = create_stdout_bytes.saturating_add(create.stdout.len());
        create_stderr_bytes = create_stderr_bytes.saturating_add(create.stderr.len());
        remove_stdout_bytes = remove_stdout_bytes.saturating_add(remove.stdout.len());
        remove_stderr_bytes = remove_stderr_bytes.saturating_add(remove.stderr.len());
        create_samples.push(create_elapsed);
        remove_samples.push(remove_elapsed);

        let path_target = format!("bench-path-{index:04}");
        let (remove_path, remove_path_elapsed) =
            create_and_remove_path(&context, &fixture.repo, &path_target, filtered)?;
        remove_path_stdout_bytes =
            remove_path_stdout_bytes.saturating_add(remove_path.stdout.len());
        remove_path_stderr_bytes =
            remove_path_stderr_bytes.saturating_add(remove_path.stderr.len());
        remove_path_samples.push(remove_path_elapsed);

        let (create_generated, create_generated_elapsed) =
            create_generated_and_remove(&context, &fixture.repo, filtered)?;
        create_generated_stdout_bytes =
            create_generated_stdout_bytes.saturating_add(create_generated.stdout.len());
        create_generated_stderr_bytes =
            create_generated_stderr_bytes.saturating_add(create_generated.stderr.len());
        create_generated_samples.push(create_generated_elapsed);
    }

    Ok(vec![
        command_report(
            "create",
            create_samples,
            create_stdout_bytes,
            create_stderr_bytes,
        ),
        command_report(
            "create-generated",
            create_generated_samples,
            create_generated_stdout_bytes,
            create_generated_stderr_bytes,
        ),
        command_report(
            "remove",
            remove_samples,
            remove_stdout_bytes,
            remove_stderr_bytes,
        ),
        command_report(
            "remove-path-only",
            remove_path_samples,
            remove_path_stdout_bytes,
            remove_path_stderr_bytes,
        ),
    ])
}

fn create_and_remove(
    context: &RunContext<'_>,
    repo: &Path,
    target: &str,
    filtered: bool,
) -> BenchResult<(Output, u128, Output, u128)> {
    let create_start = Instant::now();
    let create = run_hz(context, &new_workspace_args(target, repo, filtered))?;
    let create_elapsed = create_start.elapsed().as_micros();
    let created: CreatedWorkspaceJson = serde_json::from_slice(&create.stdout)?;
    let remove_start = Instant::now();
    let remove = run_hz(
        context,
        &[
            "remove".into(),
            target.into(),
            "--at".into(),
            repo.as_os_str().to_owned(),
            "--no-hooks".into(),
            "--json".into(),
        ],
    )?;
    let remove_elapsed = remove_start.elapsed().as_micros();
    if created.workspace.path.exists() {
        return Err(BenchError::Usage(format!(
            "mutating benchmark did not remove {}",
            created.workspace.path.display()
        )));
    }
    Ok((create, create_elapsed, remove, remove_elapsed))
}

fn create_and_remove_path(
    context: &RunContext<'_>,
    repo: &Path,
    target: &str,
    filtered: bool,
) -> BenchResult<(Output, u128)> {
    let create = run_hz(context, &new_workspace_args(target, repo, filtered))?;
    let created: CreatedWorkspaceJson = serde_json::from_slice(&create.stdout)?;
    let remove_start = Instant::now();
    let remove = run_hz(
        context,
        &[
            "remove".into(),
            target.into(),
            "--at".into(),
            repo.as_os_str().to_owned(),
            "--path-only".into(),
        ],
    )?;
    let remove_elapsed = remove_start.elapsed().as_micros();
    if created.workspace.path.exists() {
        return Err(BenchError::Usage(format!(
            "path-only benchmark did not remove {}",
            created.workspace.path.display()
        )));
    }
    if remove.stdout.is_empty() {
        return Err(BenchError::Usage(
            "path-only removal did not print a navigation path".to_owned(),
        ));
    }
    Ok((remove, remove_elapsed))
}

fn create_generated_and_remove(
    context: &RunContext<'_>,
    repo: &Path,
    filtered: bool,
) -> BenchResult<(Output, u128)> {
    let create_start = Instant::now();
    let create = run_hz(context, &generated_workspace_args(repo, filtered))?;
    let create_elapsed = create_start.elapsed().as_micros();
    let created: CreatedWorkspaceJson = serde_json::from_slice(&create.stdout)?;
    run_hz(
        context,
        &[
            "remove".into(),
            created.workspace.handle.into(),
            "--at".into(),
            repo.as_os_str().to_owned(),
            "--no-hooks".into(),
            "--json".into(),
        ],
    )?;
    if created.workspace.path.exists() {
        return Err(BenchError::Usage(format!(
            "generated-handle benchmark did not remove {}",
            created.workspace.path.display()
        )));
    }
    Ok((create, create_elapsed))
}

fn measure_subtree_remove(
    hz: &Path,
    fixture: &Fixture,
    depth: usize,
    warmup: usize,
    iterations: usize,
    filtered: bool,
) -> BenchResult<CommandReport> {
    let context = RunContext::new(hz, &fixture.home, &fixture.config_home, &fixture.repo);
    let first_target = "bench-tree-0000";
    let mut source = fixture.repo.clone();
    for index in 0..depth {
        let target = format!("bench-tree-{index:04}");
        let created = run_hz(&context, &new_workspace_args(&target, &source, filtered))?;
        source = serde_json::from_slice::<CreatedWorkspaceJson>(&created.stdout)?
            .workspace
            .path;
    }

    let mut samples = Vec::with_capacity(iterations);
    let mut stdout_bytes = 0usize;
    let mut stderr_bytes = 0usize;
    for index in 0..warmup.saturating_add(iterations) {
        let start = Instant::now();
        let removed = run_hz(
            &context,
            &[
                "remove".into(),
                first_target.into(),
                "--at".into(),
                fixture.repo.as_os_str().to_owned(),
                "--no-hooks".into(),
                "--json".into(),
            ],
        )?;
        let elapsed = start.elapsed().as_micros();
        if index >= warmup {
            samples.push(elapsed);
            stdout_bytes = stdout_bytes.saturating_add(removed.stdout.len());
            stderr_bytes = stderr_bytes.saturating_add(removed.stderr.len());
        }
        run_hz(
            &context,
            &[
                "restore".into(),
                first_target.into(),
                "--at".into(),
                fixture.repo.as_os_str().to_owned(),
                "--json".into(),
            ],
        )?;
    }
    if !source.exists() {
        return Err(BenchError::Usage(format!(
            "subtree benchmark did not restore {}",
            source.display()
        )));
    }
    Ok(command_report(
        "remove-subtree",
        samples,
        stdout_bytes,
        stderr_bytes,
    ))
}

fn command_report(
    name: &str,
    samples: Vec<u128>,
    stdout_bytes: usize,
    stderr_bytes: usize,
) -> CommandReport {
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let min_micros = sorted.first().copied().unwrap_or(0);
    let max_micros = sorted.last().copied().unwrap_or(0);
    let median_micros = match sorted.as_slice() {
        [] => 0,
        values if values.len() % 2 == 1 => values[values.len() / 2],
        values => {
            let upper = values[values.len() / 2];
            let lower = values[values.len() / 2 - 1];
            lower + (upper - lower) / 2
        }
    };
    let p95_micros = percentile(&sorted, 95, 100);
    let total = samples.iter().copied().sum::<u128>();
    let avg_micros = if samples.is_empty() {
        0
    } else {
        total / samples.len() as u128
    };
    CommandReport {
        name: name.to_owned(),
        iterations: samples.len(),
        min_micros,
        median_micros,
        avg_micros,
        p95_micros,
        max_micros,
        stdout_bytes,
        stderr_bytes,
        samples_micros: samples,
    }
}

fn percentile(sorted: &[u128], numerator: usize, denominator: usize) -> u128 {
    if sorted.is_empty() || denominator == 0 {
        return 0;
    }
    let rank = sorted.len().saturating_mul(numerator).div_ceil(denominator);
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn print_cmd_report(report: &CmdBenchReport) {
    println!(
        "fixture={} repo={} workspaces={} repo_files={} artifact_files={} file_bytes={} remove_depth={} iterations={} materialization={} copy_mode={}",
        report.fixture_root,
        report.repo,
        report.workspaces,
        report.repo_files,
        report.artifact_files,
        report.file_bytes,
        report.remove_depth,
        report.iterations,
        report.materialization,
        report.copy_mode
    );
    println!(
        "{:<20} {:>6} {:>10} {:>10} {:>10} {:>10} {:>10} {:>11} {:>11}",
        "command", "runs", "minµs", "p50µs", "avgµs", "p95µs", "maxµs", "stdout", "stderr"
    );
    for run in &report.runs {
        println!(
            "{:<20} {:>6} {:>10} {:>10} {:>10} {:>10} {:>10} {:>11} {:>11}",
            run.name,
            run.iterations,
            run.min_micros,
            run.median_micros,
            run.avg_micros,
            run.p95_micros,
            run.max_micros,
            run.stdout_bytes,
            run.stderr_bytes
        );
    }
}

struct RunContext<'a> {
    hz: &'a Path,
    home: &'a Path,
    config_home: &'a Path,
    cwd: &'a Path,
}

impl<'a> RunContext<'a> {
    fn new(hz: &'a Path, home: &'a Path, config_home: &'a Path, cwd: &'a Path) -> Self {
        Self {
            hz,
            home,
            config_home,
            cwd,
        }
    }
}

fn run_hz(context: &RunContext<'_>, args: &[OsString]) -> BenchResult<Output> {
    run_command(context.hz, context, args)
}

fn run_hz_os(context: &RunContext<'_>, args: &[OsString]) -> BenchResult<Output> {
    run_hz(context, args)
}

fn run_command(program: &Path, context: &RunContext<'_>, args: &[OsString]) -> BenchResult<Output> {
    let output = Command::new(program)
        .args(args)
        .current_dir(context.cwd)
        .env("HOME", context.home)
        .env("XDG_CONFIG_HOME", context.config_home)
        .env("HZ_DATABASE", context.config_home.join("state.sqlite"))
        .env("HZ_ASCII", "1")
        .output()?;
    if output.status.success() {
        return Ok(output);
    }

    Err(BenchError::Command {
        command: display_command(program, args),
        status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn display_command(program: &Path, args: &[OsString]) -> String {
    let mut command = program.display().to_string();
    for arg in args {
        command.push(' ');
        command.push_str(&arg.to_string_lossy());
    }
    command
}

fn initialize_repo(path: &Path) -> BenchResult<()> {
    fs::create_dir_all(path)?;
    git(path, &["init"])?;
    git(path, &["config", "core.autocrlf", "false"])?;
    git(path, &["config", "core.eol", "lf"])?;
    git(path, &["config", "commit.gpgsign", "false"])?;
    git(path, &["config", "user.name", "Benchmark User"])?;
    git(path, &["config", "user.email", "benchmark@example.com"])?;
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> BenchResult<Output> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if output.status.success() {
        return Ok(output);
    }

    Err(BenchError::Command {
        command: format!("git {}", args.join(" ")),
        status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> BenchResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn write_synthetic_files(root: &Path, count: usize, file_bytes: usize) -> BenchResult<()> {
    if count == 0 {
        return Ok(());
    }
    let contents = vec![b'x'; file_bytes];
    for index in 0..count {
        write_file(
            &root
                .join(format!("{:05}", index / 100))
                .join(format!("file-{index:08}.dat")),
            &contents,
        )?;
    }
    Ok(())
}

fn unique_temp_dir(prefix: &str) -> BenchResult<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| BenchError::Usage(format!("system clock is before unix epoch: {error}")))?
        .as_nanos();
    let path = env::temp_dir().join(format!("{prefix}-{}-{timestamp}", std::process::id()));
    fs::create_dir_all(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_report_calculates_micros() {
        let report = command_report("list", vec![30, 10, 20], 9, 0);

        assert_eq!(report.min_micros, 10);
        assert_eq!(report.median_micros, 20);
        assert_eq!(report.avg_micros, 20);
        assert_eq!(report.p95_micros, 30);
        assert_eq!(report.max_micros, 30);
        assert_eq!(report.stdout_bytes, 9);
    }

    #[test]
    fn display_command_includes_args() {
        let command = display_command(
            Path::new("hz"),
            &["git".into(), "list".into(), "--json".into()],
        );

        assert_eq!(command, "hz git list --json");
    }
}
