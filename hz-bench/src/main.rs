use std::{
    collections::HashSet,
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

type BenchResult<T> = Result<T, BenchError>;

#[derive(Debug)]
enum BenchError {
    Io(io::Error),
    Json(serde_json::Error),
    Git { command: String, stderr: String },
    Usage(String),
}

impl fmt::Display for BenchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
            Self::Git { command, stderr } => {
                if stderr.trim().is_empty() {
                    write!(formatter, "git command failed: {command}")
                } else {
                    write!(
                        formatter,
                        "git command failed: {command}: {}",
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
#[command(name = "hz-bench", about = "hz local benchmark utilities")]
struct Cli {
    #[command(subcommand)]
    command: BenchCommand,
}

#[derive(Debug, Subcommand)]
enum BenchCommand {
    #[command(about = "Generate deterministic diff benchmark fixtures")]
    Fixtures(FixturesArgs),
}

#[derive(Debug, Parser)]
struct FixturesArgs {
    /// Output directory for generated fixture directories.
    #[arg(long, value_name = "DIR")]
    out: PathBuf,
    /// Scenario to generate. May be repeated. Defaults to the standard suite.
    #[arg(long, value_enum, value_name = "NAME")]
    scenario: Vec<ScenarioKind>,
    /// Generate the standard suite. This is also the default when no scenario is passed.
    #[arg(long)]
    all: bool,
    /// Include the larger stress scenario with --all or the default suite.
    #[arg(long)]
    stress: bool,
    /// Remove an existing scenario output directory before writing it.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum ScenarioKind {
    ManySmallFiles,
    BalancedChangeset,
    LargeSingleFile,
    ManyUntrackedSmall,
    FewUntrackedLarge,
    MinifiedOneLine,
    BinaryFiles,
    StagedUnstaged,
    HugeMixedStress,
}

impl ScenarioKind {
    fn name(self) -> &'static str {
        match self {
            Self::ManySmallFiles => "many-small-files",
            Self::BalancedChangeset => "balanced-changeset",
            Self::LargeSingleFile => "large-single-file",
            Self::ManyUntrackedSmall => "many-untracked-small",
            Self::FewUntrackedLarge => "few-untracked-large",
            Self::MinifiedOneLine => "minified-one-line",
            Self::BinaryFiles => "binary-files",
            Self::StagedUnstaged => "staged-unstaged",
            Self::HugeMixedStress => "huge-mixed-stress",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::ManySmallFiles => "Many small tracked files with localized edits.",
            Self::BalancedChangeset => "Medium file count with larger per-file edits.",
            Self::LargeSingleFile => "One large tracked file with a large changed region.",
            Self::ManyUntrackedSmall => "Tracked edits plus many small untracked files.",
            Self::FewUntrackedLarge => "Tracked edits plus a few large untracked files.",
            Self::MinifiedOneLine => "A pathological single-line minified file edit.",
            Self::BinaryFiles => "Binary modified and untracked files plus a small text edit.",
            Self::StagedUnstaged => "Separate staged, unstaged, mixed, and untracked changes.",
            Self::HugeMixedStress => "Large opt-in stress case for max-size and memory testing.",
        }
    }

    fn standard() -> &'static [Self] {
        &[
            Self::ManySmallFiles,
            Self::BalancedChangeset,
            Self::LargeSingleFile,
            Self::ManyUntrackedSmall,
            Self::FewUntrackedLarge,
            Self::MinifiedOneLine,
            Self::BinaryFiles,
            Self::StagedUnstaged,
        ]
    }
}

#[derive(Debug, Clone, Copy)]
struct TextShape {
    file_count: usize,
    lines: usize,
    changed_start: Option<usize>,
    changed_lines: usize,
    extension: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct UntrackedShape {
    file_count: usize,
    lines: usize,
    extension: &'static str,
}

#[derive(Debug, Default, Serialize)]
struct FixtureCounts {
    tracked_files: usize,
    untracked_files: usize,
    binary_files: usize,
    expected_text_additions: usize,
    expected_text_deletions: usize,
}

#[derive(Debug, Serialize)]
struct FixturePaths {
    repo: String,
    patch: String,
    head_patch: String,
    unstaged_patch: String,
    staged_patch: String,
    pair_before: String,
    pair_after: String,
}

#[derive(Debug, Serialize)]
struct FixtureManifest {
    version: u8,
    scenario: &'static str,
    description: &'static str,
    paths: FixturePaths,
    counts: FixtureCounts,
    patch_bytes: u64,
    head_patch_bytes: u64,
    unstaged_patch_bytes: u64,
    staged_patch_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
enum SourceVariant {
    Baseline,
    ChangedA,
    ChangedB,
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match cli.command {
        BenchCommand::Fixtures(args) => generate_fixtures(args)?,
    }
    Ok(())
}

fn generate_fixtures(args: FixturesArgs) -> BenchResult<()> {
    let scenarios = selected_scenarios(&args);
    fs::create_dir_all(&args.out)?;

    for scenario in scenarios {
        let manifest = generate_scenario(&args.out, scenario, args.force)?;
        println!(
            "generated {}: {} files, {} untracked, {} bytes patch",
            manifest.scenario,
            manifest.counts.tracked_files,
            manifest.counts.untracked_files,
            manifest.patch_bytes
        );
    }

    Ok(())
}

fn selected_scenarios(args: &FixturesArgs) -> Vec<ScenarioKind> {
    let mut selected = if args.scenario.is_empty() || args.all {
        ScenarioKind::standard().to_vec()
    } else {
        args.scenario.clone()
    };

    if args.stress && !selected.contains(&ScenarioKind::HugeMixedStress) {
        selected.push(ScenarioKind::HugeMixedStress);
    }

    let mut seen = HashSet::new();
    selected.retain(|scenario| seen.insert(*scenario));
    selected
}

fn generate_scenario(
    output_root: &Path,
    scenario: ScenarioKind,
    force: bool,
) -> BenchResult<FixtureManifest> {
    let scenario_dir = output_root.join(scenario.name());
    prepare_output_dir(&scenario_dir, force)?;

    let manifest = match scenario {
        ScenarioKind::ManySmallFiles => generate_tracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 240,
                lines: 72,
                changed_start: None,
                changed_lines: 12,
                extension: "ts",
            },
        )?,
        ScenarioKind::BalancedChangeset => generate_tracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 96,
                lines: 420,
                changed_start: None,
                changed_lines: 96,
                extension: "ts",
            },
        )?,
        ScenarioKind::LargeSingleFile => generate_tracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 1,
                lines: 32_000,
                changed_start: Some(8_000),
                changed_lines: 16_000,
                extension: "ts",
            },
        )?,
        ScenarioKind::ManyUntrackedSmall => generate_untracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 16,
                lines: 72,
                changed_start: None,
                changed_lines: 12,
                extension: "ts",
            },
            UntrackedShape {
                file_count: 120,
                lines: 36,
                extension: "ts",
            },
        )?,
        ScenarioKind::FewUntrackedLarge => generate_untracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 8,
                lines: 80,
                changed_start: None,
                changed_lines: 16,
                extension: "ts",
            },
            UntrackedShape {
                file_count: 6,
                lines: 5_000,
                extension: "ts",
            },
        )?,
        ScenarioKind::MinifiedOneLine => {
            generate_minified_one_line_scenario(&scenario_dir, scenario, 45_000)?
        }
        ScenarioKind::BinaryFiles => generate_binary_scenario(&scenario_dir, scenario)?,
        ScenarioKind::StagedUnstaged => generate_staged_unstaged_scenario(&scenario_dir, scenario)?,
        ScenarioKind::HugeMixedStress => generate_untracked_text_scenario(
            &scenario_dir,
            scenario,
            TextShape {
                file_count: 1_000,
                lines: 600,
                changed_start: None,
                changed_lines: 120,
                extension: "ts",
            },
            UntrackedShape {
                file_count: 500,
                lines: 160,
                extension: "ts",
            },
        )?,
    };

    write_manifest(&scenario_dir, &manifest)?;
    Ok(manifest)
}

