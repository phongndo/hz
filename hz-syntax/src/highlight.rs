#![allow(unused_imports)]

use crate::*;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, HzResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};
use tree_sitter_language_pack::LanguageRegistry;

pub fn detect_language_from_path(path: &str) -> Option<String> {
    detect_language_name(path).map(|language| language.to_owned())
}

pub(crate) fn highlighted_text_from_events<'a>(
    source: &str,
    highlights: impl Iterator<Item = Result<HighlightEvent, tree_sitter_highlight::Error>> + 'a,
) -> HzResult<HighlightedText> {
    let line_count = source.split('\n').count();
    let mut lines = vec![HighlightedLine::default(); line_count];
    let mut line_index = 0;
    let mut stack = Vec::new();

    for event in highlights {
        match event
            .map_err(|error| HzError::Usage(format!("failed to highlight source: {error}")))?
        {
            HighlightEvent::HighlightStart(highlight) => stack.push(highlight.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let class = stack
                    .last()
                    .and_then(|index| HIGHLIGHT_NAMES.get(*index))
                    .and_then(|name| syntax_class(name));
                push_source_segment(
                    &mut lines,
                    &mut line_index,
                    start,
                    &source.as_bytes()[start..end],
                    class,
                );
            }
        }
    }

    Ok(HighlightedText { lines })
}

pub(crate) fn push_source_segment(
    lines: &mut [HighlightedLine],
    line_index: &mut usize,
    byte_start: usize,
    mut bytes: &[u8],
    class: Option<SyntaxClass>,
) {
    let mut offset = 0usize;
    while let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
        push_line_segment(
            lines,
            *line_index,
            byte_start.saturating_add(offset),
            byte_start.saturating_add(offset).saturating_add(newline),
            &bytes[..newline],
            class,
        );
        *line_index = line_index
            .saturating_add(1)
            .min(lines.len().saturating_sub(1));
        offset = offset.saturating_add(newline + 1);
        bytes = &bytes[newline + 1..];
    }
    push_line_segment(
        lines,
        *line_index,
        byte_start.saturating_add(offset),
        byte_start
            .saturating_add(offset)
            .saturating_add(bytes.len()),
        bytes,
        class,
    );
}

pub(crate) fn push_line_segment(
    lines: &mut [HighlightedLine],
    line_index: usize,
    byte_start: usize,
    byte_end: usize,
    bytes: &[u8],
    class: Option<SyntaxClass>,
) {
    if bytes.is_empty() || line_index >= lines.len() {
        return;
    }

    let text = String::from_utf8_lossy(bytes).into_owned();
    let Some(last) = lines[line_index].segments.last_mut() else {
        lines[line_index].segments.push(SyntaxSegment {
            byte_start,
            byte_end,
            text,
            class,
        });
        return;
    };

    if last.class == class && last.byte_end == byte_start {
        last.text.push_str(&text);
        last.byte_end = byte_end;
    } else {
        lines[line_index].segments.push(SyntaxSegment {
            byte_start,
            byte_end,
            text,
            class,
        });
    }
}

pub(crate) fn syntax_class(name: &str) -> Option<SyntaxClass> {
    let namespace = name.split('.').next().unwrap_or(name);
    let class = if namespace == "comment" {
        SyntaxClass::Comment
    } else if namespace == "keyword" || name == "boolean" {
        SyntaxClass::Keyword
    } else if namespace == "string" || name == "character" {
        SyntaxClass::String
    } else if namespace == "number" {
        SyntaxClass::Number
    } else if namespace == "type" {
        SyntaxClass::Type
    } else if namespace == "function" {
        SyntaxClass::Function
    } else if namespace == "constructor" {
        SyntaxClass::Constructor
    } else if namespace == "constant" {
        SyntaxClass::Constant
    } else if namespace == "property" {
        SyntaxClass::Property
    } else if namespace == "punctuation" {
        SyntaxClass::Punctuation
    } else if namespace == "operator" {
        SyntaxClass::Operator
    } else if namespace == "tag" {
        SyntaxClass::Tag
    } else if namespace == "attribute" {
        SyntaxClass::Attribute
    } else if namespace == "module" || namespace == "namespace" {
        SyntaxClass::Module
    } else if namespace == "label" {
        SyntaxClass::Label
    } else if namespace == "variable" {
        SyntaxClass::Variable
    } else {
        return None;
    };
    Some(class)
}
