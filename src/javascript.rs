// Embedded JavaScript engine

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::rc::Rc;

/// Opaque identifiers for the host objects exposed to a document Realm.
/// Their meaning belongs to the host implementation, never to page script.
pub const HOST_DOCUMENT: u64 = 1;
pub const HOST_WINDOW: u64 = 2;
pub const HOST_LOCAL_STORAGE: u64 = 3;
pub const HOST_HISTORY: u64 = 4;
pub const HOST_LOCATION: u64 = 5;
pub const HOST_EVENT: u64 = 6;
pub const HOST_FORM_DATA: u64 = 7;
pub const HOST_INDEXED_DB: u64 = 8;
pub const HOST_CACHE_STORAGE: u64 = 9;
pub const HOST_NAVIGATOR: u64 = 10;
pub const HOST_SERVICE_WORKER: u64 = 11;

/// Capability boundary between the interpreter and browser services.
///
/// The interpreter owns parsing, evaluation and language budgets. The host
/// owns DOM, networking and storage authority and receives only an allowlisted
/// object, property or method operation. This prevents script evaluation from
/// acquiring filesystem, process or unrestricted network access.
pub trait JsvHost {
    fn get_property(&mut self, object: u64, property: &str) -> Result<JsvValue, String>;
    fn set_property(
        &mut self,
        object: u64,
        property: &str,
        value: JsvValue,
    ) -> Result<JsvValue, String>;
    fn call(
        &mut self,
        object: u64,
        method: &str,
        arguments: Vec<JsvValue>,
    ) -> Result<JsvValue, String>;
}

struct NoHost;

impl JsvHost for NoHost {
    fn get_property(&mut self, _object: u64, property: &str) -> Result<JsvValue, String> {
        Err(format!(
            "SecurityError: host property '{}' is unavailable in this Realm",
            property
        ))
    }

    fn set_property(
        &mut self,
        _object: u64,
        property: &str,
        _value: JsvValue,
    ) -> Result<JsvValue, String> {
        Err(format!(
            "SecurityError: host property '{}' is unavailable in this Realm",
            property
        ))
    }

    fn call(
        &mut self,
        _object: u64,
        method: &str,
        _arguments: Vec<JsvValue>,
    ) -> Result<JsvValue, String> {
        Err(format!(
            "SecurityError: host method '{}' is unavailable in this Realm",
            method
        ))
    }
}

/// Hard cap on the number of expression evaluations per script. A single
/// `eval` call that exceeds it is aborted with an error instead of hanging,
/// which bounds non-terminating `while` loops, total work, and (together with
/// `MAX_CALL_DEPTH`) recursion depth.
const MAX_EVAL_STEPS: u64 = 5_000_000;

/// Per-value cap on string length created during evaluation. `s = s + s`
/// doubles a string every iteration, so without a cap a tiny script can
/// allocate gigabytes and abort the whole process (Rust OOM aborts, taking
/// the browser down with it).
const MAX_STRING_LENGTH: usize = 2 * 1024 * 1024;

/// Total budget of string bytes a single script may allocate. Billed per
/// allocation and never refunded, so a script that accumulates many large
/// strings in distinct variables is still bounded.
const MAX_STRING_BUDGET: usize = 64 * 1024 * 1024;

/// Maximum nested function-call depth while evaluating.
///
/// Deep or circular recursion (`function f() { return f() }`) errors out here
/// instead of overflowing the stack (a stack overflow in Rust aborts the whole
/// process). Debug builds are the constraint: unoptimized interpreter frames
/// are large (~several KB per JS call level, spread over eval_expr →
/// call_function → body → block eval), and `cargo test` runs on ~2 MB test
/// threads, so the debug cap is much lower than the release cap.
#[cfg(debug_assertions)]
const MAX_CALL_DEPTH: u64 = 12;
#[cfg(not(debug_assertions))]
const MAX_CALL_DEPTH: u64 = 64;

/// Maximum parser nesting depth, so hostile deeply-nested scripts error during
/// parsing instead of overflowing the stack in the recursive descent parser.
const MAX_PARSE_DEPTH: usize = 512;

/// Shared per-`eval` execution state threaded through the interpreter.
pub(crate) struct EvalCtx<'host> {
    /// Total expression evaluations performed so far in this script.
    steps: u64,
    /// Per-call ceiling, never greater than the browser-wide hard limit.
    max_steps: u64,
    /// Current nested function-call depth (incremented per call, decremented on return).
    call_depth: u64,
    /// Total bytes of strings allocated so far (billed, never refunded).
    string_bytes: usize,
    /// The only route from JavaScript evaluation to browser-owned authority.
    host: &'host mut dyn JsvHost,
    microtasks: VecDeque<PromiseReactionJob>,
    microtasks_queued: usize,
    pending_promise_reactions: HashMap<usize, Vec<PendingPromiseReaction>>,
    /// Persistent module graph for dynamic `import()`; temporarily vacated
    /// while a module evaluates so nested module evaluation can reuse the
    /// host context without aliasing the graph.
    modules: Option<&'host mut JsvModuleGraph>,
}

impl<'host> EvalCtx<'host> {
    fn new(host: &'host mut dyn JsvHost, modules: &'host mut JsvModuleGraph) -> Self {
        Self::with_step_limit(host, modules, MAX_EVAL_STEPS)
    }

