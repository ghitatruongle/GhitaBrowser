//! Phase 21 acceptance gate: the bounded ECMAScript surface expansion used by
//! representative modern applications. Strict equality, ternary, typeof,
//! template interpolation, optional chaining, nullish coalescing, compound
//! assignment, increments, String methods, JSON and switch statements must
//! evaluate with JS semantics under the same interpreter budgets.

use ghitabrowser::javascript::JsvEngine;
use ghitabrowser::javascript::JsvValue;

fn eval(script: &str) -> JsvValue {
    let mut engine = JsvEngine::new();
    engine
        .eval(script)
        .unwrap_or_else(|error| panic!("eval failed for {script:?}: {error}"))
}

fn eval_err(script: &str) -> String {
    let mut engine = JsvEngine::new();
    engine
        .eval(script)
        .expect_err(&format!("expected failure for {script:?}"))
}

fn num(value: JsvValue) -> f64 {
    value
        .as_number()
        .unwrap_or_else(|| panic!("expected number, got {value:?}"))
}

fn boolean(value: JsvValue) -> bool {
    value
        .as_boolean()
        .unwrap_or_else(|| panic!("expected boolean, got {value:?}"))
}

fn text(value: JsvValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| panic!("expected string, got {value:?}"))
        .to_string()
}

#[test]
fn strict_equality_distinguishes_types_and_nan() {
    assert!(boolean(eval("1 === 1")));
    assert!(!boolean(eval("'1' === 1")));
    assert!(!boolean(eval("null === undefined")));
    assert!(boolean(eval("null == undefined")));
    assert!(boolean(eval("'a' !== 'b'")));
    assert!(!boolean(eval("NaN === NaN")));
    assert!(boolean(eval("0 === -0")));
    assert!(boolean(eval("true === true")));
    assert!(!boolean(eval("1 === true")));
}

#[test]
fn ternary_selects_branches_with_lazy_evaluation() {
    assert_eq!(num(eval("1 < 2 ? 10 : 20")), 10.0);
    assert_eq!(num(eval("1 > 2 ? 10 : 20")), 20.0);
    assert_eq!(text(eval("true ? 'yes' : missing()")), "yes");
    assert_eq!(num(eval("let x = 5; x > 3 ? x * 2 : x")), 10.0);
    assert_eq!(num(eval("false ? 1 : false ? 2 : 3")), 3.0);
}

#[test]
fn typeof_reports_js_types() {
    assert_eq!(text(eval("typeof 5")), "number");
    assert_eq!(text(eval("typeof 'x'")), "string");
    assert_eq!(text(eval("typeof true")), "boolean");
    assert_eq!(text(eval("typeof undefined")), "undefined");
    assert_eq!(text(eval("typeof null")), "object");
    assert_eq!(text(eval("typeof {}")), "object");
    assert_eq!(text(eval("typeof []")), "object");
    assert_eq!(text(eval("typeof missing")), "undefined");
}

#[test]
fn template_literals_interpolate_expressions() {
    assert_eq!(
        text(eval("let name='world'; `hello ${name}`")),
        "hello world"
    );
    assert_eq!(text(eval("`sum: ${1 + 2}`")), "sum: 3");
    assert_eq!(text(eval("`nested ${`inner ${2}`}`")), "nested inner 2");
    assert_eq!(text(eval("let a=2; `a=${a}, b=${a * 3}`")), "a=2, b=6");
}

#[test]
fn optional_chaining_short_circuits_on_nullish() {
    assert_eq!(eval("let o=null; o?.name"), JsvValue::Undefined);
    assert_eq!(
        eval("let o={name:'x'}; o?.name"),
        JsvValue::String("x".into())
    );
    assert_eq!(num(eval("let o={a:{b:3}}; o?.a?.b")), 3.0);
    assert_eq!(eval("let o={a:{b:3}}; o?.c?.b"), JsvValue::Undefined);
    assert_eq!(num(eval("let a=[1,2]; a?.[1]")), 2.0);
    // Optional chaining does not swallow property errors on non-nullish bases.
    assert!(eval_err("let o={}; o?.a.b.c").contains("property"));
}

#[test]
fn nullish_coalescing_keeps_falsy_but_not_nullish() {
    assert_eq!(num(eval("null ?? 5")), 5.0);
    assert_eq!(num(eval("undefined ?? 5")), 5.0);
    assert_eq!(num(eval("0 ?? 5")), 0.0);
    assert_eq!(text(eval("'' ?? 5")), "");
    assert!(!boolean(eval("false ?? true")));
    assert_eq!(text(eval("let x; x ?? 'default'")), "default");
}

