// original Test262-style assertions executed against
// the expanded ECMAScript surface. These cases are written from the
// ECMA-262 specification (the same style contract as the separately
// licensed external Test262 checkout: `assert.sameValue` semantics) but are
// original — nothing is copied from Test262 or any engine.
// The external Test262 checkout itself remains a separate, ignored gate
// (`test262_subset_test.rs`, requires `TEST262_ROOT`); this file records
// the bounded original corpus that always runs offline.

use ghitabrowser::javascript::JsvEngine;

struct Harness {
    failures: Vec<String>,
}

impl Harness {
    fn new() -> Self {
        Self {
            failures: Vec::new(),
        }
    }
    /// `assert.sameValue(actual, expected)` semantics: strict equality
    /// between the evaluated result and the expected literal.
    fn same_value(&mut self, script: &str, expected: &str) {
        let mut engine = JsvEngine::new();
        match engine.eval(script) {
            Ok(value) => {
                let actual = value.to_display_string();
                if actual != expected {
                    self.failures
                        .push(format!("{script}: expected {expected}, got {actual}"));
                }
            }
            Err(error) => self
                .failures
                .push(format!("{script}: unexpected error {error}")),
        }
    }

    /// `assert.throws` semantics: evaluation must fail.
    fn throws(&mut self, script: &str) {
        let mut engine = JsvEngine::new();
        if let Ok(value) = engine.eval(script) {
            self.failures
                .push(format!("{script}: expected throw, got {value:?}"));
        }
    }

    fn finish(self) {
        assert!(
            self.failures.is_empty(),
            "conformance failures:\n{}",
            self.failures.join("\n")
        );
    }
}

#[test]
fn original_strict_equality_corpus() {
    let mut h = Harness::new();
    h.same_value("1 === 1", "true");
    h.same_value("'1' === 1", "false");
    h.same_value("null === undefined", "false");
    h.same_value("NaN === NaN", "false");
    h.same_value("0 === -0", "true");
    h.same_value("'' === ''", "true");
    h.same_value("1 !== '1'", "true");
    h.finish();
}

#[test]
fn original_ternary_and_nullish_corpus() {
    let mut h = Harness::new();
    h.same_value("true ? 1 : 2", "1");
    h.same_value("false ? 1 : 2", "2");
    h.same_value("null ?? 'd'", "d");
    h.same_value("0 ?? 'd'", "0");
    h.same_value("'' ?? 'd'", "");
    h.same_value("false ?? true", "false");
    h.finish();
}

#[test]
fn original_typeof_corpus() {
    let mut h = Harness::new();
    h.same_value("typeof 1", "number");
    h.same_value("typeof 's'", "string");
    h.same_value("typeof true", "boolean");
    h.same_value("typeof undefined", "undefined");
    h.same_value("typeof null", "object");
    h.same_value("typeof missing", "undefined");
    h.same_value("typeof function(){}", "function");
    h.finish();
}

#[test]
fn original_optional_chaining_corpus() {
    let mut h = Harness::new();
    h.same_value("null?.x", "undefined");
    h.same_value("undefined?.x", "undefined");
    h.same_value("({a:{b:2}})?.a?.b", "2");
    h.same_value("({a:{b:2}})?.c?.b", "undefined");
    h.same_value("[1,2]?.[0]", "1");
    h.throws("({})?.a.b.c");
    h.finish();
}

#[test]
fn original_template_literal_corpus() {
    let mut h = Harness::new();
    h.same_value("`plain`", "plain");
    h.same_value("`x${1+2}y`", "x3y");
    h.same_value("`nested ${`inner ${2}`}`", "nested inner 2");
    h.same_value("``", "");
    h.finish();
}

#[test]
fn original_switch_corpus() {
    let mut h = Harness::new();
    h.same_value(
        "let o='';switch(2){case 1:o='a';break;case 2:o='b';break;default:o='c'}o",
        "b",
    );
    h.same_value("let o='';switch(9){case 1:o='a';break;default:o='c'}o", "c");
    h.same_value(
        "let o='';switch('x'){case 'x':o='A';case 'y':o+='B';break;default:o='D'}o",
        "AB",
    );
    h.finish();
}

#[test]
fn original_json_corpus() {
    let mut h = Harness::new();
    h.same_value("JSON.stringify([1,'t',true,null])", "[1,\"t\",true,null]");
    h.same_value("JSON.stringify({k:1})", "{\"k\":1}");
    h.same_value("JSON.parse('{\"a\":2}').a", "2");
    h.same_value("JSON.parse('[1,2]').length", "2");
    h.same_value("JSON.parse('false')", "false");
    h.throws("JSON.parse('{bad')");
    h.finish();
}

#[test]
fn original_string_method_corpus() {
    let mut h = Harness::new();
    h.same_value("'abc'.toUpperCase()", "ABC");
    h.same_value("' ABC '.trim()", "ABC");
    h.same_value("'abc'.charAt(1)", "b");
    h.same_value("'a,b,c'.split(',').length", "3");
    h.same_value("'hello'.indexOf('ll')", "2");
    h.same_value("'hello'.replace('l','L')", "heLlo");
    h.same_value("'abc'.repeat(2)", "abcabc");
    h.same_value("'abcdef'.slice(1,4)", "bcd");
    h.same_value("'abcdef'.slice(-3)", "def");
    h.same_value("'x'.padStart(3,'0')", "00x");
    h.finish();
}

#[test]
fn original_compound_and_update_corpus() {
    let mut h = Harness::new();
    h.same_value("let x=2;x+=3;x", "5");
    h.same_value("let x=2;x*=4;x", "8");
    h.same_value("let x=1;let a=x++;a*10+x", "12");
    h.same_value("let x=1;let a=++x;a*10+x", "22");
    h.same_value("let o={v:1};o.v++;o.v", "2");
    h.finish();
}

#[test]
fn original_instanceof_and_in_corpus() {
    let mut h = Harness::new();
    h.same_value("let p={};let o=Object.create(p);o instanceof p", "true");
    h.same_value("let o=Object.create(null);o instanceof o", "false");
    h.same_value("'a' in {a:1}", "true");
    h.same_value("0 in [5]", "true");
    h.same_value("2 in [5]", "false");
    h.finish();
}

#[test]
fn original_dynamic_import_corpus() {
    // Modules are registered before evaluation in the page runtime tests;
    // here the engine-level graph is used directly through the module API.
    let mut engine = JsvEngine::new();
    engine
        .modules
        .register("m", "export const v=6;")
        .expect("register");
    let value = engine.eval("import('m').then(ns=>ns.v)").expect("import");
    assert!(matches!(
        value,
        ghitabrowser::javascript::JsvValue::Promise(_)
    ));
    let namespace = engine.modules.evaluate("m").expect("module evaluation");
    assert_eq!(
        namespace.exports.get("v"),
        Some(&ghitabrowser::javascript::JsvValue::Number(6.0))
    );
}