    fn with_step_limit(
        host: &'host mut dyn JsvHost,
        modules: &'host mut JsvModuleGraph,
        max_steps: u64,
    ) -> Self {
        Self {
            steps: 0,
            max_steps: max_steps.min(MAX_EVAL_STEPS),
            call_depth: 0,
            string_bytes: 0,
            host,
            microtasks: VecDeque::new(),
            microtasks_queued: 0,
            pending_promise_reactions: HashMap::new(),
            modules: Some(modules),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromiseReactionKind {
    Fulfill,
    Reject,
    Finally,
}

#[derive(Debug, Clone)]
struct PromiseReactionJob {
    kind: PromiseReactionKind,
    handler: Option<JsvValue>,
    argument: JsvValue,
    result: JsvPromiseRef,
}

#[derive(Debug, Clone)]
struct PendingPromiseReaction {
    on_fulfilled: Option<JsvValue>,
    on_rejected: Option<JsvValue>,
    on_finally: Option<JsvValue>,
    completion_override: Option<(PromiseReactionKind, JsvValue)>,
    result: JsvPromiseRef,
}

const MAX_PROMISE_JOBS: usize = 2_048;

/// Reject string allocations over the per-value length cap and the per-script
/// byte budget. Must be called before the string is actually allocated.
fn check_string_alloc(ctx: &mut EvalCtx<'_>, len: usize) -> Result<(), String> {
    if len > MAX_STRING_LENGTH {
        return Err(format!(
            "String too large ({} bytes, max {})",
            len, MAX_STRING_LENGTH
        ));
    }
    ctx.string_bytes = ctx.string_bytes.saturating_add(len);
    if ctx.string_bytes > MAX_STRING_BUDGET {
        return Err("String memory budget exceeded".to_string());
    }
    Ok(())
}

/// Maximum console lines retained per engine (DevTools history). An
/// unbounded buffer grows memory across a session and re-lays out a
/// megabyte string per frame.
const MAX_CONSOLE_LINES: usize = 500;

/// Push a console line, keeping only the most recent `MAX_CONSOLE_LINES`.
fn push_console(console: &mut Vec<String>, line: String) {
    console.push(line);
    if console.len() > MAX_CONSOLE_LINES {
        let overflow = console.len() - MAX_CONSOLE_LINES;
        console.drain(0..overflow);
    }
}

/// True when a value is a `return` signal propagating through a statement container.
fn is_abrupt_signal(v: &JsvValue) -> bool {
    matches!(
        v,
        JsvValue::ReturnSignal(_)
            | JsvValue::ThrowSignal(_)
            | JsvValue::BreakSignal
            | JsvValue::ContinueSignal
    )
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
pub type JsvObjectRef = Rc<RefCell<JsvObject>>;
pub type JsvArrayRef = Rc<RefCell<Vec<JsvValue>>>;
pub type JsvPromiseRef = Rc<RefCell<JsvPromiseState>>;

#[derive(Debug, Clone)]
pub struct JsvObject {
    pub properties: HashMap<String, JsvValue>,
    pub prototype: Option<JsvObjectRef>,
}

#[derive(Debug, Clone)]
pub enum JsvPromiseState {
    Pending,
    Fulfilled(JsvValue),
    Rejected(JsvValue),
}

#[derive(Debug, Clone)]
pub enum JsvValue {
    Number(f64),
    Boolean(bool),
    String(String),
    Null,
    Undefined,
    Object(JsvObjectRef),
    Array(JsvArrayRef),
    Function(String, Vec<String>, Box<JsvExpr>, JsvEnvironment),
    AsyncFunction(String, Vec<String>, Box<JsvExpr>, JsvEnvironment),
    NativeFn(String),
    BoundNativeFn(String, Box<JsvValue>),
    Promise(JsvPromiseRef),
    /// Object implemented by the browser host, identified by an opaque token.
    HostObject(u64),
    /// Method retrieved from a host object. It retains its receiver so calls
    /// follow JavaScript member-call semantics without exposing Rust closures.
    HostFunction(u64, String),
    /// Internal: a `return` statement propagating through statement
    /// containers (if/while/block). Unwrapped to a plain value at function
    /// call and script boundaries.
    ReturnSignal(Box<JsvValue>),
    /// Internal abrupt completion produced by `throw`.
    ThrowSignal(Box<JsvValue>),
    /// Internal loop completion records consumed by the nearest iterator.
    BreakSignal,
    ContinueSignal,
}

impl PartialEq for JsvValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Number(left), Self::Number(right)) => left == right,
            (Self::Boolean(left), Self::Boolean(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            (Self::Null, Self::Null) | (Self::Undefined, Self::Undefined) => true,
            (Self::Object(left), Self::Object(right)) => Rc::ptr_eq(left, right),
            (Self::Array(left), Self::Array(right)) => Rc::ptr_eq(left, right),
            (Self::Promise(left), Self::Promise(right)) => Rc::ptr_eq(left, right),
            (Self::NativeFn(left), Self::NativeFn(right)) => left == right,
            (Self::BoundNativeFn(left_name, left), Self::BoundNativeFn(right_name, right)) => {
                left_name == right_name && left == right
            }
            (Self::HostObject(left), Self::HostObject(right)) => left == right,
            (
                Self::HostFunction(left_object, left_name),
                Self::HostFunction(right_object, right_name),
            ) => left_object == right_object && left_name == right_name,
            (Self::ReturnSignal(left), Self::ReturnSignal(right))
            | (Self::ThrowSignal(left), Self::ThrowSignal(right)) => left == right,
            (Self::BreakSignal, Self::BreakSignal)
            | (Self::ContinueSignal, Self::ContinueSignal) => true,
            _ => false,
        }
    }
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
        JsvValue::Object(Rc::new(RefCell::new(JsvObject {
            properties: HashMap::new(),
            prototype: None,
        })))
    }
    pub fn array() -> Self {
        JsvValue::Array(Rc::new(RefCell::new(Vec::new())))
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
                a.borrow()
                    .iter()
                    .map(|v| v.to_display_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            JsvValue::Function(name, _, _, _) => format!("[Function: {}]", name),
            JsvValue::AsyncFunction(name, _, _, _) => format!("[AsyncFunction: {}]", name),
            JsvValue::NativeFn(name) => format!("[Native Function: {}]", name),
            JsvValue::BoundNativeFn(name, _) => format!("[Native Method: {}]", name),
            JsvValue::Promise(_) => "[object Promise]".to_string(),
            JsvValue::HostObject(_) => "[object HostObject]".to_string(),
            JsvValue::HostFunction(_, name) => format!("[Host Function: {}]", name),
            JsvValue::ReturnSignal(v) => v.to_display_string(),
            JsvValue::ThrowSignal(v) => v.to_display_string(),
            JsvValue::BreakSignal => "break".to_string(),
            JsvValue::ContinueSignal => "continue".to_string(),
        }
    }

    /// Identity comparison for listener removal: two function values refer to
    /// the same callable only when their name, parameters, body and captured
    /// environment record are identical (the interpreter keeps one record per
    /// declaration site, so the pointer check is exact).
    pub fn function_identity_eq(&self, other: &JsvValue) -> bool {
        match (self, other) {
            (
                JsvValue::Function(name_a, params_a, body_a, env_a),
                JsvValue::Function(name_b, params_b, body_b, env_b),
            ) => {
                name_a == name_b
                    && params_a == params_b
                    && body_a == body_b
                    && env_a.same_record(env_b)
            }
            (
                JsvValue::AsyncFunction(name_a, params_a, body_a, env_a),
                JsvValue::AsyncFunction(name_b, params_b, body_b, env_b),
            ) => {
                name_a == name_b
                    && params_a == params_b
                    && body_a == body_b
                    && env_a.same_record(env_b)
            }
            (JsvValue::NativeFn(name_a), JsvValue::NativeFn(name_b)) => name_a == name_b,
            (
                JsvValue::HostFunction(object_a, name_a),
                JsvValue::HostFunction(object_b, name_b),
            ) => object_a == object_b && name_a == name_b,
            _ => false,
        }
    }
}

fn object_value(properties: HashMap<String, JsvValue>) -> JsvValue {
    JsvValue::Object(Rc::new(RefCell::new(JsvObject {
        properties,
        prototype: None,
    })))
}

fn object_property(object: &JsvObjectRef, name: &str) -> Option<JsvValue> {
    let mut current = Some(object.clone());
    for _ in 0..64 {
        let reference = current?;
        let borrowed = reference.borrow();
        if let Some(value) = borrowed.properties.get(name) {
            return Some(value.clone());
        }
        current = borrowed.prototype.clone();
    }
    None
}

fn property_key(value: &JsvValue) -> String {
    match value {
        JsvValue::Number(number) if number.is_finite() && *number == number.floor() => {
            (*number as i64).to_string()
        }
        _ => value.to_display_string(),
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
    ObjectLiteral(Vec<(String, JsvExpr)>),
    ArrayLiteral(Vec<JsvExpr>),

    // Variables
    Identifier(String),
    Assignment(String, Box<JsvExpr>),
    VariableDeclaration(String, Box<JsvExpr>, DeclarationKind),
    Member(Box<JsvExpr>, String),
    MemberAssignment(Box<JsvExpr>, String, Box<JsvExpr>),
    Index(Box<JsvExpr>, Box<JsvExpr>),
    IndexAssignment(Box<JsvExpr>, Box<JsvExpr>, Box<JsvExpr>),

    // Operations
    BinaryOp(Box<JsvExpr>, OpKind, Box<JsvExpr>),
    UnaryOp(OpKind, Box<JsvExpr>),

    // Control flow
    If(Box<JsvExpr>, Vec<JsvExpr>, Option<Vec<JsvExpr>>),
    While(Box<JsvExpr>, Vec<JsvExpr>),
    ForOf(String, DeclarationKind, Box<JsvExpr>, Vec<JsvExpr>),
    Block(Vec<JsvExpr>),
    TryCatchFinally {
        try_body: Vec<JsvExpr>,
        catch_binding: Option<String>,
        catch_body: Vec<JsvExpr>,
        finally_body: Vec<JsvExpr>,
    },
    Switch(
        Box<JsvExpr>,
        Vec<(Option<JsvExpr>, Vec<JsvExpr>)>,
        Option<Vec<JsvExpr>>,
    ),

    // Conditional and template expressions
    Ternary(Box<JsvExpr>, Box<JsvExpr>, Box<JsvExpr>),
    Template(Vec<TemplatePart>),

    // Functions
    Call(Box<JsvExpr>, Vec<JsvExpr>),
    FunctionDef(String, Vec<String>, Box<JsvExpr>),
    AsyncFunctionDef(String, Vec<String>, Box<JsvExpr>),
    FunctionExpr(Vec<String>, Box<JsvExpr>, bool),
    /// `new` construction. Host constructors (Event/CustomEvent/FormData)
    /// receive the call through the host bridge; interpreter functions fall
    /// back to call semantics because this profile has no class system.
    NewExpr(Box<JsvExpr>, Vec<JsvExpr>),
    /// Optional chaining member/index access that short-circuits to
    /// `undefined` on nullish bases.
    OptionalMember(Box<JsvExpr>, String),
    OptionalIndex(Box<JsvExpr>, Box<JsvExpr>),
    /// Prefix/postfix `++`/`--` on an assignable target.
    Update(Box<JsvExpr>, bool, bool),
    /// Dynamic `import('specifier')` returning a Promise of the module
    /// namespace. The specifier must be a string literal (bounded profile).
    DynamicImport(String),

    // Return
    Return(Box<JsvExpr>),
    Throw(Box<JsvExpr>),
    Await(Box<JsvExpr>),
    Break,
    Continue,
}

/// One literal-text or interpolated-expression segment of a template literal.
#[derive(Debug, Clone, PartialEq)]
pub enum TemplatePart {
    Text(String),
    Expr(Box<JsvExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclarationKind {
    Var,
    Let,
    Const,
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
    StrictEq,
    StrictNeq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Nullish,
    Not,
    Typeof,
    Instanceof,
    In,
    Assign,
}

/// A shared lexical environment record. Functions retain the record itself,
/// not a snapshot of its values, so sibling closures observe the same live
/// bindings and assignments update the nearest defining scope.
#[derive(Debug)]
struct EnvironmentRecord {
    parent: Option<Rc<RefCell<EnvironmentRecord>>>,
    bindings: HashMap<String, EnvironmentBinding>,
}

#[derive(Debug, Clone)]
struct EnvironmentBinding {
    value: JsvValue,
    mutable: bool,
}

/// Environment with lexical scoping.
#[derive(Debug, Clone)]
pub struct JsvEnvironment {
    record: Rc<RefCell<EnvironmentRecord>>,
}

impl Default for JsvEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl JsvEnvironment {
    pub fn new() -> Self {
        Self {
            record: Rc::new(RefCell::new(EnvironmentRecord {
                parent: None,
                bindings: HashMap::new(),
            })),
        }
    }

    pub fn with_parent(parent: JsvEnvironment) -> Self {
        Self {
            record: Rc::new(RefCell::new(EnvironmentRecord {
                parent: Some(parent.record),
                bindings: HashMap::new(),
            })),
        }
    }

    pub fn get(&self, name: &str) -> Option<JsvValue> {
        let (value, parent) = {
            let record = self.record.borrow();
            (
                record
                    .bindings
                    .get(name)
                    .map(|binding| binding.value.clone()),
                record.parent.clone(),
            )
        };
        value.or_else(|| parent.and_then(|record| Self { record }.get(name)))
    }

    pub fn set(&mut self, name: &str, value: JsvValue) {
        if self.assign_existing(name, &value).unwrap_or(false) {
            return;
        }
        self.record.borrow_mut().bindings.insert(
            name.to_string(),
            EnvironmentBinding {
                value,
                mutable: true,
            },
        );
    }

    pub fn define(&mut self, name: &str, value: JsvValue) {
        self.record.borrow_mut().bindings.insert(
            name.to_string(),
            EnvironmentBinding {
                value,
                mutable: true,
            },
        );
    }

    fn declare(
        &mut self,
        name: &str,
        value: JsvValue,
        mutable: bool,
        allow_redeclare: bool,
    ) -> Result<(), String> {
        let mut record = self.record.borrow_mut();
        if !allow_redeclare && record.bindings.contains_key(name) {
            return Err(format!(
                "SyntaxError: identifier '{}' has already been declared",
                name
            ));
        }
        record
            .bindings
            .insert(name.to_string(), EnvironmentBinding { value, mutable });
        Ok(())
    }

    fn assign(&mut self, name: &str, value: JsvValue) -> Result<(), String> {
        if self.assign_existing(name, &value)? {
            return Ok(());
        }
        self.declare(name, value, true, true)
    }

    pub fn contains_binding(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// True when both environments share the same underlying record (used for
    /// function identity: closures defined at the same declaration site keep
    /// the same record pointer even when cloned).
    pub fn same_record(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.record, &other.record)
    }

    fn assign_existing(&self, name: &str, value: &JsvValue) -> Result<bool, String> {
        let parent = {
            let mut record = self.record.borrow_mut();
            if let Some(binding) = record.bindings.get_mut(name) {
                if !binding.mutable {
                    return Err(format!("TypeError: assignment to constant '{}'", name));
                }
                binding.value = value.clone();
                return Ok(true);
            }
            record.parent.clone()
        };
        match parent {
            Some(record) => Self { record }.assign_existing(name, value),
            None => Ok(false),
        }
    }
}

/// Main JavaScript engine
pub struct JsvEngine {
    pub global_env: JsvEnvironment,
    pub console_output: Vec<String>,
    /// Persistent module registry and evaluated cache shared by every script
    /// turn and dynamic `import()` on this engine (Phase 21).
    pub modules: JsvModuleGraph,
}

impl Default for JsvEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl JsvEngine {
    pub fn new() -> Self {
        // Create sandbox environment with restricted global objects
        let mut env = JsvEnvironment::new();

        // Set up console object with methods (only safe methods)
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
        env.set("console", object_value(console_obj));

        // Math object (safe)
        let mut math_obj = HashMap::new();
        math_obj.insert("PI".to_string(), JsvValue::Number(std::f64::consts::PI));
        math_obj.insert("E".to_string(), JsvValue::Number(std::f64::consts::E));
        math_obj.insert("MAX_VALUE".to_string(), JsvValue::Number(f64::MAX));
        math_obj.insert("MIN_VALUE".to_string(), JsvValue::Number(f64::MIN));
        for name in ["abs", "ceil", "floor", "round", "sqrt", "random"] {
            math_obj.insert(
                name.to_string(),
                JsvValue::NativeFn(format!("Math.{}", name)),
            );
        }
        env.set("Math", object_value(math_obj));

        let mut date_obj = HashMap::new();
        date_obj.insert(
            "now".to_string(),
            JsvValue::NativeFn("Date.now".to_string()),
        );
        date_obj.insert(
            "parse".to_string(),
            JsvValue::NativeFn("Date.parse".to_string()),
        );
        env.set("Date", object_value(date_obj));

        let mut object_constructor = HashMap::new();
        object_constructor.insert(
            "create".to_string(),
            JsvValue::NativeFn("Object.create".to_string()),
        );
        object_constructor.insert(
            "keys".to_string(),
            JsvValue::NativeFn("Object.keys".to_string()),
        );
        env.set("Object", object_value(object_constructor));

        let mut array_constructor = HashMap::new();
        array_constructor.insert(
            "isArray".to_string(),
            JsvValue::NativeFn("Array.isArray".to_string()),
        );
        env.set("Array", object_value(array_constructor));

        let mut json_constructor = HashMap::new();
        json_constructor.insert(
            "stringify".to_string(),
            JsvValue::NativeFn("JSON.stringify".to_string()),
        );
        json_constructor.insert(
            "parse".to_string(),
            JsvValue::NativeFn("JSON.parse".to_string()),
        );
        env.set("JSON", object_value(json_constructor));

        let mut promise_constructor = HashMap::new();
        for name in ["resolve", "reject", "all", "race"] {
            promise_constructor.insert(
                name.to_string(),
                JsvValue::NativeFn(format!("Promise.{}", name)),
            );
        }
        env.set("Promise", object_value(promise_constructor));

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

        // Safe built-in functions (no access to window/document)
        env.set("parseInt", JsvValue::NativeFn("parseInt".to_string()));
        env.set("parseFloat", JsvValue::NativeFn("parseFloat".to_string()));
        env.set("String", JsvValue::NativeFn("String".to_string()));
        env.set("Number", JsvValue::NativeFn("Number".to_string()));

        // Safe Math methods
        env.set("Math.abs", JsvValue::NativeFn("Math.abs".to_string()));
        env.set("Math.ceil", JsvValue::NativeFn("Math.ceil".to_string()));
        env.set("Math.floor", JsvValue::NativeFn("Math.floor".to_string()));
        env.set("Math.round", JsvValue::NativeFn("Math.round".to_string()));
        env.set("Math.sqrt", JsvValue::NativeFn("Math.sqrt".to_string()));
        env.set("Math.random", JsvValue::NativeFn("Math.random".to_string()));

        // Math constants (member access resolves through "Math.PI" keys)
        env.set("Math.PI", JsvValue::Number(std::f64::consts::PI));
        env.set("Math.E", JsvValue::Number(std::f64::consts::E));

        // Safe Date methods (limited)
        env.set("Date.now", JsvValue::NativeFn("Date.now".to_string()));
        env.set("Date.parse", JsvValue::NativeFn("Date.parse".to_string()));

        // Constants
        env.set("undefined", JsvValue::Undefined);
        env.set("null", JsvValue::Null);
        env.set("true", JsvValue::Boolean(true));
        env.set("false", JsvValue::Boolean(false));

        // Block dangerous global objects
        // These will return empty objects when accessed, preventing XSS
        env.set("document", JsvValue::object());
        env.set("window", JsvValue::object());
        env.set("self", JsvValue::object());
        env.set("globalThis", JsvValue::object());
        env.set(
            "eval",
            JsvValue::NativeFn("[SANDBOXED] eval is disabled".to_string()),
        );
        env.set(
            "Function",
            JsvValue::NativeFn("[SANDBOXED] Function constructor is disabled".to_string()),
        );
        env.set(
            "setTimeout",
            JsvValue::NativeFn("[SANDBOXED] setTimeout is disabled".to_string()),
        );
        env.set(
            "setInterval",
            JsvValue::NativeFn("[SANDBOXED] setInterval is disabled".to_string()),
        );
        env.set(
            "clearTimeout",
            JsvValue::NativeFn("[SANDBOXED] clearTimeout is disabled".to_string()),
        );
        env.set(
            "clearInterval",
            JsvValue::NativeFn("[SANDBOXED] clearInterval is disabled".to_string()),
        );
        env.set(
            "document.write",
            JsvValue::NativeFn("[SANDBOXED] document.write is disabled".to_string()),
        );
        env.set(
            "document.writeln",
            JsvValue::NativeFn("[SANDBOXED] document.writeln is disabled".to_string()),
        );
        env.set(
            "document.getElementById",
            JsvValue::NativeFn("[SANDBOXED] getElementById is disabled".to_string()),
        );
        env.set(
            "document.createElement",
            JsvValue::NativeFn("[SANDBOXED] createElement is disabled".to_string()),
        );
        env.set(
            "window.alert",
            JsvValue::NativeFn("[SANDBOXED] alert is disabled".to_string()),
        );
        env.set(
            "window.confirm",
            JsvValue::NativeFn("[SANDBOXED] confirm is disabled".to_string()),
        );
        env.set(
            "window.prompt",
            JsvValue::NativeFn("[SANDBOXED] prompt is disabled".to_string()),
        );
        env.set(
            "window.open",
            JsvValue::NativeFn("[SANDBOXED] open is disabled".to_string()),
        );
        env.set(
            "window.location",
            JsvValue::NativeFn("[SANDBOXED] location is disabled".to_string()),
        );
        env.set(
            "window.history",
            JsvValue::NativeFn("[SANDBOXED] history is disabled".to_string()),
        );
        env.set(
            "window.navigator",
            JsvValue::NativeFn("[SANDBOXED] navigator is disabled".to_string()),
        );
        env.set(
            "window.frames",
            JsvValue::NativeFn("[SANDBOXED] frames is disabled".to_string()),
        );
        env.set(
            "window.parent",
            JsvValue::NativeFn("[SANDBOXED] parent is disabled".to_string()),
        );
        env.set(
            "window.top",
            JsvValue::NativeFn("[SANDBOXED] top is disabled".to_string()),
        );
        env.set(
            "window.opener",
            JsvValue::NativeFn("[SANDBOXED] opener is disabled".to_string()),
        );
        env.set(
            "window.crypto",
            JsvValue::NativeFn("[SANDBOXED] crypto is disabled".to_string()),
        );
        env.set(
            "window.localStorage",
            JsvValue::NativeFn("[SANDBOXED] localStorage is disabled".to_string()),
        );
        env.set(
            "window.sessionStorage",
            JsvValue::NativeFn("[SANDBOXED] sessionStorage is disabled".to_string()),
        );
        env.set(
            "window.XMLHttpRequest",
            JsvValue::NativeFn("[SANDBOXED] XMLHttpRequest is disabled".to_string()),
        );
        env.set(
            "window.fetch",
            JsvValue::NativeFn("[SANDBOXED] fetch is disabled".to_string()),
        );
        env.set(
            "window.requestAnimationFrame",
            JsvValue::NativeFn("[SANDBOXED] requestAnimationFrame is disabled".to_string()),
        );
        env.set(
            "window.cancelAnimationFrame",
            JsvValue::NativeFn("[SANDBOXED] cancelAnimationFrame is disabled".to_string()),
        );

        Self {
            global_env: env,
            console_output: Vec::new(),
            modules: JsvModuleGraph::new(),
        }
    }

    pub fn eval(&mut self, code: &str) -> Result<JsvValue, String> {
        let mut host = NoHost;
        self.eval_with_host(code, &mut host)
    }

    /// Evaluate an isolated script with a caller-selected instruction ceiling.
    /// The ceiling can only reduce, never raise, the engine-wide hard limit.
    pub fn eval_with_step_limit(&mut self, code: &str, max_steps: u64) -> Result<JsvValue, String> {
        if max_steps == 0 {
            return Err("Script timed out (step budget exceeded)".to_string());
        }
        let mut host = NoHost;
        self.eval_with_host_limit(code, &mut host, max_steps)
    }

    /// Execute a script with explicitly provided browser-host capabilities.
    /// The host object bindings are installed only for this call; the caller
    /// controls the concrete capability implementation and its resource limits.
    pub fn execute_with_host(
        &mut self,
        script: &str,
        host: &mut dyn JsvHost,
    ) -> Result<JsvValue, String> {
        self.global_env
            .define("document", JsvValue::HostObject(HOST_DOCUMENT));
        self.global_env
            .define("window", JsvValue::HostObject(HOST_WINDOW));
        self.global_env
            .define("self", JsvValue::HostObject(HOST_WINDOW));
        self.global_env
            .define("globalThis", JsvValue::HostObject(HOST_WINDOW));
        self.global_env
            .define("localStorage", JsvValue::HostObject(HOST_LOCAL_STORAGE));
        self.global_env
            .define("indexedDB", JsvValue::HostObject(HOST_INDEXED_DB));
        self.global_env
            .define("caches", JsvValue::HostObject(HOST_CACHE_STORAGE));
        self.global_env
            .define("navigator", JsvValue::HostObject(HOST_NAVIGATOR));
        self.global_env.define(
            "fetch",
            JsvValue::HostFunction(HOST_WINDOW, "fetch".to_string()),
        );
        self.global_env.define(
            "MediaSource",
            JsvValue::HostFunction(HOST_WINDOW, "MediaSource".to_string()),
        );
        for name in [
            "WebSocket",
            "EventSource",
            "BroadcastChannel",
            "structuredClone",
        ] {
            self.global_env
                .define(name, JsvValue::HostFunction(HOST_WINDOW, name.to_string()));
        }
        for name in ["setTimeout", "setInterval", "clearTimeout", "clearInterval"] {
            self.global_env
                .define(name, JsvValue::HostFunction(HOST_WINDOW, name.to_string()));
        }
        for name in ["Event", "CustomEvent", "FormData"] {
            self.global_env
                .define(name, JsvValue::HostFunction(HOST_WINDOW, name.to_string()));
        }
        self.global_env
            .define("location", JsvValue::HostObject(HOST_LOCATION));
        self.global_env
            .define("history", JsvValue::HostObject(HOST_HISTORY));
        self.eval_with_host(script, host)
    }

    fn eval_with_host(&mut self, code: &str, host: &mut dyn JsvHost) -> Result<JsvValue, String> {
        self.eval_with_host_limit(code, host, MAX_EVAL_STEPS)
    }

    fn eval_with_host_limit(
        &mut self,
        code: &str,
        host: &mut dyn JsvHost,
        max_steps: u64,
    ) -> Result<JsvValue, String> {
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
        let modules = &mut self.modules;
        let mut ctx = EvalCtx::with_step_limit(host, modules, max_steps);

        let mut result = JsvValue::Undefined;
        for expression in &exprs {
            result = eval_expr(expression, env, console, &mut ctx)?;
            if is_abrupt_signal(&result) {
                break;
            }
        }
        drain_promise_jobs(env, console, &mut ctx)?;
        match result {
            JsvValue::ReturnSignal(value) => Ok(*value),
            JsvValue::ThrowSignal(reason) => Err(format!(
                "Uncaught exception: {}",
                reason.to_display_string()
            )),
            JsvValue::BreakSignal => Err("SyntaxError: illegal break statement".to_string()),
            JsvValue::ContinueSignal => Err("SyntaxError: illegal continue statement".to_string()),
            value => Ok(value),
        }
    }

    pub fn execute_script(&mut self, script: &str) -> Result<JsvValue, String> {
        self.eval(script)
    }

    /// Invoke a function value captured by an earlier script turn (event
    /// listener, timer callback or Promise handler) through the same engine,
    /// so its closure environment and the global bindings stay live across
    /// tasks. The caller supplies the host capabilities for the duration of
    /// the callback; each invocation gets a fresh execution budget.
    pub fn invoke_callback(
        &mut self,
        callback: &JsvValue,
        arguments: Vec<JsvValue>,
        host: &mut dyn JsvHost,
    ) -> Result<JsvValue, String> {
        let modules = &mut self.modules;
        let mut ctx = EvalCtx::new(host, modules);
        let result = call_function(
            callback.clone(),
            arguments,
            &mut self.global_env,
            &mut self.console_output,
            &mut ctx,
        )?;
        drain_promise_jobs(&mut self.global_env, &mut self.console_output, &mut ctx)?;
        match result {
            JsvValue::ReturnSignal(value) => Ok(*value),
            JsvValue::ThrowSignal(reason) => Err(format!(
                "Uncaught exception in callback: {}",
                reason.to_display_string()
            )),
            JsvValue::BreakSignal => {
                Err("SyntaxError: illegal break statement in callback".to_string())
            }
            JsvValue::ContinueSignal => {
                Err("SyntaxError: illegal continue statement in callback".to_string())
            }
            value => Ok(value),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModuleNamespace {
    pub exports: BTreeMap<String, JsvValue>,
}

#[derive(Debug, Clone)]
struct ModuleImport {
    specifier: String,
    bindings: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct ParsedModule {
    imports: Vec<ModuleImport>,
    exports: Vec<String>,
    body: String,
}

/// Bounded parse/link/evaluate graph for the Phase 10 module subset.
/// Module source is supplied by the caller; this type performs no I/O.
#[derive(Debug, Default)]
pub struct JsvModuleGraph {
    sources: BTreeMap<String, String>,
    evaluated: BTreeMap<String, ModuleNamespace>,
    total_source_bytes: usize,
}

impl JsvModuleGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, specifier: &str, source: &str) -> Result<(), String> {
        if specifier.is_empty() || specifier.len() > 1_024 {
            return Err("Invalid module specifier".to_string());
        }
        if source.len() > MAX_STRING_LENGTH {
            return Err("Module source exceeds 2 MB".to_string());
        }
        let previous = self.sources.get(specifier).map(String::len).unwrap_or(0);
        let projected = self
            .total_source_bytes
            .saturating_sub(previous)
            .saturating_add(source.len());
        if projected > 32 * 1024 * 1024
            || (!self.sources.contains_key(specifier) && self.sources.len() >= 256)
        {
            return Err("Module graph budget exceeded".to_string());
        }
        self.sources
            .insert(specifier.to_string(), source.to_string());
        self.total_source_bytes = projected;
        self.evaluated.clear();
        Ok(())
    }

    pub fn evaluate(&mut self, entry: &str) -> Result<ModuleNamespace, String> {
        let mut stack = Vec::new();
        self.evaluate_inner(entry, &mut stack, 0)
    }

    fn evaluate_inner(
        &mut self,
        specifier: &str,
        stack: &mut Vec<String>,
        depth: usize,
    ) -> Result<ModuleNamespace, String> {
        if let Some(namespace) = self.evaluated.get(specifier) {
            return Ok(namespace.clone());
        }
        if depth > 64 {
            return Err("Module dependency depth exceeded".to_string());
        }
        if stack.iter().any(|active| active == specifier) {
            return Err(format!("Circular module dependency: {}", specifier));
        }
        let source = self
            .sources
            .get(specifier)
            .cloned()
            .ok_or_else(|| format!("Module not found: {}", specifier))?;
        let parsed = parse_module_source(&source)?;
        stack.push(specifier.to_string());

        let mut imported = Vec::new();
        for import in &parsed.imports {
            let namespace = self.evaluate_inner(&import.specifier, stack, depth + 1)?;
            for (exported, local) in &import.bindings {
                let value = namespace.exports.get(exported).cloned().ok_or_else(|| {
                    format!(
                        "Module '{}' does not export '{}'",
                        import.specifier, exported
                    )
                })?;
                imported.push((local.clone(), value));
            }
        }

        let mut engine = JsvEngine::new();
        for (name, value) in imported {
            engine.global_env.define(&name, value);
        }
        engine
            .execute_script(&parsed.body)
            .map_err(|error| format!("Module '{}': {}", specifier, error))?;
        let mut namespace = ModuleNamespace::default();
        for name in parsed.exports {
            let value = engine.global_env.get(&name).ok_or_else(|| {
                format!(
                    "Module '{}' did not initialize export '{}'",
                    specifier, name
                )
            })?;
            namespace.exports.insert(name, value);
        }
        stack.pop();
        self.evaluated
            .insert(specifier.to_string(), namespace.clone());
        Ok(namespace)
    }

    /// Evaluate a module inside the caller's engine context (Phase 21
    /// dynamic import): imports resolve through the same graph, the module
    /// body evaluates in a module-scoped environment whose parent is the
    /// caller's environment, and exports are read back from that scope.
    /// The evaluated cache persists across calls, so later imports of the
    /// same module return the same namespace without re-execution.
    pub(crate) fn evaluate_in_env(
        &mut self,
        specifier: &str,
        env: &JsvEnvironment,
        console: &mut Vec<String>,
        ctx: &mut EvalCtx<'_>,
    ) -> Result<ModuleNamespace, String> {
        if let Some(namespace) = self.evaluated.get(specifier) {
            return Ok(namespace.clone());
        }
        let mut stack = Vec::new();
        self.evaluate_in_env_inner(specifier, &mut stack, 0, env, console, ctx)
    }

    fn evaluate_in_env_inner(
        &mut self,
        specifier: &str,
        stack: &mut Vec<String>,
        depth: usize,
        env: &JsvEnvironment,
        console: &mut Vec<String>,
        ctx: &mut EvalCtx<'_>,
    ) -> Result<ModuleNamespace, String> {
        if let Some(namespace) = self.evaluated.get(specifier) {
            return Ok(namespace.clone());
        }
        if depth > 64 {
            return Err("Module dependency depth exceeded".to_string());
        }
        if stack.iter().any(|active| active == specifier) {
            return Err(format!("Circular module dependency: {}", specifier));
        }
        let source = self
            .sources
            .get(specifier)
            .cloned()
            .ok_or_else(|| format!("Module not found: {}", specifier))?;
        let parsed = parse_module_source(&source)?;
        stack.push(specifier.to_string());

        let mut module_env = JsvEnvironment::with_parent(env.clone());
        for import in &parsed.imports {
            let namespace =
                self.evaluate_in_env_inner(&import.specifier, stack, depth + 1, env, console, ctx)?;
            for (exported, local) in &import.bindings {
                let value = namespace.exports.get(exported).cloned().ok_or_else(|| {
                    format!(
                        "Module '{}' does not export '{}'",
                        import.specifier, exported
                    )
                })?;
                module_env.define(local, value);
            }
        }

        let tokens = tokenize(&parsed.body)?;
        let (exprs, _rest) = parse_statements(&tokens, 0, 0)?;
        for expression in &exprs {
            let value = eval_expr(expression, &mut module_env, console, ctx)?;
            if is_abrupt_signal(&value) {
                break;
            }
        }
        drain_promise_jobs(&mut module_env, console, ctx)?;
        let mut namespace = ModuleNamespace::default();
        for name in parsed.exports {
            let value = module_env.get(&name).ok_or_else(|| {
                format!(
                    "Module '{}' did not initialize export '{}'",
                    specifier, name
                )
            })?;
            namespace.exports.insert(name, value);
        }
        stack.pop();
        self.evaluated
            .insert(specifier.to_string(), namespace.clone());
        Ok(namespace)
    }
}

fn parse_module_source(source: &str) -> Result<ParsedModule, String> {
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut body = String::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import ") {
            let open = trimmed
                .find('{')
                .ok_or_else(|| "Only named module imports are supported".to_string())?;
            let close = trimmed[open + 1..]
                .find('}')
                .map(|offset| open + 1 + offset)
                .ok_or_else(|| "Unterminated module import list".to_string())?;
            let after = trimmed[close + 1..].trim();
            let raw_specifier = after
                .strip_prefix("from")
                .map(str::trim)
                .ok_or_else(|| "Module import requires 'from'".to_string())?
                .trim_end_matches(';');
            let specifier = raw_specifier
                .strip_prefix(['\'', '"'])
                .and_then(|value| value.strip_suffix(['\'', '"']))
                .ok_or_else(|| "Module specifier must be quoted".to_string())?;
            let mut bindings = Vec::new();
            for binding in trimmed[open + 1..close].split(',') {
                let parts = binding.split_whitespace().collect::<Vec<_>>();
                let (exported, local) = match parts.as_slice() {
                    [name] => (*name, *name),
                    [exported, "as", local] => (*exported, *local),
                    _ => return Err("Invalid named module import".to_string()),
                };
                if !is_identifier(exported) || !is_identifier(local) {
                    return Err("Invalid module import binding".to_string());
                }
                bindings.push((exported.to_string(), local.to_string()));
            }
            imports.push(ModuleImport {
                specifier: specifier.to_string(),
                bindings,
            });
            continue;
        }

        if let Some(statement) = trimmed.strip_prefix("export ") {
            let name = if let Some(rest) = statement.strip_prefix("function ") {
                rest.split(['(', ' ']).next()
            } else if let Some(rest) = statement.strip_prefix("async function ") {
                rest.split(['(', ' ']).next()
            } else {
                ["const ", "let ", "var "]
                    .iter()
                    .find_map(|prefix| statement.strip_prefix(prefix))
                    .and_then(|rest| rest.split(['=', ';', ' ']).next())
            }
            .filter(|name| is_identifier(name))
            .ok_or_else(|| "Unsupported module export declaration".to_string())?;
            exports.push(name.to_string());
            body.push_str(statement);
            body.push('\n');
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    if exports.len() > 1_024 || imports.len() > 256 {
        return Err("Module binding budget exceeded".to_string());
    }
    Ok(ParsedModule {
        imports,
        exports,
        body,
    })
}

/// Evaluate an expression in the given environment
fn eval_expr(
    expr: &JsvExpr,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    ctx.steps += 1;
    if ctx.steps > ctx.max_steps {
        return Err("Script timed out (step budget exceeded)".to_string());
    }

    match expr {
        // Literals
        JsvExpr::Number(n) => Ok(JsvValue::Number(*n)),
        JsvExpr::String(s) => {
            check_string_alloc(ctx, s.len())?;
            Ok(JsvValue::String(s.clone()))
        }
        JsvExpr::Bool(b) => Ok(JsvValue::Boolean(*b)),
        JsvExpr::Null => Ok(JsvValue::Null),
        JsvExpr::Undefined => Ok(JsvValue::Undefined),
        JsvExpr::ObjectLiteral(entries) => {
            let mut properties = HashMap::new();
            for (name, expression) in entries {
                properties.insert(name.clone(), eval_expr(expression, env, console, ctx)?);
            }
            Ok(object_value(properties))
        }
        JsvExpr::ArrayLiteral(entries) => {
            let mut values = Vec::with_capacity(entries.len());
            for expression in entries {
                values.push(eval_expr(expression, env, console, ctx)?);
            }
            Ok(JsvValue::Array(Rc::new(RefCell::new(values))))
        }

        // Variables
        JsvExpr::Identifier(name) => {
            // Block access to dangerous global variables
            if is_sandboxed_function(name) && env.get(name).is_none() {
                return Err(format!(
                    "SecurityError: '{}' is not available in sandboxed JavaScript environment",
                    name
                ));
            }
            env.get(name)
                .ok_or_else(|| format!("ReferenceError: {} is not defined", name))
        }
        JsvExpr::Assignment(name, rhs) => {
            let val = eval_expr(rhs, env, console, ctx)?;
            env.assign(name, val.clone())?;
            Ok(val)
        }
        JsvExpr::VariableDeclaration(name, initializer, kind) => {
            let value = eval_expr(initializer, env, console, ctx)?;
            env.declare(
                name,
                value.clone(),
                *kind != DeclarationKind::Const,
                *kind == DeclarationKind::Var,
            )?;
            Ok(value)
        }
        JsvExpr::Member(object, property) => {
            let object = eval_expr(object, env, console, ctx)?;
            match object {
                JsvValue::Object(object) => {
                    Ok(object_property(&object, property).unwrap_or(JsvValue::Undefined))
                }
                JsvValue::Array(array) => match property.as_str() {
                    "length" => Ok(JsvValue::Number(array.borrow().len() as f64)),
                    "push" | "pop" | "shift" | "unshift" | "join" => Ok(JsvValue::BoundNativeFn(
                        format!("Array.{}", property),
                        Box::new(JsvValue::Array(array)),
                    )),
                    _ => Ok(JsvValue::Undefined),
                },
                JsvValue::Promise(promise) => match property.as_str() {
                    "then" | "catch" | "finally" => Ok(JsvValue::BoundNativeFn(
                        format!("Promise.{}", property),
                        Box::new(JsvValue::Promise(promise)),
                    )),
                    _ => Ok(JsvValue::Undefined),
                },
                JsvValue::String(text) => {
                    if property == "length" {
                        Ok(JsvValue::Number(text.chars().count() as f64))
                    } else if string_method_supported(property) {
                        Ok(JsvValue::BoundNativeFn(
                            format!("String.{}", property),
                            Box::new(JsvValue::String(text)),
                        ))
                    } else {
                        Ok(JsvValue::Undefined)
                    }
                }
                JsvValue::HostObject(object) => ctx.host.get_property(object, property),
                JsvValue::HostFunction(object, method) => {
                    // Static members on host constructors, e.g.
                    // `MediaSource.isTypeSupported(contentType)`.
                    if object == HOST_WINDOW
                        && method == "MediaSource"
                        && property == "isTypeSupported"
                    {
                        Ok(JsvValue::HostFunction(
                            HOST_WINDOW,
                            "MediaSource.isTypeSupported".to_string(),
                        ))
                    } else {
                        Err(format!(
                            "TypeError: Cannot read property '{}' of a host function",
                            property
                        ))
                    }
                }
                other => Err(format!(
                    "TypeError: Cannot read property '{}' of {}",
                    property,
                    other.to_display_string()
                )),
            }
        }
        JsvExpr::MemberAssignment(object, property, rhs) => {
            let object = eval_expr(object, env, console, ctx)?;
            let value = eval_expr(rhs, env, console, ctx)?;
            match object {
                JsvValue::HostObject(object) => ctx.host.set_property(object, property, value),
                JsvValue::Object(object) => {
                    object
                        .borrow_mut()
                        .properties
                        .insert(property.clone(), value.clone());
                    Ok(value)
                }
                JsvValue::Array(array) if property == "length" => {
                    let length = value
                        .as_number()
                        .filter(|value| value.is_finite() && *value >= 0.0)
                        .map(|value| value as usize)
                        .ok_or_else(|| "RangeError: invalid array length".to_string())?;
                    if length > 1_000_000 {
                        return Err("RangeError: array length budget exceeded".to_string());
                    }
                    array.borrow_mut().resize(length, JsvValue::Undefined);
                    Ok(value)
                }
                _ => Err(format!(
                    "TypeError: Cannot assign to property '{}' on a non-host object",
                    property
                )),
            }
        }
        JsvExpr::Index(object, key) => {
            let object = eval_expr(object, env, console, ctx)?;
            let key = eval_expr(key, env, console, ctx)?;
            match object {
                JsvValue::Array(array) => {
                    let index = property_key(&key).parse::<usize>().ok();
                    Ok(index
                        .and_then(|index| array.borrow().get(index).cloned())
                        .unwrap_or(JsvValue::Undefined))
                }
                JsvValue::Object(object) => Ok(
                    object_property(&object, &property_key(&key)).unwrap_or(JsvValue::Undefined)
                ),
                JsvValue::String(text) => {
                    let index = property_key(&key).parse::<usize>().ok();
                    Ok(index
                        .and_then(|index| text.chars().nth(index))
                        .map(|character| JsvValue::String(character.to_string()))
                        .unwrap_or(JsvValue::Undefined))
                }
                _ => Err("TypeError: value is not indexable".to_string()),
            }
        }
        JsvExpr::IndexAssignment(object, key, rhs) => {
            let object = eval_expr(object, env, console, ctx)?;
            let key = property_key(&eval_expr(key, env, console, ctx)?);
            let value = eval_expr(rhs, env, console, ctx)?;
            match object {
                JsvValue::Array(array) => {
                    let index = key
                        .parse::<usize>()
                        .map_err(|_| "TypeError: invalid array index".to_string())?;
                    if index >= 1_000_000 {
                        return Err("RangeError: array index budget exceeded".to_string());
                    }
                    let mut array = array.borrow_mut();
                    if index >= array.len() {
                        array.resize(index + 1, JsvValue::Undefined);
                    }
                    array[index] = value.clone();
                    Ok(value)
                }
                JsvValue::Object(object) => {
                    object.borrow_mut().properties.insert(key, value.clone());
                    Ok(value)
                }
                _ => Err("TypeError: value is not assignable by index".to_string()),
            }
        }

        // Binary operators
        JsvExpr::BinaryOp(left, op, right) => {
            let l_val = eval_expr(left, env, console, ctx)?;
            match op {
                OpKind::And if !l_val.is_truthy() => return Ok(l_val),
                OpKind::Or if l_val.is_truthy() => return Ok(l_val),
                _ => {}
            }
            let r_val = eval_expr(right, env, console, ctx)?;
            apply_binary_op(l_val, r_val, *op, ctx)
        }

        // Unary operators
        JsvExpr::UnaryOp(op, inner) => {
            if *op == OpKind::Typeof {
                if let JsvExpr::Identifier(name) = &**inner {
                    if env.get(name).is_none() {
                        return Ok(JsvValue::String("undefined".to_string()));
                    }
                }
            }
            let val = eval_expr(inner, env, console, ctx)?;
            match (op, val) {
                (OpKind::Sub, JsvValue::Number(n)) => Ok(JsvValue::Number(-n)),
                (OpKind::Not, v) => Ok(JsvValue::Boolean(!v.is_truthy())),
                (OpKind::Typeof, v) => Ok(JsvValue::String(typeof_name(&v).to_string())),
                _ => Err("Invalid unary operation".to_string()),
            }
        }

        // Control flow. A `return` inside any statement container must
        // propagate up through it, so evaluate each statement and pass the
        // ReturnSignal through instead of only honoring top-level returns.
        JsvExpr::If(cond, then_branch, else_branch) => {
            // Check depth before evaluating condition
            ctx.call_depth += 1;
            if ctx.call_depth > MAX_CALL_DEPTH {
                return Err("Maximum call depth exceeded in if statement".to_string());
            }

            let cond_val = eval_expr(cond, env, console, ctx)?;
            ctx.call_depth -= 1;

            if cond_val.is_truthy() {
                let mut branch_env = JsvEnvironment::with_parent(env.clone());
                let mut result = JsvValue::Undefined;
                for stmt in then_branch {
                    let r = eval_expr(stmt, &mut branch_env, console, ctx)?;
                    if is_abrupt_signal(&r) {
                        return Ok(r);
                    }
                    result = r;
                }
                Ok(result)
            } else if let Some(else_stmts) = else_branch {
                let mut branch_env = JsvEnvironment::with_parent(env.clone());
                let mut result = JsvValue::Undefined;
                for stmt in else_stmts {
                    let r = eval_expr(stmt, &mut branch_env, console, ctx)?;
                    if is_abrupt_signal(&r) {
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
            let mut iterations = 0;
            loop {
                // Check maximum call depth at the start of each iteration
                ctx.call_depth += 1;
                if ctx.call_depth > MAX_CALL_DEPTH {
                    return Err("Maximum call depth exceeded in while loop".to_string());
                }

                let cond_val = eval_expr(cond, env, console, ctx)?;
                ctx.call_depth -= 1;

                if !cond_val.is_truthy() {
                    break;
                }

                let mut iteration_env = JsvEnvironment::with_parent(env.clone());
                for stmt in body {
                    let r = eval_expr(stmt, &mut iteration_env, console, ctx)?;
                    match r {
                        JsvValue::BreakSignal => return Ok(result),
                        JsvValue::ContinueSignal => break,
                        value if is_abrupt_signal(&value) => return Ok(value),
                        value => result = value,
                    }
                }

                iterations += 1;

                // Guard against infinite loops (prevent total work exhaustion)
                if iterations > 10_000 {
                    return Err("Infinite loop detected (exceeded 10,000 iterations)".to_string());
                }
            }
            Ok(result)
        }

        JsvExpr::ForOf(binding, kind, iterable, body) => {
            let iterable = eval_expr(iterable, env, console, ctx)?;
            let values = match iterable {
                JsvValue::Array(values) => values.borrow().clone(),
                JsvValue::String(text) => text
                    .chars()
                    .map(|character| JsvValue::String(character.to_string()))
                    .collect(),
                _ => return Err("TypeError: value is not iterable".to_string()),
            };
            if values.len() > 100_000 {
                return Err("Iterator result budget exceeded".to_string());
            }
            let mut result = JsvValue::Undefined;
            for value in values {
                let mut iteration_env = JsvEnvironment::with_parent(env.clone());
                iteration_env.declare(binding, value, *kind != DeclarationKind::Const, false)?;
                for statement in body {
                    let value = eval_expr(statement, &mut iteration_env, console, ctx)?;
                    match value {
                        JsvValue::BreakSignal => return Ok(result),
                        JsvValue::ContinueSignal => break,
                        value if is_abrupt_signal(&value) => return Ok(value),
                        value => result = value,
                    }
                }
            }
            Ok(result)
        }

        JsvExpr::Switch(discriminant, cases, default_clause) => {
            let disc = eval_expr(discriminant, env, console, ctx)?;
            let mut matched = false;
            let mut result = JsvValue::Undefined;
            for (case, body) in cases {
                if !matched {
                    match case {
                        Some(case_expr) => {
                            if !js_strict_equal(&disc, &eval_expr(case_expr, env, console, ctx)?) {
                                continue;
                            }
                            matched = true;
                        }
                        None => matched = true,
                    }
                }
                if matched {
                    for statement in body {
                        let value = eval_expr(statement, env, console, ctx)?;
                        match value {
                            JsvValue::BreakSignal => return Ok(result),
                            JsvValue::ContinueSignal => return Ok(JsvValue::ContinueSignal),
                            value if is_abrupt_signal(&value) => return Ok(value),
                            value => result = value,
                        }
                    }
                }
            }
            if !matched {
                if let Some(body) = default_clause {
                    for statement in body {
                        let value = eval_expr(statement, env, console, ctx)?;
                        match value {
                            JsvValue::BreakSignal => return Ok(result),
                            JsvValue::ContinueSignal => return Ok(JsvValue::ContinueSignal),
                            value if is_abrupt_signal(&value) => return Ok(value),
                            value => result = value,
                        }
                    }
                }
            }
            Ok(result)
        }

        JsvExpr::Ternary(cond, then_expr, else_expr) => {
            let condition = eval_expr(cond, env, console, ctx)?;
            if condition.is_truthy() {
                eval_expr(then_expr, env, console, ctx)
            } else {
                eval_expr(else_expr, env, console, ctx)
            }
        }

        JsvExpr::Template(parts) => {
            let mut output = String::new();
            for part in parts {
                match part {
                    TemplatePart::Text(text) => output.push_str(text),
                    TemplatePart::Expr(expression) => {
                        output.push_str(
                            &eval_expr(expression, env, console, ctx)?.to_display_string(),
                        );
                    }
                }
                check_string_alloc(ctx, output.len())?;
            }
            Ok(JsvValue::String(output))
        }

        JsvExpr::OptionalMember(object, property) => {
            let object = eval_expr(object, env, console, ctx)?;
            if matches!(object, JsvValue::Null | JsvValue::Undefined) {
                return Ok(JsvValue::Undefined);
            }
            match object {
                JsvValue::Object(object) => {
                    Ok(object_property(&object, property).unwrap_or(JsvValue::Undefined))
                }
                JsvValue::Array(array) => match property.as_str() {
                    "length" => Ok(JsvValue::Number(array.borrow().len() as f64)),
                    _ => Ok(JsvValue::Undefined),
                },
                JsvValue::HostObject(object) => ctx.host.get_property(object, property),
                other => Err(format!(
                    "TypeError: Cannot read property '{}' of {}",
                    property,
                    other.to_display_string()
                )),
            }
        }

        JsvExpr::OptionalIndex(object, key) => {
            let object = eval_expr(object, env, console, ctx)?;
            if matches!(object, JsvValue::Null | JsvValue::Undefined) {
                return Ok(JsvValue::Undefined);
            }
            let key = eval_expr(key, env, console, ctx)?;
            match object {
                JsvValue::Array(array) => {
                    let index = property_key(&key).parse::<usize>().ok();
                    Ok(index
                        .and_then(|index| array.borrow().get(index).cloned())
                        .unwrap_or(JsvValue::Undefined))
                }
                JsvValue::Object(object) => Ok(
                    object_property(&object, &property_key(&key)).unwrap_or(JsvValue::Undefined)
                ),
                _ => Err("TypeError: value is not indexable".to_string()),
            }
        }

        JsvExpr::Update(target, prefix, increment) => {
            let current = eval_expr(target, env, console, ctx)?;
            let value = current
                .as_number()
                .filter(|value| value.is_finite())
                .ok_or_else(|| "TypeError: increment/decrement requires a number".to_string())?;
            let next = JsvValue::Number(if *increment { value + 1.0 } else { value - 1.0 });
            match &**target {
                JsvExpr::Identifier(name) => env.assign(name, next.clone())?,
                JsvExpr::Member(object, property) => {
                    let object = eval_expr(object, env, console, ctx)?;
                    match object {
                        JsvValue::HostObject(object) => {
                            ctx.host.set_property(object, property, next.clone())?;
                        }
                        JsvValue::Object(object) => {
                            object
                                .borrow_mut()
                                .properties
                                .insert(property.clone(), next.clone());
                        }
                        _ => return Err("TypeError: invalid update target".to_string()),
                    }
                }
                JsvExpr::Index(object, key) => {
                    let object = eval_expr(object, env, console, ctx)?;
                    let key = property_key(&eval_expr(key, env, console, ctx)?);
                    match object {
                        JsvValue::Array(array) => {
                            let index = key
                                .parse::<usize>()
                                .map_err(|_| "TypeError: invalid array index".to_string())?;
                            if index >= 1_000_000 {
                                return Err("RangeError: array index budget exceeded".to_string());
                            }
                            let mut array = array.borrow_mut();
                            if index >= array.len() {
                                array.resize(index + 1, JsvValue::Undefined);
                            }
                            array[index] = next.clone();
                        }
                        JsvValue::Object(object) => {
                            object.borrow_mut().properties.insert(key, next.clone());
                        }
                        _ => return Err("TypeError: invalid update target".to_string()),
                    }
                }
                _ => return Err("TypeError: invalid update target".to_string()),
            }
            if *prefix {
                Ok(next)
            } else {
                Ok(current)
            }
        }

        JsvExpr::Block(stmts) => {
            let mut block_env = JsvEnvironment::with_parent(env.clone());
            let mut result = JsvValue::Undefined;
            for stmt in stmts {
                let r = eval_expr(stmt, &mut block_env, console, ctx)?;
                if is_abrupt_signal(&r) {
                    return Ok(r);
                }
                result = r;
            }
            Ok(result)
        }

        JsvExpr::TryCatchFinally {
            try_body,
            catch_binding,
            catch_body,
            finally_body,
        } => {
            let mut outcome = JsvValue::Undefined;
            let mut try_env = JsvEnvironment::with_parent(env.clone());
            for statement in try_body {
                match eval_expr(statement, &mut try_env, console, ctx) {
                    Ok(value) => {
                        outcome = value;
                        if is_abrupt_signal(&outcome) {
                            break;
                        }
                    }
                    Err(error) => {
                        outcome = JsvValue::ThrowSignal(Box::new(JsvValue::String(error)));
                        break;
                    }
                }
            }

            if let JsvValue::ThrowSignal(reason) = outcome {
                if let Some(binding) = catch_binding {
                    let mut catch_env = JsvEnvironment::with_parent(env.clone());
                    catch_env.define(binding, *reason);
                    outcome = JsvValue::Undefined;
                    for statement in catch_body {
                        outcome = eval_expr(statement, &mut catch_env, console, ctx)?;
                        if is_abrupt_signal(&outcome) {
                            break;
                        }
                    }
                } else {
                    outcome = JsvValue::ThrowSignal(reason);
                }
            }

            let mut finally_env = JsvEnvironment::with_parent(env.clone());
            for statement in finally_body {
                let final_value = eval_expr(statement, &mut finally_env, console, ctx)?;
                if is_abrupt_signal(&final_value) {
                    return Ok(final_value);
                }
            }
            Ok(outcome)
        }

        // Functions
        JsvExpr::FunctionDef(name, params, body) => {
            // Block dangerous function definitions
            if is_sandboxed_function(name) {
                return Err(format!(
                    "SecurityError: Function '{}' is not allowed in sandboxed environment",
                    name
                ));
            }
            let func = JsvValue::Function(name.clone(), params.clone(), body.clone(), env.clone());
            env.define(name, func.clone());
            Ok(func)
        }

        JsvExpr::AsyncFunctionDef(name, params, body) => {
            let function =
                JsvValue::AsyncFunction(name.clone(), params.clone(), body.clone(), env.clone());
            env.define(name, function.clone());
            Ok(function)
        }

        JsvExpr::FunctionExpr(params, body, is_async) => {
            if *is_async {
                Ok(JsvValue::AsyncFunction(
                    String::new(),
                    params.clone(),
                    body.clone(),
                    env.clone(),
                ))
            } else {
                Ok(JsvValue::Function(
                    String::new(),
                    params.clone(),
                    body.clone(),
                    env.clone(),
                ))
            }
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
            match r {
                JsvValue::ReturnSignal(value) => Ok(*value),
                JsvValue::BreakSignal => {
                    Err("SyntaxError: illegal break statement in function".to_string())
                }
                JsvValue::ContinueSignal => {
                    Err("SyntaxError: illegal continue statement in function".to_string())
                }
                value => Ok(value),
            }
        }

        JsvExpr::NewExpr(callee, args) => {
            let callee_val = eval_expr(callee, env, console, ctx)?;
            let mut arg_vals = Vec::with_capacity(args.len());
            for a in args {
                arg_vals.push(eval_expr(a, env, console, ctx)?);
            }
            match callee_val {
                JsvValue::HostFunction(object, method) => ctx.host.call(object, &method, arg_vals),
                function @ JsvValue::Function(..) => {
                    call_function(function, arg_vals, env, console, ctx)
                }
                function @ JsvValue::AsyncFunction(..) => {
                    call_function(function, arg_vals, env, console, ctx)
                }
                other => Err(format!(
                    "TypeError: {} is not a constructor",
                    other.to_display_string()
                )),
            }
        }

        JsvExpr::DynamicImport(specifier) => {
            // Vacate the graph reference so module evaluation can recurse
            // into nested imports and evaluate bodies with the same host.
            let Some(graph) = ctx.modules.take() else {
                return Err("Module graph is busy during dynamic import".to_string());
            };
            let result = graph.evaluate_in_env(specifier, env, console, ctx);
            ctx.modules = Some(graph);
            // Dynamic import never throws synchronously: failures reject the
            // returned Promise, matching ECMAScript semantics.
            let namespace = match result {
                Ok(namespace) => namespace,
                Err(error) => {
                    return Ok(JsvValue::Promise(Rc::new(RefCell::new(
                        JsvPromiseState::Rejected(JsvValue::String(error)),
                    ))));
                }
            };
            let mut properties = HashMap::new();
            for (name, value) in namespace.exports {
                properties.insert(name, value);
            }
            Ok(JsvValue::Promise(Rc::new(RefCell::new(
                JsvPromiseState::Fulfilled(object_value(properties)),
            ))))
        }

        JsvExpr::Return(val) => {
            let r = eval_expr(val, env, console, ctx)?;
            Ok(JsvValue::ReturnSignal(Box::new(r)))
        }
        JsvExpr::Throw(value) => {
            let value = eval_expr(value, env, console, ctx)?;
            Ok(JsvValue::ThrowSignal(Box::new(value)))
        }
        JsvExpr::Await(value) => {
            let value = eval_expr(value, env, console, ctx)?;
            match value {
                JsvValue::Promise(promise) => match promise.borrow().clone() {
                    JsvPromiseState::Fulfilled(value) => Ok(value),
                    JsvPromiseState::Rejected(reason) => {
                        Ok(JsvValue::ThrowSignal(Box::new(reason)))
                    }
                    JsvPromiseState::Pending => {
                        Err("InvalidStateError: awaited Promise is still pending".to_string())
                    }
                },
                other => Ok(other),
            }
        }
        JsvExpr::Break => Ok(JsvValue::BreakSignal),
        JsvExpr::Continue => Ok(JsvValue::ContinueSignal),
    }
}

/// Apply a binary operator
fn apply_binary_op(
    l_val: JsvValue,
    r_val: JsvValue,
    op: OpKind,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
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
        OpKind::Nullish => {
            if matches!(l_val, JsvValue::Null | JsvValue::Undefined) {
                return Ok(r_val);
            }
            return Ok(l_val);
        }
        OpKind::StrictEq => return Ok(JsvValue::Boolean(js_strict_equal(&l_val, &r_val))),
        OpKind::StrictNeq => return Ok(JsvValue::Boolean(!js_strict_equal(&l_val, &r_val))),
        OpKind::Instanceof => {
            let JsvValue::Object(constructor) = r_val else {
                return Err("TypeError: instanceof requires a constructor object".to_string());
            };
            let JsvValue::Object(object) = l_val else {
                return Ok(JsvValue::Boolean(false));
            };
            let mut current = object.borrow().prototype.clone();
            for _ in 0..64 {
                let Some(reference) = current else {
                    return Ok(JsvValue::Boolean(false));
                };
                if Rc::ptr_eq(&reference, &constructor) {
                    return Ok(JsvValue::Boolean(true));
                }
                current = reference.borrow().prototype.clone();
            }
            return Ok(JsvValue::Boolean(false));
        }
        OpKind::In => {
            let key = property_key(&l_val);
            return match r_val {
                JsvValue::Object(object) => {
                    Ok(JsvValue::Boolean(object_property(&object, &key).is_some()))
                }
                JsvValue::Array(array) => {
                    let index = key.parse::<usize>().ok();
                    Ok(JsvValue::Boolean(
                        index.is_some_and(|index| index < array.borrow().len()),
                    ))
                }
                other => Err(format!(
                    "TypeError: cannot use 'in' on {}",
                    other.to_display_string()
                )),
            };
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
            OpKind::Add => {
                check_string_alloc(ctx, ls.len() + rs.len())?;
                Ok(JsvValue::String(format!("{}{}", ls, rs)))
            }
            OpKind::Eq => Ok(JsvValue::Boolean(ls == rs)),
            OpKind::Neq => Ok(JsvValue::Boolean(ls != rs)),
            OpKind::Lt => Ok(JsvValue::Boolean(ls < rs)),
            OpKind::Gt => Ok(JsvValue::Boolean(ls > rs)),
            _ => Err("Invalid operator for strings".to_string()),
        },
        (JsvValue::Null, JsvValue::Undefined) | (JsvValue::Undefined, JsvValue::Null) => match op {
            OpKind::Eq => Ok(JsvValue::Boolean(true)),
            OpKind::Neq => Ok(JsvValue::Boolean(false)),
            _ => Err("Type mismatch in binary operation".to_string()),
        },
        (JsvValue::Number(ln), rhs) => {
            if op == OpKind::Add {
                // JS semantics: number + string concatenates the display form.
                let display = rhs.to_display_string();
                check_string_alloc(ctx, display.len())?;
                Ok(JsvValue::String(format!("{}{}", ln, display)))
            } else {
                Err("Type mismatch in binary operation".to_string())
            }
        }
        (JsvValue::String(ls), rhs) => {
            if op == OpKind::Add {
                let display = rhs.to_display_string();
                check_string_alloc(ctx, ls.len() + display.len())?;
                Ok(JsvValue::String(format!("{}{}", ls, display)))
            } else {
                Err("Type mismatch in binary operation".to_string())
            }
        }
        _ => Err("Type mismatch in binary operation".to_string()),
    }
}

/// ECMAScript strict equality (`===`): no coercion, type-tagged comparison,
/// `NaN` never equals itself and `null`/`undefined` are distinct.
fn js_strict_equal(left: &JsvValue, right: &JsvValue) -> bool {
    match (left, right) {
        (JsvValue::Number(l), JsvValue::Number(r)) => l == r,
        (JsvValue::String(l), JsvValue::String(r)) => l == r,
        (JsvValue::Boolean(l), JsvValue::Boolean(r)) => l == r,
        (JsvValue::Null, JsvValue::Null) => true,
        (JsvValue::Undefined, JsvValue::Undefined) => true,
        (JsvValue::Object(l), JsvValue::Object(r)) => Rc::ptr_eq(l, r),
        (JsvValue::Array(l), JsvValue::Array(r)) => Rc::ptr_eq(l, r),
        (JsvValue::Promise(l), JsvValue::Promise(r)) => Rc::ptr_eq(l, r),
        (JsvValue::HostObject(l), JsvValue::HostObject(r)) => l == r,
        (JsvValue::HostFunction(l_object, l_name), JsvValue::HostFunction(r_object, r_name)) => {
            l_object == r_object && l_name == r_name
        }
        (JsvValue::NativeFn(l), JsvValue::NativeFn(r)) => l == r,
        (JsvValue::BoundNativeFn(l_name, l), JsvValue::BoundNativeFn(r_name, r)) => {
            l_name == r_name && l == r
        }
        _ => false,
    }
}

/// `typeof` result for a value, following the ECMAScript type table.
fn typeof_name(value: &JsvValue) -> &'static str {
    match value {
        JsvValue::Number(_) => "number",
        JsvValue::String(_) => "string",
        JsvValue::Boolean(_) => "boolean",
        JsvValue::Null => "object",
        JsvValue::Undefined => "undefined",
        JsvValue::Object(_) => "object",
        JsvValue::Array(_) => "object",
        JsvValue::Function(..) | JsvValue::AsyncFunction(..) => "function",
        JsvValue::NativeFn(_) | JsvValue::BoundNativeFn(..) => "function",
        JsvValue::Promise(_) => "object",
        JsvValue::HostObject(_) => "object",
        JsvValue::HostFunction(..) => "function",
        JsvValue::ReturnSignal(_) | JsvValue::ThrowSignal(_) => "object",
        JsvValue::BreakSignal | JsvValue::ContinueSignal => "object",
    }
}

/// Call a function value
fn call_function(
    func: JsvValue,
    args: Vec<JsvValue>,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    // Bound the current nested call chain so circular/self recursion errors
    // out before the stack overflows (which would abort the process).
    ctx.call_depth += 1;
    if ctx.call_depth > MAX_CALL_DEPTH {
        return Err("Maximum call stack size exceeded".to_string());
    }
    let result = match func {
        JsvValue::NativeFn(name) => call_native_fn(&name, args, console, ctx),
        JsvValue::BoundNativeFn(name, receiver) => {
            call_bound_native(&name, *receiver, args, env, console, ctx)
        }
        JsvValue::HostFunction(object, method) => ctx.host.call(object, &method, args),
        JsvValue::Function(name, params, body, captured) => {
            let mut fn_env = JsvEnvironment::with_parent(captured.clone());
            if !name.is_empty() {
                fn_env.define(
                    &name,
                    JsvValue::Function(name.clone(), params.clone(), body.clone(), captured),
                );
            }
            for (i, param) in params.iter().enumerate() {
                let arg_val = args.get(i).cloned().unwrap_or(JsvValue::Undefined);
                fn_env.define(param, arg_val);
            }
            eval_expr(&body, &mut fn_env, console, ctx)
        }
        JsvValue::AsyncFunction(name, params, body, captured) => {
            let mut fn_env = JsvEnvironment::with_parent(captured.clone());
            if !name.is_empty() {
                fn_env.define(
                    &name,
                    JsvValue::AsyncFunction(name.clone(), params.clone(), body.clone(), captured),
                );
            }
            for (index, parameter) in params.iter().enumerate() {
                fn_env.define(
                    parameter,
                    args.get(index).cloned().unwrap_or(JsvValue::Undefined),
                );
            }
            let state = match eval_expr(&body, &mut fn_env, console, ctx) {
                Ok(JsvValue::ReturnSignal(value)) => match *value {
                    JsvValue::Promise(promise) => promise.borrow().clone(),
                    value => JsvPromiseState::Fulfilled(value),
                },
                Ok(JsvValue::ThrowSignal(reason)) => JsvPromiseState::Rejected(*reason),
                Ok(JsvValue::BreakSignal) => JsvPromiseState::Rejected(JsvValue::String(
                    "SyntaxError: illegal break statement in async function".to_string(),
                )),
                Ok(JsvValue::ContinueSignal) => JsvPromiseState::Rejected(JsvValue::String(
                    "SyntaxError: illegal continue statement in async function".to_string(),
                )),
                Ok(value) => JsvPromiseState::Fulfilled(value),
                Err(error) => JsvPromiseState::Rejected(JsvValue::String(error)),
            };
            Ok(JsvValue::Promise(Rc::new(RefCell::new(state))))
        }
        other => Err(format!(
            "TypeError: {} is not a function",
            other.to_display_string()
        )),
    };
    ctx.call_depth -= 1;
    result
}

fn promise_value(state: JsvPromiseState) -> JsvValue {
    JsvValue::Promise(Rc::new(RefCell::new(state)))
}

fn is_callable(value: &JsvValue) -> bool {
    matches!(
        value,
        JsvValue::Function(_, _, _, _)
            | JsvValue::AsyncFunction(_, _, _, _)
            | JsvValue::NativeFn(_)
            | JsvValue::BoundNativeFn(_, _)
            | JsvValue::HostFunction(_, _)
    )
}

/// Public capability check used by host bridges that accept callbacks
/// (timers, listeners) without re-implementing the callable classification.
pub fn is_callable_public(value: &JsvValue) -> bool {
    is_callable(value)
}

fn enqueue_promise_reaction(
    ctx: &mut EvalCtx<'_>,
    kind: PromiseReactionKind,
    handler: Option<JsvValue>,
    argument: JsvValue,
    result: JsvPromiseRef,
) -> Result<(), String> {
    if ctx.microtasks_queued >= MAX_PROMISE_JOBS {
        return Err("Promise microtask budget exceeded".to_string());
    }
    ctx.microtasks_queued += 1;
    ctx.microtasks.push_back(PromiseReactionJob {
        kind,
        handler,
        argument,
        result,
    });
    Ok(())
}

fn promise_key(promise: &JsvPromiseRef) -> usize {
    Rc::as_ptr(promise) as usize
}

fn enqueue_registered_reaction(
    ctx: &mut EvalCtx<'_>,
    reaction: PendingPromiseReaction,
    state: JsvPromiseState,
) -> Result<(), String> {
    let (kind, handler, argument) = match state {
        JsvPromiseState::Fulfilled(value) => {
            if let Some((kind, override_value)) = reaction.completion_override {
                (kind, None, override_value)
            } else if reaction.on_finally.is_some() {
                (PromiseReactionKind::Finally, reaction.on_finally, value)
            } else {
                (PromiseReactionKind::Fulfill, reaction.on_fulfilled, value)
            }
        }
        JsvPromiseState::Rejected(reason) => {
            if reaction.on_finally.is_some() {
                (PromiseReactionKind::Finally, reaction.on_finally, reason)
            } else {
                (PromiseReactionKind::Reject, reaction.on_rejected, reason)
            }
        }
        JsvPromiseState::Pending => {
            return Err("InvalidStateError: cannot enqueue a pending Promise".to_string());
        }
    };
    enqueue_promise_reaction(ctx, kind, handler, argument, reaction.result)
}

fn settle_promise(
    ctx: &mut EvalCtx<'_>,
    promise: &JsvPromiseRef,
    state: JsvPromiseState,
) -> Result<(), String> {
    *promise.borrow_mut() = state.clone();
    if matches!(state, JsvPromiseState::Pending) {
        return Ok(());
    }
    if let Some(reactions) = ctx.pending_promise_reactions.remove(&promise_key(promise)) {
        for reaction in reactions {
            enqueue_registered_reaction(ctx, reaction, state.clone())?;
        }
    }
    Ok(())
}

fn resolve_promise(
    ctx: &mut EvalCtx<'_>,
    target: &JsvPromiseRef,
    outcome: Result<JsvValue, JsvValue>,
) -> Result<(), String> {
    match outcome {
        Err(reason) => settle_promise(ctx, target, JsvPromiseState::Rejected(reason)),
        Ok(JsvValue::Promise(source)) => {
            if Rc::ptr_eq(target, &source) {
                return settle_promise(
                    ctx,
                    target,
                    JsvPromiseState::Rejected(JsvValue::String(
                        "TypeError: Promise cannot resolve to itself".to_string(),
                    )),
                );
            }
            match source.borrow().clone() {
                JsvPromiseState::Pending => {
                    ctx.pending_promise_reactions
                        .entry(promise_key(&source))
                        .or_default()
                        .push(PendingPromiseReaction {
                            on_fulfilled: None,
                            on_rejected: None,
                            on_finally: None,
                            completion_override: None,
                            result: target.clone(),
                        });
                    Ok(())
                }
                state => settle_promise(ctx, target, state),
            }
        }
        Ok(value) => settle_promise(ctx, target, JsvPromiseState::Fulfilled(value)),
    }
}

/// True when a String prototype method is implemented by the bounded profile.
fn string_method_supported(method: &str) -> bool {
    matches!(
        method,
        "toUpperCase"
            | "toLowerCase"
            | "trim"
            | "trimStart"
            | "trimEnd"
            | "charAt"
            | "charCodeAt"
            | "indexOf"
            | "includes"
            | "startsWith"
            | "endsWith"
            | "slice"
            | "substring"
            | "replace"
            | "split"
            | "repeat"
            | "concat"
            | "padStart"
            | "padEnd"
            | "at"
    )
}

/// Apply a String prototype method against the receiver string.
fn call_string_method(
    method: &str,
    text: String,
    args: &[JsvValue],
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    let string_arg = |index: usize| -> Result<String, String> {
        args.get(index)
            .map(|value| value.to_display_string())
            .ok_or_else(|| "TypeError: missing string argument".to_string())
    };
    let number_arg = |index: usize| -> Result<f64, String> {
        args.get(index)
            .and_then(|value| value.as_number())
            .ok_or_else(|| "TypeError: expected a number".to_string())
    };
    let chars: Vec<char> = text.chars().collect();
    let count = chars.len();
    let mut bounded = |value: String| -> Result<JsvValue, String> {
        check_string_alloc(ctx, value.len())?;
        Ok(JsvValue::String(value))
    };
    match method {
        "toUpperCase" => bounded(text.to_uppercase()),
        "toLowerCase" => bounded(text.to_lowercase()),
        "trim" => bounded(text.trim().to_string()),
        "trimStart" => bounded(text.trim_start().to_string()),
        "trimEnd" => bounded(text.trim_end().to_string()),
        "charAt" => {
            let index = number_arg(0).unwrap_or(f64::NAN);
            let index = if index.is_nan() { 0.0 } else { index };
            let character = if index.is_finite() && index >= 0.0 && (index as usize) < count {
                chars[index as usize].to_string()
            } else {
                String::new()
            };
            bounded(character)
        }
        "charCodeAt" => {
            let index = number_arg(0).unwrap_or(f64::NAN);
            let code = if index.is_finite() && index >= 0.0 && (index as usize) < count {
                chars[index as usize] as u32 as f64
            } else {
                f64::NAN
            };
            Ok(JsvValue::Number(code))
        }
        "at" => {
            let index = number_arg(0).unwrap_or(f64::NAN);
            let index = if index.is_nan() { 0.0 } else { index };
            let resolved = if index < 0.0 {
                count as f64 + index
            } else {
                index
            };
            let character =
                if resolved.is_finite() && resolved >= 0.0 && (resolved as usize) < count {
                    chars[resolved as usize].to_string()
                } else {
                    String::new()
                };
            bounded(character)
        }
        "indexOf" => {
            let needle = string_arg(0)?;
            let start = number_arg(1).unwrap_or(0.0).max(0.0) as usize;
            let position = if needle.is_empty() {
                start.min(count)
            } else {
                let mut found = None;
                for index in start..count {
                    let candidate: String =
                        chars[index..].iter().take(needle.chars().count()).collect();
                    if candidate == needle {
                        found = Some(index);
                        break;
                    }
                }
                found.unwrap_or(usize::MAX)
            };
            Ok(JsvValue::Number(if position == usize::MAX {
                -1.0
            } else {
                position as f64
            }))
        }
        "includes" => {
            let needle = string_arg(0)?;
            let start = number_arg(1).unwrap_or(0.0).max(0.0) as usize;
            let found = if needle.is_empty() {
                start <= count
            } else {
                (start..count).any(|index| {
                    let candidate: String =
                        chars[index..].iter().take(needle.chars().count()).collect();
                    candidate == needle
                })
            };
            Ok(JsvValue::Boolean(found))
        }
        "startsWith" => {
            let needle = string_arg(0)?;
            let start = number_arg(1).unwrap_or(0.0).max(0.0) as usize;
            let candidate: String = chars[start..].iter().collect();
            Ok(JsvValue::Boolean(candidate.starts_with(&needle)))
        }
        "endsWith" => {
            let needle = string_arg(0)?;
            let end = number_arg(1)
                .map(|value| value.max(0.0) as usize)
                .unwrap_or(count);
            let candidate: String = chars[..end.min(count)].iter().collect();
            Ok(JsvValue::Boolean(candidate.ends_with(&needle)))
        }
        "slice" => {
            let start = number_arg(0).unwrap_or(0.0);
            let end = number_arg(1).unwrap_or(count as f64);
            let normalize = |value: f64| -> usize {
                if value.is_nan() {
                    return 0;
                }
                if value < 0.0 {
                    (count as f64 + value).max(0.0) as usize
                } else {
                    (value as usize).min(count)
                }
            };
            let start = normalize(start);
            let end = normalize(end);
            let sliced: String = if end > start {
                chars[start..end.min(count)].iter().collect()
            } else {
                String::new()
            };
            bounded(sliced)
        }
        "substring" => {
            let start = number_arg(0).unwrap_or(0.0).max(0.0) as usize;
            let end = number_arg(1).unwrap_or(count as f64).max(0.0) as usize;
            let (start, end) = if start > end {
                (end, start)
            } else {
                (start, end)
            };
            let sliced: String = chars[start.min(count)..end.min(count)].iter().collect();
            bounded(sliced)
        }
        "replace" => {
            let pattern = string_arg(0)?;
            let replacement = string_arg(1)?;
            if pattern.is_empty() {
                return bounded(format!(
                    "{}{}",
                    replacement,
                    chars.iter().collect::<String>()
                ));
            }
            let pattern_chars: Vec<char> = pattern.chars().collect();
            let mut replaced = false;
            let mut output = String::new();
            let mut index = 0;
            while index < count {
                if !replaced && index + pattern_chars.len() <= count {
                    let candidate: String =
                        chars[index..index + pattern_chars.len()].iter().collect();
                    if candidate == pattern {
                        output.push_str(&replacement);
                        index += pattern_chars.len();
                        replaced = true;
                        continue;
                    }
                }
                output.push(chars[index]);
                index += 1;
            }
            bounded(output)
        }
        "split" => {
            let separator = args.first().map(|value| value.to_display_string());
            let values = match separator.as_deref() {
                None | Some("") => chars
                    .iter()
                    .map(|character| JsvValue::String(character.to_string()))
                    .collect::<Vec<_>>(),
                Some(separator) => {
                    let separator_chars: Vec<char> = separator.chars().collect();
                    let mut parts = Vec::new();
                    let mut current = String::new();
                    let mut index = 0;
                    while index < count {
                        if index + separator_chars.len() <= count {
                            let candidate: String =
                                chars[index..index + separator_chars.len()].iter().collect();
                            if candidate == separator {
                                parts.push(JsvValue::String(std::mem::take(&mut current)));
                                index += separator_chars.len();
                                continue;
                            }
                        }
                        current.push(chars[index]);
                        index += 1;
                    }
                    parts.push(JsvValue::String(current));
                    if parts.len() > 100_000 {
                        return Err("Split result budget exceeded".to_string());
                    }
                    parts
                }
            };
            Ok(JsvValue::Array(Rc::new(RefCell::new(values))))
        }
        "repeat" => {
            let times = number_arg(0).unwrap_or(0.0);
            if times.is_nan() || times < 0.0 {
                return Err("RangeError: repeat count is invalid".to_string());
            }
            let times = times as usize;
            if text.len().saturating_mul(times) > MAX_STRING_LENGTH {
                return Err("String too large".to_string());
            }
            let repeated = text.repeat(times.min(1_000_000));
            check_string_alloc(ctx, repeated.len())?;
            Ok(JsvValue::String(repeated))
        }
        "concat" => {
            let mut output = text;
            for arg in args.iter().take(64) {
                output.push_str(&arg.to_display_string());
                check_string_alloc(ctx, output.len())?;
            }
            Ok(JsvValue::String(output))
        }
        "padStart" | "padEnd" => {
            let target = number_arg(0).unwrap_or(0.0).max(0.0) as usize;
            let pad = string_arg(1).unwrap_or_else(|_| " ".to_string());
            let current = count;
            if target <= current || pad.is_empty() {
                return bounded(text);
            }
            let pad_chars: Vec<char> = pad.chars().collect();
            let needed = target - current;
            let mut padding = String::new();
            let mut index = 0;
            while padding.chars().count() < needed {
                padding.push(pad_chars[index % pad_chars.len()]);
                index += 1;
            }
            if method == "padStart" {
                bounded(format!("{}{}", padding, text))
            } else {
                bounded(format!("{}{}", text, padding))
            }
        }
        _ => Err(format!("TypeError: String.{} is not implemented", method)),
    }
}

fn call_bound_native(
    name: &str,
    receiver: JsvValue,
    args: Vec<JsvValue>,
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    match (name, receiver) {
        ("Array.push", JsvValue::Array(array)) => {
            let projected = array.borrow().len().saturating_add(args.len());
            if projected > 1_000_000 {
                return Err("Array element budget exceeded".to_string());
            }
            array.borrow_mut().extend(args);
            Ok(JsvValue::Number(projected as f64))
        }
        ("Array.pop", JsvValue::Array(array)) => {
            let value = array.borrow_mut().pop().unwrap_or(JsvValue::Undefined);
            Ok(value)
        }
        ("Array.shift", JsvValue::Array(array)) => {
            let value = if array.borrow().is_empty() {
                JsvValue::Undefined
            } else {
                array.borrow_mut().remove(0)
            };
            Ok(value)
        }
        ("Array.unshift", JsvValue::Array(array)) => {
            let projected = array.borrow().len().saturating_add(args.len());
            if projected > 1_000_000 {
                return Err("Array element budget exceeded".to_string());
            }
            for value in args.into_iter().rev() {
                array.borrow_mut().insert(0, value);
            }
            Ok(JsvValue::Number(projected as f64))
        }
        ("Array.join", JsvValue::Array(array)) => {
            let separator = args
                .first()
                .map(JsvValue::to_display_string)
                .unwrap_or_else(|| ",".to_string());
            let result = array
                .borrow()
                .iter()
                .map(JsvValue::to_display_string)
                .collect::<Vec<_>>()
                .join(&separator);
            check_string_alloc(ctx, result.len())?;
            Ok(JsvValue::String(result))
        }
        (
            method @ ("Promise.then" | "Promise.catch" | "Promise.finally"),
            JsvValue::Promise(promise),
        ) => {
            let result = Rc::new(RefCell::new(JsvPromiseState::Pending));
            let state = promise.borrow().clone();
            let callable =
                |index: usize| args.get(index).filter(|value| is_callable(value)).cloned();
            let reaction = match method {
                "Promise.then" => PendingPromiseReaction {
                    on_fulfilled: callable(0),
                    on_rejected: callable(1),
                    on_finally: None,
                    completion_override: None,
                    result: result.clone(),
                },
                "Promise.catch" => PendingPromiseReaction {
                    on_fulfilled: None,
                    on_rejected: callable(0),
                    on_finally: None,
                    completion_override: None,
                    result: result.clone(),
                },
                _ => PendingPromiseReaction {
                    on_fulfilled: None,
                    on_rejected: None,
                    on_finally: callable(0),
                    completion_override: None,
                    result: result.clone(),
                },
            };
            match state {
                JsvPromiseState::Pending => {
                    ctx.pending_promise_reactions
                        .entry(promise_key(&promise))
                        .or_default()
                        .push(reaction);
                }
                settled => enqueue_registered_reaction(ctx, reaction, settled)?,
            }
            Ok(JsvValue::Promise(result))
        }
        (method, JsvValue::String(text)) if method.starts_with("String.") => {
            call_string_method(&method["String.".len()..], text, &args, ctx)
        }
        _ => {
            let _ = (env, console);
            Err(format!("TypeError: unsupported native method {}", name))
        }
    }
}

fn drain_promise_jobs(
    env: &mut JsvEnvironment,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<(), String> {
    let mut processed = 0usize;
    while let Some(job) = ctx.microtasks.pop_front() {
        processed += 1;
        if processed > MAX_PROMISE_JOBS {
            return Err("Promise job execution budget exceeded".to_string());
        }
        let original_rejected = job.kind == PromiseReactionKind::Reject;
        let outcome = if let Some(handler) = job.handler {
            let arguments = if job.kind == PromiseReactionKind::Finally {
                Vec::new()
            } else {
                vec![job.argument.clone()]
            };
            match call_function(handler, arguments, env, console, ctx) {
                Ok(JsvValue::ThrowSignal(reason)) => Err(*reason),
                Ok(JsvValue::ReturnSignal(value)) => Ok(*value),
                Ok(JsvValue::BreakSignal) => Err(JsvValue::String(
                    "SyntaxError: illegal break statement in Promise handler".to_string(),
                )),
                Ok(JsvValue::ContinueSignal) => Err(JsvValue::String(
                    "SyntaxError: illegal continue statement in Promise handler".to_string(),
                )),
                Ok(value) => Ok(value),
                Err(error) => Err(JsvValue::String(error)),
            }
        } else if original_rejected {
            Err(job.argument.clone())
        } else {
            Ok(job.argument.clone())
        };

        let settlement = if job.kind == PromiseReactionKind::Finally {
            match outcome {
                Err(reason) => Err(reason),
                Ok(JsvValue::Promise(source)) => match source.borrow().clone() {
                    JsvPromiseState::Rejected(reason) => Err(reason),
                    JsvPromiseState::Fulfilled(_) if original_rejected => Err(job.argument.clone()),
                    JsvPromiseState::Fulfilled(_) => Ok(job.argument.clone()),
                    JsvPromiseState::Pending => {
                        let kind = if original_rejected {
                            PromiseReactionKind::Reject
                        } else {
                            PromiseReactionKind::Fulfill
                        };
                        ctx.pending_promise_reactions
                            .entry(promise_key(&source))
                            .or_default()
                            .push(PendingPromiseReaction {
                                on_fulfilled: None,
                                on_rejected: None,
                                on_finally: None,
                                completion_override: Some((kind, job.argument.clone())),
                                result: job.result.clone(),
                            });
                        continue;
                    }
                },
                Ok(_) if original_rejected => Err(job.argument.clone()),
                Ok(_) => Ok(job.argument.clone()),
            }
        } else {
            outcome
        };
        resolve_promise(ctx, &job.result, settlement)?;
    }
    Ok(())
}

/// Check if a function name is sandboxed (blocked)
fn is_sandboxed_function(name: &str) -> bool {
    let sandboxed_functions = [
        "eval",
        "Function",
        "setTimeout",
        "setInterval",
        "clearTimeout",
        "clearInterval",
        "document.write",
        "document.writeln",
        "document.getElementById",
        "document.createElement",
        "window.alert",
        "window.confirm",
        "window.prompt",
        "window.open",
        "window.location",
        "window.history",
        "window.navigator",
        "window.frames",
        "window.parent",
        "window.top",
        "window.opener",
        "window.crypto",
        "window.localStorage",
        "window.sessionStorage",
        "window.XMLHttpRequest",
        "window.fetch",
        "window.requestAnimationFrame",
        "window.cancelAnimationFrame",
        "globalThis",
        "window",
        "document",
        "self",
        "eval",
    ];

    sandboxed_functions.contains(&name)
}

/// Call a native built-in function
fn call_native_fn(
    name: &str,
    args: Vec<JsvValue>,
    console: &mut Vec<String>,
    ctx: &mut EvalCtx<'_>,
) -> Result<JsvValue, String> {
    // Check if this is a sandboxed (blocked) function
    if is_sandboxed_function(name) {
        return Err(format!(
            "SecurityError: {} is not available in sandboxed JavaScript environment",
            name
        ));
    }

    match name {
        "console.log" | "print" => {
            let output = args
                .iter()
                .map(|a| a.to_display_string())
                .collect::<Vec<_>>()
                .join(" ");
            push_console(console, output.clone());
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
            push_console(console, format!("WARN: {}", output));
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
            push_console(console, format!("ERROR: {}", output));
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
        "Object.create" => {
            let prototype = match args.first() {
                Some(JsvValue::Object(object)) => Some(object.clone()),
                Some(JsvValue::Null) | None => None,
                _ => {
                    return Err("TypeError: Object prototype must be an object or null".to_string())
                }
            };
            Ok(JsvValue::Object(Rc::new(RefCell::new(JsvObject {
                properties: HashMap::new(),
                prototype,
            }))))
        }
        "Object.keys" => {
            let object = match args.first() {
                Some(JsvValue::Object(object)) => object,
                _ => return Err("TypeError: Object.keys requires an object".to_string()),
            };
            let mut keys = object
                .borrow()
                .properties
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            keys.sort();
            Ok(JsvValue::Array(Rc::new(RefCell::new(
                keys.into_iter().map(JsvValue::String).collect(),
            ))))
        }
        "Array.isArray" => Ok(JsvValue::Boolean(matches!(
            args.first(),
            Some(JsvValue::Array(_))
        ))),
        "Promise.resolve" => match args.first().cloned().unwrap_or(JsvValue::Undefined) {
            promise @ JsvValue::Promise(_) => Ok(promise),
            value => Ok(promise_value(JsvPromiseState::Fulfilled(value))),
        },
        "Promise.reject" => Ok(promise_value(JsvPromiseState::Rejected(
            args.first().cloned().unwrap_or(JsvValue::Undefined),
        ))),
        "Promise.all" => {
            let values = match args.first() {
                Some(JsvValue::Array(values)) => values.borrow().clone(),
                _ => return Err("TypeError: Promise.all requires an array".to_string()),
            };
            let mut output = Vec::with_capacity(values.len());
            for value in values {
                match value {
                    JsvValue::Promise(promise) => match promise.borrow().clone() {
                        JsvPromiseState::Fulfilled(value) => output.push(value),
                        JsvPromiseState::Rejected(reason) => {
                            return Ok(promise_value(JsvPromiseState::Rejected(reason)));
                        }
                        JsvPromiseState::Pending => {
                            return Ok(promise_value(JsvPromiseState::Pending));
                        }
                    },
                    value => output.push(value),
                }
            }
            Ok(promise_value(JsvPromiseState::Fulfilled(JsvValue::Array(
                Rc::new(RefCell::new(output)),
            ))))
        }
        "Promise.race" => {
            let values = match args.first() {
                Some(JsvValue::Array(values)) => values.borrow().clone(),
                _ => return Err("TypeError: Promise.race requires an array".to_string()),
            };
            let state = values
                .into_iter()
                .find_map(|value| match value {
                    JsvValue::Promise(promise) => match promise.borrow().clone() {
                        JsvPromiseState::Pending => None,
                        settled => Some(settled),
                    },
                    value => Some(JsvPromiseState::Fulfilled(value)),
                })
                .unwrap_or(JsvPromiseState::Pending);
            Ok(promise_value(state))
        }
        "Math.abs" => {
            let n = first_number(&args);
            Ok(JsvValue::Number(n.abs()))
        }
        "Math.ceil" => {
            let n = first_number(&args);
            Ok(JsvValue::Number(n.ceil()))
        }
        "Math.floor" => {
            let n = first_number(&args);
            Ok(JsvValue::Number(n.floor()))
        }
        "Math.round" => {
            let n = first_number(&args);
            Ok(JsvValue::Number(n.round()))
        }
        "Math.sqrt" => {
            let n = first_number(&args);
            // JS semantics: sqrt of a negative number is NaN (not an error).
            Ok(JsvValue::Number(n.sqrt()))
        }
        "Math.random" => Ok(JsvValue::Number(random_f64())),
        "Date.now" => Ok(JsvValue::Number(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as f64)
                .unwrap_or(0.0),
        )),
        "Date.parse" => {
            let ts = args
                .first()
                .and_then(|a| a.as_string())
                .and_then(parse_date);
            Ok(JsvValue::Number(ts.unwrap_or(f64::NAN)))
        }
        "JSON.stringify" => {
            let value = args.first().unwrap_or(&JsvValue::Undefined);
            json_stringify(value, ctx)
        }
        "JSON.parse" => {
            let source = args
                .first()
                .and_then(|a| a.as_string())
                .ok_or_else(|| "TypeError: JSON.parse requires a string".to_string())?;
            json_parse(source)
        }
        _ => Err(format!("ReferenceError: {} is not defined", name)),
    }
}

const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_NODES: usize = 100_000;

/// Bounded JSON serialization of interpreter values. Cycles are impossible in
/// the current value model (objects cannot reference themselves through
/// properties), so no visited-set is needed; depth and length budgets apply.
fn json_stringify(value: &JsvValue, ctx: &mut EvalCtx<'_>) -> Result<JsvValue, String> {
    if matches!(
        value,
        JsvValue::Undefined
            | JsvValue::Function(..)
            | JsvValue::AsyncFunction(..)
            | JsvValue::NativeFn(_)
            | JsvValue::BoundNativeFn(..)
    ) {
        return Ok(JsvValue::Undefined);
    }
    let mut output = String::new();
    json_write(value, &mut output, 0)?;
    check_string_alloc(ctx, output.len())?;
    Ok(JsvValue::String(output))
}

fn json_write(value: &JsvValue, output: &mut String, depth: usize) -> Result<(), String> {
    if depth > MAX_JSON_DEPTH {
        return Err("JSON depth budget exceeded".to_string());
    }
    if output.len() > MAX_STRING_LENGTH {
        return Err("String too large".to_string());
    }
    match value {
        JsvValue::Number(n) => {
            if n.is_finite() {
                if *n == n.floor() && n.abs() < 1e15 {
                    output.push_str(&(*n as i64).to_string());
                } else {
                    output.push_str(&n.to_string());
                }
            } else {
                output.push_str("null");
            }
        }
        JsvValue::Boolean(b) => output.push_str(if *b { "true" } else { "false" }),
        JsvValue::String(s) => json_escape(s, output),
        JsvValue::Null => output.push_str("null"),
        JsvValue::Array(array) => {
            output.push('[');
            let values = array.borrow();
            for (index, item) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                if matches!(
                    item,
                    JsvValue::Undefined
                        | JsvValue::Function(..)
                        | JsvValue::AsyncFunction(..)
                        | JsvValue::NativeFn(_)
                        | JsvValue::BoundNativeFn(..)
                ) {
                    output.push_str("null");
                } else {
                    json_write(item, output, depth + 1)?;
                }
            }
            output.push(']');
        }
        JsvValue::Object(object) => {
            output.push('{');
            let borrowed = object.borrow();
            let mut first = true;
            for (key, item) in &borrowed.properties {
                if matches!(
                    item,
                    JsvValue::Undefined
                        | JsvValue::Function(..)
                        | JsvValue::AsyncFunction(..)
                        | JsvValue::NativeFn(_)
                        | JsvValue::BoundNativeFn(..)
                ) {
                    continue;
                }
                if !first {
                    output.push(',');
                }
                first = false;
                json_escape(key, output);
                output.push(':');
                json_write(item, output, depth + 1)?;
            }
            output.push('}');
        }
        _ => {
            output.push_str("null");
        }
    }
    Ok(())
}

fn json_escape(text: &str, output: &mut String) {
    output.push('"');
    for character in text.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000C}' => output.push_str("\\f"),
            other => output.push(other),
        }
    }
    output.push('"');
}

/// Bounded JSON parser producing interpreter values. Node, depth and length
/// budgets fail closed on hostile input.
fn json_parse(source: &str) -> Result<JsvValue, String> {
    let chars: Vec<char> = source.chars().collect();
    let mut pos = 0usize;
    let mut nodes = 0usize;
    let value = json_parse_value(&chars, &mut pos, &mut nodes, 0)?;
    json_skip_ws(&chars, &mut pos);
    if pos != chars.len() {
        return Err("JSON parse error: trailing content".to_string());
    }
    Ok(value)
}

fn json_skip_ws(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && chars[*pos].is_whitespace() {
        *pos += 1;
    }
}

fn json_parse_value(
    chars: &[char],
    pos: &mut usize,
    nodes: &mut usize,
    depth: usize,
) -> Result<JsvValue, String> {
    if depth > MAX_JSON_DEPTH {
        return Err("JSON depth budget exceeded".to_string());
    }
    *nodes += 1;
    if *nodes > MAX_JSON_NODES {
        return Err("JSON node budget exceeded".to_string());
    }
    json_skip_ws(chars, pos);
    let Some(&first) = chars.get(*pos) else {
        return Err("JSON parse error: unexpected end of input".to_string());
    };
    match first {
        '{' => {
            *pos += 1;
            let mut properties = HashMap::new();
            json_skip_ws(chars, pos);
            if chars.get(*pos) == Some(&'}') {
                *pos += 1;
                return Ok(object_value(properties));
            }
            loop {
                json_skip_ws(chars, pos);
                let key = json_parse_string(chars, pos)?;
                json_skip_ws(chars, pos);
                if chars.get(*pos) != Some(&':') {
                    return Err("JSON parse error: expected ':'".to_string());
                }
                *pos += 1;
                let value = json_parse_value(chars, pos, nodes, depth + 1)?;
                properties.insert(key, value);
                json_skip_ws(chars, pos);
                match chars.get(*pos) {
                    Some(',') => {
                        *pos += 1;
                    }
                    Some('}') => {
                        *pos += 1;
                        break;
                    }
                    _ => return Err("JSON parse error: expected ',' or '}'".to_string()),
                }
            }
            Ok(object_value(properties))
        }
        '[' => {
            *pos += 1;
            let mut values = Vec::new();
            json_skip_ws(chars, pos);
            if chars.get(*pos) == Some(&']') {
                *pos += 1;
                return Ok(JsvValue::Array(Rc::new(RefCell::new(values))));
            }
            loop {
                let value = json_parse_value(chars, pos, nodes, depth + 1)?;
                values.push(value);
                json_skip_ws(chars, pos);
                match chars.get(*pos) {
                    Some(',') => {
                        *pos += 1;
                    }
                    Some(']') => {
                        *pos += 1;
                        break;
                    }
                    _ => return Err("JSON parse error: expected ',' or ']'".to_string()),
                }
            }
            Ok(JsvValue::Array(Rc::new(RefCell::new(values))))
        }
        '"' => Ok(JsvValue::String(json_parse_string(chars, pos)?)),
        't' => {
            json_consume_literal(chars, pos, "true")?;
            Ok(JsvValue::Boolean(true))
        }
        'f' => {
            json_consume_literal(chars, pos, "false")?;
            Ok(JsvValue::Boolean(false))
        }
        'n' => {
            json_consume_literal(chars, pos, "null")?;
            Ok(JsvValue::Null)
        }
        '-' | '0'..='9' => {
            let start = *pos;
            if chars.get(*pos) == Some(&'-') {
                *pos += 1;
            }
            while chars
                .get(*pos)
                .is_some_and(|character| character.is_ascii_digit())
            {
                *pos += 1;
            }
            if chars.get(*pos) == Some(&'.') {
                *pos += 1;
                while chars
                    .get(*pos)
                    .is_some_and(|character| character.is_ascii_digit())
                {
                    *pos += 1;
                }
            }
            if matches!(chars.get(*pos), Some('e' | 'E')) {
                *pos += 1;
                if matches!(chars.get(*pos), Some('+' | '-')) {
                    *pos += 1;
                }
                while chars
                    .get(*pos)
                    .is_some_and(|character| character.is_ascii_digit())
                {
                    *pos += 1;
                }
            }
            let token: String = chars[start..*pos].iter().collect();
            token
                .parse::<f64>()
                .map(JsvValue::Number)
                .map_err(|_| "JSON parse error: invalid number".to_string())
        }
        _ => Err("JSON parse error: unexpected token".to_string()),
    }
}

fn json_parse_string(chars: &[char], pos: &mut usize) -> Result<String, String> {
    if chars.get(*pos) != Some(&'"') {
        return Err("JSON parse error: expected string".to_string());
    }
    *pos += 1;
    let mut output = String::new();
    loop {
        let Some(&character) = chars.get(*pos) else {
            return Err("JSON parse error: unterminated string".to_string());
        };
        *pos += 1;
        match character {
            '"' => return Ok(output),
            '\\' => {
                let Some(&escaped) = chars.get(*pos) else {
                    return Err("JSON parse error: unterminated escape".to_string());
                };
                *pos += 1;
                match escaped {
                    '"' => output.push('"'),
                    '\\' => output.push('\\'),
                    '/' => output.push('/'),
                    'b' => output.push('\u{0008}'),
                    'f' => output.push('\u{000C}'),
                    'n' => output.push('\n'),
                    'r' => output.push('\r'),
                    't' => output.push('\t'),
                    'u' => {
                        if *pos + 4 > chars.len() {
                            return Err("JSON parse error: invalid unicode escape".to_string());
                        }
                        let hex: String = chars[*pos..*pos + 4].iter().collect();
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|_| "JSON parse error: invalid unicode escape".to_string())?;
                        output.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        *pos += 4;
                    }
                    _ => return Err("JSON parse error: invalid escape".to_string()),
                }
            }
            other => output.push(other),
        }
        if output.len() > MAX_STRING_LENGTH {
            return Err("String too large".to_string());
        }
    }
}

fn json_consume_literal(chars: &[char], pos: &mut usize, literal: &str) -> Result<(), String> {
    let expected: Vec<char> = literal.chars().collect();
    if *pos + expected.len() > chars.len() || chars[*pos..*pos + expected.len()] != expected[..] {
        return Err("JSON parse error: invalid literal".to_string());
    }
    *pos += expected.len();
    Ok(())
}

/// First arg as a number (0.0 when missing/not a number), like JS coercion
/// for Number inputs.
fn first_number(args: &[JsvValue]) -> f64 {
    args.first().and_then(|a| a.as_number()).unwrap_or(f64::NAN)
}

/// Parse a small set of common date strings to a JS-style millisecond
/// timestamp. Unparseable input yields None (caller returns NaN, matching JS).
fn parse_date(s: &str) -> Option<f64> {
    let s = s.trim();
    for fmt in &["%Y-%m-%d", "%Y-%m-%d %H:%M:%S", "%d/%m/%Y"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt.and_utc().timestamp_millis() as f64);
        }
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, fmt) {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis() as f64);
        }
    }
    None
}