#[test]
fn compound_assignment_applies_operator_semantics() {
    assert_eq!(num(eval("let x=5; x+=3; x")), 8.0);
    assert_eq!(num(eval("let x=5; x-=3; x")), 2.0);
    assert_eq!(num(eval("let x=5; x*=3; x")), 15.0);
    assert_eq!(num(eval("let x=5; x/=2; x")), 2.5);
    assert_eq!(num(eval("let x=7; x%=3; x")), 1.0);
    assert_eq!(text(eval("let s='a'; s+='b'; s")), "ab");
    assert_eq!(num(eval("let o={v:2}; o.v*=4; o.v")), 8.0);
}

#[test]
fn increments_and_decrements_return_prefix_or_postfix_values() {
    assert_eq!(num(eval("let x=1; let a=x++; a*10+x")), 12.0);
    assert_eq!(num(eval("let x=1; let a=++x; a*10+x")), 22.0);
    assert_eq!(num(eval("let x=3; let a=x--; a*10+x")), 32.0);
    assert_eq!(num(eval("let x=3; let a=--x; a*10+x")), 22.0);
    assert_eq!(num(eval("let o={v:1}; o.v++; o.v")), 2.0);
}

#[test]
fn string_methods_cover_common_application_operations() {
    assert_eq!(num(eval("'hello'.length")), 5.0);
    assert_eq!(text(eval("'HeLLo'.toLowerCase()")), "hello");
    assert_eq!(text(eval("'HeLLo'.toUpperCase()")), "HELLO");
    assert_eq!(text(eval("'  x  '.trim()")), "x");
    assert_eq!(text(eval("'abcdef'.charAt(2)")), "c");
    assert_eq!(num(eval("'hello world'.indexOf('world')")), 6.0);
    assert!(boolean(eval("'hello'.includes('ell')")));
    assert!(boolean(eval("'hello'.startsWith('he')")));
    assert!(boolean(eval("'hello'.endsWith('lo')")));
    assert_eq!(text(eval("'hello world'.slice(6)")), "world");
    assert_eq!(text(eval("'hello world'.slice(0,5)")), "hello");
    assert_eq!(text(eval("'hello'.replace('l','L')")), "heLlo");
    assert_eq!(text(eval("['a','b','c'].join('-')")), "a-b-c");
    assert_eq!(num(eval("'ab'.repeat(3).length")), 6.0);
}

#[test]
fn string_split_produces_arrays() {
    let value = eval("'a,b,c'.split(',')");
    let JsvValue::Array(array) = value else {
        panic!("expected array");
    };
    let values: Vec<String> = array
        .borrow()
        .iter()
        .map(JsvValue::to_display_string)
        .collect();
    assert_eq!(values, vec!["a", "b", "c"]);
}

#[test]
fn json_stringify_and_parse_round_trip_bounded_structures() {
    // Object key order is unspecified in the bounded value model, so verify
    // round-trip equality and canonical forms for ordered structures.
    assert_eq!(
        text(eval(
            "let o=JSON.parse(JSON.stringify({a:1,b:'x'}));o.a+o.b"
        )),
        "1x"
    );
    assert_eq!(
        text(eval("JSON.stringify([1,'two',true,null])")),
        "[1,\"two\",true,null]"
    );
    assert_eq!(num(eval("JSON.parse('{\"a\":5}').a")), 5.0);
    assert_eq!(text(eval("JSON.parse('[\"x\",\"y\"]')[1]")), "y");
    assert!(boolean(eval("JSON.parse('true')")));
    assert_eq!(text(eval("JSON.stringify({only:1})")), "{\"only\":1}");
    assert!(eval_err("JSON.parse('{invalid')").contains("JSON"));
}

#[test]
fn switch_statements_fall_through_and_break() {
    assert_eq!(
        text(eval("let out='';switch(2){case 1:out='one';break;case 2:out='two';break;default:out='other'}out")),
        "two"
    );
    assert_eq!(
        text(eval(
            "let out='';switch(9){case 1:out='one';break;default:out='other'}out"
        )),
        "other"
    );
    assert_eq!(
        text(eval(
            "let out='';switch('a'){case 'a':out='A';case 'b':out+='B';break;default:out='D'}out"
        )),
        "AB"
    );
}

#[test]
fn instance_of_and_in_operators_work_on_objects_and_arrays() {
    assert!(boolean(eval(
        "let p={x:1};let o=Object.create(p);o instanceof p"
    )));
    assert!(boolean(eval("'x' in {x:1}")));
    assert!(!boolean(eval("'y' in {x:1}")));
    assert!(boolean(eval("1 in [9,8]")));
    assert!(!boolean(eval("5 in [9,8]")));
}

#[test]
fn bounded_failures_still_error_instead_of_hanging() {
    // Non-terminating loops remain bounded.
    assert!(eval_err("while(true){}").contains("Infinite loop"));
    // Type errors stay explicit.
    assert!(eval_err("undefined.name").contains("TypeError"));
}
