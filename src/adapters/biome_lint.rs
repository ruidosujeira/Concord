use std::path::Path;

use serde_json::Value;

use crate::adapters::ParsedDiagnostics;
use crate::matching::AliasTable;
use crate::model::{Diagnostic, DiagnosticData, Severity, Span, Tool, normalize_path};

pub fn parse(
    source: &str,
    root: &Path,
    aliases: &AliasTable,
) -> std::result::Result<ParsedDiagnostics, String> {
    if source.trim().is_empty() {
        return Ok(ParsedDiagnostics {
            diagnostics: Vec::new(),
            warnings: vec!["Biome produced empty JSON output".into()],
        });
    }
    let values = parse_json_or_rdjson(source)?;
    let mut raw_diagnostics = Vec::new();
    for value in &values {
        collect_diagnostics(value, &mut raw_diagnostics);
    }
    let diagnostics = raw_diagnostics
        .into_iter()
        .map(|value| convert(value, root, aliases))
        .collect();
    Ok(ParsedDiagnostics {
        diagnostics,
        warnings: Vec::new(),
    })
}

fn parse_json_or_rdjson(source: &str) -> std::result::Result<Vec<Value>, String> {
    if let Ok(value) = serde_json::from_str(source) {
        return Ok(vec![value]);
    }
    source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|error| error.to_string()))
        .collect()
}

fn collect_diagnostics<'a>(value: &'a Value, output: &mut Vec<&'a Value>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_diagnostics(value, output);
            }
        }
        Value::Object(object) => {
            if let Some(diagnostics) = object.get("diagnostics").and_then(Value::as_array) {
                for diagnostic in diagnostics {
                    output.push(diagnostic);
                }
            } else if object.contains_key("category")
                && (object.contains_key("location") || object.contains_key("message"))
            {
                output.push(value);
            } else if let Some(diagnostic) = object.get("diagnostic") {
                output.push(diagnostic);
            }
        }
        _ => {}
    }
}

fn convert(value: &Value, root: &Path, aliases: &AliasTable) -> Diagnostic {
    let code = value
        .get("category")
        .or_else(|| value.get("code"))
        .and_then(stringish)
        .map(str::to_owned);
    let raw_path = value
        .pointer("/location/path/file")
        .or_else(|| value.pointer("/location/path"))
        .or_else(|| value.get("filePath"))
        .or_else(|| value.get("filename"))
        .and_then(stringish)
        .unwrap_or("<unknown>");
    let source_code = value
        .pointer("/location/sourceCode")
        .and_then(Value::as_str);
    Diagnostic::new(
        Tool::Biome,
        normalize_path(root, Path::new(raw_path)),
        DiagnosticData {
            canonical_code: code.as_deref().map(|rule| aliases.canonicalize(rule)),
            code,
            severity: Severity::from_value(value.get("severity").unwrap_or(&Value::Null)),
            message: diagnostic_message(value),
            span: parse_span(value, source_code),
            fix: None,
        },
    )
}

fn parse_span(value: &Value, source: Option<&str>) -> Option<Span> {
    if let Some(start) = value.pointer("/location/range/start") {
        return structured_span(start, value.pointer("/location/range/end"), true);
    }
    if let Some(start) = value.pointer("/location/start") {
        return structured_span(start, value.pointer("/location/end"), false);
    }
    if let Some(start) = value.pointer("/location/span/start") {
        if start.is_object() {
            return structured_span(start, value.pointer("/location/span/end"), false);
        }
    }
    let offsets = value.pointer("/location/span").and_then(Value::as_array);
    if let (Some(offsets), Some(source)) = (offsets, source) {
        let start = offsets.first().and_then(Value::as_u64)?;
        let end = offsets.get(1).and_then(Value::as_u64);
        let (start_line, start_column) = offset_position(source, start as usize);
        let (end_line, end_column) = end
            .map(|offset| offset_position(source, offset as usize))
            .unzip();
        return Some(Span {
            start_line,
            start_column,
            end_line,
            end_column,
        });
    }
    None
}

fn structured_span(start: &Value, end: Option<&Value>, zero_based: bool) -> Option<Span> {
    let offset = u32::from(zero_based);
    let start_line = find_number(start, &["line", "lineNumber"])? + offset;
    let start_column =
        find_number(start, &["column", "columnNumber"]).unwrap_or(1 - offset) + offset;
    Some(Span {
        start_line,
        start_column,
        end_line: end
            .and_then(|value| find_number(value, &["line", "lineNumber"]))
            .map(|value| value + offset),
        end_column: end
            .and_then(|value| find_number(value, &["column", "columnNumber"]))
            .map(|value| value + offset),
    })
}

fn diagnostic_message(value: &Value) -> String {
    if let Some(description) = value.get("description").and_then(Value::as_str) {
        return description.to_owned();
    }
    if let Some(message) = value.get("message") {
        let mut parts = Vec::new();
        collect_message_parts(message, &mut parts);
        if !parts.is_empty() {
            return parts.join("");
        }
    }
    "Biome diagnostic".into()
}

fn collect_message_parts(value: &Value, parts: &mut Vec<String>) {
    match value {
        Value::String(text) => parts.push(text.clone()),
        Value::Array(values) => {
            for value in values {
                collect_message_parts(value, parts);
            }
        }
        Value::Object(object) => {
            if let Some(content) = object.get("content") {
                collect_message_parts(content, parts);
            } else if let Some(text) = object.get("text") {
                collect_message_parts(text, parts);
            }
        }
        _ => {}
    }
}

fn stringish(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("file").and_then(Value::as_str))
        .or_else(|| value.get("name").and_then(Value::as_str))
        .or_else(|| value.get("value").and_then(Value::as_str))
}

fn find_number(value: &Value, keys: &[&str]) -> Option<u32> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
    })
}

fn offset_position(source: &str, offset: usize) -> (u32, u32) {
    let prefix = &source.as_bytes()[..offset.min(source.len())];
    let line = prefix.iter().filter(|byte| **byte == b'\n').count() as u32 + 1;
    let column = prefix
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(prefix.len(), |index| prefix.len() - index - 1) as u32
        + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::parse;
    use crate::matching::AliasTable;

    #[test]
    fn parses_biome_json_fixture() {
        let source = include_str!("../../tests/fixtures/biome-output.json");
        let parsed =
            parse(source, Path::new("/project"), &AliasTable::default()).expect("Biome JSON");
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(
            parsed.diagnostics[0].canonical_code.as_deref(),
            Some("no-debugger")
        );
        assert_eq!(
            parsed.diagnostics[0]
                .span
                .as_ref()
                .map(|span| span.start_line),
            Some(2)
        );
    }

    #[test]
    fn parses_biome_rdjson_fixture() {
        let source = include_str!("../../tests/fixtures/biome-output.rdjson");
        let parsed =
            parse(source, Path::new("/project"), &AliasTable::default()).expect("Biome RDJSON");
        assert_eq!(parsed.diagnostics.len(), 1);
        assert_eq!(
            parsed.diagnostics[0].canonical_code.as_deref(),
            Some("no-console")
        );
        let span = parsed.diagnostics[0].span.as_ref().expect("span");
        assert_eq!((span.start_line, span.start_column), (4, 3));
    }
}
