//! Structural normalization used by finding fingerprints.
//!
//! A fragment is reduced to a token stream in which identifier names become
//! positional placeholders, literal contents become type markers, and runs of
//! whitespace become a single marker that preserves the kind of whitespace but
//! not its width. Two occurrences of the same structural divergence over
//! different names or literals therefore normalize to the same string.

use std::collections::HashMap;

/// Words kept verbatim so that the shape of a fragment survives normalization.
const RESERVED: &[&str] = &[
    "abstract",
    "any",
    "as",
    "async",
    "await",
    "boolean",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "constructor",
    "continue",
    "debugger",
    "declare",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "get",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "keyof",
    "let",
    "namespace",
    "never",
    "new",
    "null",
    "number",
    "of",
    "private",
    "protected",
    "public",
    "readonly",
    "return",
    "satisfies",
    "set",
    "static",
    "string",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "undefined",
    "unknown",
    "var",
    "void",
    "while",
    "yield",
];

/// Normalize a code fragment. Whitespace runs collapse to `<sp>` or `<nl>`,
/// literals collapse to `<str>` or `<num>`, and each distinct identifier
/// collapses to `$1`, `$2`, ... in order of first appearance.
pub fn fragment(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut names: HashMap<String, String> = HashMap::new();
    let mut index = 0;
    while index < characters.len() {
        let current = characters[index];
        if current.is_whitespace() {
            let mut newline = false;
            while index < characters.len() && characters[index].is_whitespace() {
                newline |= characters[index] == '\n';
                index += 1;
            }
            tokens.push(if newline {
                "<nl>".into()
            } else {
                "<sp>".into()
            });
            continue;
        }
        if matches!(current, '\'' | '"' | '`') {
            index = skip_string(&characters, index, current);
            tokens.push("<str>".into());
            continue;
        }
        if current.is_ascii_digit() {
            while index < characters.len() && is_number_character(&characters, index) {
                index += 1;
            }
            tokens.push("<num>".into());
            continue;
        }
        if is_identifier_start(current) {
            let start = index;
            while index < characters.len() && is_identifier_part(characters[index]) {
                index += 1;
            }
            let word: String = characters[start..index].iter().collect();
            tokens.push(placeholder(&word, &mut names));
            continue;
        }
        tokens.push(current.to_string());
        index += 1;
    }
    trim_markers(&mut tokens);
    tokens.join(" ")
}

/// Normalize a diagnostic message. Prose is kept, but anything that reads as
/// an embedded identifier or literal collapses, so that the same message
/// template over different symbols normalizes identically.
pub fn message(text: &str) -> String {
    let mut result = String::new();
    let characters: Vec<char> = text.chars().collect();
    let mut index = 0;
    let mut pending_space = false;
    while index < characters.len() {
        let current = characters[index];
        if current.is_whitespace() {
            index += 1;
            pending_space = !result.is_empty();
            continue;
        }
        if matches!(current, '\'' | '"' | '`') {
            index = skip_string(&characters, index, current);
            push_token(&mut result, &mut pending_space, "<name>");
            continue;
        }
        if current.is_ascii_digit() {
            while index < characters.len() && is_number_character(&characters, index) {
                index += 1;
            }
            push_token(&mut result, &mut pending_space, "<num>");
            continue;
        }
        if is_identifier_start(current) {
            let start = index;
            while index < characters.len() && is_identifier_part(characters[index]) {
                index += 1;
            }
            let word: String = characters[start..index].iter().collect();
            let token = if looks_like_symbol(&word) {
                "<name>".to_owned()
            } else {
                word.to_ascii_lowercase()
            };
            push_token(&mut result, &mut pending_space, &token);
            continue;
        }
        push_token(&mut result, &mut pending_space, &current.to_string());
        index += 1;
    }
    result
}

fn push_token(result: &mut String, pending_space: &mut bool, token: &str) {
    if *pending_space {
        result.push(' ');
    }
    *pending_space = false;
    result.push_str(token);
}