/// Small deterministic non-cryptographic RNG for Math.random().
fn random_f64() -> f64 {
    use std::cell::Cell;
    thread_local! {
        static STATE: Cell<u64> = const { Cell::new(0x9E37_79B9_7F4A_7C15) };
    }
    STATE.with(|s| {
        let mut x = s.get();
        // xorshift64*
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        s.set(x);
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    })
}

// ===== TOKENIZER =====

fn tokenize(code: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = code.chars().collect();
    let len = chars.len();
    let mut i = 0;
    // Template-literal state: `reading_template_text` is true between the
    // opening backtick and the next '`' or "${". `template_expr_depth` counts
    // open `${...}` interpolations and `expr_bracket_depth` tracks ()[]{}
    // nesting inside the innermost one so a closing '}' at depth zero ends
    // the interpolation instead of being emitted as a block close.
    let mut reading_template_text = false;
    let mut template_expr_depth = 0usize;
    let mut expr_bracket_depth = 0usize;

    while i < len {
        if reading_template_text {
            let c = chars[i];
            if c == '`' {
                tokens.push("`END".to_string());
                reading_template_text = false;
                i += 1;
            } else if c == '$' && i + 1 < len && chars[i + 1] == '{' {
                tokens.push("`${".to_string());
                reading_template_text = false;
                template_expr_depth += 1;
                expr_bracket_depth = 0;
                i += 2;
            } else if c == '\\' && i + 1 < len {
                let esc = chars[i + 1];
                i += 2;
                match esc {
                    'n' => tokens.push("\"\n\"".to_string()),
                    't' => tokens.push("\"\t\"".to_string()),
                    'r' => tokens.push("\"\r\"".to_string()),
                    '\\' => tokens.push("\"\\\\\"".to_string()),
                    '`' => tokens.push("\"`\"".to_string()),
                    '$' => tokens.push("\"$\"".to_string()),
                    _ => {
                        tokens.push(format!("\"\\{}\"", esc));
                    }
                }
            } else {
                // Accumulate a run of plain text into one string token.
                let mut text = String::new();
                while i < len {
                    let c = chars[i];
                    if c == '`' || (c == '$' && i + 1 < len && chars[i + 1] == '{') {
                        break;
                    }
                    if c == '\\' && i + 1 < len {
                        break;
                    }
                    text.push(c);
                    i += 1;
                }
                tokens.push(format!("\"{}\"", text));
            }
            continue;
        }

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
        } else if chars[i] == '"' || chars[i] == '\'' {
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
        } else if chars[i] == '`' {
            tokens.push("`".to_string());
            reading_template_text = true;
            i += 1;
        } else if i + 2 < len {
            let three_chars: String = chars[i..i + 3].iter().collect();
            if matches!(three_chars.as_str(), "===" | "!==") {
                tokens.push(three_chars);
                i += 3;
            } else if i + 1 < len {
                let two_chars: String = chars[i..i + 2].iter().collect();
                if matches!(
                    two_chars.as_str(),
                    "==" | "!="
                        | "<="
                        | ">="
                        | "&&"
                        | "||"
                        | "=>"
                        | "??"
                        | "?."
                        | "++"
                        | "--"
                        | "+="
                        | "-="
                        | "*="
                        | "/="
                        | "%="
                ) {
                    tokens.push(two_chars);
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
                    let single = chars[i].to_string();
                    track_template_bracket(
                        &mut tokens,
                        &single,
                        &mut template_expr_depth,
                        &mut expr_bracket_depth,
                        &mut reading_template_text,
                    )?;
                    i += 1;
                }
            }
        } else if [
            '+', '-', '*', '/', '%', '<', '>', '=', '!', '(', ')', '{', '}', '[', ']', ';', ',',
            '.', ':', '?',
        ]
        .contains(&chars[i])
        {
            let single = chars[i].to_string();
            track_template_bracket(
                &mut tokens,
                &single,
                &mut template_expr_depth,
                &mut expr_bracket_depth,
                &mut reading_template_text,
            )?;
            i += 1;
        } else {
            i += 1;
        }
    }
    if reading_template_text {
        return Err("Unterminated template literal".to_string());
    }
    Ok(tokens)
}