fn prepare_output_dir(path: &Path, force: bool) -> BenchResult<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
        return Ok(());
    }

    if !force {
        return Err(BenchError::Usage(format!(
            "fixture output already exists: {} (pass --force to replace it)",
            path.display()
        )));
    }

    fs::remove_dir_all(path)?;
    fs::create_dir_all(path)?;
    Ok(())
}

fn generate_tracked_text_scenario(
    scenario_dir: &Path,
    scenario: ScenarioKind,
    shape: TextShape,
) -> BenchResult<FixtureManifest> {
    let repo = create_text_repo(scenario_dir, shape)?;
    write_pair_fixture(scenario_dir, shape, 9_999)?;
    let counts = FixtureCounts {
        tracked_files: shape.file_count,
        expected_text_additions: shape.file_count * shape.changed_lines,
        expected_text_deletions: shape.file_count * shape.changed_lines,
        ..FixtureCounts::default()
    };

    finish_manifest(scenario_dir, scenario, counts, &repo, &[])
}

fn generate_untracked_text_scenario(
    scenario_dir: &Path,
    scenario: ScenarioKind,
    tracked: TextShape,
    untracked: UntrackedShape,
) -> BenchResult<FixtureManifest> {
    let repo = create_text_repo(scenario_dir, tracked)?;
    let untracked_paths = add_untracked_text_files(&repo, untracked)?;
    write_pair_fixture(scenario_dir, tracked, 9_999)?;

    let counts = FixtureCounts {
        tracked_files: tracked.file_count,
        untracked_files: untracked.file_count,
        expected_text_additions: tracked.file_count * tracked.changed_lines
            + untracked.file_count * untracked.lines,
        expected_text_deletions: tracked.file_count * tracked.changed_lines,
        ..FixtureCounts::default()
    };

    finish_manifest(scenario_dir, scenario, counts, &repo, &untracked_paths)
}

