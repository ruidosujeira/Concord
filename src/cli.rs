use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

use clap::{Args, Parser, Subcommand, ValueEnum};
use rayon::prelude::*;

use crate::adapters::{FormatterAdapter, compare_format_file, run_linter};
use crate::capabilities::{ComparisonPlan, ToolAvailability, UnsupportedPolicy};
use crate::config::{self, LoadedConfig};
use crate::discovery::discover_excluding;
use crate::error::{ConcordError, ErrorKind, Result};
use crate::matching::{AliasTable, RuleMappingTable, compare_with_mappings};
use crate::model::Tool;
use crate::process::ProcessRunner;
use crate::reduce::{ReduceMode, ReductionRequest, reduce};
use crate::report::{
    ComparisonProfile, FormatFileResult, FormatReport, LintReport, PlanReport, PlanRuleMapping,
    json, terminal,
};
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
    /// Plan a comparison without executing linters or formatters
    Plan(PlanArgs),
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
pub struct PlanArgs {
    #[command(subcommand)]
    pub mode: PlanCommand,
}

#[derive(Debug, Subcommand)]
pub enum PlanCommand {
    /// Plan a linter comparison
    Lint(PlanLintArgs),
    /// Plan a formatter comparison
    Format(PlanFormatArgs),
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
pub struct ReportArgs {
    /// Report format written to stdout
    #[arg(long, value_enum, default_value = "terminal")]
    pub output: OutputKind,
    /// Atomically write JSON to this path instead of .concord/reports
    #[arg(long, value_name = "PATH")]
    pub report_file: Option<PathBuf>,
    /// Do not save a JSON copy
    #[arg(long)]
    pub no_save_report: bool,
}

#[derive(Debug, Args)]
pub struct PlanLintArgs {
    /// Reference linter
    #[arg(long, value_enum)]
    pub baseline: LinterName,
    /// Linter being evaluated
    #[arg(long, value_enum)]
    pub candidate: LinterName,
    /// Files or directories to inspect
    #[arg(value_name = "PATH", default_value = ".")]
    pub paths: Vec<PathBuf>,
    #[command(flatten)]
    pub report: ReportArgs,
}

#[derive(Debug, Args)]
pub struct PlanFormatArgs {
    /// Reference formatter
    #[arg(long, value_enum)]
    pub baseline: FormatterName,
    /// Formatter being evaluated
    #[arg(long, value_enum)]
    pub candidate: FormatterName,
    /// Files or directories to inspect
    #[arg(value_name = "PATH", default_value = ".")]
    pub paths: Vec<PathBuf>,
    #[command(flatten)]
    pub report: ReportArgs,
}

#[derive(Debug, Args)]
pub struct LintArgs {
    /// Reference linter
    #[arg(long, value_enum)]
    pub baseline: LinterName,
    /// Linter being evaluated
    #[arg(long, value_enum)]
    pub candidate: LinterName,
    /// Select raw or mapped-rule comparable metrics and exit behavior
    #[arg(long, value_enum, default_value = "raw")]
    pub profile: ComparisonProfile,
    /// Override comparison.unsupported
    #[arg(long, value_enum)]
    pub unsupported_policy: Option<UnsupportedPolicy>,
    /// Files or directories to compare
    #[arg(value_name = "PATH", default_value = ".")]
    pub paths: Vec<PathBuf>,
    #[command(flatten)]
    pub report: ReportArgs,
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
    /// Override comparison.unsupported
    #[arg(long, value_enum)]
    pub unsupported_policy: Option<UnsupportedPolicy>,
    #[command(flatten)]
    pub report: ReportArgs,
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
    /// Atomically write the JSON reduction report to this path
    #[arg(long, value_name = "PATH")]
    pub report_file: Option<PathBuf>,
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
        Command::Doctor => {
            let loaded = load_with_warnings(cli.config.as_deref(), &cwd)?;
            doctor(loaded)
        }
        Command::Plan(arguments) => {
            let loaded = load_with_warnings(cli.config.as_deref(), &cwd)?;
            match arguments.mode {
                PlanCommand::Lint(arguments) => plan_lint(loaded, arguments),
                PlanCommand::Format(arguments) => plan_format(loaded, arguments),
            }
        }
        Command::Compare(arguments) => {
            let loaded = load_with_warnings(cli.config.as_deref(), &cwd)?;
            match arguments.mode {
                CompareCommand::Lint(arguments) => compare_lint(loaded, arguments),
                CompareCommand::Format(arguments) => compare_format(loaded, arguments),
            }
        }
        Command::Reduce(arguments) => {
            let loaded = load_with_warnings(cli.config.as_deref(), &cwd)?;
            reduce_command(loaded, arguments)
        }
    }
}