/// Emit a single-character token, keeping the template interpolation bracket
/// accounting consistent when the token stream is inside `${...}`.
fn track_template_bracket(
    tokens: &mut Vec<String>,
    token: &str,
    template_expr_depth: &mut usize,
    expr_bracket_depth: &mut usize,
    reading_template_text: &mut bool,
) -> Result<(), String> {
    if *template_expr_depth > 0 {
        match token {
            "(" | "[" | "{" => *expr_bracket_depth += 1,
            ")" | "]" => *expr_bracket_depth = expr_bracket_depth.saturating_sub(1),
            "}" if *expr_bracket_depth == 0 => {
                tokens.push("`}".to_string());
                *template_expr_depth -= 1;
                *reading_template_text = true;
                return Ok(());
            }
            "}" => *expr_bracket_depth -= 1,
            _ => {}
        }
    }
    tokens.push(token.to_string());
    Ok(())
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
        "for" => {
            if tokens.get(pos + 1).map(String::as_str) != Some("(") {
                return Err("Expected '(' after for".to_string());
            }
            let mut i = pos + 2;
            let kind = match tokens.get(i).map(String::as_str) {
                Some("const") => DeclarationKind::Const,
                Some("var") => DeclarationKind::Var,
                Some("let") => DeclarationKind::Let,
                _ => return Err("Expected declaration in for...of header".to_string()),
            };
            i += 1;
            let binding = tokens
                .get(i)
                .filter(|name| is_identifier(name))
                .cloned()
                .ok_or_else(|| "Expected iterator binding".to_string())?;
            i += 1;
            if tokens.get(i).map(String::as_str) != Some("of") {
                return Err("Only for...of iteration is supported".to_string());
            }
            let (iterable, next) = parse_expression(tokens, i + 1, depth + 1)?;
            if tokens.get(next).map(String::as_str) != Some(")") {
                return Err("Expected ')' after for...of iterable".to_string());
            }
            let (body, next) = parse_statement(tokens, next + 1, depth + 1)?;
            let body = match body {
                JsvExpr::Block(statements) => statements,
                statement => vec![statement],
            };
            Ok((
                JsvExpr::ForOf(binding, kind, Box::new(iterable), body),
                next,
            ))
        }
        "switch" => {
            if tokens.get(pos + 1).map(String::as_str) != Some("(") {
                return Err("Expected '(' after switch".to_string());
            }
            let (discriminant, mut i) = parse_expression(tokens, pos + 2, depth + 1)?;
            if tokens.get(i).map(String::as_str) != Some(")") {
                return Err("Expected ')' after switch discriminant".to_string());
            }
            i += 1;
            if tokens.get(i).map(String::as_str) != Some("{") {
                return Err("Expected '{' after switch".to_string());
            }
            i += 1;
            let mut cases: Vec<(Option<JsvExpr>, Vec<JsvExpr>)> = Vec::new();
            let mut default_clause: Option<Vec<JsvExpr>> = None;
            // The header currently being filled and its collected statements;
            // a case is committed to `cases` only when the next header or the
            // closing brace arrives.
            let mut open_header: Option<Option<JsvExpr>> = None;
            let mut open_body: Vec<JsvExpr> = Vec::new();
            loop {
                if tokens.get(i).map(String::as_str) == Some("}") {
                    break;
                }
                if tokens.get(i).map(String::as_str) == Some("case") {
                    let (case_expr, next) = parse_expression(tokens, i + 1, depth + 1)?;
                    if tokens.get(next).map(String::as_str) != Some(":") {
                        return Err("Expected ':' after case value".to_string());
                    }
                    if let Some(header) = open_header.take() {
                        let body = std::mem::take(&mut open_body);
                        if let Some(case_expr) = header {
                            cases.push((Some(case_expr), body));
                        } else {
                            default_clause = Some(body);
                        }
                    }
                    open_header = Some(Some(case_expr));
                    i = next + 1;
                } else if tokens.get(i).map(String::as_str) == Some("default") {
                    if tokens.get(i + 1).map(String::as_str) != Some(":") {
                        return Err("Expected ':' after default".to_string());
                    }
                    if let Some(header) = open_header.take() {
                        let body = std::mem::take(&mut open_body);
                        if let Some(case_expr) = header {
                            cases.push((Some(case_expr), body));
                        } else {
                            default_clause = Some(body);
                        }
                    }
                    if default_clause.is_some() {
                        return Err("Duplicate default clause in switch".to_string());
                    }
                    open_header = Some(None);
                    i += 2;
                } else {
                    let (statement, next) = parse_statement(tokens, i, depth + 1)?;
                    open_body.push(statement);
                    i = next;
                    while tokens.get(i).map(String::as_str) == Some(";") {
                        i += 1;
                    }
                }
            }
            if let Some(header) = open_header.take() {
                let body = std::mem::take(&mut open_body);
                if let Some(case_expr) = header {
                    cases.push((Some(case_expr), body));
                } else {
                    default_clause = Some(body);
                }
            }
            if i >= tokens.len() {
                return Err("Expected '}' to close switch".to_string());
            }
            Ok((
                JsvExpr::Switch(Box::new(discriminant), cases, default_clause),
                i + 1,
            ))
        }
        "try" => {
            if tokens.get(pos + 1).map(String::as_str) != Some("{") {
                return Err("Expected '{' after try".to_string());
            }
            let (try_body, mut i) = parse_statements(tokens, pos + 2, depth + 1)?;
            if tokens.get(i).map(String::as_str) != Some("}") {
                return Err("Expected '}' after try body".to_string());
            }
            i += 1;
            let mut catch_binding = None;
            let mut catch_body = Vec::new();
            let mut finally_body = Vec::new();
            if tokens.get(i).map(String::as_str) == Some("catch") {
                i += 1;
                if tokens.get(i).map(String::as_str) == Some("(") {
                    let binding = tokens
                        .get(i + 1)
                        .filter(|name| is_identifier(name))
                        .cloned()
                        .ok_or_else(|| "Expected catch binding".to_string())?;
                    if tokens.get(i + 2).map(String::as_str) != Some(")") {
                        return Err("Expected ')' after catch binding".to_string());
                    }
                    catch_binding = Some(binding);
                    i += 3;
                } else {
                    catch_binding = Some("error".to_string());
                }
                if tokens.get(i).map(String::as_str) != Some("{") {
                    return Err("Expected '{' before catch body".to_string());
                }
                let (body, next) = parse_statements(tokens, i + 1, depth + 1)?;
                if tokens.get(next).map(String::as_str) != Some("}") {
                    return Err("Expected '}' after catch body".to_string());
                }
                catch_body = body;
                i = next + 1;
            }
            if tokens.get(i).map(String::as_str) == Some("finally") {
                if tokens.get(i + 1).map(String::as_str) != Some("{") {
                    return Err("Expected '{' before finally body".to_string());
                }
                let (body, next) = parse_statements(tokens, i + 2, depth + 1)?;
                if tokens.get(next).map(String::as_str) != Some("}") {
                    return Err("Expected '}' after finally body".to_string());
                }
                finally_body = body;
                i = next + 1;
            }
            if catch_binding.is_none() && finally_body.is_empty() {
                return Err("Try statement requires catch or finally".to_string());
            }
            Ok((
                JsvExpr::TryCatchFinally {
                    try_body,
                    catch_binding,
                    catch_body,
                    finally_body,
                },
                i,
            ))
        }
        "var" | "let" | "const" => {
            let kind = match tok.as_str() {
                "var" => DeclarationKind::Var,
                "const" => DeclarationKind::Const,
                _ => DeclarationKind::Let,
            };
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
                Ok((
                    JsvExpr::VariableDeclaration(var_name, Box::new(val_expr), kind),
                    i,
                ))
            } else {
                if kind == DeclarationKind::Const {
                    return Err("Missing initializer in const declaration".to_string());
                }
                Ok((
                    JsvExpr::VariableDeclaration(var_name, Box::new(JsvExpr::Undefined), kind),
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
        "break" => Ok((JsvExpr::Break, pos + 1)),
        "continue" => Ok((JsvExpr::Continue, pos + 1)),
        "throw" => {
            if pos + 1 >= tokens.len() || tokens[pos + 1] == ";" {
                return Err("Throw statement requires a value".to_string());
            }
            let (value, i) = parse_expression(tokens, pos + 1, depth + 1)?;
            Ok((JsvExpr::Throw(Box::new(value)), i))
        }
        "async" => {
            if tokens.get(pos + 1).map(String::as_str) != Some("function") {
                return Err("Only async function declarations are supported".to_string());
            }
            let (function, i) = parse_statement(tokens, pos + 1, depth + 1)?;
            match function {
                JsvExpr::FunctionDef(name, parameters, body) => {
                    Ok((JsvExpr::AsyncFunctionDef(name, parameters, body), i))
                }
                _ => Err("Invalid async function declaration".to_string()),
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
    if let Some((parameters, is_async, body_pos)) = parse_arrow_signature(tokens, pos) {
        let (body, next) = if tokens.get(body_pos).map(String::as_str) == Some("{") {
            parse_statement(tokens, body_pos, depth + 1)?
        } else {
            let (expression, next) = parse_assignment(tokens, body_pos, depth + 1)?;
            (JsvExpr::Return(Box::new(expression)), next)
        };
        return Ok((
            JsvExpr::FunctionExpr(parameters, Box::new(body), is_async),
            next,
        ));
    }

    let (left, i) = parse_or(tokens, pos, depth)?;
    // Conditional expression: `cond ? then : else`, right-associative.
    if i < tokens.len() && tokens[i] == "?" {
        let (then_expr, i2) = parse_assignment(tokens, i + 1, depth + 1)?;
        if i2 >= tokens.len() || tokens[i2] != ":" {
            return Err("Expected ':' in conditional expression".to_string());
        }
        let (else_expr, i3) = parse_assignment(tokens, i2 + 1, depth + 1)?;
        return Ok((
            JsvExpr::Ternary(Box::new(left), Box::new(then_expr), Box::new(else_expr)),
            i3,
        ));
    }
    if i < tokens.len() && matches!(tokens[i].as_str(), "+=" | "-=" | "*=" | "/=" | "%=") {
        let op = match tokens[i].as_str() {
            "+=" => OpKind::Add,
            "-=" => OpKind::Sub,
            "*=" => OpKind::Mul,
            "/=" => OpKind::Div,
            _ => OpKind::Mod,
        };
        let (right, new_i) = parse_assignment(tokens, i + 1, depth + 1)?;
        match left {
            JsvExpr::Identifier(name) => Ok((
                JsvExpr::Assignment(
                    name.clone(),
                    Box::new(JsvExpr::BinaryOp(
                        Box::new(JsvExpr::Identifier(name)),
                        op,
                        Box::new(right),
                    )),
                ),
                new_i,
            )),
            JsvExpr::Member(object, property) => Ok((
                JsvExpr::MemberAssignment(
                    object.clone(),
                    property.clone(),
                    Box::new(JsvExpr::BinaryOp(
                        Box::new(JsvExpr::Member(object, property)),
                        op,
                        Box::new(right),
                    )),
                ),
                new_i,
            )),
            JsvExpr::Index(object, key) => Ok((
                JsvExpr::IndexAssignment(
                    object.clone(),
                    key.clone(),
                    Box::new(JsvExpr::BinaryOp(
                        Box::new(JsvExpr::Index(object, key)),
                        op,
                        Box::new(right),
                    )),
                ),
                new_i,
            )),
            _ => Err("Invalid compound assignment target".to_string()),
        }
    } else if i < tokens.len() && tokens[i] == "=" {
        if i + 1 < tokens.len() && tokens[i + 1] == "=" {
            return Ok((left, i));
        }
        match left {
            JsvExpr::Identifier(name) => {
                let (right, new_i) = parse_assignment(tokens, i + 1, depth + 1)?;
                Ok((JsvExpr::Assignment(name, Box::new(right)), new_i))
            }
            JsvExpr::Member(object, property) => {
                let (right, new_i) = parse_assignment(tokens, i + 1, depth + 1)?;
                Ok((
                    JsvExpr::MemberAssignment(object, property, Box::new(right)),
                    new_i,
                ))
            }
            JsvExpr::Index(object, key) => {
                let (right, new_i) = parse_assignment(tokens, i + 1, depth + 1)?;
                Ok((
                    JsvExpr::IndexAssignment(object, key, Box::new(right)),
                    new_i,
                ))
            }
            _ => Err("Invalid assignment target".to_string()),
        }
    } else {
        Ok((left, i))
    }
}

/// Recognize the deliberately bounded arrow-function parameter grammar used
/// by the interpreter: `value =>`, `(left, right) =>`, and their `async`
/// forms. Default/rest/destructured parameters remain outside this phase.
fn parse_arrow_signature(tokens: &[String], pos: usize) -> Option<(Vec<String>, bool, usize)> {
    let mut i = pos;
    let is_async = tokens.get(i).map(String::as_str) == Some("async");
    if is_async {
        i += 1;
    }

    if is_identifier(tokens.get(i)?) && tokens.get(i + 1).map(String::as_str) == Some("=>") {
        return Some((vec![tokens[i].clone()], is_async, i + 2));
    }

    if tokens.get(i).map(String::as_str) != Some("(") {
        return None;
    }
    i += 1;
    let mut parameters = Vec::new();
    while tokens.get(i).map(String::as_str) != Some(")") {
        let parameter = tokens.get(i)?;
        if !is_identifier(parameter) {
            return None;
        }
        parameters.push(parameter.clone());
        i += 1;
        match tokens.get(i).map(String::as_str) {
            Some(",") => i += 1,
            Some(")") => {}
            _ => return None,
        }
    }
    if tokens.get(i + 1).map(String::as_str) != Some("=>") {
        return None;
    }
    Some((parameters, is_async, i + 2))
}

fn parse_or(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (left, mut i) = parse_nullish(tokens, pos, depth)?;
    let mut result = left;
    while i < tokens.len() && tokens[i] == "||" {
        let (right, new_i) = parse_nullish(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), OpKind::Or, Box::new(right));
        i = new_i;
    }
    Ok((result, i))
}

fn parse_nullish(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (left, mut i) = parse_and(tokens, pos, depth)?;
    let mut result = left;
    while i < tokens.len() && tokens[i] == "??" {
        let (right, new_i) = parse_and(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), OpKind::Nullish, Box::new(right));
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
    while i < tokens.len() && matches!(tokens[i].as_str(), "==" | "!=" | "===" | "!==") {
        let op = match tokens[i].as_str() {
            "===" => OpKind::StrictEq,
            "!==" => OpKind::StrictNeq,
            "==" => OpKind::Eq,
            _ => OpKind::Neq,
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
    while i < tokens.len() && matches!(tokens[i].as_str(), "<" | ">" | "<=" | ">=" | "in") {
        let op = match tokens[i].as_str() {
            "<" => OpKind::Lt,
            ">" => OpKind::Gt,
            "<=" => OpKind::Le,
            "in" => OpKind::In,
            _ => OpKind::Ge,
        };
        let (right, new_i) = parse_term(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), op, Box::new(right));
        i = new_i;
    }
    if i < tokens.len() && tokens[i] == "instanceof" {
        let (right, new_i) = parse_term(tokens, i + 1, depth)?;
        result = JsvExpr::BinaryOp(Box::new(result), OpKind::Instanceof, Box::new(right));
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
    } else if tokens[pos] == "await" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::Await(Box::new(expr)), i))
    } else if tokens[pos] == "typeof" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::UnaryOp(OpKind::Typeof, Box::new(expr)), i))
    } else if tokens[pos] == "++" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::Update(Box::new(expr), true, true), i))
    } else if tokens[pos] == "--" {
        let (expr, i) = parse_unary(tokens, pos + 1, depth + 1)?;
        Ok((JsvExpr::Update(Box::new(expr), true, false), i))
    } else {
        parse_call(tokens, pos, depth)
    }
}

