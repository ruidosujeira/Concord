//! One observed divergence, and the fingerprint that identifies it across
//! projects.
//!
//! A fingerprint is the hex SHA-256 digest of a canonical serialization of
//! six components joined with U+001E:
//!
//! 1. the literal tag `concord-corpus-finding/1`;
//! 2. the comparison mode, `lint` or `format`;
//! 3. the baseline tool key;
//! 4. the candidate tool key;
//! 5. the divergence category as the existing comparison types express it;
//! 6. the rule identity, or the empty string when the divergence has none;
//! 7. the normalized shape of the disagreement.
//!
//! No component depends on the project, the file path, the file name, a line
//! or column number, or an absolute path, so the same underlying tool bug
//! fingerprints identically wherever it is observed.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::{Severity, Tool};
use crate::process::truncate;
use crate::reduce::{DiagnosticSignature, MismatchSignature, ReduceMode};

use super::normalize;

const FINGERPRINT_TAG: &str = "concord-corpus-finding/1";
const SEPARATOR: char = '\u{1e}';
const MAX_DETAIL: usize = 4_096;

/// The part of a corpus run that is constant across every finding.
#[derive(Debug, Clone, Copy)]
pub struct FindingContext {
    pub mode: ReduceMode,
    pub baseline: Tool,
    pub candidate: Tool,
}

impl FindingContext {
    pub fn mode_name(self) -> &'static str {
        match self.mode {
            ReduceMode::Lint => "lint",
            ReduceMode::Format => "format",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub fingerprint: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// The affected project, in manifest form.
    pub project: String,
    /// Relative to the project root, with forward slashes on every platform.
    pub path: String,
    pub file_bytes: u64,
    /// Human-readable evidence. Never contains an absolute path.
    pub detail: String,
    /// The mismatch identity the reducer preserves. Carried so that a
    /// representative occurrence can be reduced without re-deriving it.
    pub signature: MismatchSignature,
}

impl Finding {
    pub fn new(
        context: FindingContext,
        signature: MismatchSignature,
        shape: &str,
        project: &str,
        path: &str,
        file_bytes: u64,
        detail: &str,
    ) -> Self {
        let fingerprint = fingerprint(
            context,
            &signature.category,
            signature.canonical_code.as_deref(),
            shape,
        );
        Self {
            fingerprint,
            category: signature.category.clone(),
            rule: signature.canonical_code.clone(),
            project: project.to_owned(),
            path: path.to_owned(),
            file_bytes,
            detail: truncate(detail, MAX_DETAIL),
            signature,
        }
    }

    /// A lint divergence. The rule identity already names the check, so the
    /// shape carries severities only; messages are folded in when there is no
    /// rule identity, or when the message difference is itself the divergence.
    pub fn lint(
        context: FindingContext,
        signature: MismatchSignature,
        project: &str,
        path: &str,
        file_bytes: u64,
    ) -> Self {
        let shape = lint_shape(&signature);
        let detail = lint_detail(&signature);
        Self::new(
            context, signature, &shape, project, path, file_bytes, &detail,
        )
    }