fn generate_minified_one_line_scenario(
    scenario_dir: &Path,
    scenario: ScenarioKind,
    tokens: usize,
) -> BenchResult<FixtureManifest> {
    let repo = scenario_dir.join("repo");
    initialize_repo(&repo)?;

    let path = repo.join("src/bundle.min.js");
    write_file(
        &path,
        minified_source(tokens, SourceVariant::Baseline).as_bytes(),
    )?;
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-m", "initial benchmark fixture"])?;
    write_file(
        &path,
        minified_source(tokens, SourceVariant::ChangedA).as_bytes(),
    )?;

    let pair = scenario_dir.join("pair");
    write_file(
        &pair.join("before.js"),
        minified_source(tokens, SourceVariant::Baseline).as_bytes(),
    )?;
    write_file(
        &pair.join("after.js"),
        minified_source(tokens, SourceVariant::ChangedA).as_bytes(),
    )?;

    let counts = FixtureCounts {
        tracked_files: 1,
        expected_text_additions: 1,
        expected_text_deletions: 1,
        ..FixtureCounts::default()
    };

    finish_manifest(scenario_dir, scenario, counts, &repo, &[])
}

fn generate_binary_scenario(
    scenario_dir: &Path,
    scenario: ScenarioKind,
) -> BenchResult<FixtureManifest> {
    let repo = scenario_dir.join("repo");
    initialize_repo(&repo)?;

    write_file(
        &repo.join("src/readme.txt"),
        synthetic_source(1, SourceVariant::Baseline, 24, None, 6).as_bytes(),
    )?;
    write_file(&repo.join("bin/blob.dat"), &binary_blob(32 * 1024, 17))?;
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-m", "initial benchmark fixture"])?;

    write_file(
        &repo.join("src/readme.txt"),
        synthetic_source(1, SourceVariant::ChangedA, 24, None, 6).as_bytes(),
    )?;
    write_file(&repo.join("bin/blob.dat"), &binary_blob(32 * 1024, 91))?;
    write_file(
        &repo.join("bin/new-untracked.dat"),
        &binary_blob(64 * 1024, 143),
    )?;

    let pair = scenario_dir.join("pair");
    write_file(&pair.join("before.bin"), &binary_blob(8 * 1024, 1))?;
    write_file(&pair.join("after.bin"), &binary_blob(8 * 1024, 2))?;

    let counts = FixtureCounts {
        tracked_files: 2,
        untracked_files: 1,
        binary_files: 2,
        expected_text_additions: 6,
        expected_text_deletions: 6,
    };

    finish_manifest(
        scenario_dir,
        scenario,
        counts,
        &repo,
        &[PathBuf::from("bin/new-untracked.dat")],
    )
}

