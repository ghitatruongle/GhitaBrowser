use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use ghitabrowser::javascript::JsvEngine;

const POSITIVE_SAME_VALUE_CASES: &[&str] = &[
    "test/language/expressions/arrow-function/expression-body-implicit-return.js",
    "test/language/expressions/arrow-function/empty-function-body-returns-undefined.js",
];

const NEGATIVE_CASES: &[&str] = &[
    "test/language/statements/break/S12.8_A1_T1.js",
    "test/language/statements/continue/S12.7_A1_T1.js",
    "test/language/statements/const/syntax/block-scope-syntax-const-declarations-without-initialiser.js",
];

fn external_root() -> PathBuf {
    PathBuf::from(env::var_os("TEST262_ROOT").expect(
        "TEST262_ROOT must point to an external Test262 checkout; suite content is not vendored",
    ))
}

fn read_case(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative)).unwrap_or_else(|error| panic!("{relative}: {error}"))
}

fn split_assertion_arguments(arguments: &str) -> (&str, &str) {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in arguments.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => quote = Some(character),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return (&arguments[..index], &arguments[index + 1..]),
            _ => {}
        }
    }
    panic!("Test262 assert.sameValue call has no top-level comma")
}

fn closing_parenthesis(source: &str, open: usize) -> usize {
    let mut depth = 1usize;
    let mut quote = None;
    let mut escaped = false;
    for (offset, character) in source[open + 1..].char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => quote = Some(character),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return open + 1 + offset;
                }
            }
            _ => {}
        }
    }
    panic!("unterminated Test262 assert.sameValue call")
}

fn run_same_value_case(root: &Path, relative: &str) {
    const MARKER: &str = "assert.sameValue";
    let source = read_case(root, relative);
    let mut engine = JsvEngine::new();
    let mut cursor = 0usize;
    let mut assertions = 0usize;

    while let Some(found) = source[cursor..].find(MARKER) {
        let assertion_start = cursor + found;
        engine
            .eval(&source[cursor..assertion_start])
            .unwrap_or_else(|error| panic!("{relative}: setup failed: {error}"));
        let open = source[assertion_start + MARKER.len()..]
            .find('(')
            .map(|offset| assertion_start + MARKER.len() + offset)
            .expect("assert.sameValue opening parenthesis");
        let close = closing_parenthesis(&source, open);
        let (actual_source, expected_source) = split_assertion_arguments(&source[open + 1..close]);
        let actual = engine
            .eval(actual_source)
            .unwrap_or_else(|error| panic!("{relative}: actual expression failed: {error}"));
        let expected = engine
            .eval(expected_source)
            .unwrap_or_else(|error| panic!("{relative}: expected expression failed: {error}"));
        assert_eq!(actual, expected, "{relative}: assert.sameValue failed");
        assertions += 1;
        cursor = close + 1;
        while source
            .as_bytes()
            .get(cursor)
            .is_some_and(u8::is_ascii_whitespace)
            || source.as_bytes().get(cursor) == Some(&b';')
        {
            cursor += 1;
        }
    }
    assert!(assertions > 0, "{relative}: no supported assertions found");
    engine
        .eval(&source[cursor..])
        .unwrap_or_else(|error| panic!("{relative}: trailing source failed: {error}"));
}

fn run_negative_case(root: &Path, relative: &str) {
    let source = read_case(root, relative).replace("$DONOTEVALUATE();", "");
    let mut engine = JsvEngine::new();
    assert!(
        engine.eval(&source).is_err(),
        "{relative}: expected syntax rejection"
    );
}

#[test]
#[ignore = "requires a separately licensed external Test262 checkout"]
fn phase10_external_test262_subsets() {
    let root = external_root();
    for relative in POSITIVE_SAME_VALUE_CASES {
        run_same_value_case(&root, relative);
    }
    for relative in NEGATIVE_CASES {
        run_negative_case(&root, relative);
    }
}
