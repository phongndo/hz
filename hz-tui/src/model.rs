use std::{collections::HashMap, sync::Arc};

use hz_diff::{Changeset, DiffLine, DiffLineKind};

use crate::{controls::DiffLayoutMode, syntax::DiffSide};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiRow {
    FileSeparator,
    FileHeader(usize),
    BinaryFile(usize),
    Collapsed {
        file: usize,
        hunk: usize,
        old_start: usize,
        new_start: usize,
        lines: usize,
        expanded: usize,
    },
    ContextLine {
        file: usize,
        old_line: usize,
        new_line: usize,
    },
    ContextHide {
        file: usize,
        hunk: usize,
        lines: usize,
    },
    HunkHeader {
        file: usize,
        hunk: usize,
    },
    UnifiedLine {
        file: usize,
        hunk: usize,
        line: usize,
    },
    SplitLine {
        file: usize,
        hunk: usize,
        left: Option<usize>,
        right: Option<usize>,
    },
    MetaLine {
        file: usize,
        hunk: usize,
        line: usize,
    },
}

impl UiRow {
    pub(crate) fn hunk_key(self) -> Option<(usize, usize)> {
        match self {
            Self::HunkHeader { file, hunk }
            | Self::UnifiedLine { file, hunk, .. }
            | Self::SplitLine { file, hunk, .. }
            | Self::MetaLine { file, hunk, .. } => Some((file, hunk)),
            Self::FileSeparator
            | Self::FileHeader(_)
            | Self::BinaryFile(_)
            | Self::Collapsed { .. }
            | Self::ContextLine { .. }
            | Self::ContextHide { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ContextKey {
    pub(crate) file: usize,
    pub(crate) hunk: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextExpansionDirection {
    Up,
    Down,
}

pub(crate) fn context_expansion_direction(hunk: usize) -> ContextExpansionDirection {
    if hunk == 0 {
        ContextExpansionDirection::Up
    } else {
        ContextExpansionDirection::Down
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ContextSourceKey {
    pub(crate) file: usize,
    pub(crate) side: DiffSide,
}

#[derive(Debug, Clone)]
pub(crate) enum ContextSourceEntry {
    Lines(Arc<Vec<String>>),
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiModel {
    pub(crate) rows: Vec<UiRow>,
    pub(crate) file_start_rows: Vec<Option<usize>>,
    pub(crate) file_row_starts: Vec<(usize, usize)>,
    pub(crate) visible_files: Vec<usize>,
    pub(crate) hunk_start_rows: Vec<usize>,
}

impl UiModel {
    pub(crate) fn new(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
    ) -> Self {
        let visible_files: Vec<_> = (0..changeset.files.len()).collect();
        Self::new_filtered(changeset, layout, context_expansions, &visible_files)
    }

    pub(crate) fn new_filtered(
        changeset: &Changeset,
        layout: DiffLayoutMode,
        context_expansions: &HashMap<ContextKey, usize>,
        visible_files: &[usize],
    ) -> Self {
        let total_hunks = changeset
            .files
            .iter()
            .map(|file| file.hunks.len())
            .sum::<usize>();
        let total_hunk_lines = changeset
            .files
            .iter()
            .flat_map(|file| file.hunks.iter())
            .map(|hunk| hunk.lines.len())
            .sum::<usize>();
        let binary_or_empty_rows = changeset
            .files
            .iter()
            .filter(|file| file.is_binary || file.hunks.is_empty())
            .count();
        let file_separator_rows = changeset.files.len().saturating_sub(1);
        let expanded_context_rows = context_expansions.values().copied().sum::<usize>();
        let expanded_context_controls = context_expansions
            .values()
            .filter(|expanded| **expanded > 0)
            .count();
        let mut rows = Vec::with_capacity(
            changeset
                .files
                .len()
                .saturating_add(file_separator_rows)
                .saturating_add(binary_or_empty_rows)
                .saturating_add(total_hunks.saturating_mul(2))
                .saturating_add(total_hunk_lines)
                .saturating_add(expanded_context_rows)
                .saturating_add(expanded_context_controls),
        );
        let mut file_start_rows = vec![None; changeset.files.len()];
        let mut file_row_starts = Vec::with_capacity(visible_files.len());
        let mut hunk_start_rows = Vec::with_capacity(total_hunks);

        for (visible_index, file_index) in visible_files.iter().copied().enumerate() {
            let Some(file) = changeset.files.get(file_index) else {
                continue;
            };
            if visible_index > 0 {
                rows.push(UiRow::FileSeparator);
            }
            file_start_rows[file_index] = Some(rows.len());
            file_row_starts.push((file_index, rows.len()));
            rows.push(UiRow::FileHeader(file_index));

            if file.is_binary || file.hunks.is_empty() {
                rows.push(UiRow::BinaryFile(file_index));
                continue;
            }

            let mut next_old_line = 1;
            let mut next_new_line = 1;
            for (hunk_index, hunk) in file.hunks.iter().enumerate() {
                let collapsed_lines = hunk
                    .old_start
                    .saturating_sub(next_old_line)
                    .min(hunk.new_start.saturating_sub(next_new_line));
                if collapsed_lines > 0 {
                    let key = ContextKey {
                        file: file_index,
                        hunk: hunk_index,
                    };
                    let expanded = context_expansions
                        .get(&key)
                        .copied()
                        .unwrap_or_default()
                        .min(collapsed_lines);
                    let remaining = collapsed_lines.saturating_sub(expanded);
                    let direction = context_expansion_direction(hunk_index);

                    match direction {
                        ContextExpansionDirection::Up => {
                            if remaining > 0 {
                                rows.push(UiRow::Collapsed {
                                    file: file_index,
                                    hunk: hunk_index,
                                    old_start: next_old_line,
                                    new_start: next_new_line,
                                    lines: remaining,
                                    expanded,
                                });
                            }

                            if expanded > 0 {
                                let old_start = hunk.old_start.saturating_sub(expanded);
                                let new_start = hunk.new_start.saturating_sub(expanded);
                                for offset in 0..expanded {
                                    rows.push(UiRow::ContextLine {
                                        file: file_index,
                                        old_line: old_start + offset,
                                        new_line: new_start + offset,
                                    });
                                }
                                rows.push(UiRow::ContextHide {
                                    file: file_index,
                                    hunk: hunk_index,
                                    lines: expanded,
                                });
                            }
                        }
                        ContextExpansionDirection::Down => {
                            if expanded > 0 {
                                rows.push(UiRow::ContextHide {
                                    file: file_index,
                                    hunk: hunk_index,
                                    lines: expanded,
                                });
                                for offset in 0..expanded {
                                    rows.push(UiRow::ContextLine {
                                        file: file_index,
                                        old_line: next_old_line + offset,
                                        new_line: next_new_line + offset,
                                    });
                                }
                            }

                            if remaining > 0 {
                                rows.push(UiRow::Collapsed {
                                    file: file_index,
                                    hunk: hunk_index,
                                    old_start: next_old_line,
                                    new_start: next_new_line,
                                    lines: remaining,
                                    expanded,
                                });
                            }
                        }
                    }
                }

                hunk_start_rows.push(rows.len());
                rows.push(UiRow::HunkHeader {
                    file: file_index,
                    hunk: hunk_index,
                });

                match layout {
                    DiffLayoutMode::Unified => {
                        for line_index in 0..hunk.lines.len() {
                            rows.push(UiRow::UnifiedLine {
                                file: file_index,
                                hunk: hunk_index,
                                line: line_index,
                            });
                        }
                    }
                    DiffLayoutMode::Split => push_split_hunk_rows(
                        &mut rows,
                        file_index,
                        hunk_index,
                        hunk.lines.as_slice(),
                    ),
                }

                next_old_line = hunk.old_start.saturating_add(hunk.old_count);
                next_new_line = hunk.new_start.saturating_add(hunk.new_count);
            }
        }

        Self {
            rows,
            file_start_rows,
            file_row_starts,
            visible_files: visible_files.to_vec(),
            hunk_start_rows,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub(crate) fn row(&self, index: usize) -> Option<UiRow> {
        self.rows.get(index).copied()
    }

    pub(crate) fn file_start_row(&self, file: usize) -> Option<usize> {
        self.file_start_rows.get(file).copied().flatten()
    }

    pub(crate) fn file_at_row(&self, row: usize) -> Option<usize> {
        if self.file_row_starts.is_empty() {
            return None;
        }
        match self
            .file_row_starts
            .binary_search_by_key(&row, |(_, start)| *start)
        {
            Ok(index) => self.file_row_starts.get(index).map(|(file, _)| *file),
            Err(0) => self.file_row_starts.first().map(|(file, _)| *file),
            Err(index) => self.file_row_starts.get(index - 1).map(|(file, _)| *file),
        }
    }

    pub(crate) fn visible_files(&self) -> &[usize] {
        &self.visible_files
    }

    pub(crate) fn visible_file_position(&self, file: usize) -> Option<usize> {
        self.visible_files
            .iter()
            .position(|candidate| *candidate == file)
    }

    pub(crate) fn next_hunk_row(&self, row: usize) -> Option<usize> {
        self.hunk_start_rows
            .iter()
            .copied()
            .find(|start| *start > row)
    }

    pub(crate) fn previous_hunk_row(&self, row: usize) -> Option<usize> {
        self.hunk_start_rows
            .iter()
            .rev()
            .copied()
            .find(|start| *start < row)
    }
}

pub(crate) fn push_split_hunk_rows(
    rows: &mut Vec<UiRow>,
    file_index: usize,
    hunk_index: usize,
    lines: &[DiffLine],
) {
    let mut index = 0;
    while index < lines.len() {
        match lines[index].kind {
            DiffLineKind::Context => {
                rows.push(UiRow::SplitLine {
                    file: file_index,
                    hunk: hunk_index,
                    left: Some(index),
                    right: Some(index),
                });
                index += 1;
            }
            DiffLineKind::Meta => {
                rows.push(UiRow::MetaLine {
                    file: file_index,
                    hunk: hunk_index,
                    line: index,
                });
                index += 1;
            }
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

                let paired_rows = deletions.len().max(additions.len());
                for pair_index in 0..paired_rows {
                    rows.push(UiRow::SplitLine {
                        file: file_index,
                        hunk: hunk_index,
                        left: deletions.get(pair_index).copied(),
                        right: additions.get(pair_index).copied(),
                    });
                }
            }
        }
    }
}