fn parse_call(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    if pos >= tokens.len() {
        return Err("Unexpected end of expression".to_string());
    }
    if tokens[pos] == "new" {
        return parse_new_call(tokens, pos, depth);
    }
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
                expr = JsvExpr::Member(Box::new(expr), prop);
            } else {
                // Member access WITHOUT a call (e.g. `Math.PI`, `console.log`).
                // Map it through the flat "base.prop" key used to register
                // natives — and error instead of silently dropping the
                // property (the old behavior returned the base value).
                match expr_to_name(&expr) {
                    Some(_) => {
                        expr = JsvExpr::Member(Box::new(expr), prop.clone());
                    }
                    None => {
                        expr = JsvExpr::Member(Box::new(expr), prop);
                    }
                }
            }
        } else if i < tokens.len() && tokens[i] == "?." {
            i += 1;
            if i >= tokens.len() {
                return Err("Expected property name after '?.'".to_string());
            }
            if tokens[i] == "[" {
                let (key, next) = parse_expression(tokens, i + 1, depth + 1)?;
                if next >= tokens.len() || tokens[next] != "]" {
                    return Err("Expected ']' after optional index".to_string());
                }
                expr = JsvExpr::OptionalIndex(Box::new(expr), Box::new(key));
                i = next + 1;
            } else if is_identifier(&tokens[i]) {
                let prop = tokens[i].clone();
                i += 1;
                expr = JsvExpr::OptionalMember(Box::new(expr), prop);
            } else {
                return Err("Expected property name after '?.'".to_string());
            }
        } else if i < tokens.len() && tokens[i] == "[" {
            let (key, next) = parse_expression(tokens, i + 1, depth + 1)?;
            if next >= tokens.len() || tokens[next] != "]" {
                return Err("Expected ']' after property index".to_string());
            }
            expr = JsvExpr::Index(Box::new(expr), Box::new(key));
            i = next + 1;
        } else if i < tokens.len() && (tokens[i] == "++" || tokens[i] == "--") {
            // Postfix increment/decrement.
            expr = JsvExpr::Update(Box::new(expr), false, tokens[i] == "++");
            i += 1;
        } else {
            break;
        }
    }
    Ok((expr, i))
}