fn load_with_warnings(explicit: Option<&Path>, cwd: &Path) -> Result<LoadedConfig> {
    let loaded = config::load(explicit, cwd)?;
    for warning in &loaded.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(loaded)
}

fn doctor(loaded: LoadedConfig) -> Result<Outcome> {
    let runner = ProcessRunner::new(loaded.root.clone(), loaded.config.clone(), None);
    println!("Concord doctor\n");
    println!("Project root  {}", loaded.root.display());
    println!(
        "Configuration {}",
        loaded.path.as_ref().map_or_else(
            || "<not found; using defaults>".into(),
            |path| path.display().to_string()
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

fn plan_lint(loaded: LoadedConfig, arguments: PlanLintArgs) -> Result<Outcome> {
    plan_command(
        loaded,
        "lint",
        Tool::from(arguments.baseline),
        Tool::from(arguments.candidate),
        arguments.paths,
        arguments.report,
    )
}

fn plan_format(loaded: LoadedConfig, arguments: PlanFormatArgs) -> Result<Outcome> {
    plan_command(
        loaded,
        "format",
        Tool::from(arguments.baseline),
        Tool::from(arguments.candidate),
        arguments.paths,
        arguments.report,
    )
}

fn plan_command(
    loaded: LoadedConfig,
    mode: &str,
    baseline: Tool,
    candidate: Tool,
    paths: Vec<PathBuf>,
    report_args: ReportArgs,
) -> Result<Outcome> {
    validate_pair(baseline, candidate)?;
    validate_report_args(&report_args)?;
    let report_path = resolved_report_path(&loaded.root, report_args.report_file.as_deref());
    let exclusions: Vec<_> = report_path.iter().cloned().collect();
    let files = discover_excluding(&loaded.root, &paths, &loaded.config.discovery, &exclusions)?;
    let runner = ProcessRunner::new(loaded.root.clone(), loaded.config.clone(), None);
    let tools = vec![
        availability(&runner, baseline),
        availability(&runner, candidate),
    ];
    let plan = ComparisonPlan::build(
        &loaded.root,
        &files,
        baseline,
        candidate,
        None,
        None,
        &loaded.capabilities,
        tools,
    );
    let rule_mappings = configured_plan_mappings(&loaded, baseline, candidate);
    let report = PlanReport::new(mode, baseline, candidate, plan, rule_mappings);
    emit_report(
        &loaded.root,
        &format!("plan-{mode}"),
        &report,
        &report_args,
        || terminal::plan(&report),
    )?;
    Ok(Outcome::Clean)
}

fn compare_lint(loaded: LoadedConfig, arguments: LintArgs) -> Result<Outcome> {
    let baseline_tool = Tool::from(arguments.baseline);
    let candidate_tool = Tool::from(arguments.candidate);
    validate_pair(baseline_tool, candidate_tool)?;
    validate_report_args(&arguments.report)?;
    let report_path = resolved_report_path(&loaded.root, arguments.report.report_file.as_deref());
    let exclusions: Vec<_> = report_path.iter().cloned().collect();
    let files = discover_excluding(
        &loaded.root,
        &arguments.paths,
        &loaded.config.discovery,
        &exclusions,
    )?;
    let runner = ProcessRunner::new(loaded.root.clone(), loaded.config.clone(), None);
    let plan = ComparisonPlan::build(
        &loaded.root,
        &files,
        baseline_tool,
        candidate_tool,
        None,
        None,
        &loaded.capabilities,
        vec![
            availability(&runner, baseline_tool),
            availability(&runner, candidate_tool),
        ],
    );
    let comparable_files = plan.comparable_paths(&loaded.root);
    let aliases = AliasTable::new(&loaded.config.matching.aliases);
    let mappings = RuleMappingTable::new(
        &loaded.config.matching.rules,
        &loaded.config.matching.aliases,
    );
    let (baseline_join, candidate_join) = thread::scope(|scope| {
        let baseline_handle =
            scope.spawn(|| run_linter(&runner, baseline_tool, &comparable_files, &aliases));
        let candidate_handle =
            scope.spawn(|| run_linter(&runner, candidate_tool, &comparable_files, &aliases));
        (baseline_handle.join(), candidate_handle.join())
    });
    let baseline = baseline_join
        .map_err(|_| ConcordError::operational("baseline linter worker panicked"))??;
    let candidate = candidate_join
        .map_err(|_| ConcordError::operational("candidate linter worker panicked"))??;
    let result = compare_with_mappings(
        baseline.diagnostics.clone(),
        candidate.diagnostics.clone(),
        &mappings,
        baseline_tool,
        candidate_tool,
    );
    let (raw_summary, comparable_summary, mapping_summary) = scoring::calculate(
        &result,
        &baseline.diagnostics,
        &candidate.diagnostics,
        &mappings,
        baseline_tool,
        candidate_tool,
        plan.discovered_count(),
        plan.comparable_count(),
    );
    let report = LintReport::new(
        &loaded.root,
        arguments.profile,
        plan,
        baseline,
        candidate,
        result,
        raw_summary,
        comparable_summary,
        mapping_summary,
    );
    emit_report(&loaded.root, "lint", &report, &arguments.report, || {
        terminal::lint(&report)
    })?;
    let policy = arguments
        .unsupported_policy
        .unwrap_or(loaded.config.comparison.unsupported);
    apply_unsupported_policy(report.plan.unsupported_count(), policy)?;
    if report.has_differences(
        arguments.profile,
        loaded.config.matching.count_probable_as_match,
    ) || (report.plan.unsupported_count() > 0 && policy == UnsupportedPolicy::Difference)
    {
        Ok(Outcome::Differences)
    } else {
        Ok(Outcome::Clean)
    }
}

fn compare_format(loaded: LoadedConfig, arguments: FormatArgs) -> Result<Outcome> {
    let baseline_tool = Tool::from(arguments.baseline);
    let candidate_tool = Tool::from(arguments.candidate);
    validate_pair(baseline_tool, candidate_tool)?;
    validate_report_args(&arguments.report)?;
    let report_path = resolved_report_path(&loaded.root, arguments.report.report_file.as_deref());
    let exclusions: Vec<_> = report_path.iter().cloned().collect();
    let files = discover_excluding(
        &loaded.root,
        &arguments.paths,
        &loaded.config.discovery,
        &exclusions,
    )?;
    let runner = ProcessRunner::new(loaded.root.clone(), loaded.config.clone(), None);
    let tools = vec![
        availability(&runner, baseline_tool),
        availability(&runner, candidate_tool),
    ];
    let preliminary_plan = ComparisonPlan::build(
        &loaded.root,
        &files,
        baseline_tool,
        candidate_tool,
        None,
        None,
        &loaded.capabilities,
        tools.clone(),
    );
    let (plan, mut results) = if preliminary_plan.comparable_count() == 0 {
        (preliminary_plan, Vec::new())
    } else {
        let baseline = FormatterAdapter::resolve(&runner, baseline_tool)?;
        let candidate = FormatterAdapter::resolve(&runner, candidate_tool)?;
        let plan = ComparisonPlan::build(
            &loaded.root,
            &files,
            baseline_tool,
            candidate_tool,
            Some(baseline.version()),
            Some(candidate.version()),
            &loaded.capabilities,
            tools,
        );
        let comparable_files = plan.comparable_paths(&loaded.root);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(loaded.config.execution.formatter_jobs)
            .build()
            .map_err(|error| {
                ConcordError::operational(format!(
                    "failed to create formatter worker pool: {error}"
                ))
            })?;
        let executed: Result<Vec<_>> = pool.install(|| {
            comparable_files
                .par_iter()
                .map(|path| {
                    let input = fs::read(path).map_err(|error| {
                        ConcordError::io("failed to read source file", path, error)
                    })?;
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
        (plan, executed?)
    };
    results.extend(
        plan.discovered
            .iter()
            .filter(|file| {
                !(file.baseline.status.is_comparable() && file.candidate.status.is_comparable())
            })
            .map(FormatFileResult::from_plan),
    );
    let report = FormatReport::new(&loaded.root, baseline_tool, candidate_tool, plan, results);
    emit_report(&loaded.root, "format", &report, &arguments.report, || {
        terminal::format(&report)
    })?;
    if report.has_failures() {
        return Err(ConcordError::operational(
            "one or more formatter executions failed; see the report above",
        ));
    }
    let policy = arguments
        .unsupported_policy
        .unwrap_or(loaded.config.comparison.unsupported);
    apply_unsupported_policy(report.summary.unsupported, policy)?;
    if report.has_differences()
        || (report.summary.unsupported > 0 && policy == UnsupportedPolicy::Difference)
    {
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
    if arguments.report_file.is_some() && arguments.no_save_report {
        return Err(ConcordError::usage(
            "--report-file cannot be used with --no-save-report",
        ));
    }
    validate_pair(arguments.baseline, arguments.candidate)?;
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
        input: absolute_from_root(&loaded.root, arguments.path),
        output: arguments
            .output
            .map(|path| absolute_from_root(&loaded.root, path)),
        mismatch: arguments.mismatch,
        timeout_seconds: arguments.timeout,
    };
    let result = reduce(&loaded.root, &loaded.config, request)?;
    println!("{}", result.terminal_summary());
    if let Some(path) = resolved_report_path(&loaded.root, arguments.report_file.as_deref()) {
        json::save_to(&path, &result)?;
        eprintln!("Report saved: {}", path.display());
    } else if !arguments.no_save_report {
        let path = json::save(&loaded.root, "reduce", &result)?;
        eprintln!("Report saved: {}", path.display());
    }
    Ok(Outcome::Differences)
}

fn validate_pair(baseline: Tool, candidate: Tool) -> Result<()> {
    if baseline == candidate {
        Err(ConcordError::usage(
            "baseline and candidate must be different tools",
        ))
    } else {
        Ok(())
    }
}

fn validate_report_args(arguments: &ReportArgs) -> Result<()> {
    if arguments.report_file.is_some() && arguments.no_save_report {
        Err(ConcordError::usage(
            "--report-file cannot be used with --no-save-report",
        ))
    } else {
        Ok(())
    }
}

fn availability(runner: &ProcessRunner, tool: Tool) -> ToolAvailability {
    match runner.resolve(tool) {
        Ok(resolved) => ToolAvailability {
            tool,
            available: true,
            executable: Some(resolved.executable.to_string_lossy().into_owned()),
            reason: None,
        },
        Err(error) => ToolAvailability {
            tool,
            available: false,
            executable: None,
            reason: Some(error.to_string()),
        },
    }
}

fn configured_plan_mappings(
    loaded: &LoadedConfig,
    baseline: Tool,
    candidate: Tool,
) -> Vec<PlanRuleMapping> {
    let mut result = Vec::new();
    for mapping in &loaded.config.matching.rules {
        if mapping.baseline_tool == baseline && mapping.candidate_tool == candidate {
            result.push(PlanRuleMapping {
                baseline_tool: baseline,
                baseline: mapping.baseline.clone(),
                candidate_tool: candidate,
                candidate: mapping.candidate.clone(),
                confidence: mapping.confidence,
                notes: mapping.notes.clone(),
            });
        } else if mapping.baseline_tool == candidate && mapping.candidate_tool == baseline {
            result.push(PlanRuleMapping {
                baseline_tool: baseline,
                baseline: mapping.candidate.clone(),
                candidate_tool: candidate,
                candidate: mapping.baseline.clone(),
                confidence: mapping.confidence,
                notes: mapping.notes.clone(),
            });
        }
    }
    for alias in &loaded.config.matching.aliases {
        let baseline_code = alias
            .tool_values()
            .find_map(|(tool, code)| (tool == baseline).then_some(code));
        let candidate_code = alias
            .tool_values()
            .find_map(|(tool, code)| (tool == candidate).then_some(code));
        if let (Some(baseline_code), Some(candidate_code)) = (baseline_code, candidate_code) {
            result.push(PlanRuleMapping {
                baseline_tool: baseline,
                baseline: baseline_code.into(),
                candidate_tool: candidate,
                candidate: candidate_code.into(),
                confidence: crate::matching::MappingConfidence::Exact,
                notes: Some("converted from deprecated matching.aliases".into()),
            });
        }
    }
    result.sort();
    result.dedup();
    result
}

fn apply_unsupported_policy(count: usize, policy: UnsupportedPolicy) -> Result<()> {
    if count > 0 && policy == UnsupportedPolicy::Error {
        Err(ConcordError::operational(format!(
            "{count} unsupported file(s) encountered under the error policy"
        )))
    } else {
        Ok(())
    }
}

fn absolute_from_root(root: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn resolved_report_path(root: &Path, path: Option<&Path>) -> Option<PathBuf> {
    path.map(|path| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        }
    })
}

fn emit_report<T: serde::Serialize>(
    root: &Path,
    mode: &str,
    report: &T,
    arguments: &ReportArgs,
    terminal_renderer: impl FnOnce() -> String,
) -> Result<()> {
    match arguments.output {
        OutputKind::Terminal => print!("{}", terminal_renderer()),
        OutputKind::Json => println!("{}", json::render(report)?),
    }
    if let Some(path) = resolved_report_path(root, arguments.report_file.as_deref()) {
        json::save_to(&path, report)?;
        eprintln!("Report saved: {}", path.display());
    } else if !arguments.no_save_report {
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
