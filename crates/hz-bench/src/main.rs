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
    /// Synthetic worktrees to create before measuring read-only commands.
    #[arg(long, default_value_t = 12)]
    worktrees: usize,
    /// Warmup runs per measured command.
    #[arg(long, default_value_t = 3)]
    warmup: usize,
    /// Measured iterations per command.
    #[arg(long, default_value_t = 10)]
    iterations: usize,
    /// Also measure create/remove command latency.
    #[arg(long)]
    mutating: bool,
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
    worktrees: usize,
    warmup: usize,
    iterations: usize,
    runs: Vec<CommandReport>,
}

#[derive(Debug, Serialize)]
struct CommandReport {
    name: String,
    iterations: usize,
    min_micros: u128,
    avg_micros: u128,
    max_micros: u128,
    stdout_bytes: usize,
    stderr_bytes: usize,
    samples_micros: Vec<u128>,
}

#[derive(Debug, serde::Deserialize)]
struct CreatedWorktreeJson {
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
        runs.push(measure_create_remove(
            &hz,
            &fixture,
            args.warmup,
            args.iterations,
        )?);
    }

    let report = CmdBenchReport {
        version: 1,
        hz: hz.display().to_string(),
        fixture_root: fixture.root.display().to_string(),
        repo: fixture.repo.display().to_string(),
        worktrees: fixture.targets.len(),
        warmup: args.warmup,
        iterations: args.iterations,
        runs,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_cmd_report(&report);
    }

    if !fixture.keep {
        fs::remove_dir_all(&fixture.root)?;
    }
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
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-m", "initial"])?;
    run_hz(
        &RunContext::new(hz, &home, &config_home, &repo),
        &["init".into()],
    )?;
    git(&repo, &["add", ".hz"])?;
    git(&repo, &["commit", "-m", "add hz lifecycle config"])?;

    let mut targets = Vec::with_capacity(args.worktrees);
    let context = RunContext::new(hz, &home, &config_home, &repo);
    for index in 0..args.worktrees {
        let target = format!("bench/{index:04}");
        let output = run_hz(
            &context,
            &[
                "new".into(),
                target.clone().into(),
                "--repo".into(),
                repo.as_os_str().to_owned(),
                "--max-branch-worktrees".into(),
                "0".into(),
                "--json".into(),
            ],
        )?;
        let created: CreatedWorktreeJson = serde_json::from_slice(&output.stdout)?;
        if index == 0 {
            write_file(&created.path.join("dirty.txt"), b"dirty\n")?;
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
            args: vec!["list".into(), "--repo".into(), repo.clone()],
        },
        CommandSpec {
            name: "list-json",
            args: vec![
                "list".into(),
                "--repo".into(),
                repo.clone(),
                "--json".into(),
            ],
        },
        CommandSpec {
            name: "path-local",
            args: vec!["path".into(), "local".into(), "--repo".into(), repo.clone()],
        },
        CommandSpec {
            name: "path-worktree",
            args: vec![
                "path".into(),
                sample_target.into(),
                "--repo".into(),
                repo.clone(),
            ],
        },
        CommandSpec {
            name: "complete-targets",
            args: vec![
                "__complete".into(),
                "worktree-targets".into(),
                "--repo".into(),
                repo.clone(),
            ],
        },
        CommandSpec {
            name: "complete-removable",
            args: vec![
                "__complete".into(),
                "removable-worktrees".into(),
                "--repo".into(),
                repo,
            ],
        },
    ]
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
) -> BenchResult<CommandReport> {
    let context = RunContext::new(hz, &fixture.home, &fixture.config_home, &fixture.repo);
    for index in 0..warmup {
        create_and_remove(&context, &fixture.repo, &format!("bench/warmup-{index:04}"))?;
    }

    let mut samples = Vec::with_capacity(iterations);
    let mut stdout_bytes = 0usize;
    let mut stderr_bytes = 0usize;
    for index in 0..iterations {
        let target = format!("bench/mutate-{index:04}");
        let start = Instant::now();
        let outputs = create_and_remove(&context, &fixture.repo, &target)?;
        let elapsed = start.elapsed().as_micros();
        stdout_bytes = stdout_bytes.saturating_add(outputs.0.stdout.len() + outputs.1.stdout.len());
        stderr_bytes = stderr_bytes.saturating_add(outputs.0.stderr.len() + outputs.1.stderr.len());
        samples.push(elapsed);
    }

    Ok(command_report(
        "create-remove",
        samples,
        stdout_bytes,
        stderr_bytes,
    ))
}

fn create_and_remove(
    context: &RunContext<'_>,
    repo: &Path,
    target: &str,
) -> BenchResult<(Output, Output)> {
    let create = run_hz(
        context,
        &[
            "new".into(),
            target.into(),
            "--repo".into(),
            repo.as_os_str().to_owned(),
            "--max-branch-worktrees".into(),
            "0".into(),
            "--json".into(),
        ],
    )?;
    let created: CreatedWorktreeJson = serde_json::from_slice(&create.stdout)?;
    let remove = run_hz(
        context,
        &[
            "remove".into(),
            target.into(),
            "--repo".into(),
            repo.as_os_str().to_owned(),
            "--force".into(),
            "--json".into(),
        ],
    )?;
    if created.path.exists() {
        return Err(BenchError::Usage(format!(
            "mutating benchmark did not remove {}",
            created.path.display()
        )));
    }
    Ok((create, remove))
}

fn command_report(
    name: &str,
    samples: Vec<u128>,
    stdout_bytes: usize,
    stderr_bytes: usize,
) -> CommandReport {
    let min_micros = samples.iter().copied().min().unwrap_or(0);
    let max_micros = samples.iter().copied().max().unwrap_or(0);
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
        avg_micros,
        max_micros,
        stdout_bytes,
        stderr_bytes,
        samples_micros: samples,
    }
}

fn print_cmd_report(report: &CmdBenchReport) {
    println!(
        "fixture={} repo={} worktrees={} iterations={}",
        report.fixture_root, report.repo, report.worktrees, report.iterations
    );
    println!(
        "{:<20} {:>6} {:>10} {:>10} {:>10} {:>11} {:>11}",
        "command", "runs", "minµs", "avgµs", "maxµs", "stdout", "stderr"
    );
    for run in &report.runs {
        println!(
            "{:<20} {:>6} {:>10} {:>10} {:>10} {:>11} {:>11}",
            run.name,
            run.iterations,
            run.min_micros,
            run.avg_micros,
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
        assert_eq!(report.avg_micros, 20);
        assert_eq!(report.max_micros, 30);
        assert_eq!(report.stdout_bytes, 9);
    }

    #[test]
    fn display_command_includes_args() {
        let command = display_command(Path::new("hz"), &["list".into(), "--json".into()]);

        assert_eq!(command, "hz list --json");
    }
}
