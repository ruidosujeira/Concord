use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    if env::args().any(|argument| argument == "--version") {
        println!("1.0.0-test");
        return ExitCode::SUCCESS;
    }
    let executable = env::current_exe().unwrap_or_default();
    let name = executable
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .trim_end_matches(".cmd")
        .to_ascii_lowercase();
    match name.as_str() {
        "eslint" => fake_eslint(),
        "biome" => fake_biome(),
        "oxlint" => fake_oxlint(),
        "prettier" => fake_formatter(false),
        "oxfmt" => fake_formatter(true),
        _ => {
            eprintln!("unknown fake tool name: {name}");
            ExitCode::from(2)
        }
    }
}

fn fake_eslint() -> ExitCode {
    let files = source_files();
    let mut reports = Vec::new();
    let mut found = false;
    for path in files {
        let source = fs::read_to_string(&path).unwrap_or_default();
        if source.contains("SLOW") {
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
        let mut messages = Vec::new();
        if let Some((line, column)) = locate(&source, "debugger") {
            found = true;
            messages.push(serde_json::json!({
                "ruleId": "no-debugger",
                "severity": 2,
                "message": "Unexpected debugger statement.",
                "line": line,
                "column": column,
                "endLine": line,
                "endColumn": column + 8
            }));
        }
        reports.push(serde_json::json!({
            "filePath": absolute(&path),
            "messages": messages,
            "errorCount": usize::from(found),
            "warningCount": 0
        }));
    }
    println!("{}", serde_json::Value::Array(reports));
    ExitCode::from(u8::from(found))
}

fn fake_biome() -> ExitCode {
    if env::args().any(|argument| argument == "format") {
        return fake_formatter(false);
    }
    let files = source_files();
    let mut diagnostics = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path).unwrap_or_default();
        if let Some((line, column)) = locate(&source, "console") {
            diagnostics.push(serde_json::json!({
                "category": "lint/suspicious/noConsole",
                "severity": "warning",
                "description": "Do not use console.",
                "location": {
                    "path": {"file": absolute(&path)},
                    "start": {"line": line, "column": column},
                    "end": {"line": line, "column": column + 7}
                }
            }));
        }
    }
    let found = !diagnostics.is_empty();
    println!("{}", serde_json::json!({"diagnostics": diagnostics}));
    ExitCode::from(u8::from(found))
}

fn fake_oxlint() -> ExitCode {
    let files = source_files();
    let mut diagnostics = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path).unwrap_or_default();
        if let Some((line, column)) = locate(&source, "debugger") {
            diagnostics.push(serde_json::json!({
                "filename": absolute(&path),
                "code": "eslint(no-debugger)",
                "severity": "error",
                "message": "Unexpected debugger statement.",
                "line": line,
                "column": column,
                "endLine": line,
                "endColumn": column + 8
            }));
        }
    }
    let found = !diagnostics.is_empty();
    println!("{}", serde_json::json!({"diagnostics": diagnostics}));
    ExitCode::from(u8::from(found))
}

fn fake_formatter(different: bool) -> ExitCode {
    let mut input = String::new();
    if io::stdin().read_to_string(&mut input).is_err() {
        return ExitCode::from(2);
    }
    if different && input.contains("DIFF") && !input.ends_with("// oxfmt\n") {
        input.push_str("// oxfmt\n");
    }
    print!("{input}");
    ExitCode::SUCCESS
}

fn source_files() -> Vec<String> {
    env::args()
        .skip(1)
        .filter(|argument| {
            !argument.starts_with('-')
                && !matches!(argument.as_str(), "json" | "check" | "format" | "none")
                && Path::new(argument).extension().is_some()
        })
        .collect()
}

fn locate(source: &str, needle: &str) -> Option<(u32, u32)> {
    source.lines().enumerate().find_map(|(line_index, line)| {
        line.find(needle)
            .map(|column| (line_index as u32 + 1, column as u32 + 1))
    })
}

fn absolute(path: &str) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| Path::new(path).to_path_buf())
        .to_string_lossy()
        .into_owned()
}