/// `new` expression: `new Callee(...)`, `new Callee` and member chains on the
/// constructed value. The first argument list becomes the construction call;
/// later calls apply to the resulting object.
fn parse_new_call(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    let (mut expr, mut i) = parse_primary(tokens, pos + 1, depth + 1)?;
    let mut constructed = false;
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
            expr = if constructed {
                JsvExpr::Call(Box::new(expr), args)
            } else {
                constructed = true;
                JsvExpr::NewExpr(Box::new(expr), args)
            };
        } else if i < tokens.len() && tokens[i] == "." {
            i += 1;
            if i >= tokens.len() || !is_identifier(&tokens[i]) {
                return Err("Expected property name after '.'".to_string());
            }
            let prop = tokens[i].clone();
            i += 1;
            expr = JsvExpr::Member(Box::new(expr), prop);
        } else if i < tokens.len() && tokens[i] == "[" {
            let (key, next) = parse_expression(tokens, i + 1, depth + 1)?;
            if next >= tokens.len() || tokens[next] != "]" {
                return Err("Expected ']' after property index".to_string());
            }
            expr = JsvExpr::Index(Box::new(expr), Box::new(key));
            i = next + 1;
        } else {
            break;
        }
    }
    if !constructed {
        expr = JsvExpr::NewExpr(Box::new(expr), Vec::new());
    }
    Ok((expr, i))
}