    /// A formatter divergence. The shape is the normalized before/after
    /// fragments of the unified diff.
    pub fn format(
        context: FindingContext,
        signature: MismatchSignature,
        diff: Option<&str>,
        project: &str,
        path: &str,
        file_bytes: u64,
    ) -> Self {
        let shape = diff.map(diff_shape).unwrap_or_default();
        let detail = diff.unwrap_or("no textual diff is available for this divergence");
        Self::new(
            context, signature, &shape, project, path, file_bytes, detail,
        )
    }
}

pub fn fingerprint(
    context: FindingContext,
    category: &str,
    rule: Option<&str>,
    shape: &str,
) -> String {
    let material = [
        FINGERPRINT_TAG,
        context.mode_name(),
        context.baseline.config_key(),
        context.candidate.config_key(),
        category,
        rule.unwrap_or_default(),
        shape,
    ]
    .join(&SEPARATOR.to_string());
    format!("{:x}", Sha256::digest(material.as_bytes()))
}

fn lint_shape(signature: &MismatchSignature) -> String {
    let with_message =
        signature.canonical_code.is_none() || signature.category == "message_changed";
    let describe = |side: &Option<DiagnosticSignature>| match side {
        None => "-".to_owned(),
        Some(value) => {
            let severity = severity_name(value.severity);
            if with_message {
                format!("{severity}|{}", normalize::message(&value.message))
            } else {
                severity.to_owned()
            }
        }
    };
    format!(
        "{}>{}",
        describe(&signature.baseline),
        describe(&signature.candidate)
    )
}

/// Keep only the changed lines of a unified diff, normalized. File headers and
/// hunk headers are dropped because they carry paths and line numbers.
fn diff_shape(diff: &str) -> String {
    diff.lines()
        .filter(|line| {
            (line.starts_with('-') || line.starts_with('+'))
                && !line.starts_with("---")
                && !line.starts_with("+++")
        })
        .map(|line| {
            let (marker, rest) = line.split_at(1);
            format!("{marker}{}", normalize::fragment(rest))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn lint_detail(signature: &MismatchSignature) -> String {
    let mut detail = format!("{} ({})", signature.category, signature.side);
    if let Some(rule) = &signature.canonical_code {
        detail.push_str(&format!("\n  rule: {rule}"));
    }
    for (label, side) in [
        ("baseline", &signature.baseline),
        ("candidate", &signature.candidate),
    ] {
        if let Some(value) = side {
            detail.push_str(&format!(
                "\n  {label}: [{}] {}",
                severity_name(value.severity),
                value.message.replace('\n', " ")
            ));
        }
    }
    detail
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Unknown => "unknown",
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{Finding, FindingContext};
    use crate::model::{Severity, Tool};
    use crate::reduce::{DiagnosticSignature, MismatchSignature, ReduceMode};

    pub(crate) fn context() -> FindingContext {
        FindingContext {
            mode: ReduceMode::Lint,
            baseline: Tool::Eslint,
            candidate: Tool::Biome,
        }
    }

    pub(crate) fn signature(
        category: &str,
        rule: Option<&str>,
        message: &str,
    ) -> MismatchSignature {
        MismatchSignature {
            side: "baseline".into(),
            category: category.into(),
            canonical_code: rule.map(str::to_owned),
            baseline: Some(DiagnosticSignature {
                canonical_code: rule.map(str::to_owned),
                message: message.into(),
                severity: Severity::Warning,
            }),
            candidate: None,
        }
    }

    pub(crate) fn lint_finding(
        category: &str,
        rule: Option<&str>,
        message: &str,
        project: &str,
        path: &str,
        bytes: u64,
    ) -> Finding {
        Finding::lint(
            context(),
            signature(category, rule, message),
            project,
            path,
            bytes,
        )
    }

    #[test]
    fn project_path_and_position_do_not_affect_the_fingerprint() {
        let left = lint_finding(
            "baseline_only",
            Some("no-unused-vars"),
            "'a' is unused.",
            "path:one",
            "src/a.ts",
            10,
        );
        let right = lint_finding(
            "baseline_only",
            Some("no-unused-vars"),
            "'a' is unused.",
            "npm:other@1.0.0",
            "lib/deeply/nested/b.ts",
            9_999,
        );
        assert_eq!(left.fingerprint, right.fingerprint);
    }

    #[test]
    fn identifier_and_literal_content_still_collapses() {
        let left = lint_finding(
            "baseline_only",
            Some("no-unused-vars"),
            "'userTryingToGet' is assigned a value but never used.",
            "path:one",
            "a.ts",
            1,
        );
        let right = lint_finding(
            "baseline_only",
            Some("no-unused-vars"),
            "'total' is assigned a value but never used.",
            "path:two",
            "b.ts",
            2,
        );
        assert_eq!(left.fingerprint, right.fingerprint);

        let unruled_left = lint_finding(
            "baseline_only",
            None,
            "unused symbol 'aVeryLong'",
            "p",
            "a",
            1,
        );
        let unruled_right = lint_finding(
            "baseline_only",
            None,
            "unused symbol 'shortOne'",
            "p",
            "a",
            1,
        );
        assert_eq!(unruled_left.fingerprint, unruled_right.fingerprint);
    }

    #[test]
    fn category_rule_tool_pair_and_shape_each_change_the_fingerprint() {
        let base = lint_finding("baseline_only", Some("no-unused-vars"), "m", "p", "a", 1);

        let other_category =
            lint_finding("candidate_only", Some("no-unused-vars"), "m", "p", "a", 1);
        assert_ne!(base.fingerprint, other_category.fingerprint);

        let other_rule = lint_finding("baseline_only", Some("no-debugger"), "m", "p", "a", 1);
        assert_ne!(base.fingerprint, other_rule.fingerprint);

        let mut swapped = context();
        swapped.candidate = Tool::Oxlint;
        let other_pair = Finding::lint(
            swapped,
            super::tests::signature("baseline_only", Some("no-unused-vars"), "m"),
            "p",
            "a",
            1,
        );
        assert_ne!(base.fingerprint, other_pair.fingerprint);

        let mut severe = super::tests::signature("baseline_only", Some("no-unused-vars"), "m");
        if let Some(side) = severe.baseline.as_mut() {
            side.severity = Severity::Error;
        }
        let other_shape = Finding::lint(context(), severe, "p", "a", 1);
        assert_ne!(base.fingerprint, other_shape.fingerprint);
    }

    #[test]
    fn message_changed_keeps_the_message_pair_in_the_shape() {
        let build = |baseline: &str, candidate: &str| {
            let mut signature =
                super::tests::signature("message_changed", Some("no-debugger"), baseline);
            signature.candidate = Some(DiagnosticSignature {
                canonical_code: Some("no-debugger".into()),
                message: candidate.into(),
                severity: Severity::Warning,
            });
            Finding::lint(context(), signature, "p", "a", 1)
        };
        assert_eq!(
            build("left text", "right text").fingerprint,
            build("left text", "right text").fingerprint
        );
        assert_ne!(
            build("left text", "right text").fingerprint,
            build("left text", "different words entirely").fingerprint
        );
    }

    #[test]
    fn format_diffs_collapse_over_identifiers_and_literals() {
        let context = FindingContext {
            mode: ReduceMode::Format,
            baseline: Tool::Prettier,
            candidate: Tool::Oxfmt,
        };
        let signature = MismatchSignature {
            side: "both".into(),
            category: "different".into(),
            canonical_code: None,
            baseline: None,
            candidate: None,
        };
        let left = Finding::format(
            context,
            signature.clone(),
            Some(
                "--- baseline/a.ts\n+++ candidate/a.ts\n@@ -1 +1 @@\n-const total = 1;\n+const total = 1;;\n",
            ),
            "path:one",
            "a.ts",
            17,
        );
        let right = Finding::format(
            context,
            signature.clone(),
            Some(
                "--- baseline/deep/b.ts\n+++ candidate/deep/b.ts\n@@ -9 +9 @@\n-const amount = 4096;\n+const amount = 4096;;\n",
            ),
            "path:two",
            "deep/b.ts",
            20,
        );
        assert_eq!(left.fingerprint, right.fingerprint);

        let structural = Finding::format(
            context,
            signature,
            Some(
                "--- baseline/a.ts\n+++ candidate/a.ts\n@@ -1 +1 @@\n-const total = 1;\n+const total=1;\n",
            ),
            "path:three",
            "a.ts",
            17,
        );
        assert_ne!(left.fingerprint, structural.fingerprint);
    }
}
