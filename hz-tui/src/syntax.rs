use std::{
    collections::{HashMap, HashSet, VecDeque, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    panic::{self, AssertUnwindSafe},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use crate::{
    MAX_INLINE_DIFF_LINE_BYTES, MAX_INLINE_DIFF_TOKENS, SYNTAX_THEME_ID, SyntaxBenchmarkReport,
};
use hz_core::HzResult;
use hz_diff::{Changeset, DiffLine, DiffLineKind, DiffOptions, DiffScope, DiffSource};
use hz_syntax::{
    HighlightedLine, SyntaxHighlighter, SyntaxLanguageSet, SyntaxLimits, SyntaxSettings,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DiffSide {
    Old,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SyntaxPosition {
    pub(crate) generation: u64,
    pub(crate) file: usize,
    pub(crate) hunk: usize,
    pub(crate) side: DiffSide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SyntaxSourceKind {
    HunkSide { hunk: usize },
    FullFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SyntaxSourceId {
    pub(crate) generation: u64,
    pub(crate) file: usize,
    pub(crate) side: DiffSide,
    pub(crate) kind: SyntaxSourceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SyntaxKey {
    pub(crate) source: SyntaxSourceId,
    pub(crate) language_hash: u64,
    pub(crate) theme_id: u64,
}

impl SyntaxKey {
    pub(crate) fn generation(self) -> u64 {
        self.source.generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InlineHunkKey {
    pub(crate) generation: u64,
    pub(crate) file: usize,
    pub(crate) hunk: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlineRange {
    pub(crate) byte_start: usize,
    pub(crate) byte_end: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InlineLineEmphasis {
    pub(crate) ranges: Vec<InlineRange>,
}

#[derive(Debug)]
pub(crate) struct InlineHunkEmphasisCache {
    pub(crate) lines: Vec<Option<InlineLineEmphasis>>,
    pub(crate) blocks: Vec<InlineChangedBlock>,
}

#[derive(Debug)]
pub(crate) struct InlineChangedBlock {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) deletions: Vec<usize>,
    pub(crate) additions: Vec<usize>,
}

impl InlineHunkEmphasisCache {
    pub(crate) fn new(lines: &[DiffLine]) -> Self {
        let mut blocks = Vec::new();
        let mut index = 0usize;

        while index < lines.len() {
            if !matches!(
                lines[index].kind,
                DiffLineKind::Deletion | DiffLineKind::Addition
            ) {
                index += 1;
                continue;
            }

            let start = index;
            let mut deletions = Vec::new();
            let mut additions = Vec::new();
            while index < lines.len()
                && matches!(
                    lines[index].kind,
                    DiffLineKind::Deletion | DiffLineKind::Addition
                )
            {
                match lines[index].kind {
                    DiffLineKind::Deletion => deletions.push(index),
                    DiffLineKind::Addition => additions.push(index),
                    DiffLineKind::Context | DiffLineKind::Meta => {}
                }
                index += 1;
            }
            blocks.push(InlineChangedBlock {
                start,
                end: index,
                deletions,
                additions,
            });
        }

        Self {
            lines: vec![None; lines.len()],
            blocks,
        }
    }

    pub(crate) fn ranges_for_line(&mut self, lines: &[DiffLine], line: usize) -> Vec<InlineRange> {
        if let Some(Some(emphasis)) = self.lines.get(line) {
            return emphasis.ranges.clone();
        }

        self.compute_line(lines, line);
        self.lines
            .get(line)
            .and_then(|emphasis| emphasis.as_ref())
            .map(|emphasis| emphasis.ranges.clone())
            .unwrap_or_default()
    }

    pub(crate) fn compute_line(&mut self, lines: &[DiffLine], line: usize) {
        let Some(diff_line) = lines.get(line) else {
            return;
        };
        if !matches!(
            diff_line.kind,
            DiffLineKind::Deletion | DiffLineKind::Addition
        ) {
            self.set_emphasis(line, Vec::new());
            return;
        }

        let Some(block) = self
            .blocks
            .iter()
            .find(|block| line >= block.start && line < block.end)
        else {
            self.set_emphasis(line, Vec::new());
            return;
        };

        if block.deletions.is_empty() || block.additions.is_empty() {
            let (start, end) = (block.start, block.end);
            for line in start..end {
                self.set_emphasis(line, Vec::new());
            }
            return;
        }

        let (old_index, new_index) = match diff_line.kind {
            DiffLineKind::Deletion => {
                let Ok(pair_index) = block.deletions.binary_search(&line) else {
                    self.set_emphasis(line, Vec::new());
                    return;
                };
                let Some(new_index) = block.additions.get(pair_index).copied() else {
                    self.set_emphasis(line, Vec::new());
                    return;
                };
                (line, new_index)
            }
            DiffLineKind::Addition => {
                let Ok(pair_index) = block.additions.binary_search(&line) else {
                    self.set_emphasis(line, Vec::new());
                    return;
                };
                let Some(old_index) = block.deletions.get(pair_index).copied() else {
                    self.set_emphasis(line, Vec::new());
                    return;
                };
                (old_index, line)
            }
            DiffLineKind::Context | DiffLineKind::Meta => unreachable!(),
        };

        let (old_ranges, new_ranges) =
            changed_token_ranges(&lines[old_index].text, &lines[new_index].text);
        self.set_emphasis(old_index, old_ranges);
        self.set_emphasis(new_index, new_ranges);
    }

    pub(crate) fn set_emphasis(&mut self, line: usize, ranges: Vec<InlineRange>) {
        if let Some(emphasis) = self.lines.get_mut(line) {
            *emphasis = Some(InlineLineEmphasis { ranges });
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntaxSkipReason {
    InvalidPosition,
    NoPath,
    NoLanguage,
    NoSource,
    TooLarge,
    QueueClosed,
    HighlightError,
}

#[derive(Debug, Clone)]
pub(crate) struct HighlightedSide {
    pub(crate) lines: Vec<HighlightedLine>,
}

impl HighlightedSide {
    pub(crate) fn memory_bytes(&self) -> usize {
        self.lines
            .iter()
            .flat_map(|line| line.segments.iter())
            .map(|segment| segment.text.len())
            .sum::<usize>()
            .saturating_add(self.lines.len() * std::mem::size_of::<HighlightedLine>())
    }
}

#[derive(Debug)]
pub(crate) struct LruCache<K, V> {
    entries: HashMap<K, V>,
    order: VecDeque<K>,
    capacity: usize,
}

impl<K, V> LruCache<K, V>
where
    K: Copy + Eq + Hash,
{
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.values()
    }

    pub(crate) fn contains_key(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    pub(crate) fn insert(&mut self, key: K, value: V) {
        if self.capacity == 0 {
            return;
        }

        if let Some(entry) = self.entries.get_mut(&key) {
            *entry = value;
            self.touch(&key);
            return;
        }

        while self.entries.len() >= self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }

        self.entries.insert(key, value);
        self.order.push_back(key);
    }

    pub(crate) fn get(&mut self, key: &K) -> Option<&V> {
        if !self.entries.contains_key(key) {
            return None;
        }

        self.touch(key);
        self.entries.get(key)
    }

    pub(crate) fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        if !self.entries.contains_key(key) {
            return None;
        }

        self.touch(key);
        self.entries.get_mut(key)
    }

    pub(crate) fn touch(&mut self, key: &K) {
        // O(n) is intentional here: the syntax cache is capped at a small fixed
        // size, so avoiding another dependency/index keeps this simple.
        if let Some(index) = self.order.iter().position(|candidate| candidate == key) {
            self.order.remove(index);
        }
        self.order.push_back(*key);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntaxPriority {
    Visible,
    Prefetch,
}

#[derive(Debug, Clone)]
pub(crate) struct SyntaxWorkerQueue {
    pub(crate) inner: Arc<SyntaxWorkerQueueInner>,
}

#[derive(Debug)]
pub(crate) struct SyntaxWorkerQueueInner {
    pub(crate) state: Mutex<SyntaxWorkerQueueState>,
    pub(crate) ready: Condvar,
    pub(crate) capacity: usize,
}

#[derive(Debug)]
pub(crate) struct SyntaxWorkerQueueState {
    pub(crate) generation: u64,
    pub(crate) visible: VecDeque<SyntaxJob>,
    pub(crate) prefetch: VecDeque<SyntaxJob>,
    pub(crate) closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SyntaxQueuePush {
    pub(crate) dropped: Option<SyntaxKey>,
    pub(crate) depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntaxQueueError {
    Full,
    Closed,
    Stale,
}

impl SyntaxWorkerQueue {
    pub(crate) fn new(capacity: usize, generation: u64) -> Self {
        Self {
            inner: Arc::new(SyntaxWorkerQueueInner {
                state: Mutex::new(SyntaxWorkerQueueState {
                    generation,
                    visible: VecDeque::new(),
                    prefetch: VecDeque::new(),
                    closed: false,
                }),
                ready: Condvar::new(),
                capacity,
            }),
        }
    }

    pub(crate) fn try_push(
        &self,
        job: SyntaxJob,
        priority: SyntaxPriority,
    ) -> Result<SyntaxQueuePush, SyntaxQueueError> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| SyntaxQueueError::Closed)?;
        if state.closed {
            return Err(SyntaxQueueError::Closed);
        }
        if job.key.generation() != state.generation {
            return Err(SyntaxQueueError::Stale);
        }
        if self.inner.capacity == 0 {
            return Err(SyntaxQueueError::Full);
        }

        let mut dropped = None;
        if state.len() >= self.inner.capacity {
            match priority {
                SyntaxPriority::Visible => {
                    let Some(evicted) = state.prefetch.pop_back() else {
                        return Err(SyntaxQueueError::Full);
                    };
                    dropped = Some(evicted.key);
                }
                SyntaxPriority::Prefetch => return Err(SyntaxQueueError::Full),
            }
        }

        match priority {
            SyntaxPriority::Visible => state.visible.push_back(job),
            SyntaxPriority::Prefetch => state.prefetch.push_back(job),
        }
        let depth = state.len();
        self.inner.ready.notify_one();
        Ok(SyntaxQueuePush { dropped, depth })
    }

    pub(crate) fn promote(&self, key: SyntaxKey) -> bool {
        let Ok(mut state) = self.inner.state.lock() else {
            return false;
        };
        if state.closed {
            return false;
        }

        let Some(index) = state.prefetch.iter().position(|job| job.key == key) else {
            return false;
        };
        let Some(job) = state.prefetch.remove(index) else {
            return false;
        };
        state.visible.push_back(job);
        self.inner.ready.notify_one();
        true
    }

    pub(crate) fn set_generation(&self, generation: u64) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        state.generation = generation;
        state
            .visible
            .retain(|job| job.key.generation() == generation);
        state
            .prefetch
            .retain(|job| job.key.generation() == generation);
        self.inner.ready.notify_all();
    }

    pub(crate) fn pop(&self) -> Option<SyntaxJob> {
        let mut state = self.inner.state.lock().ok()?;
        loop {
            if state.closed {
                return None;
            }

            let job = state
                .visible
                .pop_front()
                .or_else(|| state.prefetch.pop_front());
            if let Some(job) = job {
                if job.key.generation() == state.generation {
                    return Some(job);
                }
                continue;
            }

            state = self.inner.ready.wait(state).ok()?;
        }
    }

    pub(crate) fn close(&self) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        state.closed = true;
        state.visible.clear();
        state.prefetch.clear();
        self.inner.ready.notify_all();
    }

    pub(crate) fn len(&self) -> usize {
        let Ok(state) = self.inner.state.lock() else {
            return 0;
        };
        state.len()
    }

    #[cfg(test)]
    pub(crate) fn try_pop(&self) -> Option<SyntaxJob> {
        let mut state = self.inner.state.lock().ok()?;
        state
            .visible
            .pop_front()
            .or_else(|| state.prefetch.pop_front())
    }
}

impl SyntaxWorkerQueueState {
    pub(crate) fn len(&self) -> usize {
        self.visible.len() + self.prefetch.len()
    }
}

#[derive(Debug)]
pub(crate) struct SyntaxRuntime {
    pub(crate) languages: SyntaxLanguageSet,
    pub(crate) limits: SyntaxLimits,
    pub(crate) result_rx: Receiver<SyntaxResult>,
    pub(crate) queue: SyntaxWorkerQueue,
    pub(crate) cache: LruCache<SyntaxKey, HighlightedSide>,
    pub(crate) pending: HashSet<SyntaxKey>,
    pub(crate) source_keys: HashMap<SyntaxSourceId, SyntaxKey>,
    pub(crate) position_keys: HashMap<SyntaxPosition, SyntaxKey>,
    pub(crate) line_maps: HashMap<SyntaxPosition, Vec<Option<usize>>>,
    pub(crate) skipped: HashMap<SyntaxPosition, SyntaxSkipReason>,
    pub(crate) skipped_sources: HashSet<SyntaxSourceId>,
    pub(crate) unavailable_full_files: HashSet<SyntaxKey>,
    pub(crate) failed: HashSet<SyntaxKey>,
    pub(crate) stats: SyntaxBenchmarkReport,
    pub(crate) worker: Option<thread::JoinHandle<()>>,
}

impl SyntaxRuntime {
    pub(crate) fn start(settings: &SyntaxSettings) -> HzResult<Option<Self>> {
        let languages = SyntaxLanguageSet::load_with_mode(settings.mode)?;
        Ok(Self::start_with_language_set(languages, settings.limits))
    }

    pub(crate) fn start_with_language_set(
        languages: SyntaxLanguageSet,
        limits: SyntaxLimits,
    ) -> Option<Self> {
        if languages.is_empty() {
            return None;
        }

        let (result_tx, result_rx) = mpsc::channel();
        let queue = SyntaxWorkerQueue::new(limits.queue_entries, 0);
        let worker_queue = queue.clone();
        let worker = thread::spawn(move || run_syntax_worker(worker_queue, result_tx));

        Some(Self {
            languages,
            limits,
            result_rx,
            queue,
            cache: LruCache::new(limits.cache_entries),
            pending: HashSet::new(),
            source_keys: HashMap::new(),
            position_keys: HashMap::new(),
            line_maps: HashMap::new(),
            skipped: HashMap::new(),
            skipped_sources: HashSet::new(),
            unavailable_full_files: HashSet::new(),
            failed: HashSet::new(),
            stats: SyntaxBenchmarkReport::default(),
            worker: Some(worker),
        })
    }

    pub(crate) fn start_with_languages(
        languages: Vec<String>,
        limits: SyntaxLimits,
    ) -> Option<Self> {
        let languages = SyntaxLanguageSet::from_enabled_languages(&languages);
        Self::start_with_language_set(languages, limits)
    }

    pub(crate) fn clear(&mut self, generation: u64) {
        self.cache.clear();
        self.pending.clear();
        self.source_keys.clear();
        self.position_keys.clear();
        self.line_maps.clear();
        self.skipped.clear();
        self.skipped_sources.clear();
        self.unavailable_full_files.clear();
        self.failed.clear();
        self.queue.set_generation(generation);
    }

    pub(crate) fn queue_hunk(
        &mut self,
        options: &DiffOptions,
        changeset: &Changeset,
        position: SyntaxPosition,
        priority: SyntaxPriority,
    ) {
        let SyntaxPosition {
            generation,
            file,
            hunk,
            side,
        } = position;
        self.stats.queue_requests = self.stats.queue_requests.saturating_add(1);
        if let Some(key) = self.position_keys.get(&position).copied() {
            if self.cache.contains_key(&key) {
                return;
            }
            if self.pending.contains(&key) {
                if priority == SyntaxPriority::Visible {
                    self.queue.promote(key);
                }
                return;
            }
        }
        if self.skipped.contains_key(&position) {
            return;
        }

        let Some(file_diff) = changeset.files.get(file) else {
            self.skip(position, SyntaxSkipReason::InvalidPosition);
            return;
        };
        let Some(path) = syntax_path(file_diff, side) else {
            self.skip(position, SyntaxSkipReason::NoPath);
            return;
        };
        let Some(language) = self.languages.language_for_path(path) else {
            self.skip(position, SyntaxSkipReason::NoLanguage);
            return;
        };
        let Some(hunk_diff) = file_diff.hunks.get(hunk) else {
            self.skip(position, SyntaxSkipReason::InvalidPosition);
            return;
        };

        if let Some(source) = full_file_source(&changeset.repo, options, file_diff, side) {
            let key = SyntaxKey {
                source: SyntaxSourceId {
                    generation,
                    file,
                    side,
                    kind: SyntaxSourceKind::FullFile,
                },
                language_hash: hash_text(&language),
                theme_id: SYNTAX_THEME_ID,
            };

            if !self.unavailable_full_files.contains(&key) {
                if self.failed.contains(&key) {
                    self.skip(position, SyntaxSkipReason::HighlightError);
                    return;
                }

                let line_map = match build_full_file_line_map(&hunk_diff.lines, side) {
                    Ok(line_map) => line_map,
                    Err(reason) => {
                        self.skip(position, reason);
                        return;
                    }
                };

                self.source_keys.insert(key.source, key);
                self.position_keys.insert(position, key);
                self.line_maps.insert(position, line_map);
                if self.queue_job(
                    key,
                    language,
                    SyntaxJobSource::FullFile(source),
                    priority,
                    Some(position),
                ) {
                    return;
                }
                return;
            }
        }

        let source = match build_hunk_source(&hunk_diff.lines, side, self.limits) {
            Ok(source) => source,
            Err(reason) => {
                self.skip(position, reason);
                return;
            }
        };

        let key = SyntaxKey {
            source: SyntaxSourceId {
                generation,
                file,
                side,
                kind: SyntaxSourceKind::HunkSide { hunk },
            },
            language_hash: hash_text(&language),
            theme_id: SYNTAX_THEME_ID,
        };
        self.source_keys.insert(key.source, key);
        self.position_keys.insert(position, key);
        self.line_maps.insert(position, source.line_map.clone());
        if self.failed.contains(&key) {
            self.skip(position, SyntaxSkipReason::HighlightError);
            return;
        }

        self.queue_job(
            key,
            language,
            SyntaxJobSource::Hunk(source),
            priority,
            Some(position),
        );
    }

    pub(crate) fn queue_full_file(
        &mut self,
        options: &DiffOptions,
        changeset: &Changeset,
        generation: u64,
        file: usize,
        side: DiffSide,
        priority: SyntaxPriority,
    ) {
        self.stats.queue_requests = self.stats.queue_requests.saturating_add(1);
        let source_id = SyntaxSourceId {
            generation,
            file,
            side,
            kind: SyntaxSourceKind::FullFile,
        };
        if let Some(key) = self.source_keys.get(&source_id).copied() {
            if self.cache.contains_key(&key) {
                return;
            }
            if self.pending.contains(&key) {
                if priority == SyntaxPriority::Visible {
                    self.queue.promote(key);
                }
                return;
            }
        }
        if self.skipped_sources.contains(&source_id) {
            return;
        }

        let Some(file_diff) = changeset.files.get(file) else {
            self.skip_source(source_id);
            return;
        };
        let Some(path) = syntax_path(file_diff, side) else {
            self.skip_source(source_id);
            return;
        };
        let Some(language) = self.languages.language_for_path(path) else {
            self.skip_source(source_id);
            return;
        };
        let Some(source) = full_file_source(&changeset.repo, options, file_diff, side) else {
            self.skip_source(source_id);
            return;
        };

        let key = SyntaxKey {
            source: source_id,
            language_hash: hash_text(&language),
            theme_id: SYNTAX_THEME_ID,
        };
        self.source_keys.insert(source_id, key);
        if self.unavailable_full_files.contains(&key) || self.failed.contains(&key) {
            self.skip_source(source_id);
            return;
        }

        self.queue_job(
            key,
            language,
            SyntaxJobSource::FullFile(source),
            priority,
            None,
        );
    }

    pub(crate) fn queue_job(
        &mut self,
        key: SyntaxKey,
        language: String,
        source: SyntaxJobSource,
        priority: SyntaxPriority,
        position: Option<SyntaxPosition>,
    ) -> bool {
        if self.cache.contains_key(&key) {
            return true;
        }
        if self.pending.contains(&key) {
            if priority == SyntaxPriority::Visible {
                self.queue.promote(key);
            }
            return true;
        }

        let source_bytes = source.known_bytes();
        let source_lines = source.known_lines();

        let job = SyntaxJob {
            key,
            language,
            source,
            limits: self.limits,
        };

        match self.queue.try_push(job, priority) {
            Ok(push) => {
                if let Some(dropped) = push.dropped {
                    self.pending.remove(&dropped);
                    self.stats.jobs_evicted = self.stats.jobs_evicted.saturating_add(1);
                }
                self.stats.jobs_queued = self.stats.jobs_queued.saturating_add(1);
                self.stats.queue_depth_peak = self.stats.queue_depth_peak.max(push.depth);
                if let Some(source_bytes) = source_bytes {
                    self.stats.source_bytes_queued =
                        self.stats.source_bytes_queued.saturating_add(source_bytes);
                }
                if let Some(source_lines) = source_lines {
                    self.stats.source_lines_queued =
                        self.stats.source_lines_queued.saturating_add(source_lines);
                }
                self.pending.insert(key);
                true
            }
            Err(SyntaxQueueError::Full | SyntaxQueueError::Stale) => {
                self.stats.jobs_rejected = self.stats.jobs_rejected.saturating_add(1);
                false
            }
            Err(SyntaxQueueError::Closed) => {
                if let Some(position) = position {
                    self.skip(position, SyntaxSkipReason::QueueClosed);
                } else {
                    self.skip_source(key.source);
                }
                false
            }
        }
    }

    pub(crate) fn skip(&mut self, position: SyntaxPosition, reason: SyntaxSkipReason) {
        if self.skipped.insert(position, reason).is_none() {
            self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
        }
    }

    pub(crate) fn skip_source(&mut self, source_id: SyntaxSourceId) {
        if self.skipped_sources.insert(source_id) {
            self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
        }
    }

    pub(crate) fn drain(&mut self, generation: u64, max_results: usize) -> bool {
        let mut changed = false;
        for _ in 0..max_results {
            let Ok(result) = self.result_rx.try_recv() else {
                break;
            };
            self.pending.remove(&result.key);
            if result.key.generation() != generation {
                self.stats.stale_results = self.stats.stale_results.saturating_add(1);
                continue;
            }

            match result.side {
                Ok(success) => {
                    self.cache.insert(result.key, success.side);
                    self.stats.jobs_completed = self.stats.jobs_completed.saturating_add(1);
                    if let Some(source_bytes) = success.source_bytes {
                        self.stats.source_bytes_queued =
                            self.stats.source_bytes_queued.saturating_add(source_bytes);
                    }
                    if let Some(source_lines) = success.source_lines {
                        self.stats.source_lines_queued =
                            self.stats.source_lines_queued.saturating_add(source_lines);
                    }
                    self.stats.cache_entries_peak =
                        self.stats.cache_entries_peak.max(self.cache.len());
                    self.stats.estimated_memory_peak_bytes = self
                        .stats
                        .estimated_memory_peak_bytes
                        .max(self.estimated_memory_bytes() as u64);
                    changed = true;
                }
                Err(SyntaxJobFailure::Unavailable) => {
                    self.handle_unavailable_source(result.key);
                    self.stats.jobs_skipped = self.stats.jobs_skipped.saturating_add(1);
                    changed = true;
                }
                Err(SyntaxJobFailure::HighlightError) => {
                    self.failed.insert(result.key);
                    let positions = self.positions_for_key(result.key);
                    for position in positions {
                        self.skipped
                            .insert(position, SyntaxSkipReason::HighlightError);
                    }
                    self.stats.jobs_failed = self.stats.jobs_failed.saturating_add(1);
                }
            }
        }
        changed
    }

    pub(crate) fn handle_unavailable_source(&mut self, key: SyntaxKey) {
        self.source_keys.remove(&key.source);
        if matches!(key.source.kind, SyntaxSourceKind::FullFile) {
            self.unavailable_full_files.insert(key);
        } else {
            let positions = self.positions_for_key(key);
            for position in positions {
                self.skipped.insert(position, SyntaxSkipReason::NoSource);
            }
        }

        let positions = self.positions_for_key(key);
        for position in positions {
            self.position_keys.remove(&position);
            self.line_maps.remove(&position);
        }
    }

    pub(crate) fn positions_for_key(&self, key: SyntaxKey) -> Vec<SyntaxPosition> {
        self.position_keys
            .iter()
            .filter_map(|(position, position_key)| (*position_key == key).then_some(*position))
            .collect()
    }

    pub(crate) fn line(
        &mut self,
        position: SyntaxPosition,
        line: usize,
    ) -> Option<HighlightedLine> {
        let highlighted = self.position_keys.get(&position).copied().and_then(|key| {
            let source_line = self
                .line_maps
                .get(&position)
                .and_then(|line_map| line_map.get(line))
                .and_then(|source_line| *source_line)?;
            self.cache
                .get(&key)
                .and_then(|side| side.lines.get(source_line))
                .cloned()
        });
        if highlighted.is_some() {
            self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
        } else {
            self.stats.cache_misses = self.stats.cache_misses.saturating_add(1);
        }
        highlighted
    }

    pub(crate) fn full_file_line(
        &mut self,
        generation: u64,
        file: usize,
        side: DiffSide,
        line_number: usize,
    ) -> Option<HighlightedLine> {
        let source_line = line_number.checked_sub(1)?;
        let source_id = SyntaxSourceId {
            generation,
            file,
            side,
            kind: SyntaxSourceKind::FullFile,
        };
        let key = self.source_keys.get(&source_id).copied();
        let highlighted = key
            .and_then(|key| self.cache.get(&key))
            .and_then(|side| side.lines.get(source_line))
            .cloned();
        if highlighted.is_some() {
            self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
        } else {
            self.stats.cache_misses = self.stats.cache_misses.saturating_add(1);
        }
        highlighted
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.queue.len() == 0
    }

    pub(crate) fn stats(&self) -> SyntaxBenchmarkReport {
        self.stats.clone()
    }

    pub(crate) fn estimated_memory_bytes(&self) -> usize {
        self.cache.values().map(HighlightedSide::memory_bytes).sum()
    }
}

impl Drop for SyntaxRuntime {
    fn drop(&mut self) {
        self.queue.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug)]
pub(crate) struct SyntaxJob {
    pub(crate) key: SyntaxKey,
    pub(crate) language: String,
    pub(crate) source: SyntaxJobSource,
    pub(crate) limits: SyntaxLimits,
}

#[derive(Debug)]
pub(crate) struct SyntaxResult {
    pub(crate) key: SyntaxKey,
    pub(crate) side: Result<SyntaxSuccess, SyntaxJobFailure>,
}

#[derive(Debug)]
pub(crate) struct SyntaxSuccess {
    pub(crate) side: HighlightedSide,
    pub(crate) source_bytes: Option<u64>,
    pub(crate) source_lines: Option<u64>,
}

#[derive(Debug)]
pub(crate) enum SyntaxJobFailure {
    Unavailable,
    HighlightError,
}

#[derive(Debug)]
pub(crate) struct HunkSource {
    pub(crate) text: String,
    pub(crate) line_map: Vec<Option<usize>>,
    pub(crate) source_lines: usize,
}

#[derive(Debug)]
pub(crate) enum SyntaxJobSource {
    Hunk(HunkSource),
    FullFile(FullFileSource),
}

impl SyntaxJobSource {
    pub(crate) fn known_bytes(&self) -> Option<u64> {
        match self {
            Self::Hunk(source) => Some(source.text.len() as u64),
            Self::FullFile(_) => None,
        }
    }

    pub(crate) fn known_lines(&self) -> Option<u64> {
        match self {
            Self::Hunk(source) => Some(source.source_lines as u64),
            Self::FullFile(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FullFileSource {
    pub(crate) repo: PathBuf,
    pub(crate) kind: FullFileSourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FullFileSourceKind {
    Worktree {
        path: String,
    },
    GitRevision {
        rev: String,
        path: String,
    },
    GitIndex {
        path: String,
    },
    GitMergeBase {
        base: String,
        head: String,
        path: String,
    },
}

pub(crate) fn run_syntax_worker(queue: SyntaxWorkerQueue, result_tx: Sender<SyntaxResult>) {
    let mut highlighter = SyntaxHighlighter::new();
    while let Some(job) = queue.pop() {
        let side = panic::catch_unwind(AssertUnwindSafe(|| {
            let (source, source_bytes, source_lines) = load_job_source(job.source, job.limits)?;
            highlighter
                .highlight(&job.language, &source)
                .map(|highlighted| SyntaxSuccess {
                    side: HighlightedSide {
                        lines: highlighted.lines,
                    },
                    source_bytes,
                    source_lines,
                })
                .map_err(|_| SyntaxJobFailure::HighlightError)
        }))
        .unwrap_or(Err(SyntaxJobFailure::HighlightError));
        if result_tx.send(SyntaxResult { key: job.key, side }).is_err() {
            break;
        }
    }
}

pub(crate) fn load_job_source(
    source: SyntaxJobSource,
    limits: SyntaxLimits,
) -> Result<(String, Option<u64>, Option<u64>), SyntaxJobFailure> {
    match source {
        SyntaxJobSource::Hunk(source) => Ok((source.text, None, None)),
        SyntaxJobSource::FullFile(source) => {
            let text = load_full_file_source(&source).map_err(|_| SyntaxJobFailure::Unavailable)?;
            validate_highlight_source(&text, limits).map_err(|_| SyntaxJobFailure::Unavailable)?;
            let source_bytes = text.len() as u64;
            let source_lines = source_line_count(&text) as u64;
            Ok((text, Some(source_bytes), Some(source_lines)))
        }
    }
}

pub(crate) fn syntax_path(file: &hz_diff::DiffFile, side: DiffSide) -> Option<&str> {
    match side {
        DiffSide::Old => file.old_path.as_deref().or(file.new_path.as_deref()),
        DiffSide::New => file.new_path.as_deref().or(file.old_path.as_deref()),
    }
}

pub(crate) fn build_hunk_source(
    lines: &[DiffLine],
    side: DiffSide,
    limits: SyntaxLimits,
) -> Result<HunkSource, SyntaxSkipReason> {
    let mut text = String::new();
    let mut line_map = vec![None; lines.len()];
    let mut source_lines = 0;

    for (index, line) in lines.iter().enumerate() {
        if !line_belongs_to_side(line.kind, side) {
            continue;
        }
        if line.text.len() > limits.max_line_bytes {
            return Err(SyntaxSkipReason::TooLarge);
        }
        if source_lines > 0 {
            text.push('\n');
        }
        text.push_str(&line.text);
        if text.len() > limits.max_source_bytes {
            return Err(SyntaxSkipReason::TooLarge);
        }
        line_map[index] = Some(source_lines);
        source_lines += 1;
    }

    if source_lines == 0 {
        return Err(SyntaxSkipReason::NoSource);
    }

    Ok(HunkSource {
        text,
        line_map,
        source_lines,
    })
}

pub(crate) fn build_full_file_line_map(
    lines: &[DiffLine],
    side: DiffSide,
) -> Result<Vec<Option<usize>>, SyntaxSkipReason> {
    let mut line_map = vec![None; lines.len()];
    let mut source_lines = 0;

    for (index, line) in lines.iter().enumerate() {
        if !line_belongs_to_side(line.kind, side) {
            continue;
        }

        let Some(line_number) = diff_line_number(line, side) else {
            continue;
        };
        let Some(source_line) = line_number.checked_sub(1) else {
            continue;
        };
        line_map[index] = Some(source_line);
        source_lines += 1;
    }

    if source_lines == 0 {
        return Err(SyntaxSkipReason::NoSource);
    }

    Ok(line_map)
}

pub(crate) fn diff_line_number(line: &DiffLine, side: DiffSide) -> Option<usize> {
    match side {
        DiffSide::Old => line.old_line,
        DiffSide::New => line.new_line,
    }
}

pub(crate) fn full_file_source(
    repo: &Path,
    options: &DiffOptions,
    file: &hz_diff::DiffFile,
    side: DiffSide,
) -> Option<FullFileSource> {
    if matches!(options.source, DiffSource::Patch(_)) {
        return None;
    }
    if !repo.is_dir() {
        return None;
    }

    let path = file_path_for_side(file, side)?.to_owned();
    let kind = match (&options.source, options.scope, side) {
        (DiffSource::Worktree, DiffScope::All, DiffSide::Old) => FullFileSourceKind::GitRevision {
            rev: "HEAD".to_owned(),
            path,
        },
        (DiffSource::Worktree, DiffScope::All, DiffSide::New) => {
            FullFileSourceKind::Worktree { path }
        }
        (DiffSource::Worktree, DiffScope::Staged, DiffSide::Old) => {
            FullFileSourceKind::GitRevision {
                rev: "HEAD".to_owned(),
                path,
            }
        }
        (DiffSource::Worktree, DiffScope::Staged, DiffSide::New) => {
            FullFileSourceKind::GitIndex { path }
        }
        (DiffSource::Worktree, DiffScope::Unstaged, DiffSide::Old) => {
            FullFileSourceKind::GitIndex { path }
        }
        (DiffSource::Worktree, DiffScope::Unstaged, DiffSide::New) => {
            FullFileSourceKind::Worktree { path }
        }
        (DiffSource::Base(base), DiffScope::All, DiffSide::Old) => {
            FullFileSourceKind::GitMergeBase {
                base: base.clone(),
                head: "HEAD".to_owned(),
                path,
            }
        }
        (DiffSource::Base(_), DiffScope::All, DiffSide::New) => FullFileSourceKind::GitRevision {
            rev: "HEAD".to_owned(),
            path,
        },
        (DiffSource::Branch { base, head }, DiffScope::All, DiffSide::Old) => {
            FullFileSourceKind::GitMergeBase {
                base: base.clone(),
                head: head.clone(),
                path,
            }
        }
        (DiffSource::Branch { head, .. }, DiffScope::All, DiffSide::New) => {
            FullFileSourceKind::GitRevision {
                rev: head.clone(),
                path,
            }
        }
        (DiffSource::Range { left, .. }, DiffScope::All, DiffSide::Old) => {
            FullFileSourceKind::GitRevision {
                rev: left.clone(),
                path,
            }
        }
        (DiffSource::Range { right, .. }, DiffScope::All, DiffSide::New) => {
            FullFileSourceKind::GitRevision {
                rev: right.clone(),
                path,
            }
        }
        _ => return None,
    };

    Some(FullFileSource {
        repo: repo.to_owned(),
        kind,
    })
}

pub(crate) fn file_path_for_side(file: &hz_diff::DiffFile, side: DiffSide) -> Option<&str> {
    match side {
        DiffSide::Old => file.old_path.as_deref(),
        DiffSide::New => file.new_path.as_deref(),
    }
}

pub(crate) fn load_full_file_source(source: &FullFileSource) -> Result<String, SyntaxSkipReason> {
    let bytes = match &source.kind {
        FullFileSourceKind::Worktree { path } => read_worktree_file(&source.repo, path)?,
        FullFileSourceKind::GitRevision { rev, path } => {
            git_blob(&source.repo, &format!("{rev}:{path}"))?
        }
        FullFileSourceKind::GitIndex { path } => git_blob(&source.repo, &format!(":{path}"))?,
        FullFileSourceKind::GitMergeBase { base, head, path } => {
            let rev = git_merge_base(&source.repo, base, head)?;
            git_blob(&source.repo, &format!("{rev}:{path}"))?
        }
    };

    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn read_worktree_file(repo: &Path, path: &str) -> Result<Vec<u8>, SyntaxSkipReason> {
    let path = safe_repo_join(repo, path).ok_or(SyntaxSkipReason::NoPath)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| SyntaxSkipReason::NoSource)?;
    if !metadata.file_type().is_file() {
        return Err(SyntaxSkipReason::NoSource);
    }
    fs::read(path).map_err(|_| SyntaxSkipReason::NoSource)
}

pub(crate) fn safe_repo_join(repo: &Path, path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() {
        return None;
    }

    let mut joined = repo.to_owned();
    for component in path.components() {
        match component {
            Component::Normal(part) => joined.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(joined)
}

pub(crate) fn git_blob(repo: &Path, object: &str) -> Result<Vec<u8>, SyntaxSkipReason> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "show",
            "--no-ext-diff",
            "--no-color",
            "--end-of-options",
            object,
        ])
        .output()
        .map_err(|_| SyntaxSkipReason::NoSource)?;
    if !output.status.success() {
        return Err(SyntaxSkipReason::NoSource);
    }
    Ok(output.stdout)
}

pub(crate) fn git_merge_base(
    repo: &Path,
    base: &str,
    head: &str,
) -> Result<String, SyntaxSkipReason> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--end-of-options", base, head])
        .output()
        .map_err(|_| SyntaxSkipReason::NoSource)?;
    if !output.status.success() {
        return Err(SyntaxSkipReason::NoSource);
    }

    let rev = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if rev.is_empty() {
        return Err(SyntaxSkipReason::NoSource);
    }
    Ok(rev)
}

pub(crate) fn validate_highlight_source(
    source: &str,
    limits: SyntaxLimits,
) -> Result<(), SyntaxSkipReason> {
    if source.len() > limits.max_source_bytes {
        return Err(SyntaxSkipReason::TooLarge);
    }
    if source
        .lines()
        .any(|line| line.len() > limits.max_line_bytes)
    {
        return Err(SyntaxSkipReason::TooLarge);
    }
    Ok(())
}

pub(crate) fn source_line_count(source: &str) -> usize {
    source.lines().count().max(1)
}

pub(crate) fn split_context_source_lines(source: &str) -> Vec<String> {
    source.lines().map(str::to_owned).collect()
}

pub(crate) fn available_context_lines(
    source_start: usize,
    total: usize,
    source_line_count: usize,
) -> usize {
    let Some(source_index_start) = source_start.checked_sub(1) else {
        return 0;
    };
    source_line_count
        .saturating_sub(source_index_start)
        .min(total)
}

pub(crate) fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn line_belongs_to_side(kind: DiffLineKind, side: DiffSide) -> bool {
    matches!(
        (side, kind),
        (
            DiffSide::Old,
            DiffLineKind::Context | DiffLineKind::Deletion
        ) | (
            DiffSide::New,
            DiffLineKind::Context | DiffLineKind::Addition
        )
    )
}

pub(crate) fn unified_syntax_side(kind: DiffLineKind) -> Option<DiffSide> {
    match kind {
        DiffLineKind::Deletion => Some(DiffSide::Old),
        DiffLineKind::Addition | DiffLineKind::Context => Some(DiffSide::New),
        DiffLineKind::Meta => None,
    }
}

#[cfg(test)]
pub(crate) fn compute_hunk_inline_emphasis(lines: &[DiffLine]) -> Vec<InlineLineEmphasis> {
    let mut emphasis = vec![InlineLineEmphasis::default(); lines.len()];
    let mut index = 0usize;

    while index < lines.len() {
        match lines[index].kind {
            DiffLineKind::Deletion | DiffLineKind::Addition => {
                let mut deletions = Vec::new();
                let mut additions = Vec::new();
                while index < lines.len()
                    && matches!(
                        lines[index].kind,
                        DiffLineKind::Deletion | DiffLineKind::Addition
                    )
                {
                    match lines[index].kind {
                        DiffLineKind::Deletion => deletions.push(index),
                        DiffLineKind::Addition => additions.push(index),
                        DiffLineKind::Context | DiffLineKind::Meta => {}
                    }
                    index += 1;
                }
                compute_changed_block_inline_emphasis(lines, &deletions, &additions, &mut emphasis);
            }
            DiffLineKind::Context | DiffLineKind::Meta => index += 1,
        }
    }

    emphasis
}

#[cfg(test)]
pub(crate) fn compute_changed_block_inline_emphasis(
    lines: &[DiffLine],
    deletions: &[usize],
    additions: &[usize],
    emphasis: &mut [InlineLineEmphasis],
) {
    let paired_rows = deletions.len().max(additions.len());
    for pair_index in 0..paired_rows {
        match (deletions.get(pair_index), additions.get(pair_index)) {
            (Some(deletion), Some(addition)) => {
                let (old_ranges, new_ranges) =
                    changed_token_ranges(&lines[*deletion].text, &lines[*addition].text);
                emphasis[*deletion].ranges = old_ranges;
                emphasis[*addition].ranges = new_ranges;
            }
            (Some(deletion), None) => {
                emphasis[*deletion].ranges = Vec::new();
            }
            (None, Some(addition)) => {
                emphasis[*addition].ranges = Vec::new();
            }
            (None, None) => {}
        }
    }
}

pub(crate) fn changed_token_ranges(old: &str, new: &str) -> (Vec<InlineRange>, Vec<InlineRange>) {
    if old == new {
        return (Vec::new(), Vec::new());
    }
    if old.len() > MAX_INLINE_DIFF_LINE_BYTES || new.len() > MAX_INLINE_DIFF_LINE_BYTES {
        return (Vec::new(), Vec::new());
    }

    let old_tokens = inline_tokens(old);
    let new_tokens = inline_tokens(new);
    if old_tokens.len() > MAX_INLINE_DIFF_TOKENS || new_tokens.len() > MAX_INLINE_DIFF_TOKENS {
        return (Vec::new(), Vec::new());
    }

    let mut old_changed = vec![true; old_tokens.len()];
    let mut new_changed = vec![true; new_tokens.len()];
    mark_unchanged_lcs_tokens(
        old,
        &old_tokens,
        new,
        &new_tokens,
        &mut old_changed,
        &mut new_changed,
    );

    (
        inline_ranges_from_tokens(&old_tokens, &old_changed),
        inline_ranges_from_tokens(&new_tokens, &new_changed),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlineToken {
    pub(crate) byte_start: usize,
    pub(crate) byte_end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InlineCharClass {
    Word,
    Whitespace,
    Other,
}

pub(crate) fn inline_tokens(text: &str) -> Vec<InlineToken> {
    if text.is_ascii() {
        return inline_tokens_ascii(text);
    }

    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();

    while let Some((start, ch)) = chars.next() {
        let class = inline_char_class(ch);
        let mut end = start + ch.len_utf8();

        if class != InlineCharClass::Other {
            while let Some((_, next)) = chars.peek().copied() {
                if inline_char_class(next) != class {
                    break;
                }
                let Some((next_start, next)) = chars.next() else {
                    break;
                };
                end = next_start + next.len_utf8();
            }
        }

        tokens.push(InlineToken {
            byte_start: start,
            byte_end: end,
        });
    }

    tokens
}

pub(crate) fn inline_tokens_ascii(text: &str) -> Vec<InlineToken> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut start = 0usize;

    while start < bytes.len() {
        let class = inline_ascii_class(bytes[start]);
        let mut end = start + 1;

        if class != InlineCharClass::Other {
            while end < bytes.len() && inline_ascii_class(bytes[end]) == class {
                end += 1;
            }
        }

        tokens.push(InlineToken {
            byte_start: start,
            byte_end: end,
        });
        start = end;
    }

    tokens
}

pub(crate) fn inline_ascii_class(byte: u8) -> InlineCharClass {
    if byte.is_ascii_whitespace() || byte == 0x0B {
        InlineCharClass::Whitespace
    } else if byte == b'_' || byte.is_ascii_alphanumeric() {
        InlineCharClass::Word
    } else {
        InlineCharClass::Other
    }
}

pub(crate) fn inline_char_class(ch: char) -> InlineCharClass {
    if ch.is_whitespace() {
        InlineCharClass::Whitespace
    } else if ch == '_' || ch.is_alphanumeric() {
        InlineCharClass::Word
    } else {
        InlineCharClass::Other
    }
}

pub(crate) fn mark_unchanged_lcs_tokens(
    old: &str,
    old_tokens: &[InlineToken],
    new: &str,
    new_tokens: &[InlineToken],
    old_changed: &mut [bool],
    new_changed: &mut [bool],
) {
    let cols = new_tokens.len() + 1;
    let mut lengths = vec![0u16; (old_tokens.len() + 1) * cols];

    for old_index in 0..old_tokens.len() {
        for new_index in 0..new_tokens.len() {
            let cell = (old_index + 1) * cols + new_index + 1;
            lengths[cell] = if inline_token_text(old, old_tokens[old_index])
                == inline_token_text(new, new_tokens[new_index])
            {
                lengths[old_index * cols + new_index].saturating_add(1)
            } else {
                lengths[old_index * cols + new_index + 1]
                    .max(lengths[(old_index + 1) * cols + new_index])
            };
        }
    }

    let mut old_index = old_tokens.len();
    let mut new_index = new_tokens.len();
    while old_index > 0 && new_index > 0 {
        if inline_token_text(old, old_tokens[old_index - 1])
            == inline_token_text(new, new_tokens[new_index - 1])
        {
            old_changed[old_index - 1] = false;
            new_changed[new_index - 1] = false;
            old_index -= 1;
            new_index -= 1;
        } else if lengths[(old_index - 1) * cols + new_index]
            >= lengths[old_index * cols + new_index - 1]
        {
            old_index -= 1;
        } else {
            new_index -= 1;
        }
    }
}

pub(crate) fn inline_token_text(text: &str, token: InlineToken) -> &str {
    &text[token.byte_start..token.byte_end]
}

pub(crate) fn inline_ranges_from_tokens(
    tokens: &[InlineToken],
    changed: &[bool],
) -> Vec<InlineRange> {
    let mut ranges: Vec<InlineRange> = Vec::new();
    for (token, is_changed) in tokens.iter().zip(changed) {
        if !*is_changed {
            continue;
        }
        if let Some(last) = ranges.last_mut()
            && last.byte_end == token.byte_start
        {
            last.byte_end = token.byte_end;
            continue;
        }
        ranges.push(InlineRange {
            byte_start: token.byte_start,
            byte_end: token.byte_end,
        });
    }
    ranges
}
