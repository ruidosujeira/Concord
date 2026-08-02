use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let executable = env::current_exe().unwrap_or_default();
    let name = executable
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .trim_end_matches(".cmd")
        .to_ascii_lowercase();
    record_invocation(&name);
    if env::args().any(|argument| argument == "--version") {
        if name == "oxfmt" {
            println!(
                "{}",
                env::var("CONCORD_FAKE_OXFMT_VERSION").unwrap_or_else(|_| "1.0.0-test".into())
            );
        } else {
            println!("1.0.0-test");
        }
        return ExitCode::SUCCESS;
    }
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

fn record_invocation(name: &str) {
    let Ok(path) = env::var("CONCORD_FAKE_INVOCATION_LOG") else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let arguments = env::args().skip(1).collect::<Vec<_>>().join(" ");
        let _ = writeln!(file, "{name} {arguments}");
    }
}

fn fake_eslint() -> ExitCode {
    fake_delay("CONCORD_FAKE_ESLINT_DELAY_MS");
    let files = source_files();
    let mut reports = Vec::new();
    let mut found = false;
    for path in files {
        let source = fs::read_to_string(&path).unwrap_or_default();
        if source.contains("SLOW") {
            println!("partial stdout before timeout");
            eprintln!("partial stderr before timeout");
            let _ = io::stdout().flush();
            let _ = io::stderr().flush();
            std::thread::sleep(std::time::Duration::from_secs(8));
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
        for (needle, variable) in [
            ("catch (error)", "error"),
            ("userTryingToGet", "userTryingToGet"),
        ] {
            if let Some((line, column)) = locate(&source, needle) {
                found = true;
                let width = u32::try_from(needle.len()).unwrap_or(u32::MAX);
                messages.push(serde_json::json!({
                    "ruleId": "no-unused-vars",
                    "severity": 1,
                    "message": format!("This variable {variable} is unused."),
                    "line": line,
                    "column": column,
                    "endLine": line,
                    "endColumn": column.saturating_add(width)
                }));
            }
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
    fake_delay("CONCORD_FAKE_BIOME_DELAY_MS");
    let files = source_files();
    let mut diagnostics = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path).unwrap_or_default();
        if source.contains("INVALID_BIOME_JSON") {
            println!("invalid biome stdout");
            eprintln!("invalid biome stderr");
            return ExitCode::SUCCESS;
        }
        if source.contains("BIOME_CRASH") {
            println!("partial biome stdout");
            eprintln!("biome crashed after starting");
            return ExitCode::from(2);
        }
        if source.contains("BIOME_SUCCESS_WARNING") {
            eprintln!("Biome configuration warning");
        }
        if let Some((line, column)) = locate(&source, "CUSTOM_BIOME") {
            diagnostics.push(serde_json::json!({
                "category": "lint/custom/noMagic",
                "severity": "warning",
                "description": "Custom unmapped diagnostic.",
                "location": {
                    "path": {"file": absolute(&path)},
                    "start": {"line": line, "column": column},
                    "end": {"line": line, "column": column + 12}
                }
            }));
        }
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
    let (duration, scanner_duration) = fake_biome_timings();
    println!(
        "{}",
        serde_json::json!({
            "summary": {
                "duration": duration,
                "scannerDuration": scanner_duration
            },
            "diagnostics": diagnostics
        })
    );
    ExitCode::from(u8::from(found))
}

fn fake_biome_timings() -> (u64, u64) {
    let path = env::current_dir()
        .unwrap_or_default()
        .join(".concord-fake-biome-run");
    let run = fs::read_to_string(&path)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
        + 1;
    let _ = fs::write(path, run.to_string());
    if run == 1 { (12, 4) } else { (89, 31) }
}

fn fake_delay(variable: &str) {
    if let Some(milliseconds) = env::var(variable)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(milliseconds));
    }
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
    if input.contains("FORMAT_CRASH") {
        print!("partial formatter stdout");
        eprintln!("formatter crashed after starting");
        return ExitCode::from(2);
    }
    if input.contains("UNKNOWN_UNSUPPORTED") {
        eprintln!("unsupported internal state caused a crash");
        return ExitCode::from(2);
    }
    if input.contains("SLOW_FORMAT") {
        println!("partial formatter stdout before timeout");
        eprintln!("partial formatter stderr before timeout");
        let _ = io::stdout().flush();
        let _ = io::stderr().flush();
        std::thread::sleep(std::time::Duration::from_secs(8));
    }
    if input.contains("FORMAT_SUCCESS_WARNING") {
        eprintln!("formatter configuration warning");
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
