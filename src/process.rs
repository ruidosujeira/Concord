use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use command_group::CommandGroup;

use crate::config::Config;
use crate::error::{ConcordError, Result, ToolFailure};
use crate::model::Tool;

type ToolResult<T> = std::result::Result<T, Box<ToolFailure>>;

#[derive(Debug, Clone)]
pub struct ResolvedTool {
    pub tool: Tool,
    pub executable: PathBuf,
    pub checked: Vec<String>,
    pub explicitly_configured: bool,
}

#[derive(Debug)]
pub struct ProcessOutput {
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Clone)]
pub struct ProcessRunner {
    root: PathBuf,
    config: Config,
    timeout: Duration,
}

impl ProcessRunner {
    pub fn new(root: PathBuf, config: Config, timeout_seconds: Option<u64>) -> Self {
        Self {
            root,
            timeout: Duration::from_secs(
                timeout_seconds.unwrap_or(config.execution.timeout_seconds),
            ),
            config,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve(&self, tool: Tool) -> ToolResult<ResolvedTool> {
        let configured = self.config.tools.get(tool).command.as_deref();
        let mut checked = Vec::new();

        if let Some(command) = configured {
            let configured_path = Path::new(command);
            if configured_path.is_absolute() || command.contains(['/', '\\']) {
                let candidate = if configured_path.is_absolute() {
                    configured_path.to_path_buf()
                } else {
                    self.root.join(configured_path)
                };
                checked.push(candidate.display().to_string());
                if is_file(&candidate) {
                    return Ok(ResolvedTool {
                        tool,
                        executable: candidate,
                        checked,
                        explicitly_configured: true,
                    });
                }
            } else {
                checked.push(format!("PATH ({command})"));
                if let Ok(candidate) = which::which(command) {
                    return Ok(ResolvedTool {
                        tool,
                        executable: candidate,
                        checked,
                        explicitly_configured: true,
                    });
                }
            }
            return Err(Box::new(not_found(tool, checked)));
        }

        for candidate in local_executable_candidates(&self.root, tool.config_key()) {
            checked.push(candidate.display().to_string());
            if is_file(&candidate) {
                return Ok(ResolvedTool {
                    tool,
                    executable: candidate,
                    checked,
                    explicitly_configured: false,
                });
            }
        }

        checked.push("PATH".into());
        if let Ok(candidate) = which::which(tool.config_key()) {
            return Ok(ResolvedTool {
                tool,
                executable: candidate,
                checked,
                explicitly_configured: false,
            });
        }
        Err(Box::new(not_found(tool, checked)))
    }

    pub fn version(&self, resolved: &ResolvedTool) -> ToolResult<String> {
        let output = self.run_resolved(resolved, vec![OsString::from("--version")], None)?;
        if output.exit_code != Some(0) {
            return Err(Box::new(ToolFailure::Exit {
                tool: resolved.tool.to_string(),
                executable: resolved.executable.display().to_string(),
                arguments: "--version".into(),
                exit_code: output.exit_code,
                stderr: truncate(&String::from_utf8_lossy(&output.stderr), 4_096),
            }));
        }
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let version = if stdout.is_empty() { stderr } else { stdout };
        Ok(if version.is_empty() {
            "unknown".into()
        } else {
            version
        })
    }

    pub fn run(
        &self,
        tool: Tool,
        arguments: Vec<OsString>,
        stdin: Option<&[u8]>,
    ) -> Result<(ResolvedTool, ProcessOutput)> {
        let resolved = self.resolve(tool).map_err(ConcordError::from)?;
        let output = self
            .run_resolved(&resolved, arguments, stdin)
            .map_err(ConcordError::from)?;
        Ok((resolved, output))
    }

    pub fn run_resolved(
        &self,
        resolved: &ResolvedTool,
        arguments: Vec<OsString>,
        stdin: Option<&[u8]>,
    ) -> ToolResult<ProcessOutput> {
        let argument_text = display_arguments(&arguments);
        let mut command = Command::new(&resolved.executable);
        command
            .args(&arguments)
            .current_dir(&self.root)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = Instant::now();
        let mut child = command.group_spawn().map_err(|source| {
            Box::new(ToolFailure::Spawn {
                tool: resolved.tool.to_string(),
                executable: resolved.executable.display().to_string(),
                arguments: argument_text.clone(),
                source,
            })
        })?;

        let stdout = child.inner().stdout.take();
        let stderr = child.inner().stderr.take();
        let stdout_reader = thread::spawn(move || read_all(stdout));
        let stderr_reader = thread::spawn(move || read_all(stderr));
        let input = stdin.map(ToOwned::to_owned);
        let child_stdin = child.inner().stdin.take();
        let stdin_writer = thread::spawn(move || write_all(child_stdin, input));

        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() < self.timeout => {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdin_writer.join();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(Box::new(ToolFailure::Timeout {
                        tool: resolved.tool.to_string(),
                        seconds: self.timeout.as_secs(),
                        executable: resolved.executable.display().to_string(),
                        arguments: argument_text,
                    }));
                }
                Err(source) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Box::new(ToolFailure::Spawn {
                        tool: resolved.tool.to_string(),
                        executable: resolved.executable.display().to_string(),
                        arguments: argument_text,
                        source,
                    }));
                }
            }
        };

        let _ = stdin_writer.join();
        let stdout = stdout_reader.join().unwrap_or_default();
        let stderr = stderr_reader.join().unwrap_or_default();
        Ok(ProcessOutput {
            exit_code: status.code(),
            duration_ms: started.elapsed().as_millis(),
            stdout,
            stderr,
        })
    }
}

fn local_executable_candidates(root: &Path, name: &str) -> Vec<PathBuf> {
    let base = root.join("node_modules").join(".bin").join(name);
    if cfg!(windows) {
        ["cmd", "exe", "bat", "ps1", ""]
            .into_iter()
            .map(|extension| {
                if extension.is_empty() {
                    base.clone()
                } else {
                    base.with_extension(extension)
                }
            })
            .collect()
    } else {
        vec![base]
    }
}

fn is_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn not_found(tool: Tool, checked: Vec<String>) -> ToolFailure {
    ToolFailure::NotFound {
        tool: tool.to_string(),
        config_key: tool.config_key().into(),
        checked: checked
            .iter()
            .map(|item| format!("  {item}"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn read_all<R: Read>(reader: Option<R>) -> Vec<u8> {
    let mut output = Vec::new();
    if let Some(mut reader) = reader {
        let _ = reader.read_to_end(&mut output);
    }
    output
}

fn write_all<W: Write>(writer: Option<W>, input: Option<Vec<u8>>) {
    if let (Some(mut writer), Some(input)) = (writer, input) {
        let _ = writer.write_all(&input);
    }
}

pub fn display_arguments(arguments: &[OsString]) -> String {
    arguments
        .iter()
        .map(|argument| {
            let text = argument.to_string_lossy();
            if text.contains(char::is_whitespace) {
                format!("{text:?}")
            } else {
                text.into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn truncate(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    let truncated: String = value.chars().take(maximum).collect();
    format!("{truncated}\n… output truncated")
}