fn generate_staged_unstaged_scenario(
    scenario_dir: &Path,
    scenario: ScenarioKind,
) -> BenchResult<FixtureManifest> {
    let repo = scenario_dir.join("repo");
    initialize_repo(&repo)?;

    for (index, name) in ["staged", "unstaged", "mixed", "untouched"]
        .into_iter()
        .enumerate()
    {
        write_file(
            &repo.join(format!("src/{name}.ts")),
            synthetic_source(index + 1, SourceVariant::Baseline, 80, None, 12).as_bytes(),
        )?;
    }
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-m", "initial benchmark fixture"])?;

    write_file(
        &repo.join("src/staged.ts"),
        synthetic_source(1, SourceVariant::ChangedA, 80, None, 12).as_bytes(),
    )?;
    git(&repo, &["add", "src/staged.ts"])?;

    write_file(
        &repo.join("src/unstaged.ts"),
        synthetic_source(2, SourceVariant::ChangedA, 80, None, 12).as_bytes(),
    )?;

    write_file(
        &repo.join("src/mixed.ts"),
        synthetic_source(3, SourceVariant::ChangedA, 80, None, 12).as_bytes(),
    )?;
    git(&repo, &["add", "src/mixed.ts"])?;
    write_file(
        &repo.join("src/mixed.ts"),
        synthetic_source(3, SourceVariant::ChangedB, 80, None, 12).as_bytes(),
    )?;

    write_file(
        &repo.join("untracked/new.ts"),
        synthetic_source(4, SourceVariant::ChangedA, 40, None, 8).as_bytes(),
    )?;

    write_pair_fixture(
        scenario_dir,
        TextShape {
            file_count: 1,
            lines: 80,
            changed_start: None,
            changed_lines: 12,
            extension: "ts",
        },
        4_444,
    )?;

    let counts = FixtureCounts {
        tracked_files: 3,
        untracked_files: 1,
        expected_text_additions: 3 * 12 + 40,
        expected_text_deletions: 3 * 12,
        ..FixtureCounts::default()
    };

    finish_manifest(
        scenario_dir,
        scenario,
        counts,
        &repo,
        &[PathBuf::from("untracked/new.ts")],
    )
}

fn create_text_repo(scenario_dir: &Path, shape: TextShape) -> BenchResult<PathBuf> {
    let repo = scenario_dir.join("repo");
    initialize_repo(&repo)?;

    for index in 1..=shape.file_count {
        let relative = text_file_path(index, shape.extension);
        write_file(
            &repo.join(&relative),
            synthetic_source(
                index,
                SourceVariant::Baseline,
                shape.lines,
                shape.changed_start,
                shape.changed_lines,
            )
            .as_bytes(),
        )?;
    }

    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-m", "initial benchmark fixture"])?;

    for index in 1..=shape.file_count {
        let relative = text_file_path(index, shape.extension);
        write_file(
            &repo.join(&relative),
            synthetic_source(
                index,
                SourceVariant::ChangedA,
                shape.lines,
                shape.changed_start,
                shape.changed_lines,
            )
            .as_bytes(),
        )?;
    }

    Ok(repo)
}

fn add_untracked_text_files(repo: &Path, shape: UntrackedShape) -> BenchResult<Vec<PathBuf>> {
    let mut paths = Vec::with_capacity(shape.file_count);
    for index in 1..=shape.file_count {
        let relative = PathBuf::from(format!("untracked/new{index}.{}", shape.extension));
        write_file(
            &repo.join(&relative),
            synthetic_source(
                index,
                SourceVariant::ChangedA,
                shape.lines,
                None,
                shape.lines / 4,
            )
            .as_bytes(),
        )?;
        paths.push(relative);
    }
    Ok(paths)
}

fn write_pair_fixture(scenario_dir: &Path, shape: TextShape, file_index: usize) -> BenchResult<()> {
    let pair = scenario_dir.join("pair");
    write_file(
        &pair.join(format!("before.{}", shape.extension)),
        synthetic_source(
            file_index,
            SourceVariant::Baseline,
            shape.lines,
            shape.changed_start,
            shape.changed_lines,
        )
        .as_bytes(),
    )?;
    write_file(
        &pair.join(format!("after.{}", shape.extension)),
        synthetic_source(
            file_index,
            SourceVariant::ChangedA,
            shape.lines,
            shape.changed_start,
            shape.changed_lines,
        )
        .as_bytes(),
    )?;
    Ok(())
}

