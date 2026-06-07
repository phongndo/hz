#![allow(unused_imports)]

use crate::*;
use std::{
    collections::HashMap,
    env, fs,
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult};
use serde::{Deserialize, Serialize};

pub fn diff(input: DiffOptions) -> HzResult<String> {
    hz_diff::render(input)
}

pub fn diff_bytes(input: DiffOptions) -> HzResult<Vec<u8>> {
    hz_diff::render_bytes(input)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitHubPullRequest {
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) number: u64,
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
            patch: Arc::from(patch.into_boxed_slice()),
        }),
        scope: DiffScope::All,
        include_untracked: false,
        stat,
    })
}

pub(crate) fn github_pull_request_from_target(
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

pub(crate) fn local_github_pull_request(
    repo: Option<&Path>,
    number: u64,
) -> HzResult<GitHubPullRequest> {
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

pub(crate) fn github_pull_request_from_url(url: &str) -> Option<GitHubPullRequest> {
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

pub(crate) fn github_repo_from_remote_url(url: &str) -> Option<(String, String)> {
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

pub(crate) fn redact_url_userinfo(url: &str) -> String {
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

pub(crate) fn valid_github_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

pub(crate) fn github_pull_request_label(pull_request: &GitHubPullRequest) -> String {
    format!(
        "github pr {}/{}#{}",
        pull_request.owner, pull_request.repo, pull_request.number
    )
}

pub(crate) fn github_pull_request_diff_url(pull_request: &GitHubPullRequest) -> String {
    format!(
        "https://github.com/{}/{}/pull/{}.diff",
        pull_request.owner, pull_request.repo, pull_request.number
    )
}

pub(crate) fn fetch_github_pull_request_diff(
    pull_request: &GitHubPullRequest,
) -> HzResult<Vec<u8>> {
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

    Ok(output.stdout)
}

pub(crate) fn github_curl_config(url: &str, token: Option<&str>) -> String {
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

pub(crate) fn push_curl_config_value(config: &mut String, key: &str, value: &str) {
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

pub(crate) fn github_token() -> Option<String> {
    env::var("GH_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .or_else(|| {
            env::var("GITHUB_TOKEN")
                .ok()
                .filter(|token| !token.is_empty())
        })
}

pub(crate) fn github_fetch_error(
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