/// A word is treated as a symbol when it could not plausibly be prose: mixed
/// case beyond a leading capital, digits, or an underscore or dollar sign.
fn looks_like_symbol(word: &str) -> bool {
    if word
        .chars()
        .any(|c| matches!(c, '_' | '$') || c.is_ascii_digit())
    {
        return true;
    }
    word.chars()
        .skip(1)
        .any(|c| c.is_uppercase() && !word.chars().all(char::is_uppercase))
}

fn placeholder(word: &str, names: &mut HashMap<String, String>) -> String {
    if RESERVED.contains(&word) {
        return word.to_owned();
    }
    let next = names.len() + 1;
    names
        .entry(word.to_owned())
        .or_insert_with(|| format!("${next}"))
        .clone()
}

fn skip_string(characters: &[char], start: usize, quote: char) -> usize {
    let mut index = start + 1;
    while index < characters.len() {
        match characters[index] {
            '\\' => index += 2,
            character if character == quote => return index + 1,
            _ => index += 1,
        }
    }
    characters.len()
}

fn is_number_character(characters: &[char], index: usize) -> bool {
    let current = characters[index];
    if current.is_ascii_alphanumeric() || matches!(current, '.' | '_') {
        return true;
    }
    // Exponent sign, e.g. 1e-9.
    matches!(current, '+' | '-')
        && index > 0
        && matches!(characters[index - 1], 'e' | 'E')
        && index + 1 < characters.len()
        && characters[index + 1].is_ascii_digit()
}

fn is_identifier_start(character: char) -> bool {
    character.is_alphabetic() || matches!(character, '_' | '$')
}

fn is_identifier_part(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '$')
}

fn trim_markers(tokens: &mut Vec<String>) {
    while tokens.first().is_some_and(is_whitespace_marker) {
        tokens.remove(0);
    }
    while tokens.last().is_some_and(is_whitespace_marker) {
        tokens.pop();
    }
}

fn is_whitespace_marker(token: &String) -> bool {
    token == "<sp>" || token == "<nl>"
}

#[cfg(test)]
mod tests {
    use super::{fragment, message};

    #[test]
    fn identifiers_become_positional_placeholders() {
        assert_eq!(
            fragment("const total = compute(total);"),
            fragment("const amount = derive(amount);")
        );
        assert_ne!(
            fragment("const total = compute(other);"),
            fragment("const total = compute(total);")
        );
    }

    #[test]
    fn literal_contents_collapse_to_type_markers() {
        assert_eq!(fragment("call('one', 12)"), fragment("call('two', 4096)"));
        assert_eq!(fragment("const x = `a`;"), fragment("const y = 'b';"));
    }

    #[test]
    fn keywords_and_punctuation_survive() {
        assert_eq!(
            fragment("const a = 1;"),
            "const <sp> $1 <sp> = <sp> <num> ;"
        );
        assert_ne!(fragment("const a = 1;"), fragment("let a = 1;"));
    }

    #[test]
    fn whitespace_width_collapses_but_presence_does_not() {
        assert_eq!(fragment("a  +   b"), fragment("a + b"));
        assert_ne!(fragment("a + b"), fragment("a+b"));
        assert_ne!(fragment("a b"), fragment("a\nb"));
        assert_eq!(fragment("   a + b  "), fragment("a + b"));
    }

    #[test]
    fn messages_collapse_embedded_symbols_but_keep_prose() {
        assert_eq!(
            message("'userName' is assigned a value but never used."),
            message("'total' is assigned a value but never used.")
        );
        assert_eq!(
            message("This variable userTryingToGet is unused."),
            message("This variable otherUnusedThing is unused.")
        );
        assert_ne!(
            message("'a' is assigned a value but never used."),
            message("'a' is defined but never used.")
        );
    }

    #[test]
    fn messages_normalize_case_and_whitespace() {
        assert_eq!(
            message("Unexpected  debugger\nstatement."),
            message("unexpected debugger statement.")
        );
    }
}
