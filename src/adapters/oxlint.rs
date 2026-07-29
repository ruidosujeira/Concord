use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::adapters::ParsedDiagnostics;
use crate::adapters::eslint::number_u32;
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
            warnings: vec!["Oxlint produced empty JSON output".into()],
        });
    }
    let value: Value = serde_json::from_str(source).map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    collect_entries(&value, None, &mut entries);
    let diagnostics = entries
        .into_iter()
        .map(|(diagnostic, inherited_path)| convert(diagnostic, inherited_path, root, aliases))
        .collect();
    Ok(ParsedDiagnostics {
        diagnostics,
        warnings: Vec::new(),
    })
}

fn collect_entries<'a>(
    value: &'a Value,
    inherited_path: Option<&'a str>,
    output: &mut Vec<(&'a Value, Option<&'a str>)>,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_entries(value, inherited_path, output);
            }
        }
        Value::Object(object) => {
            let path = object
                .get("filePath")
                .or_else(|| object.get("filename"))
                .and_then(Value::as_str)
                .or(inherited_path);
            if let Some(messages) = object.get("messages").and_then(Value::as_array) {
                for message in messages {
                    output.push((message, path));
                }
            } else if let Some(diagnostics) = object.get("diagnostics").and_then(Value::as_array) {
                for diagnostic in diagnostics {
                    output.push((diagnostic, path));
                }
            } else if object.contains_key("message")
                && (object.contains_key("code") || object.contains_key("ruleId"))
            {
                output.push((value, path));
            }
        }
        _ => {}
    }
}

fn convert(
    value: &Value,
    inherited_path: Option<&str>,
    root: &Path,
    aliases: &AliasTable,
) -> Diagnostic {
    let code = value
        .get("code")
        .or_else(|| value.get("ruleId"))
        .and_then(code_string)
        .map(str::to_owned);
    let raw_path = value
        .get("filename")
        .or_else(|| value.get("filePath"))
        .and_then(Value::as_str)
        .or(inherited_path)
        .unwrap_or("<unknown>");
    let normalized_path = normalize_path(root, Path::new(raw_path));
    let source = fs::read_to_string(if Path::new(raw_path).is_absolute() {
        Path::new(raw_path).to_path_buf()
    } else {
        root.join(raw_path)
    })
    .ok();
    Diagnostic::new(
        Tool::Oxlint,
        normalized_path,
        DiagnosticData {
            canonical_code: code.as_deref().map(|rule| aliases.canonicalize(rule)),
            code,
            severity: Severity::from_value(value.get("severity").unwrap_or(&Value::Null)),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Oxlint diagnostic")
                .to_owned(),
            span: parse_span(value, source.as_deref()),
            fix: None,
        },
    )
}

fn parse_span(value: &Value, source: Option<&str>) -> Option<Span> {
    if let Some(line) = value.get("line").and_then(number_u32) {
        return Some(Span {
            start_line: line,
            start_column: value.get("column").and_then(number_u32).unwrap_or(1),
            end_line: value.get("endLine").and_then(number_u32),
            end_column: value.get("endColumn").and_then(number_u32),
        });
    }
    let span = value
        .get("labels")
        .and_then(Value::as_array)
        .and_then(|labels| labels.first())
        .and_then(|label| label.get("span"))
        .or_else(|| value.get("span"))?;
    if let Some(line) = span.get("line").and_then(number_u32) {
        let calculated_end = source.and_then(|source| {
            let offset = span.get("offset").and_then(Value::as_u64)? as usize;
            let length = span.get("length").and_then(Value::as_u64)? as usize;
            Some(offset_position(source, offset.saturating_add(length)))
        });
        return Some(Span {
            start_line: line,
            start_column: span.get("column").and_then(number_u32).unwrap_or(1),
            end_line: span
                .get("endLine")
                .and_then(number_u32)
                .or_else(|| calculated_end.map(|position| position.0)),
            end_column: span
                .get("endColumn")
                .and_then(number_u32)
                .or_else(|| calculated_end.map(|position| position.1)),
        });
    }
    let source = source?;
    let offset = span.get("offset").and_then(Value::as_u64)? as usize;
    let length = span.get("length").and_then(Value::as_u64).unwrap_or(0) as usize;
    let (start_line, start_column) = offset_position(source, offset);
    let (end_line, end_column) = offset_position(source, offset.saturating_add(length));
    Some(Span {
        start_line,
        start_column,
        end_line: Some(end_line),
        end_column: Some(end_column),
    })
}

fn code_string(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("name").and_then(Value::as_str))
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
    fn parses_oxlint_fixture() {
        let source = include_str!("../../tests/fixtures/oxlint-output.json");
        let parsed =
            parse(source, Path::new("/project"), &AliasTable::default()).expect("Oxlint JSON");
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
}
