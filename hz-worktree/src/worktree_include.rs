use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path},
};

use hz_core::{HzError, HzResult};

const WORKTREE_INCLUDE_FILE: &str = ".worktreeinclude";

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorktreeIncludePattern {
    pattern: String,
    negated: bool,
}

pub(crate) fn copy_worktree_includes(source: &Path, destination: &Path) -> HzResult<()> {
    let patterns = load_worktree_include(source)?;
    let mut has_include = false;
    let mut pathspecs = Vec::new();
    for pattern in &patterns {
        let Some(pathspec) = worktree_include_pathspec(pattern) else {
            continue;
        };
        has_include |= !pattern.negated;
        pathspecs.push(pathspec);
    }
    if !has_include {
        return Ok(());
    }

    let paths = hz_git::ignored_paths_matching(source, &pathspecs)?;

    for relative_path in paths {
        copy_included_path(source, destination, &relative_path)?;
    }

    Ok(())
}

fn load_worktree_include(repo: &Path) -> HzResult<Vec<WorktreeIncludePattern>> {
    let path = repo.join(WORKTREE_INCLUDE_FILE);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };

    Ok(parse_worktree_include(&contents))
}

fn parse_worktree_include(contents: &str) -> Vec<WorktreeIncludePattern> {
    contents
        .lines()
        .filter_map(parse_worktree_include_line)
        .collect()
}

fn parse_worktree_include_line(line: &str) -> Option<WorktreeIncludePattern> {
    let mut pattern = line.trim();
    if pattern.is_empty() || pattern.starts_with('#') {
        return None;
    }

    let negated = pattern.starts_with('!');
    if negated {
        pattern = pattern[1..].trim_start();
    } else if pattern.starts_with("\\#") || pattern.starts_with("\\!") {
        pattern = &pattern[1..];
    }

    if pattern.is_empty() {
        return None;
    }

    Some(WorktreeIncludePattern {
        pattern: pattern.to_owned(),
        negated,
    })
}

fn worktree_include_pathspec(pattern: &WorktreeIncludePattern) -> Option<String> {
    let mut body = pattern.pattern.as_str();
    let anchored = body.starts_with('/');
    body = body.trim_start_matches('/');
    let directory = body.ends_with('/');
    body = body.trim_end_matches('/');
    if body.is_empty() {
        return None;
    }

    let recursive = !anchored && !body.contains('/');
    let body = if directory {
        if recursive {
            format!("**/{body}/**")
        } else {
            format!("{body}/**")
        }
    } else if recursive {
        format!("**/{body}")
    } else {
        body.to_owned()
    };

    let mut magic = Vec::new();
    if !recursive {
        magic.push("top");
    }
    if recursive || directory || has_glob_meta(&body) {
        magic.push("glob");
    }
    if pattern.negated {
        magic.push("exclude");
    }

    if magic.is_empty() {
        Some(body)
    } else {
        Some(format!(":({}){body}", magic.join(",")))
    }
}

fn has_glob_meta(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[')
}

fn copy_included_path(source: &Path, destination: &Path, relative_path: &Path) -> HzResult<()> {
    if !is_safe_relative_path(relative_path) {
        return Err(HzError::Usage(format!(
            "ignored file path is not repository-relative: {}",
            relative_path.display()
        )));
    }

    let source_path = source.join(relative_path);
    let metadata = fs::symlink_metadata(&source_path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(());
    }

    let destination_path = destination.join(relative_path);
    if fs::symlink_metadata(&destination_path).is_ok() {
        return Ok(());
    }

    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source_path, destination_path)?;

    Ok(())
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn include_parser_ignores_blank_lines_and_comments() {
        assert_eq!(
            parse_worktree_include("\n# comment\n.env\n"),
            vec![WorktreeIncludePattern {
                pattern: ".env".to_owned(),
                negated: false,
            }]
        );
    }

    #[test]
    fn include_pathspecs_match_common_gitignore_shapes() {
        let patterns = parse_worktree_include(
            ".env\n*.pem\nconfig/\n/config/secrets.json\nlogs/*.json\n!.env.example\n",
        );

        let pathspecs = patterns
            .iter()
            .filter_map(worktree_include_pathspec)
            .collect::<Vec<_>>();

        assert_eq!(
            pathspecs,
            vec![
                ":(glob)**/.env",
                ":(glob)**/*.pem",
                ":(glob)**/config/**",
                ":(top)config/secrets.json",
                ":(top,glob)logs/*.json",
                ":(glob,exclude)**/.env.example",
            ]
        );
    }

    #[test]
    fn safe_relative_paths_reject_absolute_and_parent_components() {
        assert!(is_safe_relative_path(Path::new(".env")));
        assert!(is_safe_relative_path(Path::new("config/secrets.json")));
        assert!(!is_safe_relative_path(Path::new("../secrets.json")));
        assert!(!is_safe_relative_path(&PathBuf::from("/tmp/secrets.json")));
    }
}
