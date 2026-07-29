use std::env;
use std::fs;
use std::path::PathBuf;
use std::thread;

use clap::{Args, Parser, Subcommand, ValueEnum};
use rayon::prelude::*;

use crate::adapters::{FormatterAdapter, compare_format_file, run_linter};
use crate::config::{self, LoadedConfig};
use crate::discovery::discover;
use crate::error::{ConcordError, ErrorKind, Result};
use crate::matching::{AliasTable, compare};
use crate::model::Tool;
use crate::process::ProcessRunner;
use crate::reduce::{ReduceMode, ReductionRequest, reduce};
use crate::report::{FormatReport, LintReport, json, terminal};
use crate::scoring;

#[derive(Debug, Parser)]
#[command(
    name = "concord",
    version,
    about = "Compare JavaScript and TypeScript linters and formatters",
    arg_required_else_help = true
)]
pub struct Cli {
    /// Use a specific concord.toml file
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a minimal concord.toml in the current directory
    Init(InitArgs),
    /// Inspect configuration and installed tools
    Doctor,
    /// Compare two tools
    Compare(CompareArgs),
    /// Minimize a file while preserving a selected mismatch
    Reduce(ReduceArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Overwrite an existing concord.toml
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct CompareArgs {
    #[command(subcommand)]
    pub mode: CompareCommand,
}

#[derive(Debug, Subcommand)]
pub enum CompareCommand {
    /// Compare normalized linter diagnostics
    Lint(LintArgs),
    /// Compare formatter output and idempotency
    Format(FormatArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LinterName {
    Eslint,
    Biome,
    Oxlint,
}

impl From<LinterName> for Tool {
    fn from(value: LinterName) -> Self {
        match value {
            LinterName::Eslint => Self::Eslint,
            LinterName::Biome => Self::Biome,
            LinterName::Oxlint => Self::Oxlint,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum FormatterName {
    Prettier,
    Biome,
    Oxfmt,
}

impl From<FormatterName> for Tool {
    fn from(value: FormatterName) -> Self {
        match value {
            FormatterName::Prettier => Self::Prettier,
            FormatterName::Biome => Self::Biome,
            FormatterName::Oxfmt => Self::Oxfmt,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputKind {
    Terminal,
    Json,
}

#[derive(Debug, Args)]
pub struct LintArgs {
    /// Reference linter
    #[arg(long, value_enum)]
    pub baseline: LinterName,
    /// Linter being evaluated
    #[arg(long, value_enum)]
    pub candidate: LinterName,
    /// Files or directories to compare
    #[arg(value_name = "PATH", default_value = ".")]
    pub paths: Vec<PathBuf>,
    /// Report format written to stdout
    #[arg(long, value_enum, default_value = "terminal")]
    pub output: OutputKind,
    /// Do not save a JSON copy under .concord/reports
    #[arg(long)]
    pub no_save_report: bool,
}

#[derive(Debug, Args)]
pub struct FormatArgs {
    /// Reference formatter
    #[arg(long, value_enum)]
    pub baseline: FormatterName,
    /// Formatter being evaluated
    #[arg(long, value_enum)]
    pub candidate: FormatterName,
    /// Files or directories to compare
    #[arg(value_name = "PATH", default_value = ".")]
    pub paths: Vec<PathBuf>,
    /// Treat CRLF and LF as equal; preserve all other byte differences
    #[arg(long)]
    pub normalize_eol: bool,
    /// Report format written to stdout
    #[arg(long, value_enum, default_value = "terminal")]
    pub output: OutputKind,
    /// Do not save a JSON copy under .concord/reports
    #[arg(long)]
    pub no_save_report: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReduceModeArg {
    Lint,
    Format,
}

#[derive(Debug, Args)]
pub struct ReduceArgs {
    /// Kind of comparison to preserve
    #[arg(long, value_enum)]
    pub mode: ReduceModeArg,
    /// Reference tool
    #[arg(long)]
    pub baseline: Tool,
    /// Tool being evaluated
    #[arg(long)]
    pub candidate: Tool,
    /// Source file to minimize
    #[arg(value_name = "PATH")]
    pub path: PathBuf,
    /// Destination for the reduced reproduction
    #[arg(long, value_name = "PATH")]
    pub output: Option<PathBuf>,
    /// Per-process timeout in seconds
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<u64>,
    /// Zero-based mismatch index
    #[arg(long, default_value_t = 0)]
    pub mismatch: usize,
    /// Do not save a JSON reduction report
    #[arg(long)]
    pub no_save_report: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Clean,
    Differences,
}

pub fn run(cli: Cli) -> Result<Outcome> {
    let cwd = env::current_dir().map_err(|error| {
        ConcordError::operational(format!("failed to read current directory: {error}"))
    })?;
    match cli.command {
        Command::Init(arguments) => {
            let path = cli.config.unwrap_or_else(|| cwd.join("concord.toml"));
            config::init(&path, arguments.force)?;
            println!("Created {}", path.display());
            Ok(Outcome::Clean)
        }
        Command::Doctor => doctor(config::load(cli.config.as_deref(), &cwd)?),
        Command::Compare(arguments) => {
            let loaded = config::load(cli.config.as_deref(), &cwd)?;
            match arguments.mode {
                CompareCommand::Lint(arguments) => compare_lint(loaded, arguments),
                CompareCommand::Format(arguments) => compare_format(loaded, arguments),
            }
        }
        Command::Reduce(arguments) => {
            let loaded = config::load(cli.config.as_deref(), &cwd)?;
            reduce_command(loaded, arguments)
        }
    }
}

fn doctor(loaded: LoadedConfig) -> Result<Outcome> {
    let runner = ProcessRunner::new(loaded.root.clone(), loaded.config.clone(), None);
    println!("Concord doctor\n");
    println!("Project root  {}", loaded.root.display());
    println!(
        "Configuration {}",
        loaded.path.as_ref().map_or_else(
            || "<not found; using defaults>".into(),
            |path| { path.display().to_string() }
        )
    );
    println!("\nTools");
    let mut explicit_failure = false;
    for tool in Tool::ALL {
        match runner.resolve(tool) {
            Ok(resolved) => match runner.version(&resolved) {
                Ok(version) => {
                    println!(
                        "  {:<9} found    {} ({})",
                        tool.display_name(),
                        resolved.executable.display(),
                        version.lines().next().unwrap_or("unknown")
                    );
                }
                Err(error) => {
                    println!(
                        "  {:<9} unusable {} ({error})",
                        tool.display_name(),
                        resolved.executable.display()
                    );
                    explicit_failure |= resolved.explicitly_configured;
                }
            },
            Err(error) => {
                println!("  {:<9} missing", tool.display_name());
                if loaded.config.tools.get(tool).command.is_some() {
                    explicit_failure = true;
                    println!("    {error}");
                }
            }
        }
    }
    if explicit_failure {
        Err(ConcordError::operational(
            "one or more explicitly configured tools are unavailable",
        ))
    } else {
        Ok(Outcome::Clean)
    }
}

fn compare_lint(loaded: LoadedConfig, arguments: LintArgs) -> Result<Outcome> {
    let baseline_tool = Tool::from(arguments.baseline);
    let candidate_tool = Tool::from(arguments.candidate);
    if baseline_tool == candidate_tool {
        return Err(ConcordError::usage(
            "baseline and candidate must be different tools",
        ));
    }
    let files = discover(&loaded.root, &arguments.paths, &loaded.config.discovery)?;
    let runner = ProcessRunner::new(loaded.root.clone(), loaded.config.clone(), None);
    let aliases = AliasTable::new(&loaded.config.matching.aliases);
    let (baseline_join, candidate_join) = thread::scope(|scope| {
        let baseline_handle = scope.spawn(|| run_linter(&runner, baseline_tool, &files, &aliases));
        let candidate_handle =
            scope.spawn(|| run_linter(&runner, candidate_tool, &files, &aliases));
        (baseline_handle.join(), candidate_handle.join())
    });
    let baseline = baseline_join
        .map_err(|_| ConcordError::operational("baseline linter worker panicked"))??;
    let candidate = candidate_join
        .map_err(|_| ConcordError::operational("candidate linter worker panicked"))??;
    let result = compare(baseline.diagnostics.clone(), candidate.diagnostics.clone());
    let summary = scoring::calculate(&result);
    let report = LintReport::new(
        &loaded.root,
        files.len(),
        baseline,
        candidate,
        result,
        summary,
    );
    emit_report(
        &loaded.root,
        "lint",
        &report,
        arguments.output,
        arguments.no_save_report,
        || terminal::lint(&report),
    )?;
    if report.has_differences(loaded.config.matching.count_probable_as_match) {
        Ok(Outcome::Differences)
    } else {
        Ok(Outcome::Clean)
    }
}

fn compare_format(loaded: LoadedConfig, arguments: FormatArgs) -> Result<Outcome> {
    let baseline_tool = Tool::from(arguments.baseline);
    let candidate_tool = Tool::from(arguments.candidate);
    if baseline_tool == candidate_tool {
        return Err(ConcordError::usage(
            "baseline and candidate must be different tools",
        ));
    }
    let files = discover(&loaded.root, &arguments.paths, &loaded.config.discovery)?;
    let runner = ProcessRunner::new(loaded.root.clone(), loaded.config.clone(), None);
    let baseline = FormatterAdapter::resolve(&runner, baseline_tool)?;
    let candidate = FormatterAdapter::resolve(&runner, candidate_tool)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(loaded.config.execution.formatter_jobs)
        .build()
        .map_err(|error| {
            ConcordError::operational(format!("failed to create formatter worker pool: {error}"))
        })?;
    let results: Result<Vec<_>> = pool.install(|| {
        files
            .par_iter()
            .map(|path| {
                let input = fs::read(path)
                    .map_err(|error| ConcordError::io("failed to read source file", path, error))?;
                Ok(compare_format_file(
                    &loaded.root,
                    path,
                    &input,
                    &baseline,
                    &candidate,
                    arguments.normalize_eol,
                ))
            })
            .collect()
    });
    let report = FormatReport::new(&loaded.root, baseline_tool, candidate_tool, results?);
    emit_report(
        &loaded.root,
        "format",
        &report,
        arguments.output,
        arguments.no_save_report,
        || terminal::format(&report),
    )?;
    if report.has_failures() {
        return Err(ConcordError::operational(
            "one or more formatter executions failed; see the report above",
        ));
    }
    if report.has_differences() {
        Ok(Outcome::Differences)
    } else {
        Ok(Outcome::Clean)
    }
}

fn reduce_command(loaded: LoadedConfig, arguments: ReduceArgs) -> Result<Outcome> {
    let mode = match arguments.mode {
        ReduceModeArg::Lint => ReduceMode::Lint,
        ReduceModeArg::Format => ReduceMode::Format,
    };
    if arguments.baseline == arguments.candidate {
        return Err(ConcordError::usage(
            "baseline and candidate must be different tools",
        ));
    }
    if arguments.timeout == Some(0) {
        return Err(ConcordError::usage(
            "timeout must be greater than zero seconds",
        ));
    }
    match mode {
        ReduceMode::Lint if !arguments.baseline.is_linter() || !arguments.candidate.is_linter() => {
            return Err(ConcordError::usage(
                "lint reduction supports eslint, biome, and oxlint",
            ));
        }
        ReduceMode::Format
            if !arguments.baseline.is_formatter() || !arguments.candidate.is_formatter() =>
        {
            return Err(ConcordError::usage(
                "format reduction supports prettier, biome, and oxfmt",
            ));
        }
        _ => {}
    }
    let request = ReductionRequest {
        mode,
        baseline: arguments.baseline,
        candidate: arguments.candidate,
        input: if arguments.path.is_absolute() {
            arguments.path
        } else {
            loaded.root.join(arguments.path)
        },
        output: arguments.output.map(|path| {
            if path.is_absolute() {
                path
            } else {
                loaded.root.join(path)
            }
        }),
        mismatch: arguments.mismatch,
        timeout_seconds: arguments.timeout,
    };
    let result = reduce(&loaded.root, &loaded.config, request)?;
    println!("{}", result.terminal_summary());
    if !arguments.no_save_report {
        let path = json::save(&loaded.root, "reduce", &result)?;
        eprintln!("Report saved: {}", path.display());
    }
    Ok(Outcome::Differences)
}

fn emit_report<T: serde::Serialize>(
    root: &std::path::Path,
    mode: &str,
    report: &T,
    output: OutputKind,
    no_save: bool,
    terminal_renderer: impl FnOnce() -> String,
) -> Result<()> {
    match output {
        OutputKind::Terminal => print!("{}", terminal_renderer()),
        OutputKind::Json => println!("{}", json::render(report)?),
    }
    if !no_save {
        let path = json::save(root, mode, report)?;
        eprintln!("Report saved: {}", path.display());
    }
    Ok(())
}

pub fn exit_code(result: &Result<Outcome>) -> u8 {
    match result {
        Ok(Outcome::Clean) => 0,
        Ok(Outcome::Differences) => 1,
        Err(error) if error.kind == ErrorKind::Usage => 2,
        Err(_) => 3,
    }
}