fn finish_manifest(
    scenario_dir: &Path,
    scenario: ScenarioKind,
    counts: FixtureCounts,
    repo: &Path,
    untracked_paths: &[PathBuf],
) -> BenchResult<FixtureManifest> {
    let head_patch = append_untracked_patches(
        git_diff(
            repo,
            &[
                "diff",
                "HEAD",
                "--binary",
                "--no-ext-diff",
                "--no-color",
                "--find-renames",
            ],
        )?,
        repo,
        untracked_paths,
    )?;
    let unstaged_patch = append_untracked_patches(
        git_diff(
            repo,
            &[
                "diff",
                "--binary",
                "--no-ext-diff",
                "--no-color",
                "--find-renames",
            ],
        )?,
        repo,
        untracked_paths,
    )?;
    let staged_patch = git_diff(
        repo,
        &[
            "diff",
            "--cached",
            "--binary",
            "--no-ext-diff",
            "--no-color",
            "--find-renames",
        ],
    )?;

    write_file(&scenario_dir.join("patch.diff"), head_patch.as_bytes())?;
    write_file(&scenario_dir.join("head.patch"), head_patch.as_bytes())?;
    write_file(
        &scenario_dir.join("unstaged.patch"),
        unstaged_patch.as_bytes(),
    )?;
    write_file(&scenario_dir.join("staged.patch"), staged_patch.as_bytes())?;

    Ok(FixtureManifest {
        version: 1,
        scenario: scenario.name(),
        description: scenario.description(),
        paths: FixturePaths {
            repo: "repo".to_owned(),
            patch: "patch.diff".to_owned(),
            head_patch: "head.patch".to_owned(),
            unstaged_patch: "unstaged.patch".to_owned(),
            staged_patch: "staged.patch".to_owned(),
            pair_before: pair_before_path(scenario_dir),
            pair_after: pair_after_path(scenario_dir),
        },
        counts,
        patch_bytes: head_patch.len() as u64,
        head_patch_bytes: head_patch.len() as u64,
        unstaged_patch_bytes: unstaged_patch.len() as u64,
        staged_patch_bytes: staged_patch.len() as u64,
    })
}

fn pair_before_path(scenario_dir: &Path) -> String {
    pair_file_path(scenario_dir, "before")
}

fn pair_after_path(scenario_dir: &Path) -> String {
    pair_file_path(scenario_dir, "after")
}

fn pair_file_path(scenario_dir: &Path, prefix: &str) -> String {
    let pair = scenario_dir.join("pair");
    let Ok(entries) = fs::read_dir(pair) else {
        return format!("pair/{prefix}.ts");
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(prefix) {
            return format!("pair/{name}");
        }
    }

    format!("pair/{prefix}.ts")
}

fn write_manifest(scenario_dir: &Path, manifest: &FixtureManifest) -> BenchResult<()> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    write_file(&scenario_dir.join("manifest.json"), &bytes)
}

fn append_untracked_patches(
    mut patch: String,
    repo: &Path,
    untracked_paths: &[PathBuf],
) -> BenchResult<String> {
    for relative in untracked_paths {
        let path = repo.join(relative);
        let bytes = fs::read(&path)?;
        if bytes.contains(&0) {
            append_separator(&mut patch);
            patch.push_str(&format!(
                "diff --git a/{path} b/{path}\nnew file mode 100644\nBinary files /dev/null and b/{path} differ\n",
                path = patch_path(relative)
            ));
            continue;
        }

        let text = String::from_utf8_lossy(&bytes);
        append_separator(&mut patch);
        patch.push_str(&new_file_patch(relative, &text));
    }
    Ok(patch)
}

fn append_separator(patch: &mut String) {
    if !patch.is_empty() && !patch.ends_with('\n') {
        patch.push('\n');
    }
}

fn new_file_patch(relative: &Path, contents: &str) -> String {
    let path = patch_path(relative);
    let lines: Vec<&str> = contents.lines().collect();
    let mut patch = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\nindex 0000000..0000000\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    patch
}

