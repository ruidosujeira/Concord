use std::path::Path;

use serde_json::Value;

use crate::adapters::ParsedDiagnostics;
use crate::matching::AliasTable;
use crate::model::{Diagnostic, DiagnosticData, Fix, Severity, Span, Tool, normalize_path};

pub fn parse(
    source: &str,
    root: &Path,
    aliases: &AliasTable,
) -> std::result::Result<ParsedDiagnostics, String> {
    if source.trim().is_empty() {
        return Ok(ParsedDiagnostics {
            diagnostics: Vec::new(),
            warnings: vec!["ESLint produced empty JSON output".into()],
        });
    }
    let reports: Vec<Value> = serde_json::from_str(source).map_err(|error| error.to_string())?;
    let mut diagnostics = Vec::new();
    for report in reports {
        let file_path = report
            .get("filePath")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let path = normalize_path(root, Path::new(file_path));
        let messages = report
            .get("messages")
            .and_then(Value::as_array)
            .ok_or_else(|| "ESLint result is missing `messages`".to_owned())?;
        for message in messages {
            let code = message
                .get("ruleId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let span = line_span(message);
            let fix = message.get("fix").and_then(parse_fix);
            diagnostics.push(Diagnostic::new(
                Tool::Eslint,
                path.clone(),
                DiagnosticData {
                    canonical_code: code.as_deref().map(|value| aliases.canonicalize(value)),
                    code,
                    severity: Severity::from_value(message.get("severity").unwrap_or(&Value::Null)),
                    message: message
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("ESLint diagnostic")
                        .to_owned(),
                    span,
                    fix,
                },
            ));
        }
    }
    Ok(ParsedDiagnostics {
        diagnostics,
        warnings: Vec::new(),
    })
}

fn line_span(value: &Value) -> Option<Span> {
    let start_line = number_u32(value.get("line")?)?;
    let start_column = number_u32(value.get("column").unwrap_or(&Value::Null)).unwrap_or(1);
    Some(Span {
        start_line,
        start_column,
        end_line: value.get("endLine").and_then(number_u32),
        end_column: value.get("endColumn").and_then(number_u32),
    })
}

fn parse_fix(value: &Value) -> Option<Fix> {
    let range = value.get("range").and_then(Value::as_array);
    Some(Fix {
        start: range
            .and_then(|values| values.first())
            .and_then(Value::as_u64),
        end: range
            .and_then(|values| values.get(1))
            .and_then(Value::as_u64),
        replacement: value.get("text").and_then(Value::as_str).map(str::to_owned),
    })
}

pub(crate) fn number_u32(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|number| u32::try_from(number).ok())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::parse;
    use crate::matching::AliasTable;
    use crate::model::Severity;

    #[test]
    fn parses_eslint_fixture() {
        let source = include_str!("../../tests/fixtures/eslint-output.json");
        let parsed =
            parse(source, Path::new("/project"), &AliasTable::default()).expect("ESLint JSON");
        assert_eq!(parsed.diagnostics.len(), 2);
        assert_eq!(
            parsed.diagnostics[0].canonical_code.as_deref(),
            Some("no-debugger")
        );
        assert_eq!(parsed.diagnostics[0].severity, Severity::Error);
        assert!(parsed.diagnostics[0].fix.is_some());
    }
}
