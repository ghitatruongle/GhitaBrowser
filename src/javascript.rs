// src/javascript.rs - JavaScript Engine with variables, functions, control flow (v0.6.1)


use std::collections::HashMap;

/// Hard cap on the number of expression evaluations per script. A single
/// `eval` call that exceeds it is aborted with an error instead of hanging,
/// which bounds non-terminating `while` loops, total work, and (together with
/// `MAX_CALL_DEPTH`) recursion depth.
const MAX_EVAL_STEPS: u64 = 5_000_000;

/// Maximum nested function-call depth while evaluating, so deep or circular
/// recursion (`function f() { return f() }`) errors instead of overflowing the
/// stack (a stack overflow in Rust aborts the whole process). Kept conservative:
/// the interpreter runs on the GUI thread and each nested call costs several
/// interpreter frames (~20 KB per level in unoptimized debug builds, where
/// `cargo test` runs on a 2 MB thread), so 64 is the largest value that passes
/// tests without overflowing the stack.
const MAX_CALL_DEPTH: u64 = 64;

/// Maximum parser nesting depth, so hostile deeply-nested scripts error during
/// parsing instead of overflowing the stack in the recursive descent parser.
const MAX_PARSE_DEPTH: usize = 512;

/// Shared per-`eval` execution state threaded through the interpreter.
#[derive(Default)]
struct EvalCtx {
    /// Total expression evaluations performed so far in this script.
    steps: u64,
    /// Current nested function-call depth (incremented per call, decremented on return).
    call_depth: u64,
}

/// True when a value is a `return` signal propagating through a statement container.
fn is_return_signal(v: &JsvValue) -> bool {
    matches!(v, JsvValue::ReturnSignal(_))
}

/// Collapse a `return` signal at a function/script boundary into a plain value.
fn unwrap_return_signal(v: JsvValue) -> JsvValue {
    match v {
        JsvValue::ReturnSignal(inner) => *inner,
        other => other,
    }
}

