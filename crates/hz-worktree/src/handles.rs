use std::{
    collections::HashSet,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use crate::same_path;
use crate::{Registry, WorktreeEntry, discover_entries};
use hz_core::{HzError, HzResult};

pub(crate) fn generate_unique_handle(registry: &Registry, repo: &Path) -> HzResult<String> {
    let targets = discover_entries(registry, repo)?
        .iter()
        .flat_map(worktree_targets)
        .collect();
    Ok(generate_unique_handle_from_seed_with_targets(
        handle_seed(),
        handle_space_size(),
        &targets,
    ))
}

#[cfg(test)]
pub(crate) fn generate_unique_handle_from_seed(
    registry: &Registry,
    repo: &Path,
    seed: u128,
) -> String {
    generate_unique_handle_from_seed_with_limit(registry, repo, seed, handle_space_size())
}

#[cfg(test)]
pub(crate) fn generate_unique_handle_from_seed_with_limit(
    registry: &Registry,
    repo: &Path,
    seed: u128,
    max_attempts: u128,
) -> String {
    let targets = registry
        .entries
        .iter()
        .filter(|entry| same_path(&entry.repo, repo))
        .flat_map(worktree_targets)
        .collect();
    generate_unique_handle_from_seed_with_targets(seed, max_attempts, &targets)
}

pub(crate) fn generate_unique_handle_from_seed_with_targets(
    seed: u128,
    max_attempts: u128,
    targets: &HashSet<String>,
) -> String {
    for attempt in 0..max_attempts {
        let handle = generate_handle_from_seed(seed, attempt);
        if !targets.contains(&handle) {
            return handle;
        }
    }

    let fallback = generate_handle_from_seed(seed, max_attempts);
    for suffix in 2.. {
        let handle = format!("{fallback}-{suffix}");
        if !targets.contains(&handle) {
            return handle;
        }
    }

    unreachable!("suffix search is unbounded")
}

pub(crate) fn worktree_targets(entry: &WorktreeEntry) -> Vec<String> {
    let mut targets = vec![entry.id.clone(), entry.handle.clone()];
    if let Some(branch) = &entry.branch {
        targets.push(branch.clone());
    }
    targets
}

pub(crate) const HANDLE_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
pub(crate) const HANDLE_LENGTH: usize = 4;

pub(crate) fn generate_handle_from_seed(seed: u128, attempt: u128) -> String {
    let mut offset = mixed_handle_offset(seed).wrapping_add(attempt) % handle_space_size();
    let mut handle = [0_u8; HANDLE_LENGTH];

    for character in handle.iter_mut().rev() {
        *character = HANDLE_ALPHABET[(offset % HANDLE_ALPHABET.len() as u128) as usize];
        offset /= HANDLE_ALPHABET.len() as u128;
    }

    String::from_utf8(handle.to_vec()).expect("handle alphabet should be valid UTF-8")
}

pub(crate) fn handle_seed() -> u128 {
    let mut bytes = [0_u8; 16];
    if getrandom::getrandom(&mut bytes).is_ok() {
        return u128::from_le_bytes(bytes);
    }

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

pub(crate) fn mixed_handle_offset(seed: u128) -> u128 {
    let mut value = seed;
    value ^= seed / HANDLE_ALPHABET.len() as u128;
    value ^= seed >> 32;
    value ^= seed >> 64;
    value
}

pub(crate) fn handle_space_size() -> u128 {
    (HANDLE_ALPHABET.len() as u128).pow(HANDLE_LENGTH as u32)
}

pub(crate) fn unix_now() -> HzResult<u64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| HzError::Usage(format!("system clock is before unix epoch: {error}")))?;
    Ok(duration.as_secs())
}

pub(crate) fn new_uuid_v4() -> HzResult<String> {
    let mut bytes = [0_u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|error| {
        HzError::Io(std::io::Error::other(format!(
            "failed to read random bytes: {error}"
        )))
    })?;

    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    ))
}