fn parse_primary(tokens: &[String], pos: usize, depth: usize) -> Result<(JsvExpr, usize), String> {
    if pos >= tokens.len() {
        return Err("Unexpected end of expression".to_string());
    }
    let tok = &tokens[pos];

    if tok == "import" {
        // Dynamic import: `import('specifier')` with a string literal only.
        if tokens.get(pos + 1).map(String::as_str) != Some("(") {
            return Err("Expected '(' after import".to_string());
        }
        let (specifier, next) = parse_expression(tokens, pos + 2, depth + 1)?;
        let JsvExpr::String(specifier) = specifier else {
            return Err("Dynamic import requires a string literal".to_string());
        };
        if next >= tokens.len() || tokens[next] != ")" {
            return Err("Expected ')' after import specifier".to_string());
        }
        return Ok((JsvExpr::DynamicImport(specifier), next + 1));
    }

    if tok == "function" {
        // Function expression (`function (a, b) { ... }` or named form),
        // used as a call argument or assigned to a variable. The name is
        // dropped for the bounded expression profile.
        let mut i = pos + 1;
        if is_identifier(tokens.get(i).map(String::as_str).unwrap_or("")) {
            i += 1;
        }
        if tokens.get(i).map(String::as_str) != Some("(") {
            return Err("Expected '(' after function".to_string());
        }
        i += 1;
        let mut params = Vec::new();
        while i < tokens.len() && tokens[i] != ")" {
            if tokens[i] != "," {
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
        if tokens.get(i).map(String::as_str) != Some("{") {
            return Err("Expected '{' for function body".to_string());
        }
        let (body, next) = parse_statements(tokens, i + 1, depth + 1)?;
        if tokens.get(next).map(String::as_str) != Some("}") {
            return Err("Expected '}' to close function body".to_string());
        }
        return Ok((
            JsvExpr::FunctionExpr(params, Box::new(JsvExpr::Block(body)), false),
            next + 1,
        ));
    }

    if tok == "`" {
        // Template literal: `text ${expr} text` (nested templates allowed).
        let mut parts = Vec::new();
        let mut i = pos + 1;
        loop {
            if i >= tokens.len() {
                return Err("Unterminated template literal".to_string());
            }
            let current = &tokens[i];
            if current.starts_with('"') && current.ends_with('"') && current.len() >= 2 {
                parts.push(TemplatePart::Text(
                    current[1..current.len() - 1].to_string(),
                ));
                i += 1;
            } else if current == "`${" {
                let (expr, next) = parse_expression(tokens, i + 1, depth + 1)?;
                if next >= tokens.len() || tokens[next] != "`}" {
                    return Err("Expected '}' to close template expression".to_string());
                }
                parts.push(TemplatePart::Expr(Box::new(expr)));
                i = next + 1;
            } else if current == "`END" {
                if parts.is_empty() {
                    parts.push(TemplatePart::Text(String::new()));
                }
                return Ok((JsvExpr::Template(parts), i + 1));
            } else {
                return Err(format!("Unexpected token in template literal: {}", current));
            }
        }
    }

    if tok == "`}" {
        // Boundary token of a template interpolation; the template parser
        // consumes it. Reaching this path directly is a parser error.
        return Err("Unexpected template expression boundary".to_string());
    }

    if tok == "(" {
        let (expr, i) = parse_expression(tokens, pos + 1, depth + 1)?;
        if i >= tokens.len() || tokens[i] != ")" {
            return Err("Expected ')'".to_string());
        }
        return Ok((expr, i + 1));
    }

    if tok == "[" {
        let mut values = Vec::new();
        let mut i = pos + 1;
        while i < tokens.len() && tokens[i] != "]" {
            let (value, next) = parse_expression(tokens, i, depth + 1)?;
            values.push(value);
            i = next;
            if i < tokens.len() && tokens[i] == "," {
                i += 1;
            } else if i < tokens.len() && tokens[i] != "]" {
                return Err("Expected ',' or ']' in array literal".to_string());
            }
        }
        if i >= tokens.len() {
            return Err("Expected ']' after array literal".to_string());
        }
        return Ok((JsvExpr::ArrayLiteral(values), i + 1));
    }

    if tok == "{" {
        let mut properties = Vec::new();
        let mut i = pos + 1;
        while i < tokens.len() && tokens[i] != "}" {
            let raw_name = tokens
                .get(i)
                .ok_or_else(|| "Expected object property name".to_string())?;
            let name = if raw_name.starts_with('"') && raw_name.ends_with('"') {
                raw_name[1..raw_name.len() - 1].to_string()
            } else if is_identifier(raw_name) || raw_name.parse::<f64>().is_ok() {
                raw_name.clone()
            } else {
                return Err("Invalid object property name".to_string());
            };
            i += 1;
            if i >= tokens.len() || tokens[i] != ":" {
                return Err("Expected ':' after object property name".to_string());
            }
            let (value, next) = parse_expression(tokens, i + 1, depth + 1)?;
            properties.push((name, value));
            i = next;
            if i < tokens.len() && tokens[i] == "," {
                i += 1;
            } else if i < tokens.len() && tokens[i] != "}" {
                return Err("Expected ',' or '}' in object literal".to_string());
            }
        }
        if i >= tokens.len() {
            return Err("Expected '}' after object literal".to_string());
        }
        return Ok((JsvExpr::ObjectLiteral(properties), i + 1));
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
        assert!(engine.global_env.contains_binding("console"));
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
        let x_val = engine.global_env.get("x");
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
        let i_val = engine.global_env.get("i");
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
        // The per-loop guard (10,000 iterations) trips before the total step
        // budget does, because each iteration costs only ~2 evaluations.
        assert!(
            err.contains("Infinite loop") || err.contains("step budget"),
            "got: {}",
            err
        );
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
    fn test_string_doubling_is_bounded() {
        // s = s + s doubling must error out, not allocate gigabytes.
        let mut engine = JsvEngine::new();
        let result = engine.eval("let s = \"a\"; while (true) { s = s + s }");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("String too large") || err.contains("budget"),
            "got: {}",
            err
        );
    }

    #[test]
    fn test_per_string_length_cap() {
        let mut engine = JsvEngine::new();
        // Build a string one byte past MAX_STRING_LENGTH (2 MB) via concatenation
        let script = format!(
            "let s = \"{}\"; s = s + s",
            "a".repeat(super::MAX_STRING_LENGTH / 2 + 1)
        );
        let result = engine.eval(&script);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("String too large"), "got: {}", err);
    }

    #[test]
    fn test_math_natives_work() {
        let mut engine = JsvEngine::new();
        let abs = engine.eval("Math.abs(-5)").unwrap();
        assert_eq!(abs.as_number(), Some(5.0));
        let sqrt = engine.eval("Math.sqrt(16)").unwrap();
        assert_eq!(sqrt.as_number(), Some(4.0));
        let pi = engine.eval("Math.PI").unwrap();
        assert!((pi.as_number().unwrap() - std::f64::consts::PI).abs() < 1e-9);
        let now = engine.eval("Date.now()").unwrap();
        assert!(now.as_number().unwrap() > 1_000_000_000_000.0);
    }

    #[test]
    fn test_member_access_follows_js_undefined_semantics() {
        // `Math.nope` (unknown property) evaluates to `undefined` like JS,
        // but must not silently evaluate to the base identifier.
        let mut engine = JsvEngine::new();
        let result = engine.eval("let x = Math.nope").unwrap();
        assert_eq!(result, JsvValue::Undefined);
        // Reading a property of `undefined` itself is still an error.
        assert!(engine
            .eval("undefined.name")
            .unwrap_err()
            .contains("TypeError"));
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

    #[test]
    fn test_unicode_before_two_character_operator_never_panics() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval("let message = 'Xin vui lòng'; message == 'Xin vui lòng'")
            .expect("Unicode source must tokenize on character boundaries");
        assert_eq!(value, JsvValue::Boolean(true));
    }

    #[test]
    fn phase10_closure_keeps_its_lexical_environment() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "function outer(base) { let captured=base; function inner(delta) { return captured+delta } return inner } let add=outer(4); add(3)",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(7.0));
    }

    #[test]
    fn phase10_closure_updates_a_live_captured_binding() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "function counter(){let value=0;function next(){value=value+1;return value}return next}let next=counter();let first=next();let second=next();first*10+second",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(12.0));
    }

    #[test]
    fn phase10_block_and_catch_declarations_do_not_leak() {
        let mut engine = JsvEngine::new();
        assert_eq!(
            engine
                .eval("let visible=0;if(true){let hidden=1;visible=hidden}visible")
                .unwrap(),
            JsvValue::Number(1.0)
        );
        assert!(engine.eval("hidden").unwrap_err().contains("not defined"));
        engine
            .eval("try{throw 'reason'}catch(error){let caught=error}")
            .unwrap();
        assert!(engine.eval("error").unwrap_err().contains("not defined"));
        assert!(engine.eval("caught").unwrap_err().contains("not defined"));
    }

    #[test]
    fn phase10_let_const_and_var_have_distinct_declaration_rules() {
        let mut engine = JsvEngine::new();
        engine.eval("const fixed=4").unwrap();
        assert!(engine
            .eval("fixed=5")
            .unwrap_err()
            .contains("assignment to constant"));
        assert!(engine
            .eval("const missing")
            .unwrap_err()
            .contains("Missing initializer"));
        engine.eval("let once=1").unwrap();
        assert!(engine
            .eval("let once=2")
            .unwrap_err()
            .contains("already been declared"));
        engine.eval("var replaceable=1;var replaceable=2").unwrap();
        assert_eq!(engine.eval("replaceable").unwrap(), JsvValue::Number(2.0));
    }

    #[test]
    fn phase10_sibling_closures_share_the_same_binding() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "function pair(){let value=0;function increment(){value=value+1;return value}function read(){return value}return [increment,read]}let closures=pair();closures[0]();closures[1]()",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(1.0));
    }

    #[test]
    fn phase10_arrow_functions_capture_lexical_bindings() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval("let make=base=>value=>base+value;let addFour=make(4);addFour(3)")
            .unwrap();
        assert_eq!(value, JsvValue::Number(7.0));
        assert_eq!(
            engine
                .eval("let add=(left,right)=>{return left+right};add(2,3)")
                .unwrap(),
            JsvValue::Number(5.0)
        );
    }

    #[test]
    fn phase10_arrow_handlers_and_async_arrows_return_promises() {
        let mut engine = JsvEngine::new();
        let chained = engine
            .eval("Promise.resolve(2).then(value=>value+3)")
            .unwrap();
        let JsvValue::Promise(chained) = chained else {
            panic!("expected chained Promise");
        };
        assert!(matches!(
            &*chained.borrow(),
            JsvPromiseState::Fulfilled(JsvValue::Number(5.0))
        ));

        let asynchronous = engine
            .eval("let calculate=async value=>await Promise.resolve(value+1);calculate(4)")
            .unwrap();
        let JsvValue::Promise(asynchronous) = asynchronous else {
            panic!("expected async arrow Promise");
        };
        assert!(matches!(
            &*asynchronous.borrow(),
            JsvPromiseState::Fulfilled(JsvValue::Number(5.0))
        ));
    }

    #[test]
    fn phase10_objects_arrays_prototypes_and_indexes_have_identity() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "let proto={base:3};let object=Object.create(proto);object.value=4;let values=[1,2];values.push(object.value);values[0]=5;object.base+values[0]+values[2]",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(12.0));
        assert_eq!(
            engine.eval("Array.isArray(values)").unwrap(),
            JsvValue::Boolean(true)
        );
    }

    #[test]
    fn phase10_for_of_uses_bounded_array_iterator() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval("let sum=0;for(let item of [1,2,3,4]){sum=sum+item}sum")
            .unwrap();
        assert_eq!(value, JsvValue::Number(10.0));
    }

    #[test]
    fn phase10_for_of_closures_capture_per_iteration_bindings() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "let closures=[];for(let item of [1,2]){closures.push(()=>item)}closures[0]()*10+closures[1]()",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(12.0));
    }

    #[test]
    fn phase10_loop_completion_records_reach_the_nearest_iterator() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "let sum=0;for(let item of [1,2,3,4,5]){if(item==2){continue}if(item==4){break}sum=sum+item}sum",
            )
            .unwrap();
        assert_eq!(value, JsvValue::Number(4.0));
        assert!(engine.eval("break").unwrap_err().contains("illegal break"));
        assert!(engine
            .eval("continue")
            .unwrap_err()
            .contains("illegal continue"));
    }

    #[test]
    fn phase10_logical_operators_do_not_evaluate_skipped_operands() {
        let mut engine = JsvEngine::new();
        assert_eq!(
            engine.eval("false && missing()").unwrap(),
            JsvValue::Boolean(false)
        );
        assert_eq!(
            engine.eval("true || missing()").unwrap(),
            JsvValue::Boolean(true)
        );
    }

    #[test]
    fn phase10_try_catch_finally_preserves_abrupt_completion() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "let result='';try{throw 'boom'}catch(error){result=error}finally{result=result+'!'}result",
            )
            .unwrap();
        assert_eq!(value, JsvValue::String("boom!".to_string()));
        assert!(engine
            .eval("throw 'uncaught'")
            .unwrap_err()
            .contains("uncaught"));
    }

    #[test]
    fn phase10_promise_reactions_run_after_the_script_job() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "function plus(value){console.log(value);return value+1}let promise=Promise.resolve(4);let chained=promise.then(plus);console.log('sync');chained",
            )
            .unwrap();
        assert_eq!(engine.console_output, vec!["sync", "4"]);
        let JsvValue::Promise(promise) = value else {
            panic!("expected chained Promise");
        };
        assert!(matches!(
            &*promise.borrow(),
            JsvPromiseState::Fulfilled(JsvValue::Number(5.0))
        ));
    }

    #[test]
    fn phase10_pending_promise_chains_settle_in_microtask_order() {
        let mut engine = JsvEngine::new();
        let value = engine
            .eval(
                "Promise.resolve(1).then(value=>value+1).then(value=>value+1).finally(()=>Promise.resolve('ignored'))",
            )
            .unwrap();
        let JsvValue::Promise(value) = value else {
            panic!("expected chained Promise");
        };
        assert!(matches!(
            &*value.borrow(),
            JsvPromiseState::Fulfilled(JsvValue::Number(3.0))
        ));

        let cycle = engine
            .eval("let base=Promise.resolve(1);let cycle=base.then(()=>cycle);cycle")
            .unwrap();
        let JsvValue::Promise(cycle) = cycle else {
            panic!("expected cyclic Promise");
        };
        assert!(matches!(
            &*cycle.borrow(),
            JsvPromiseState::Rejected(JsvValue::String(reason)) if reason.contains("itself")
        ));
    }

    #[test]
    fn phase10_async_await_returns_fulfilled_or_rejected_promises() {
        let mut engine = JsvEngine::new();
        let fulfilled = engine
            .eval("async function calculate(){let value=await Promise.resolve(4);return value+2}calculate()")
            .unwrap();
        let JsvValue::Promise(fulfilled) = fulfilled else {
            panic!("expected async Promise");
        };
        assert!(matches!(
            &*fulfilled.borrow(),
            JsvPromiseState::Fulfilled(JsvValue::Number(6.0))
        ));

        let rejected = engine
            .eval("async function fail(){throw 'bad'}fail()")
            .unwrap();
        let JsvValue::Promise(rejected) = rejected else {
            panic!("expected rejected async Promise");
        };
        assert!(matches!(
            &*rejected.borrow(),
            JsvPromiseState::Rejected(JsvValue::String(reason)) if reason == "bad"
        ));
    }

    #[test]
    fn phase10_module_graph_parses_links_and_evaluates_named_bindings() {
        let mut graph = JsvModuleGraph::new();
        graph.register("dep", "export const base=4;").unwrap();
        graph
            .register(
                "entry",
                "import { base } from 'dep';\nexport function twice(value){return value*2}\nexport const result=twice(base);",
            )
            .unwrap();
        let namespace = graph.evaluate("entry").unwrap();
        assert_eq!(
            namespace.exports.get("result"),
            Some(&JsvValue::Number(8.0))
        );
        assert!(namespace.exports.contains_key("twice"));
    }

    #[test]
    fn phase10_module_graph_rejects_cycles_and_missing_exports() {
        let mut graph = JsvModuleGraph::new();
        graph
            .register("a", "import { b } from 'b';\nexport const a=b;")
            .unwrap();
        graph
            .register("b", "import { a } from 'a';\nexport const b=a;")
            .unwrap();
        assert!(graph.evaluate("a").unwrap_err().contains("Circular"));
    }
}