/// Maximum `(`, `[`, `{` nesting in the token stream. Bounds the recursion
/// depth of the recursive-descent parser (parens, calls, blocks, unary chains
/// also recurse but each is capped by its own checks below).
fn max_parse_depth(tokens: &[String]) -> usize {
    let mut depth = 0usize;
    let mut max = 0usize;
    for t in tokens {
        match t.as_str() {
            "(" | "[" | "{" => {
                depth += 1;
                max = max.max(depth);
            }
            ")" | "]" | "}" => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

/// JavaScript value types
#[derive(Debug, Clone, PartialEq)]
pub enum JsvValue {
    Number(f64),
    Boolean(bool),
    String(String),
    Null,
    Undefined,
    Object(HashMap<String, JsvValue>),
    Array(Vec<JsvValue>),
    Function(String, Vec<String>, Box<JsvExpr>), // (name, params, body)
    NativeFn(String),                            // Built-in functions
    /// Internal: a `return` statement propagating through statement
    /// containers (if/while/block). Unwrapped to a plain value at function
    /// call and script boundaries.
    ReturnSignal(Box<JsvValue>),
}

impl JsvValue {
    pub fn number(n: f64) -> Self {
        JsvValue::Number(n)
    }
    pub fn boolean(b: bool) -> Self {
        JsvValue::Boolean(b)
    }
    pub fn string<S: Into<String>>(s: S) -> Self {
        JsvValue::String(s.into())
    }
    pub fn object() -> Self {
        JsvValue::Object(HashMap::new())
    }
    pub fn array() -> Self {
        JsvValue::Array(Vec::new())
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            JsvValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            JsvValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            JsvValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            JsvValue::Null | JsvValue::Undefined => false,
            JsvValue::Boolean(b) => *b,
            JsvValue::Number(n) => *n != 0.0,
            JsvValue::String(s) => !s.is_empty(),
            _ => true,
        }
    }

    pub fn to_display_string(&self) -> String {
        match self {
            JsvValue::Number(n) => {
                if *n == n.floor() && n.is_finite() {
                    format!("{}", *n as i64)
                } else {
                    format!("{}", n)
                }
            }
            JsvValue::Boolean(b) => b.to_string(),
            JsvValue::String(s) => s.clone(),
            JsvValue::Null => "null".to_string(),
            JsvValue::Undefined => "undefined".to_string(),
            JsvValue::Object(_) => "[object Object]".to_string(),
            JsvValue::Array(a) => format!(
                "[{}]",
                a.iter()
                    .map(|v| v.to_display_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            JsvValue::Function(name, _, _) => format!("[Function: {}]", name),
            JsvValue::NativeFn(name) => format!("[Native Function: {}]", name),
            JsvValue::ReturnSignal(v) => v.to_display_string(),
        }
    }
}

/// JavaScript expression types
#[derive(Debug, Clone, PartialEq)]
pub enum JsvExpr {
    // Literals
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Undefined,

    // Variables
    Identifier(String),
    Assignment(String, Box<JsvExpr>),

    // Operations
    BinaryOp(Box<JsvExpr>, OpKind, Box<JsvExpr>),
    UnaryOp(OpKind, Box<JsvExpr>),

    // Control flow
    If(Box<JsvExpr>, Vec<JsvExpr>, Option<Vec<JsvExpr>>),
    While(Box<JsvExpr>, Vec<JsvExpr>),
    Block(Vec<JsvExpr>),

    // Functions
    Call(Box<JsvExpr>, Vec<JsvExpr>),
    FunctionDef(String, Vec<String>, Box<JsvExpr>),

    // Return
    Return(Box<JsvExpr>),
}

/// Operator kinds
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OpKind {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Not,
    Assign,
}

/// Environment with lexical scoping
#[derive(Debug, Clone)]
pub struct JsvEnvironment {
    parent: Option<Box<JsvEnvironment>>,
    pub bindings: HashMap<String, JsvValue>,
}

impl Default for JsvEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl JsvEnvironment {
    pub fn new() -> Self {
        Self {
            parent: None,
            bindings: HashMap::new(),
        }
    }

    pub fn with_parent(parent: JsvEnvironment) -> Self {
        Self {
            parent: Some(Box::new(parent)),
            bindings: HashMap::new(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&JsvValue> {
        if let Some(value) = self.bindings.get(name) {
            return Some(value);
        }
        self.parent.as_ref().and_then(|p| p.get(name))
    }

    pub fn set(&mut self, name: &str, value: JsvValue) {
        if self.bindings.contains_key(name) {
            self.bindings.insert(name.to_string(), value);
        } else if let Some(ref mut parent) = self.parent {
            if parent.get(name).is_some() {
                parent.set(name, value);
            } else {
                self.bindings.insert(name.to_string(), value);
            }
        } else {
            self.bindings.insert(name.to_string(), value);
        }
    }

    pub fn define(&mut self, name: &str, value: JsvValue) {
        self.bindings.insert(name.to_string(), value);
    }
}

/// Main JavaScript engine
pub struct JsvEngine {
    pub global_env: JsvEnvironment,
    pub console_output: Vec<String>,
}

impl Default for JsvEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl JsvEngine {
    pub fn new() -> Self {
        let mut env = JsvEnvironment::new();

        // Set up console object with methods
        let mut console_obj = HashMap::new();
        console_obj.insert(
            "log".to_string(),
            JsvValue::NativeFn("console.log".to_string()),
        );
        console_obj.insert(
            "warn".to_string(),
            JsvValue::NativeFn("console.warn".to_string()),
        );
        console_obj.insert(
            "error".to_string(),
            JsvValue::NativeFn("console.error".to_string()),
        );
        env.set("console", JsvValue::Object(console_obj));

        // Math object
        let mut math_obj = HashMap::new();
        math_obj.insert("PI".to_string(), JsvValue::Number(std::f64::consts::PI));
        env.set("Math", JsvValue::Object(math_obj));

        // Member functions registered as top-level identifiers (for simplified parsing)
        env.set("console.log", JsvValue::NativeFn("console.log".to_string()));
        env.set(
            "console.warn",
            JsvValue::NativeFn("console.warn".to_string()),
        );
        env.set(
            "console.error",
            JsvValue::NativeFn("console.error".to_string()),
        );

        // Built-in functions
        env.set("parseInt", JsvValue::NativeFn("parseInt".to_string()));
        env.set("parseFloat", JsvValue::NativeFn("parseFloat".to_string()));
        env.set("String", JsvValue::NativeFn("String".to_string()));
        env.set("Number", JsvValue::NativeFn("Number".to_string()));

        // Constants
        env.set("undefined", JsvValue::Undefined);
        env.set("null", JsvValue::Null);
        env.set("true", JsvValue::Boolean(true));
        env.set("false", JsvValue::Boolean(false));

        Self {
            global_env: env,
            console_output: Vec::new(),
        }
    }

    pub fn eval(&mut self, code: &str) -> Result<JsvValue, String> {
        let tokens = tokenize(code)?;
        if tokens.is_empty() {
            return Ok(JsvValue::Undefined);
        }
        if max_parse_depth(&tokens) > MAX_PARSE_DEPTH {
            return Err("Script too deeply nested".to_string());
        }

        let (exprs, _rest) = parse_statements(&tokens, 0, 0)?;

        let env = &mut self.global_env;
        let console = &mut self.console_output;
        let mut ctx = EvalCtx::default();

        if exprs.len() == 1 {
            eval_expr(&exprs[0], env, console, &mut ctx).map(unwrap_return_signal)
        } else {
            let mut result = JsvValue::Undefined;
            for expr in &exprs {
                result = unwrap_return_signal(eval_expr(expr, env, console, &mut ctx)?);
            }
            Ok(result)
        }
    }

    pub fn execute_script(&mut self, script: &str) -> Result<JsvValue, String> {
        self.eval(script)
    }
}

/// Evaluate an expression in the given environment
fn eval_expr(
    expr: &JsvExpr,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx,
) -> Result<JsvValue, String> {
    ctx.steps += 1;
    if ctx.steps > MAX_EVAL_STEPS {
        return Err("Script timed out (step budget exceeded)".to_string());
    }

    match expr {
        // Literals
        JsvExpr::Number(n) => Ok(JsvValue::Number(*n)),
        JsvExpr::String(s) => Ok(JsvValue::String(s.clone())),
        JsvExpr::Bool(b) => Ok(JsvValue::Boolean(*b)),
        JsvExpr::Null => Ok(JsvValue::Null),
        JsvExpr::Undefined => Ok(JsvValue::Undefined),

        // Variables
        JsvExpr::Identifier(name) => env
            .get(name)
            .cloned()
            .ok_or_else(|| format!("ReferenceError: {} is not defined", name)),
        JsvExpr::Assignment(name, rhs) => {
            let val = eval_expr(rhs, env, console, ctx)?;
            env.set(name, val.clone());
            Ok(val)
        }

        // Binary operators
        JsvExpr::BinaryOp(left, op, right) => {
            let l_val = eval_expr(left, env, console, ctx)?;
            let r_val = eval_expr(right, env, console, ctx)?;
            apply_binary_op(l_val, r_val, *op)
        }

        // Unary operators
        JsvExpr::UnaryOp(op, inner) => {
            let val = eval_expr(inner, env, console, ctx)?;
            match (op, val) {
                (OpKind::Sub, JsvValue::Number(n)) => Ok(JsvValue::Number(-n)),
                (OpKind::Not, v) => Ok(JsvValue::Boolean(!v.is_truthy())),
                _ => Err("Invalid unary operation".to_string()),
            }
        }

        // Control flow. A `return` inside any statement container must
        // propagate up through it, so evaluate each statement and pass the
        // ReturnSignal through instead of only honoring top-level returns.
        JsvExpr::If(cond, then_branch, else_branch) => {
            let cond_val = eval_expr(cond, env, console, ctx)?;
            if cond_val.is_truthy() {
                let mut result = JsvValue::Undefined;
                for stmt in then_branch {
                    let r = eval_expr(stmt, env, console, ctx)?;
                    if is_return_signal(&r) {
                        return Ok(r);
                    }
                    result = r;
                }
                Ok(result)
            } else if let Some(else_stmts) = else_branch {
                let mut result = JsvValue::Undefined;
                for stmt in else_stmts {
                    let r = eval_expr(stmt, env, console, ctx)?;
                    if is_return_signal(&r) {
                        return Ok(r);
                    }
                    result = r;
                }
                Ok(result)
            } else {
                Ok(JsvValue::Undefined)
            }
        }

        JsvExpr::While(cond, body) => {
            let mut result = JsvValue::Undefined;
            loop {
                let cond_val = eval_expr(cond, env, console, ctx)?;
                if !cond_val.is_truthy() {
                    break;
                }
                for stmt in body {
                    let r = eval_expr(stmt, env, console, ctx)?;
                    if is_return_signal(&r) {
                        return Ok(r);
                    }
                    result = r;
                }
            }
            Ok(result)
        }

        JsvExpr::Block(stmts) => {
            let mut block_env = JsvEnvironment::with_parent(env.clone());
            let mut result = JsvValue::Undefined;
            for stmt in stmts {
                let r = eval_expr(stmt, &mut block_env, console, ctx)?;
                if is_return_signal(&r) {
                    return Ok(r);
                }
                result = r;
            }
            // Sync new variables back to parent
            for (k, v) in block_env.bindings {
                if env.get(&k).is_none() {
                    env.define(&k, v);
                }
            }
            Ok(result)
        }

        // Functions
        JsvExpr::FunctionDef(name, params, body) => {
            let func = JsvValue::Function(name.clone(), params.clone(), body.clone());
            env.define(name, func.clone());
            Ok(func)
        }

        JsvExpr::Call(callee, args) => {
            let callee_val = eval_expr(callee, env, console, ctx)?;
            let mut arg_vals = Vec::with_capacity(args.len());
            for a in args {
                arg_vals.push(eval_expr(a, env, console, ctx)?);
            }
            let r = call_function(callee_val, arg_vals, env, console, ctx)?;
            // A `return` inside the function body ends the function call,
            // turning the signal into the call's plain return value.
            Ok(unwrap_return_signal(r))
        }

        JsvExpr::Return(val) => {
            let r = eval_expr(val, env, console, ctx)?;
            Ok(JsvValue::ReturnSignal(Box::new(r)))
        }
    }
}

/// Apply a binary operator
fn apply_binary_op(l_val: JsvValue, r_val: JsvValue, op: OpKind) -> Result<JsvValue, String> {
    // Short-circuit evaluation for logical operators
    match op {
        OpKind::And => {
            // JS semantics: returns first falsy value, or last value if all truthy
            if !l_val.is_truthy() {
                return Ok(l_val);
            }
            return Ok(r_val);
        }
        OpKind::Or => {
            // JS semantics: returns first truthy value, or last value if all falsy
            if l_val.is_truthy() {
                return Ok(l_val);
            }
            return Ok(r_val);
        }
        _ => {}
    }

    match (l_val, r_val) {
        (JsvValue::Number(ln), JsvValue::Number(rn)) => match op {
            OpKind::Add => Ok(JsvValue::Number(ln + rn)),
            OpKind::Sub => Ok(JsvValue::Number(ln - rn)),
            OpKind::Mul => Ok(JsvValue::Number(ln * rn)),
            OpKind::Div => {
                if rn == 0.0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(JsvValue::Number(ln / rn))
                }
            }
            OpKind::Mod => {
                if rn == 0.0 {
                    Err("Modulo by zero".to_string())
                } else {
                    Ok(JsvValue::Number(ln % rn))
                }
            }
            OpKind::Eq => Ok(JsvValue::Boolean(ln == rn)),
            OpKind::Neq => Ok(JsvValue::Boolean(ln != rn)),
            OpKind::Lt => Ok(JsvValue::Boolean(ln < rn)),
            OpKind::Le => Ok(JsvValue::Boolean(ln <= rn)),
            OpKind::Gt => Ok(JsvValue::Boolean(ln > rn)),
            OpKind::Ge => Ok(JsvValue::Boolean(ln >= rn)),
            _ => Err("Invalid operator for numbers".to_string()),
        },
        (JsvValue::String(ls), JsvValue::String(rs)) => match op {
            OpKind::Add => Ok(JsvValue::String(format!("{}{}", ls, rs))),
            OpKind::Eq => Ok(JsvValue::Boolean(ls == rs)),
            OpKind::Neq => Ok(JsvValue::Boolean(ls != rs)),
            OpKind::Lt => Ok(JsvValue::Boolean(ls < rs)),
            OpKind::Gt => Ok(JsvValue::Boolean(ls > rs)),
            _ => Err("Invalid operator for strings".to_string()),
        },
        (JsvValue::String(ls), rhs) => {
            if op == OpKind::Add {
                Ok(JsvValue::String(format!(
                    "{}{}",
                    ls,
                    rhs.to_display_string()
                )))
            } else {
                Err("Type mismatch in binary operation".to_string())
            }
        }
        _ => Err("Type mismatch in binary operation".to_string()),
    }
}

/// Flatten the visible scope of `env` (walking the parent chain innermost
/// first, then outer scopes, so nearer bindings shadow outer ones) into a
/// single map. Used at function-call time so a frame carries the whole
/// visible scope in flat `bindings` instead of a deep parent chain — cloning
/// the chain on every nested call is O(depth²) stack work and overflows on
/// deep recursion.
fn flatten_env(env: &JsvEnvironment) -> HashMap<String, JsvValue> {
    let mut chain: Vec<&JsvEnvironment> = Vec::new();
    let mut current = Some(env);
    while let Some(e) = current {
        chain.push(e);
        current = e.parent.as_deref();
    }
    // Outermost first, innermost last — inner bindings overwrite outer ones.
    let mut out = HashMap::new();
    for e in chain.iter().rev() {
        for (k, v) in &e.bindings {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// Call a function value
fn call_function(
    func: JsvValue,
    args: Vec<JsvValue>,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx,
) -> Result<JsvValue, String> {
    // Bound the current nested call chain so circular/self recursion errors
    // out before the stack overflows (which would abort the process).
    ctx.call_depth += 1;
    if ctx.call_depth > MAX_CALL_DEPTH {
        return Err("Maximum call stack size exceeded".to_string());
    }
    let result = match func {
        JsvValue::NativeFn(name) => call_native_fn(&name, args, console),
        JsvValue::Function(_name, params, body) => {
            // Frame starts with the caller's visible scope flattened into its
            // own bindings (no parent chain), then parameters shadow it.
            let mut fn_env = JsvEnvironment::new();
            fn_env.bindings = flatten_env(env);
            for (i, param) in params.iter().enumerate() {
                let arg_val = args.get(i).cloned().unwrap_or(JsvValue::Undefined);
                fn_env.define(param, arg_val);
            }
            eval_expr(&body, &mut fn_env, console, ctx)
        }
        other => Err(format!(
            "TypeError: {} is not a function",
            other.to_display_string()
        )),
    };
    ctx.call_depth -= 1;
    result
}

/// Call a native built-in function
fn call_native_fn(
    name: &str,
    args: Vec<JsvValue>,
    console: &mut Vec<String>,
) -> Result<JsvValue, String> {
    match name {
        "console.log" | "print" => {
            let output = args
                .iter()
                .map(|a| a.to_display_string())
                .collect::<Vec<_>>()
                .join(" ");
            console.push(output.clone());
            #[cfg(debug_assertions)]
            println!("[JS] {}", output);
            Ok(JsvValue::Undefined)
        }
        "console.warn" => {
            let output = args
                .iter()
                .map(|a| a.to_display_string())
                .collect::<Vec<_>>()
                .join(" ");
            console.push(format!("WARN: {}", output));
            #[cfg(debug_assertions)]
            println!("[JS Warning] {}", output);
            Ok(JsvValue::Undefined)
        }
        "console.error" => {
            let output = args
                .iter()
                .map(|a| a.to_display_string())
                .collect::<Vec<_>>()
                .join(" ");
            console.push(format!("ERROR: {}", output));
            #[cfg(debug_assertions)]
            eprintln!("[JS Error] {}", output);
            Ok(JsvValue::Undefined)
        }
        "parseInt" => {
            if let Some(s) = args.first().and_then(|a| a.as_string()) {
                if let Ok(n) = s.parse::<f64>() {
                    return Ok(JsvValue::Number(n));
                }
            }
            Ok(JsvValue::Number(f64::NAN))
        }
        "parseFloat" => {
            if let Some(s) = args.first().and_then(|a| a.as_string()) {
                if let Ok(n) = s.parse::<f64>() {
                    return Ok(JsvValue::Number(n));
                }
            }
            Ok(JsvValue::Number(f64::NAN))
        }
        "String" => {
            if let Some(a) = args.first() {
                Ok(JsvValue::String(a.to_display_string()))
            } else {
                Ok(JsvValue::String("".to_string()))
            }
        }
        "Number" => {
            if let Some(a) = args.first() {
                match a {
                    JsvValue::Number(n) => Ok(JsvValue::Number(*n)),
                    JsvValue::String(s) => s
                        .parse::<f64>()
                        .map(JsvValue::Number)
                        .or(Ok(JsvValue::Number(f64::NAN))),
                    JsvValue::Boolean(b) => Ok(JsvValue::Number(if *b { 1.0 } else { 0.0 })),
                    _ => Ok(JsvValue::Number(f64::NAN)),
                }
            } else {
                Ok(JsvValue::Number(f64::NAN))
            }
        }
        _ => Err(format!("ReferenceError: {} is not defined", name)),
    }
}

// ===== TOKENIZER =====

fn tokenize(code: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = code.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }

        if chars[i].is_ascii_digit()
            || (chars[i] == '.' && i + 1 < len && chars[i + 1].is_ascii_digit())
        {
            let start = i;
            while i < len
                && (chars[i].is_ascii_digit()
                    || chars[i] == '.'
                    || chars[i] == 'e'
                    || chars[i] == 'E')
            {
                i += 1;
            }
            tokens.push(chars[start..i].iter().collect());
        } else if chars[i].is_alphabetic() || chars[i] == '_' || chars[i] == '$' {
            let start = i;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                i += 1;
            }
            tokens.push(chars[start..i].iter().collect());
        } else if chars[i] == '"' || chars[i] == '\'' || chars[i] == '`' {
            let quote = chars[i];
            i += 1;
            let mut s = String::new();
            let mut closed = false;
            while i < len {
                let c = chars[i];
                if c == quote {
                    closed = true;
                    i += 1;
                    break;
                }
                if c == '\\' && i + 1 < len {
                    let esc = chars[i + 1];
                    i += 2;
                    match esc {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        'b' => s.push('\u{0008}'),
                        'f' => s.push('\u{000C}'),
                        '0' => s.push('\0'),
                        '\\' => s.push('\\'),
                        '\'' => s.push('\''),
                        '"' => s.push('"'),
                        '`' => s.push('`'),
                        _ => {
                            s.push('\\');
                            s.push(esc);
                        }
                    }
                } else {
                    s.push(c);
                    i += 1;
                }
            }
            if !closed {
                return Err("Unterminated string literal".to_string());
            }
            tokens.push(format!("\"{}\"", s));
        } else if i + 1 < len {
            let two_chars = &code[i..i + 2];
            if matches!(two_chars, "==" | "!=" | "<=" | ">=" | "&&" | "||" | "=>") {
                tokens.push(two_chars.to_string());
                i += 2;
            } else if chars[i] == '/' && i + 1 < len && chars[i + 1] == '/' {
                i += 2;
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
            } else if chars[i] == '/' && i + 1 < len && chars[i + 1] == '*' {
                i += 2;
                while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            } else {
                tokens.push(chars[i].to_string());
                i += 1;
            }
        } else if [
            '+', '-', '*', '/', '%', '<', '>', '=', '!', '(', ')', '{', '}', '[', ']', ';', ',',
            '.',
        ]
        .contains(&chars[i])
        {
            tokens.push(chars[i].to_string());
            i += 1;
        } else {
            i += 1;
        }
    }
    Ok(tokens)
}

// ===== PARSER =====

fn parse_statements(
    tokens: &[String],
    pos: usize,
    depth: usize,
) -> Result<(Vec<JsvExpr>, usize), String> {
    if depth > MAX_PARSE_DEPTH {
        return Err("Script too deeply nested".to_string());
    }
    let mut stmts = Vec::new();
    let mut i = pos;

    while i < tokens.len() {
        let tok = &tokens[i];
        if tok == "}" || tok == ")" || tok == "]" {
            break;
        }
        if tok == ";" {
            i += 1;
            continue;
        }

        let (stmt, new_i) = parse_statement(tokens, i, depth)?;
        stmts.push(stmt);
        i = new_i;
        while i < tokens.len() && tokens[i] == ";" {
            i += 1;
        }
    }
    Ok((stmts, i))
}

fn parse_statement(
    tokens: &[String],
    pos: usize,
    depth: usize,
) -> Result<(JsvExpr, usize), String> {
    if pos >= tokens.len() {
        return Err("Unexpected end of input".to_string());
    }
    let tok = &tokens[pos];

    match tok.as_str() {
        "{" => {
            let (stmts, i) = parse_statements(tokens, pos + 1, depth + 1)?;
            if i >= tokens.len() || tokens[i] != "}" {
                return Err("Expected '}'".to_string());
            }
            Ok((JsvExpr::Block(stmts), i + 1))
        }
        "if" => {
            if pos + 1 >= tokens.len() || tokens[pos + 1] != "(" {
                return Err("Expected '(' after if".to_string());
            }
            let (cond, i) = parse_expression(tokens, pos + 2, depth + 1)?;
            if i >= tokens.len() || tokens[i] != ")" {
                return Err("Expected ')' after if condition".to_string());
            }
            let (then_stmt, i) = parse_statement(tokens, i + 1, depth + 1)?;
            let then_block = match then_stmt {
                JsvExpr::Block(stmts) => stmts,
                stmt => vec![stmt],
            };
            if i < tokens.len() && tokens[i] == "else" {
                let (else_stmt, i) = parse_statement(tokens, i + 1, depth + 1)?;
                let else_block = match else_stmt {
                    JsvExpr::Block(stmts) => stmts,
                    stmt => vec![stmt],
                };
                Ok((JsvExpr::If(Box::new(cond), then_block, Some(else_block)), i))
            } else {
                Ok((JsvExpr::If(Box::new(cond), then_block, None), i))
            }
        }
        "while" => {
            if pos + 1 >= tokens.len() || tokens[pos + 1] != "(" {
                return Err("Expected '(' after while".to_string());
            }
            let (cond, i) = parse_expression(tokens, pos + 2, depth + 1)?;
            if i >= tokens.len() || tokens[i] != ")" {
                return Err("Expected ')' after while condition".to_string());
            }
            let (body_stmt, i) = parse_statement(tokens, i + 1, depth + 1)?;
            let body = match body_stmt {
                JsvExpr::Block(stmts) => stmts,
                stmt => vec![stmt],
            };
            Ok((JsvExpr::While(Box::new(cond), body), i))
        }
        "var" | "let" | "const" => {
            if pos + 1 >= tokens.len() {
                return Err(format!("Expected variable name after {}", tok));
            }
            let var_name = tokens[pos + 1].clone();
            if !is_identifier(&var_name) {
                return Err(format!("Invalid variable name: {}", var_name));
            }
            let mut i = pos + 2;
            if i < tokens.len() && tokens[i] == "=" {
                let (val_expr, new_i) = parse_expression(tokens, i + 1, depth + 1)?;
                i = new_i;
                Ok((JsvExpr::Assignment(var_name, Box::new(val_expr)), i))
            } else {
                Ok((
                    JsvExpr::Assignment(var_name, Box::new(JsvExpr::Undefined)),
                    i,
                ))
            }
        }
        "return" => {
            if pos + 1 < tokens.len() && tokens[pos + 1] != ";" && tokens[pos + 1] != "}" {
                let (val, i) = parse_expression(tokens, pos + 1, depth + 1)?;
                Ok((JsvExpr::Return(Box::new(val)), i))
            } else {
                Ok((JsvExpr::Return(Box::new(JsvExpr::Undefined)), pos + 1))
            }
        }
        "function" => {
            if pos + 1 >= tokens.len() {
                return Err("Expected function name".to_string());
            }
            let name = tokens[pos + 1].clone();
            if pos + 2 >= tokens.len() || tokens[pos + 2] != "(" {
                return Err("Expected '(' after function name".to_string());
            }
            let mut params = Vec::new();
            let mut i = pos + 3;
            while i < tokens.len() && tokens[i] != ")" {
                if tokens[i] != "," && tokens[i] != ")" {
                    params.push(tokens[i].clone());
                }
                i += 1;
                while i < tokens.len() && tokens[i] == "," {
                    i += 1;
                }
            }
            if i >= tokens.len() {
                return Err("Expected ')' after parameters".to_string());
            }
            i += 1;
            if i >= tokens.len() || tokens[i] != "{" {
                return Err("Expected '{' for function body".to_string());
            }
            let (body_stmts, new_i) = parse_statements(tokens, i + 1, depth + 1)?;
            let mut i = new_i;
            if i >= tokens.len() || tokens[i] != "}" {
                return Err("Expected '}' to close function body".to_string());
            }
            i += 1;
            Ok((
                JsvExpr::FunctionDef(name, params, Box::new(JsvExpr::Block(body_stmts))),
                i,
            ))
        }
        _ => parse_expression(tokens, pos, depth + 1),
    }
}

// Expression parsing with precedence
fn parse_expression(
    tokens: &[String],
    pos: usize,
    depth: usize,
) -> Result<(JsvExpr, usize), String> {
    if depth > MAX_PARSE_DEPTH {
        return Err("Script too deeply nested".to_string());
    }
    parse_assignment(tokens, pos, depth)
}

fn parse_assignment(
    tokens: &[String],
    pos: usize,
    depth: usize,
) -> Result<(JsvExpr, usize), String> {
    let (left, i) = parse_or(tokens, pos, depth)?;
    if i < tokens.len() && tokens[i] == "=" {
        if i + 1 < tokens.len() && tokens[i + 1] == "=" {
            return Ok((left, i));
        }
        match left {
            JsvExpr::Identifier(name) => {
                let (right, new_i) = parse_assignment(tokens, i + 1, depth + 1)?;
                Ok((JsvExpr::Assignment(name, Box::new(right)), new_i))
            }
            _ => Err("Invalid assignment target".to_string()),
        }
    } else {
        Ok((left, i))
    }
}

fn parse_or(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (left, mut i) = parse_and(tokens, pos, depth)?;
    let mut result = left;
    while i < tokens.len() && tokens[i] == "||" {
        let (right, new_i) = parse_and(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), OpKind::Or, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_and(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (left, mut i) = parse_equality(tokens, pos, depth)?;
    let mut result = left;
    while i < tokens.len() && tokens[i] == "&&" {
        let (right, new_i) = parse_equality(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), OpKind::And, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_equality(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (mut result, mut i) = parse_comparison(tokens, pos, depth)?;
    // Left-associative loop so chained comparisons like `a == b == c` parse
    // instead of failing on the second operator.
    while i < tokens.len() && (tokens[i] == "==" || tokens[i] == "!=") {
        let op = if tokens[i] == "==" {
            OpKind::Eq
        } else {
            OpKind::Neq
        };
        let (right, new_i) = parse_comparison(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), op, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_comparison(
    tokens: &[String],
    pos: usize,
    depth: usize,
) -> Result<(JsvExpr, usize), String> {
    let (mut result, mut i) = parse_term(tokens, pos, depth)?;
    while i < tokens.len() && matches!(tokens[i].as_str(), "<" | ">" | "<=" | ">=") {
        let op = match tokens[i].as_str() {
            "<" => OpKind::Lt,
            ">" => OpKind::Gt,
            "<=" => OpKind::Le,
            _ => OpKind::Ge,
        };
        let (right, new_i) = parse_term(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), op, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_term(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (mut result, mut i) = parse_factor(tokens, pos, depth)?;
    // Left-associative loop so `a + b + c` (and `-` chains) parse.
    while i < tokens.len() && (tokens[i] == "+" || tokens[i] == "-") {
        let op = if tokens[i] == "+" {
            OpKind::Add
        } else {
            OpKind::Sub
        };
        let (right, new_i) = parse_factor(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), op, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_factor(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (mut result, mut i) = parse_unary(tokens, pos, depth)?;
    // Left-associative loop so `a * b * c` (and `/`, `%`) parse.
    while i < tokens.len() && (tokens[i] == "*" || tokens[i] == "/" || tokens[i] == "%") {
        let op = match tokens[i].as_str() {
            "*" => OpKind::Mul,
            "/" => OpKind::Div,
            _ => OpKind::Mod,
        };
        let (right, new_i) = parse_unary(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), op, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_unary(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    if pos >= tokens.len() {
        return Err("Unexpected end of expression".to_string());
    }
    if tokens[pos] == "-" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::UnaryOp(OpKind::Sub, Box::new(expr)), i))
    } else if tokens[pos] == "!" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::UnaryOp(OpKind::Not, Box::new(expr)), i))
    } else {
        parse_call(tokens, pos, depth)
    }
}

fn parse_call(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (mut expr, mut i) = parse_primary(tokens, pos, depth)?;

    loop {
        if i < tokens.len() && tokens[i] == "(" {
            let mut args = Vec::new();
            i += 1;
            while i < tokens.len() && tokens[i] != ")" {
                if tokens[i] != "," && tokens[i] != ")" {
                    let (arg, new_i) = parse_expression(tokens, i, depth + 1)?;
                    args.push(arg);
                    i = new_i;
                }
                while i < tokens.len() && tokens[i] == "," {
                    i += 1;
                }
            }
            if i >= tokens.len() {
                return Err("Expected ')'".to_string());
            }
            i += 1;
            expr = JsvExpr::Call(Box::new(expr), args);
        } else if i < tokens.len() && tokens[i] == "." {
            i += 1;
            if i >= tokens.len() || !is_identifier(&tokens[i]) {
                return Err("Expected property name after '.'".to_string());
            }
            let prop = tokens[i].clone();
            i += 1;
            if i < tokens.len() && tokens[i] == "(" {
                let name = format!("{}.{}", expr_to_name(&expr).unwrap_or_default(), prop);
                let mut args = Vec::new();
                i += 1;
                while i < tokens.len() && tokens[i] != ")" {
                    if tokens[i] != "," {
                        let (arg, new_i) = parse_expression(tokens, i, depth + 1)?;
                        args.push(arg);
                        i = new_i;
                    }
                    while i < tokens.len() && tokens[i] == "," {
                        i += 1;
                    }
                }
                if i >= tokens.len() {
                    return Err("Expected ')'".to_string());
                }
                i += 1;
                expr = JsvExpr::Call(Box::new(JsvExpr::Identifier(name)), args);
            }
        } else {
            break;
        }
    }
    Ok((expr, i))
}

fn parse_primary(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    if pos >= tokens.len() {
        return Err("Unexpected end of expression".to_string());
    }
    let tok = &tokens[pos];

    if tok == "(" {
        let (expr, i) = parse_expression(tokens, pos + 1, depth + 1)?;
        if i >= tokens.len() || tokens[i] != ")" {
            return Err("Expected ')'".to_string());
        }
        return Ok((expr, i + 1));
    }

    if let Ok(n) = tok.parse::<f64>() {
        return Ok((JsvExpr::Number(n), pos + 1));
    }

    if tok.starts_with('"') && tok.ends_with('"') {
        return Ok((JsvExpr::String(tok[1..tok.len() - 1].to_string()), pos + 1));
    }

    match tok.as_str() {
        "true" => return Ok((JsvExpr::Bool(true), pos + 1)),
        "false" => return Ok((JsvExpr::Bool(false), pos + 1)),
        "null" => return Ok((JsvExpr::Null, pos + 1)),
        "undefined" => return Ok((JsvExpr::Undefined, pos + 1)),
        _ => {}
    }

    if is_identifier(tok) {
        return Ok((JsvExpr::Identifier(tok.clone()), pos + 1));
    }

    Err(format!("Unexpected token: {}", tok))
}

fn is_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let chars: Vec<char> = s.chars().collect();
    if !chars[0].is_alphabetic() && chars[0] != '_' && chars[0] != '$' {
        return false;
    }
    chars
        .iter()
        .all(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
}

fn expr_to_name(expr: &JsvExpr) -> Option<String> {
    match expr {
        JsvExpr::Identifier(name) => Some(name.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let engine = JsvEngine::new();
        assert!(engine.global_env.bindings.contains_key("console"));
    }

    #[test]
    fn test_eval_basic_arithmetic() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("1 + 1");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_number(), Some(2.0));
    }

    #[test]
    fn test_eval_precedence_and_parens() {
        let mut engine = JsvEngine::new();
        let res1 = engine.eval("2 + 3 * 4").unwrap();
        assert_eq!(res1.as_number(), Some(14.0));
        let res2 = engine.eval("(2 + 3) * 4").unwrap();
        assert_eq!(res2.as_number(), Some(20.0));
    }

    #[test]
    fn test_variable_assignment() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("let x = 42");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_number(), Some(42.0));
        let x_val = engine.global_env.get("x").cloned();
        assert_eq!(x_val.and_then(|v| v.as_number()), Some(42.0));
    }

    #[test]
    fn test_variable_usage() {
        let mut engine = JsvEngine::new();
        engine.eval("let x = 10").unwrap();
        let result = engine.eval("x + 20").unwrap();
        assert_eq!(result.as_number(), Some(30.0));
    }

    #[test]
    fn test_string_concatenation() {
        let mut engine = JsvEngine::new();
        let result = engine.eval(r#""Hello, " + "World!""#).unwrap();
        assert_eq!(result.as_string(), Some("Hello, World!"));
    }

    #[test]
    fn test_comparison() {
        let mut engine = JsvEngine::new();
        assert_eq!(engine.eval("5 == 5").unwrap().as_boolean(), Some(true));
        assert_eq!(engine.eval("5 != 3").unwrap().as_boolean(), Some(true));
        assert_eq!(engine.eval("5 < 3").unwrap().as_boolean(), Some(false));
        assert_eq!(engine.eval("5 > 3").unwrap().as_boolean(), Some(true));
    }

    #[test]
    fn test_if_statement() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("if (true) { 42 } else { 0 }").unwrap();
        assert_eq!(result.as_number(), Some(42.0));
        let result2 = engine.eval("if (false) { 42 } else { 0 }").unwrap();
        assert_eq!(result2.as_number(), Some(0.0));
    }

    #[test]
    fn test_while_loop() {
        let mut engine = JsvEngine::new();
        engine.eval("let i = 0").unwrap();
        engine.eval("while (i < 3) { i = i + 1 }").unwrap();
        let i_val = engine.global_env.get("i").cloned();
        assert_eq!(i_val.and_then(|v| v.as_number()), Some(3.0));
    }

    #[test]
    fn test_function_definition_and_call() {
        let mut engine = JsvEngine::new();
        engine.eval("function add(a, b) { return a + b }").unwrap();
        let result = engine.eval("add(3, 4)").unwrap();
        assert_eq!(result.as_number(), Some(7.0));
    }

    #[test]
    fn test_console_log() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("console.log(\"Hello\")");
        assert!(result.is_ok());
        assert_eq!(engine.console_output.len(), 1);
        assert_eq!(engine.console_output[0], "Hello");
    }

    #[test]
    fn test_complex_expression() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("(10 + 5) * 2 - 3").unwrap();
        assert_eq!(result.as_number(), Some(27.0));
    }

    #[test]
    fn test_division_by_zero() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("1 / 0");
        assert!(result.is_err());
    }

    #[test]
    fn test_while_true_is_bounded() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("while (true) { }");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("step budget"), "got: {}", err);
    }

    #[test]
    fn test_infinite_recursion_errors_not_aborts() {
        let mut engine = JsvEngine::new();
        engine.eval("function f() { return f() }").unwrap();
        let result = engine.eval("f()");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("call stack"), "got: {}", err);
    }

    #[test]
    fn test_deeply_nested_script_is_rejected() {
        let mut engine = JsvEngine::new();
        let deep = format!(
            "{}{}",
            "(".repeat(2000),
            "1".to_string() + &")".repeat(2000)
        );
        let result = engine.eval(&deep);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("nested"), "got: {}", err);
    }

    #[test]
    fn test_return_inside_if_propagates() {
        let mut engine = JsvEngine::new();
        engine
            .eval("function f() { if (true) { return 5 } return 6 }")
            .unwrap();
        let result = engine.eval("f()").unwrap();
        assert_eq!(result.as_number(), Some(5.0));
    }

    #[test]
    fn test_return_inside_while_exits_loop() {
        let mut engine = JsvEngine::new();
        engine
            .eval(
                "function f() { let i = 0; while (true) { i = i + 1; if (i >= 3) { return i } } }",
            )
            .unwrap();
        let result = engine.eval("f()").unwrap();
        assert_eq!(result.as_number(), Some(3.0));
    }

    #[test]
    fn test_chained_operators() {
        let mut engine = JsvEngine::new();
        assert_eq!(engine.eval("1 + 2 + 3").unwrap().as_number(), Some(6.0));
        assert_eq!(engine.eval("10 - 3 - 2").unwrap().as_number(), Some(5.0));
        assert_eq!(engine.eval("2 * 3 * 4").unwrap().as_number(), Some(24.0));
        assert_eq!(engine.eval("100 / 2 / 5").unwrap().as_number(), Some(10.0));
        assert_eq!(
            engine.eval("1 + 2 * 3 + 4").unwrap().as_number(),
            Some(11.0)
        );
    }

    #[test]
    fn test_string_escapes() {
        let mut engine = JsvEngine::new();
        let result = engine.eval(r#""a\nb""#).unwrap();
        assert_eq!(result.as_string(), Some("a\nb"));
        let result = engine.eval(r#""say \"hi\"""#).unwrap();
        assert_eq!(result.as_string(), Some("say \"hi\""));
    }

    #[test]
    fn test_unterminated_string_is_error() {
        let mut engine = JsvEngine::new();
        let result = engine.eval(r#""oops"#);
        assert!(result.is_err());
    }
}