fn patch_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn initialize_repo(path: &Path) -> BenchResult<()> {
    fs::create_dir_all(path)?;
    git(path, &["init"])?;
    git(path, &["config", "user.name", "Benchmark User"])?;
    git(path, &["config", "user.email", "benchmark@example.com"])?;
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> BenchResult<String> {
    git_with_program(cwd, "git", args)
}

fn git_diff(cwd: &Path, args: &[&str]) -> BenchResult<String> {
    git_with_program(cwd, "git", args)
}

fn git_with_program(cwd: &Path, program: &str, args: &[&str]) -> BenchResult<String> {
    let output = Command::new(program).current_dir(cwd).args(args).output()?;
    if !output.status.success() {
        return Err(BenchError::Git {
            command: format!("{program} {}", args.join(" ")),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn text_file_path(index: usize, extension: &str) -> PathBuf {
    PathBuf::from(format!("src/bench{index}.{extension}"))
}

fn write_file(path: &Path, bytes: &[u8]) -> BenchResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn synthetic_source(
    file_index: usize,
    variant: SourceVariant,
    lines: usize,
    changed_start: Option<usize>,
    changed_lines: usize,
) -> String {
    let start = changed_start.unwrap_or(lines / 3).min(lines);
    let end = (start + changed_lines).min(lines);
    let mut text = String::new();

    for line_index in 0..lines {
        let line = line_index + 1;
        let in_changed_region = line_index >= start && line_index < end;
        if in_changed_region {
            match variant {
                SourceVariant::Baseline => text.push_str(&format!(
                    "export function bench{file_index}_{line}(value: number) {{ return value + {line}; }}\n"
                )),
                SourceVariant::ChangedA => text.push_str(&format!(
                    "export function bench{file_index}_{line}(value: number) {{ return value * {line} + {file_index}; }}\n"
                )),
                SourceVariant::ChangedB => text.push_str(&format!(
                    "export function bench{file_index}_{line}(value: number) {{ return value - {line} - {file_index}; }}\n"
                )),
            }
        } else {
            text.push_str(&format!(
                "export function bench{file_index}_{line}(value: number) {{ return value + {line}; }}\n"
            ));
        }
    }

    text
}

fn minified_source(tokens: usize, variant: SourceVariant) -> String {
    let mut text = String::from("const hzBenchBundle=[");
    for index in 0..tokens {
        if index > 0 {
            text.push(',');
        }
        match variant {
            SourceVariant::Baseline => text.push_str(&format!("\"token_{index}\"")),
            SourceVariant::ChangedA => text.push_str(&format!("\"token_{index}_changed\"")),
            SourceVariant::ChangedB => text.push_str(&format!("\"token_{index}_again\"")),
        }
    }
    text.push_str("];console.log(hzBenchBundle.length);");
    text
}

fn binary_blob(size: usize, seed: u8) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(size);
    for index in 0..size {
        bytes.push(seed.wrapping_add((index % 251) as u8));
    }
    if !bytes.is_empty() {
        bytes[0] = 0;
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_names_are_unique() {
        let scenarios = [
            ScenarioKind::ManySmallFiles,
            ScenarioKind::BalancedChangeset,
            ScenarioKind::LargeSingleFile,
            ScenarioKind::ManyUntrackedSmall,
            ScenarioKind::FewUntrackedLarge,
            ScenarioKind::MinifiedOneLine,
            ScenarioKind::BinaryFiles,
            ScenarioKind::StagedUnstaged,
            ScenarioKind::HugeMixedStress,
        ];
        let mut names = HashSet::new();
        for scenario in scenarios {
            assert!(names.insert(scenario.name()));
        }
    }

    #[test]
    fn synthetic_source_changes_only_requested_region() {
        let baseline = synthetic_source(1, SourceVariant::Baseline, 10, Some(3), 2);
        let changed = synthetic_source(1, SourceVariant::ChangedA, 10, Some(3), 2);
        let baseline_lines: Vec<_> = baseline.lines().collect();
        let changed_lines: Vec<_> = changed.lines().collect();

        for index in [0, 1, 2, 5, 6, 7, 8, 9] {
            assert_eq!(baseline_lines[index], changed_lines[index]);
        }
        assert_ne!(baseline_lines[3], changed_lines[3]);
        assert_ne!(baseline_lines[4], changed_lines[4]);
    }

    #[test]
    fn new_file_patch_uses_git_paths_and_addition_lines() {
        let patch = new_file_patch(Path::new("dir/file.ts"), "one\ntwo\n");
        assert!(patch.contains("diff --git a/dir/file.ts b/dir/file.ts"));
        assert!(patch.contains("@@ -0,0 +1,2 @@"));
        assert!(patch.contains("+one\n+two\n"));
    }
}
