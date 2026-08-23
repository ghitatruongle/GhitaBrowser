//! Bounded clean-room WebAssembly MVP interpreter (WebAssembly
//! Extension). Executes the instruction subset validated by `crate::wasm`.
//! Every runtime failure (type mismatch, out-of-bounds, budget) fails
//! closed with an explicit error; the interpreter never panics on input.

use crate::wasm::{FuncType, ValueType, WasmModule};

/// Runtime value on the operand stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WasmValue {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

/// Hard step budget per function invocation.
pub const MAX_INTERPRETER_STEPS: u64 = 1_000_000;

pub const MAX_MEMORY_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CALL_DEPTH_WASM: usize = 512;
pub const MAX_STACK_VALUES: usize = 10_000;
pub const MAX_LABELS: usize = 1_024;

const PAGE_SIZE: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlKind {
    Block,
    Loop,
    If,
}

#[derive(Debug, Clone)]
struct ControlFrame {
    kind: ControlKind,
    /// For `if`: pc just past the `else` opcode (or the `end` when no else).
    else_pc: usize,
    /// pc of the matching `end` opcode (branch target for non-loops).
    end_pc: usize,
    /// Operand stack height at block entry.
    stack_height: usize,
    /// Number of result values the block produces (0 or 1 in this profile).
    result_count: usize,
}

struct Frame {
    function_index: u32,
    pc: usize,
    locals: Vec<WasmValue>,
    stack: Vec<WasmValue>,
    controls: Vec<ControlFrame>,
    /// Operand-stack height at function entry (kept for call boundaries).
    stack_height: usize,
    steps: u64,
}

/// A bound instance of a validated module. Memory is capped at 16 MiB;
/// globals and the function table are initialized at instantiation.
#[derive(Debug)]
pub struct WasmInstance {
    module: WasmModule,
    globals: Vec<WasmValue>,
    memory: Vec<u8>,
    table: Vec<Option<u32>>,
}

/// Read-only views for the host bindings.
impl WasmInstance {
    pub fn memory(&self) -> &[u8] {
        &self.memory
    }

    pub fn memory_mut(&mut self) -> &mut [u8] {
        &mut self.memory
    }

    pub fn memory_bytes(&self) -> usize {
        self.memory.len()
    }

    pub fn table(&self) -> &[Option<u32>] {
        &self.table
    }

    pub fn function_type(&self, function_index: u32) -> Option<&FuncType> {
        let type_index = self
            .module
            .function_type_indices
            .get(function_index as usize)?;
        self.module.types.get(*type_index as usize)
    }

    pub fn exported_function(&self, name: &str) -> Option<u32> {
        self.module.exports.iter().find_map(|export| {
            (export.name == name && matches!(export.kind, crate::wasm::ExportKind::Function))
                .then_some(export.index)
        })
    }
}

impl WasmInstance {
    pub fn instantiate(module: WasmModule) -> Result<Self, String> {
        let memory_pages = module
            .memories
            .first()
            .map(|memory| memory.limits.min)
            .unwrap_or(0);
        let memory_bytes = (memory_pages as usize)
            .checked_mul(PAGE_SIZE)
            .ok_or_else(|| "Memory size overflow".to_string())?;
        if memory_bytes > MAX_MEMORY_BYTES {
            return Err("Memory exceeds 16 MiB".to_string());
        }
        let mut table = Vec::new();
        if let Some(table_type) = module.tables.first() {
            let entries = table_type.limits.min as usize;
            if entries > crate::wasm::MAX_TABLE_ENTRIES {
                return Err("Table exceeds budget".to_string());
            }
            table = vec![None; entries];
        }
        let mut instance = Self {
            module,
            globals: Vec::new(),
            memory: vec![0; memory_bytes],
            table,
        };
        // Evaluate global initializers (i32/i64/f32/f64.const + global.get).
        for index in 0..instance.module.globals.len() {
            let value = instance.eval_const_expr(&instance.module.globals[index].init)?;
            instance.globals.push(value);
        }
        // Element segments: table[i] = function index.
        for segment in &instance.module.elements.clone() {
            let base = instance.eval_const_expr(&segment.offset)?;
            let base = match base {
                WasmValue::I32(value) if value >= 0 => value as usize,
                _ => return Err("Element offset must be a non-negative i32".to_string()),
            };
            for (offset, function_index) in segment.function_indices.iter().enumerate() {
                if function_index >= &instance.module.total_functions {
                    return Err("Element function index out of range".to_string());
                }
                let slot = base + offset;
                if slot >= instance.table.len() {
                    return Err("Element segment exceeds table size".to_string());
                }
                instance.table[slot] = Some(*function_index);
            }
        }
        // Data segments into memory.
        for segment in &instance.module.data.clone() {
            let base = instance.eval_const_expr(&segment.offset)?;
            let base = match base {
                WasmValue::I32(value) if value >= 0 => value as usize,
                _ => return Err("Data offset must be a non-negative i32".to_string()),
            };
            let end = base
                .checked_add(segment.bytes.len())
                .ok_or_else(|| "Data segment offset overflow".to_string())?;
            if end > instance.memory.len() {
                return Err("Data segment exceeds memory".to_string());
            }
            instance.memory[base..end].copy_from_slice(&segment.bytes);
        }
        Ok(instance)
    }

    fn eval_const_expr(&self, expr: &[u8]) -> Result<WasmValue, String> {
        let mut cursor = crate::wasm::Cursor::new(expr);
        let mut value = None;
        while !cursor.is_empty() {
            let opcode = cursor.read_u8()?;
            match opcode {
                0x41 => value = Some(WasmValue::I32(cursor.read_leb_i32()?)),
                0x42 => value = Some(WasmValue::I64(cursor.read_leb_i64()?)),
                0x43 => {
                    let bytes = cursor.read_bytes(4)?;
                    value = Some(WasmValue::F32(f32::from_le_bytes(
                        bytes.try_into().expect("4 bytes"),
                    )));
                }
                0x44 => {
                    let bytes = cursor.read_bytes(8)?;
                    value = Some(WasmValue::F64(f64::from_le_bytes(
                        bytes.try_into().expect("8 bytes"),
                    )));
                }
                0x23 => {
                    let index = cursor.read_leb_u32()? as usize;
                    value = Some(
                        *self
                            .globals
                            .get(index)
                            .ok_or_else(|| "Global index out of range".to_string())?,
                    );
                }
                0x0B => break,
                other => return Err(format!("Unsupported const opcode 0x{other:02X}")),
            }
        }
        value.ok_or_else(|| "Constant expression produced no value".to_string())
    }

    /// Invoke an exported (defined) function by index with arguments.
    pub fn invoke(
        &mut self,
        function_index: u32,
        args: &[WasmValue],
    ) -> Result<Vec<WasmValue>, String> {
        let type_index = self
            .module
            .function_type_indices
            .get(function_index as usize)
            .copied()
            .ok_or_else(|| "Function index out of range or is an import".to_string())?;
        let func_type = self
            .module
            .types
            .get(type_index as usize)
            .cloned()
            .ok_or_else(|| "Function type index out of range".to_string())?;
        if args.len() != func_type.parameters.len() {
            return Err("Argument count does not match function type".to_string());
        }
        if self.module.bodies.get(function_index as usize).is_none() {
            return Err(
                "Cannot invoke an imported function (host imports unsupported)".to_string(),
            );
        }
        let mut frames: Vec<Frame> = Vec::new();
        let results = self.run_function(function_index, args, &mut frames)?;
        if results.len() != func_type.results.len() {
            return Err("Result count does not match function type".to_string());
        }
        Ok(results)
    }

    fn run_function(
        &mut self,
        function_index: u32,
        args: &[WasmValue],
        frames: &mut Vec<Frame>,
    ) -> Result<Vec<WasmValue>, String> {
        if frames.len() >= MAX_CALL_DEPTH_WASM {
            return Err("WASM call depth exceeded".to_string());
        }
        let body = self
            .module
            .bodies
            .get(function_index as usize)
            .ok_or_else(|| "Function has no body (import)".to_string())?
            .clone();
        let mut locals = args.to_vec();
        for decl in &body.locals {
            for _ in 0..decl.count {
                locals.push(match decl.value_type {
                    ValueType::I32 => WasmValue::I32(0),
                    ValueType::I64 => WasmValue::I64(0),
                    ValueType::F32 => WasmValue::F32(0.0),
                    ValueType::F64 => WasmValue::F64(0.0),
                    _ => return Err("Non-numeric local type".to_string()),
                });
            }
        }
        frames.push(Frame {
            function_index,
            pc: 0,
            locals,
            stack: Vec::new(),
            controls: Vec::new(),
            stack_height: 0,
            steps: 0,
        });
        self.execute_frame(frames)
    }

    fn execute_frame(&mut self, frames: &mut Vec<Frame>) -> Result<Vec<WasmValue>, String> {
        loop {
            let frame = frames.last_mut().expect("frame exists");
            frame.steps += 1;
            if frame.steps > MAX_INTERPRETER_STEPS {
                return Err("WASM step budget exceeded".to_string());
            }
            if frame.stack.len() > MAX_STACK_VALUES {
                return Err("WASM operand stack budget exceeded".to_string());
            }
            let code = self.module.bodies[frame.function_index as usize]
                .code
                .clone();
            if frame.pc >= code.len() {
                return Err("Function ran past its body".to_string());
            }
            let opcode = code[frame.pc];
            frame.pc += 1;
            if opcode == 0x0B {
                // end: close the innermost control frame, or finish the function.
                let frame = frames.last_mut().expect("frame exists");
                if frame.controls.is_empty() {
                    let results = frame.stack[frame.stack_height..].to_vec();
                    frames.pop();
                    return Ok(results);
                }
                let control = frame.controls.pop().expect("control exists");
                let base = control.stack_height;
                let above = frame.stack.split_off(base);
                let result_count = control.result_count.min(above.len());
                let keep = above[above.len() - result_count..].to_vec();
                frame.stack.extend(keep);
                // Reaching a loop's `end` by fall-through EXITS the loop per
                // spec; branching back to the head is exclusively what `br`
                // to a loop label does. The old re-entry here turned every
                // naturally-exiting loop into an infinite one.
                continue;
            }
            let action = self.exec_instruction(opcode, &code, frames)?;
            match action {
                Action::Continue => {}
                Action::Branch(label) => {
                    let frame = frames.last_mut().expect("frame exists");
                    let control_index = frame.controls.len().saturating_sub(1 + label as usize);
                    if control_index >= frame.controls.len() {
                        return Err("br label out of range".to_string());
                    }
                    let control = frame.controls[control_index].clone();
                    let is_loop = control.kind == ControlKind::Loop;
                    let target = if is_loop {
                        control.else_pc
                    } else {
                        control.end_pc
                    };
                    // A branch keeps the target's result arity on the stack
                    // (spec §control); truncating everything above
                    // stack_height destroyed block result values.
                    let keep_count = control
                        .result_count
                        .min(frame.stack.len().saturating_sub(control.stack_height));
                    let keep: Vec<_> = frame.stack[frame.stack.len() - keep_count..].to_vec();
                    frame.stack.truncate(control.stack_height);
                    frame.stack.extend(keep);
                    // Remove controls above the target; the target itself is
                    // left for the end handler to close (or returned for the
                    // function-level branch).
                    frame.controls.truncate(control_index + 1);
                    if control_index == 0 && !is_loop {
                        // Branch to the function's outermost block: return.
                        let results = frame.stack[frame.stack_height..].to_vec();
                        frames.pop();
                        return Ok(results);
                    }
                    frame.pc = target;
                }
                Action::Call(index) => {
                    let param_count = self
                        .function_type(index)
                        .ok_or_else(|| "Function index out of range".to_string())?
                        .parameters
                        .len();
                    let frame = frames.last_mut().expect("frame exists");
                    if frame.stack.len() < param_count {
                        return Err("call missing arguments on stack".to_string());
                    }
                    let split_at = frame.stack.len() - param_count;
                    let callee_args = frame.stack.split_off(split_at);
                    let callee_results = self.run_function(index, &callee_args, frames)?;
                    frames
                        .last_mut()
                        .expect("frame exists")
                        .stack
                        .extend(callee_results);
                }
                Action::CallIndirect(type_index) => {
                    let frame = frames.last_mut().expect("frame exists");
                    let table_value = frame
                        .stack
                        .pop()
                        .ok_or_else(|| "call_indirect missing table index".to_string())?;
                    let WasmValue::I32(index) = table_value else {
                        return Err("call_indirect table index must be i32".to_string());
                    };
                    if index < 0 {
                        return Err("call_indirect negative index".to_string());
                    }
                    let entry = self
                        .table
                        .get(index as usize)
                        .and_then(|entry| *entry)
                        .ok_or_else(|| "call_indirect null or out of range".to_string())?;
                    let callee_type_index = self
                        .module
                        .function_type_indices
                        .get(entry as usize)
                        .copied()
                        .ok_or_else(|| "call_indirect target is an import".to_string())?;
                    if callee_type_index != type_index {
                        return Err("call_indirect type mismatch".to_string());
                    }
                    let func_type = self
                        .module
                        .types
                        .get(type_index as usize)
                        .ok_or_else(|| "call_indirect type index out of range".to_string())?;
                    let param_count = func_type.parameters.len();
                    if frame.stack.len() < param_count {
                        return Err("call_indirect missing arguments on stack".to_string());
                    }
                    let split_at = frame.stack.len() - param_count;
                    let callee_args = frame.stack.split_off(split_at);
                    let callee_results = self.run_function(entry, &callee_args, frames)?;
                    frames
                        .last_mut()
                        .expect("frame exists")
                        .stack
                        .extend(callee_results);
                }
                Action::Return => {
                    let frame = frames.last().expect("frame exists");
                    let results = frame.stack[frame.stack_height..].to_vec();
                    frames.pop();
                    return Ok(results);
                }
            }
        }
    }

    /// Execute one instruction. Returns an action for control flow; numeric
    /// instructions mutate the operand stack directly.
    fn exec_instruction(
        &mut self,
        opcode: u8,
        code: &[u8],
        frames: &mut [Frame],
    ) -> Result<Action, String> {
        let read_leb_u32 = |frame: &mut Frame| -> Result<u32, String> {
            let mut result = 0u32;
            let mut shift = 0u32;
            for _ in 0..5 {
                let byte = *code
                    .get(frame.pc)
                    .ok_or_else(|| "Instruction immediate out of range".to_string())?;
                frame.pc += 1;
                result |= u32::from(byte & 0x7F) << shift;
                if byte & 0x80 == 0 {
                    return Ok(result);
                }
                shift += 7;
            }
            Err("Instruction immediate too long".to_string())
        };
        let read_leb_i32 = |frame: &mut Frame| -> Result<i32, String> {
            let mut result = 0i32;
            let mut shift = 0u32;
            let mut byte;
            loop {
                byte = *code
                    .get(frame.pc)
                    .ok_or_else(|| "Instruction immediate out of range".to_string())?;
                frame.pc += 1;
                result |= i32::from(byte & 0x7F) << shift;
                shift += 7;
                if byte & 0x80 == 0 {
                    break;
                }
                if shift >= 35 {
                    return Err("Instruction immediate too long".to_string());
                }
            }
            if shift < 32 && byte & 0x40 != 0 {
                result |= -1i32 << shift;
            }
            Ok(result)
        };
        let read_leb_i64 = |frame: &mut Frame| -> Result<i64, String> {
            let mut result = 0i64;
            let mut shift = 0u32;
            let mut byte;
            loop {
                byte = *code
                    .get(frame.pc)
                    .ok_or_else(|| "Instruction immediate out of range".to_string())?;
                frame.pc += 1;
                result |= i64::from(byte & 0x7F) << shift;
                shift += 7;
                if byte & 0x80 == 0 {
                    break;
                }
                if shift >= 70 {
                    return Err("Instruction immediate too long".to_string());
                }
            }
            if shift < 64 && byte & 0x40 != 0 {
                result |= -1i64 << shift;
            }
            Ok(result)
        };

        let frame = frames.last_mut().expect("frame exists");
        match opcode {
            // ---- Control ----
            0x02..=0x04 => {
                // block / loop / if: read block type (validated structurally).
                let block_type = *code
                    .get(frame.pc)
                    .ok_or_else(|| "Missing block type".to_string())?;
                frame.pc += 1;
                if !matches!(block_type, 0x40 | 0x7F | 0x7E | 0x7D | 0x7C) {
                    // Type-index block type: consume the LEB.
                    let mut tmp = 0u32;
                    let mut shift = 0u32;
                    loop {
                        let byte = *code
                            .get(frame.pc)
                            .ok_or_else(|| "Block type index out of range".to_string())?;
                        frame.pc += 1;
                        tmp |= u32::from(byte & 0x7F) << shift;
                        if byte & 0x80 == 0 {
                            break;
                        }
                        shift += 7;
                    }
                    let _ = tmp;
                }
                let kind = match opcode {
                    0x02 => ControlKind::Block,
                    0x03 => ControlKind::Loop,
                    _ => ControlKind::If,
                };
                let result_count = match block_type {
                    0x40 => 0,
                    0x7C..=0x7F => 1,
                    type_index => {
                        let types = &self.module.types;
                        let type_index = type_index as usize;
                        if type_index >= types.len() {
                            return Err("Block type index out of range".to_string());
                        }
                        types[type_index].results.len().min(1)
                    }
                };
                if kind == ControlKind::If {
                    let cond = frame
                        .stack
                        .pop()
                        .ok_or_else(|| "if missing condition".to_string())?;
                    let WasmValue::I32(condition) = cond else {
                        return Err("if condition must be i32".to_string());
                    };
                    if condition == 0 {
                        // False if: jump to the else body (if present) or the
                        // end opcode; the end handler closes the frame.
                        let else_pc = find_else_or_end(code, frame.pc)?;
                        let end_pc = find_end(code, frame.pc)?;
                        frame.pc = if else_pc < end_pc { else_pc } else { end_pc };
                        frame.controls.push(ControlFrame {
                            kind,
                            else_pc,
                            end_pc,
                            stack_height: frame.stack.len(),
                            result_count,
                        });
                        return Ok(Action::Continue);
                    }
                }
                // True if / block / loop: push the frame and continue.
                let end_pc = find_end(code, frame.pc)?;
                frame.controls.push(ControlFrame {
                    kind,
                    else_pc: frame.pc,
                    end_pc,
                    stack_height: frame.stack.len(),
                    result_count,
                });
                Ok(Action::Continue)
            }
            0x05 => {
                // else: jump to the matching end; the end handler closes the
                // if frame and keeps its result values.
                let end_pc = find_end(code, frame.pc)?;
                frame.pc = end_pc;
                Ok(Action::Continue)
            }
            0x0C | 0x0D => {
                let label = read_leb_u32(frame)?;
                if label as usize >= frame.controls.len().saturating_add(1) {
                    return Err("br label out of range".to_string());
                }
                if opcode == 0x0D {
                    let condition = frame
                        .stack
                        .pop()
                        .ok_or_else(|| "br_if missing condition".to_string())?;
                    let WasmValue::I32(condition) = condition else {
                        return Err("br_if condition must be i32".to_string());
                    };
                    if condition == 0 {
                        return Ok(Action::Continue);
                    }
                }
                Ok(Action::Branch(label))
            }
            0x0E => {
                let count = read_leb_u32(frame)?;
                if count > MAX_LABELS as u32 {
                    return Err("br_table label budget exceeded".to_string());
                }
                let mut labels = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    labels.push(read_leb_u32(frame)?);
                }
                let default_label = read_leb_u32(frame)?;
                let index = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "br_table missing index".to_string())?;
                let WasmValue::I32(index) = index else {
                    return Err("br_table index must be i32".to_string());
                };
                let label = if index >= 0 && (index as usize) < labels.len() {
                    labels[index as usize]
                } else {
                    default_label
                };
                Ok(Action::Branch(label))
            }
            0x0F => Ok(Action::Return),
            0x10 => {
                let index = read_leb_u32(frame)?;
                Ok(Action::Call(index))
            }
            0x11 => {
                let type_index = read_leb_u32(frame)?;
                // Reserved byte must be zero in MVP.
                let reserved = *code
                    .get(frame.pc)
                    .ok_or_else(|| "call_indirect reserved byte missing".to_string())?;
                frame.pc += 1;
                if reserved != 0 {
                    return Err("call_indirect reserved byte must be zero".to_string());
                }
                Ok(Action::CallIndirect(type_index))
            }
            0x12 => {
                let index = read_leb_u32(frame)?;
                Ok(Action::Call(index))
            }
            0x13 => {
                let index = read_leb_u32(frame)?;
                Ok(Action::CallIndirect(index))
            }
            // ---- Parametric ----
            0x1A => {
                frame.stack.pop().ok_or("drop on empty stack")?;
                Ok(Action::Continue)
            }
            0x1B => {
                let frame = frames.last_mut().expect("frame exists");
                let condition = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "select missing condition".to_string())?;
                let WasmValue::I32(condition) = condition else {
                    return Err("select condition must be i32".to_string());
                };
                let second = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "select missing operand".to_string())?;
                let first = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "select missing operand".to_string())?;
                if std::mem::discriminant(&first) != std::mem::discriminant(&second) {
                    return Err("select operand type mismatch".to_string());
                }
                frame
                    .stack
                    .push(if condition != 0 { first } else { second });
                Ok(Action::Continue)
            }
            // ---- Variable ----
            0x20 => {
                let index = read_leb_u32(frame)?;
                let value = *frame
                    .locals
                    .get(index as usize)
                    .ok_or_else(|| "local.get index out of range".to_string())?;
                frame.stack.push(value);
                Ok(Action::Continue)
            }
            0x21 => {
                let index = read_leb_u32(frame)?;
                let value = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "local.set on empty stack".to_string())?;
                let slot = frame
                    .locals
                    .get_mut(index as usize)
                    .ok_or_else(|| "local.set index out of range".to_string())?;
                *slot = value;
                Ok(Action::Continue)
            }
            0x22 => {
                let index = read_leb_u32(frame)?;
                let value = *frame
                    .stack
                    .last()
                    .ok_or_else(|| "local.tee on empty stack".to_string())?;
                let slot = frame
                    .locals
                    .get_mut(index as usize)
                    .ok_or_else(|| "local.tee index out of range".to_string())?;
                *slot = value;
                Ok(Action::Continue)
            }
            0x23 => {
                let index = read_leb_u32(frame)?;
                let value = *self
                    .globals
                    .get(index as usize)
                    .ok_or_else(|| "global.get index out of range".to_string())?;
                frame.stack.push(value);
                Ok(Action::Continue)
            }
            0x24 => {
                let index = read_leb_u32(frame)?;
                let value = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "global.set on empty stack".to_string())?;
                let slot = self
                    .globals
                    .get_mut(index as usize)
                    .ok_or_else(|| "global.set index out of range".to_string())?;
                *slot = value;
                Ok(Action::Continue)
            }
            // ---- Memory loads/stores ----
            0x28..=0x35 => {
                let align = read_leb_u32(frame)?;
                let offset = read_leb_u32(frame)?;
                let _ = align;
                let address = self.effective_address(frame, offset as usize)?;
                let frame = frames.last_mut().expect("frame exists");
                let value = self.memory_load(address, opcode)?;
                frame.stack.push(value);
                Ok(Action::Continue)
            }
            0x36..=0x3E => {
                let align = read_leb_u32(frame)?;
                let offset = read_leb_u32(frame)?;
                let _ = align;
                let frame = frames.last_mut().expect("frame exists");
                let value = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "store on empty stack".to_string())?;
                let address = self.effective_address(frame, offset as usize)?;
                self.memory_store(address, opcode, value)?;
                Ok(Action::Continue)
            }
            0x3F => {
                let reserved = *code
                    .get(frame.pc)
                    .ok_or_else(|| "memory.size reserved byte missing".to_string())?;
                frame.pc += 1;
                if reserved != 0 {
                    return Err("memory.size reserved byte must be zero".to_string());
                }
                let pages = (self.memory.len() / PAGE_SIZE) as i32;
                frame.stack.push(WasmValue::I32(pages));
                Ok(Action::Continue)
            }
            0x40 => {
                let reserved = *code
                    .get(frame.pc)
                    .ok_or_else(|| "memory.grow reserved byte missing".to_string())?;
                frame.pc += 1;
                if reserved != 0 {
                    return Err("memory.grow reserved byte must be zero".to_string());
                }
                let delta = frame
                    .stack
                    .pop()
                    .ok_or_else(|| "memory.grow missing delta".to_string())?;
                let WasmValue::I32(delta) = delta else {
                    return Err("memory.grow delta must be i32".to_string());
                };
                let old_pages = (self.memory.len() / PAGE_SIZE) as i32;
                if delta < 0 {
                    frame.stack.push(WasmValue::I32(-1));
                    return Ok(Action::Continue);
                }
                let new_bytes = self
                    .memory
                    .len()
                    .checked_add((delta as usize).saturating_mul(PAGE_SIZE))
                    .ok_or_else(|| "memory.grow overflow".to_string())?;
                if new_bytes > MAX_MEMORY_BYTES {
                    frame.stack.push(WasmValue::I32(-1));
                    return Ok(Action::Continue);
                }
                self.memory.resize(new_bytes, 0);
                frame.stack.push(WasmValue::I32(old_pages));
                Ok(Action::Continue)
            }
            // ---- Constants ----
            0x41 => {
                let value = read_leb_i32(frame)?;
                frame.stack.push(WasmValue::I32(value));
                Ok(Action::Continue)
            }
            0x42 => {
                let value = read_leb_i64(frame)?;
                frame.stack.push(WasmValue::I64(value));
                Ok(Action::Continue)
            }
            0x43 => {
                let bytes = code
                    .get(frame.pc..frame.pc + 4)
                    .ok_or_else(|| "f32.const missing bytes".to_string())?;
                frame.pc += 4;
                frame.stack.push(WasmValue::F32(f32::from_le_bytes(
                    bytes.try_into().expect("4"),
                )));
                Ok(Action::Continue)
            }
            0x44 => {
                let bytes = code
                    .get(frame.pc..frame.pc + 8)
                    .ok_or_else(|| "f64.const missing bytes".to_string())?;
                frame.pc += 8;
                frame.stack.push(WasmValue::F64(f64::from_le_bytes(
                    bytes.try_into().expect("8"),
                )));
                Ok(Action::Continue)
            }
            // ---- Numeric: comparison and arithmetic (pop/push on stack) ----
            0x45..=0xC4 => {
                self.exec_numeric(opcode, frames)?;
                Ok(Action::Continue)
            }
            other => Err(format!("Unsupported opcode 0x{other:02X}")),
        }
    }

    fn effective_address(&self, frame: &mut Frame, offset: usize) -> Result<usize, String> {
        let base = frame
            .stack
            .pop()
            .ok_or_else(|| "memory access missing address".to_string())?;
        let WasmValue::I32(base) = base else {
            return Err("memory address must be i32".to_string());
        };
        if base < 0 {
            return Err("negative memory address".to_string());
        }
        (base as usize)
            .checked_add(offset)
            .ok_or_else(|| "memory address overflow".to_string())
    }

    fn memory_load(&self, address: usize, opcode: u8) -> Result<WasmValue, String> {
        let read = |size: usize| -> Result<u64, String> {
            let end = address
                .checked_add(size)
                .ok_or_else(|| "memory access out of range".to_string())?;
            let slice = self
                .memory
                .get(address..end)
                .ok_or_else(|| "memory access out of range".to_string())?;
            let mut bytes = [0u8; 8];
            bytes[..size].copy_from_slice(slice);
            Ok(u64::from_le_bytes(bytes))
        };
        match opcode {
            0x28 => Ok(WasmValue::I32(read(4)? as u32 as i32)),
            0x29 => Ok(WasmValue::I64(read(8)? as i64)),
            0x2A => Ok(WasmValue::F32(f32::from_bits(read(4)? as u32))),
            0x2B => Ok(WasmValue::F64(f64::from_bits(read(8)?))),
            0x2C => Ok(WasmValue::I32(read(1)? as u8 as i32)),
            0x2D => Ok(WasmValue::I32(read(2)? as u16 as i32)),
            0x2E => Ok(WasmValue::I32(read(1)? as u8 as i8 as i32)),
            0x2F => Ok(WasmValue::I32(read(2)? as u16 as i16 as i32)),
            0x30 => Ok(WasmValue::I64(read(1)? as u8 as i64)),
            0x31 => Ok(WasmValue::I64(read(2)? as u16 as i64)),
            0x32 => Ok(WasmValue::I64(read(4)? as u32 as i64)),
            0x33 => Ok(WasmValue::I64(read(1)? as u8 as i8 as i64)),
            0x34 => Ok(WasmValue::I64(read(2)? as u16 as i16 as i64)),
            0x35 => Ok(WasmValue::I64(read(4)? as u32 as i32 as i64)),
            other => Err(format!("Unknown load opcode 0x{other:02X}")),
        }
    }

    fn memory_store(&mut self, address: usize, opcode: u8, value: WasmValue) -> Result<(), String> {
        let (size, bytes) = match (opcode, value) {
            (0x36, WasmValue::I32(v)) => (4, (v as u32 as u64).to_le_bytes()),
            (0x37, WasmValue::I64(v)) => (8, (v as u64).to_le_bytes()),
            (0x38, WasmValue::F32(v)) => (4, (v.to_bits() as u64).to_le_bytes()),
            (0x39, WasmValue::F64(v)) => (8, v.to_bits().to_le_bytes()),
            (0x3A, WasmValue::I32(v)) => (1, ((v as u8) as u64).to_le_bytes()),
            (0x3B, WasmValue::I32(v)) => (2, ((v as u16) as u64).to_le_bytes()),
            (0x3C, WasmValue::I64(v)) => (1, ((v as u8) as u64).to_le_bytes()),
            (0x3D, WasmValue::I64(v)) => (2, ((v as u16) as u64).to_le_bytes()),
            (0x3E, WasmValue::I64(v)) => (4, ((v as u32) as u64).to_le_bytes()),
            (opcode, value) => {
                return Err(format!(
                    "Store opcode 0x{opcode:02X} with incompatible value {:?}",
                    value
                ))
            }
        };
        let end = address
            .checked_add(size)
            .ok_or_else(|| "memory access out of range".to_string())?;
        let slice = self
            .memory
            .get_mut(address..end)
            .ok_or_else(|| "memory access out of range".to_string())?;
        slice.copy_from_slice(&bytes[..size]);
        Ok(())
    }

    fn exec_numeric(&mut self, opcode: u8, frames: &mut [Frame]) -> Result<(), String> {
        let frame = frames.last_mut().expect("frame exists");
        let mut pop = |frame: &mut Frame| -> Result<WasmValue, String> {
            frame
                .stack
                .pop()
                .ok_or_else(|| "numeric op on empty stack".to_string())
        };
        let push = |frame: &mut Frame, value: WasmValue| {
            frame.stack.push(value);
        };
        match opcode {
            // i32 comparisons
            0x45 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a == b)));
            }
            0x46 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a != b)));
            }
            0x47 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x48 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x49 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x4A => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }
            0x4B => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x4C => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x4D => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x4E => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }
            // i64 comparisons
            0x50 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a == b)));
            }
            0x51 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a != b)));
            }
            0x52 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x53 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x54 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x55 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }
            0x56 => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x57 => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x58 => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x59 => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }
            // f32 comparisons
            0x5B => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a == b)));
            }
            0x5C => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a != b)));
            }
            0x5D => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x5E => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x5F => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x60 => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }
            // f64 comparisons
            0x61 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a == b)));
            }
            0x62 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a != b)));
            }
            0x63 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a < b)));
            }
            0x64 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a > b)));
            }
            0x65 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a <= b)));
            }
            0x66 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(i32::from(a >= b)));
            }
            // i32 arithmetic
            0x67 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a.wrapping_add(b)));
            }
            0x68 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a.wrapping_sub(b)));
            }
            0x69 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a.wrapping_mul(b)));
            }
            0x6A => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                if b == 0 {
                    return Err("i32.div_s by zero".to_string());
                }
                push(&mut *frame, WasmValue::I32(a.wrapping_div(b)));
            }
            0x6B => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                if b == 0 {
                    return Err("i32.div_u by zero".to_string());
                }
                push(&mut *frame, WasmValue::I32((a / b) as i32));
            }
            0x6C => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                if b == 0 {
                    return Err("i32.rem_s by zero".to_string());
                }
                push(&mut *frame, WasmValue::I32(a.wrapping_rem(b)));
            }
            0x6D => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                if b == 0 {
                    return Err("i32.rem_u by zero".to_string());
                }
                push(&mut *frame, WasmValue::I32((a % b) as i32));
            }
            0x6E => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a & b));
            }
            0x6F => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a | b));
            }
            0x70 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a ^ b));
            }
            0x71 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a << (b as u32 & 31)));
            }
            0x72 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a >> (b as u32 & 31)));
            }
            0x73 => {
                let (b, a) = (pop_u32(&mut pop, frame)?, pop_u32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32((a >> (b & 31)) as i32));
            }
            0x74 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a.rotate_left(b as u32 & 31)));
            }
            0x75 => {
                let (b, a) = (pop_i32(&mut pop, frame)?, pop_i32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I32(a.rotate_right(b as u32 & 31)));
            }
            0x76 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a.wrapping_neg()));
            }
            0x77 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(!a));
            }
            0x78 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(i32::from(a == 0)));
            }
            // i64 arithmetic
            0x79 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a.wrapping_add(b)));
            }
            0x7A => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a.wrapping_sub(b)));
            }
            0x7B => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a.wrapping_mul(b)));
            }
            0x7C => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                if b == 0 {
                    return Err("i64.div_s by zero".to_string());
                }
                push(&mut *frame, WasmValue::I64(a.wrapping_div(b)));
            }
            0x7D => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                if b == 0 {
                    return Err("i64.div_u by zero".to_string());
                }
                push(&mut *frame, WasmValue::I64((a / b) as i64));
            }
            0x7E => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                if b == 0 {
                    return Err("i64.rem_s by zero".to_string());
                }
                push(&mut *frame, WasmValue::I64(a.wrapping_rem(b)));
            }
            0x7F => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                if b == 0 {
                    return Err("i64.rem_u by zero".to_string());
                }
                push(&mut *frame, WasmValue::I64((a % b) as i64));
            }
            0x80 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a & b));
            }
            0x81 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a | b));
            }
            0x82 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a ^ b));
            }
            0x83 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a << (b as u32 & 63)));
            }
            0x84 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a >> (b as u32 & 63)));
            }
            0x85 => {
                let (b, a) = (pop_u64(&mut pop, frame)?, pop_u64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64((a >> (b & 63)) as i64));
            }
            0x86 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a.rotate_left(b as u32 & 63)));
            }
            0x87 => {
                let (b, a) = (pop_i64(&mut pop, frame)?, pop_i64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::I64(a.rotate_right(b as u32 & 63)));
            }
            0x88 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a.wrapping_neg()));
            }
            0x89 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(!a));
            }
            0x8A => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(i32::from(a == 0)));
            }
            // f32 arithmetic
            0x8B => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a + b));
            }
            0x8C => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a - b));
            }
            0x8D => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a * b));
            }
            0x8E => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a / b));
            }
            0x8F => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a % b));
            }
            0x90 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(-a));
            }
            0x91 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.abs()));
            }
            0x92 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.ceil()));
            }
            0x93 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.floor()));
            }
            0x94 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.trunc()));
            }
            0x95 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.round()));
            }
            0x96 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a.sqrt()));
            }
            0x97 => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a.min(b)));
            }
            0x98 => {
                let (b, a) = (pop_f32(&mut pop, frame)?, pop_f32(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F32(a.max(b)));
            }
            // f64 arithmetic
            0x99 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a + b));
            }
            0x9A => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a - b));
            }
            0x9B => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a * b));
            }
            0x9C => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a / b));
            }
            0x9D => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a % b));
            }
            0x9E => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(-a));
            }
            0x9F => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.abs()));
            }
            0xA0 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.ceil()));
            }
            0xA1 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.floor()));
            }
            0xA2 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.trunc()));
            }
            0xA3 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.round()));
            }
            0xA4 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a.sqrt()));
            }
            0xA5 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a.min(b)));
            }
            0xA6 => {
                let (b, a) = (pop_f64(&mut pop, frame)?, pop_f64(&mut pop, frame)?);
                push(&mut *frame, WasmValue::F64(a.max(b)));
            }
            // Conversions (bounded subset)
            0xA7 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a as i64));
            }
            0xA8 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64((a as u32 as u64) as i64));
            }
            0xA9 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a as f32));
            }
            0xAA => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32((a as u32) as f32));
            }
            0xAB => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a as f64));
            }
            0xAC => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64((a as u32) as f64));
            }
            0xAD => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a as i32));
            }
            0xAE => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32((a as u64 as u32) as i32));
            }
            0xAF => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a as f32));
            }
            0xB0 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32((a as u64) as f32));
            }
            0xB1 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a as f64));
            }
            0xB2 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64((a as u64) as f64));
            }
            0xB3 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a as i32));
            }
            0xB4 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32((a as u32) as i32));
            }
            0xB5 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a as i64));
            }
            0xB6 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64((a as u64) as i64));
            }
            0xB7 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(a as f64));
            }
            0xB8 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a as i32));
            }
            0xB9 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32((a as u32) as i32));
            }
            0xBA => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a as i64));
            }
            0xBB => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64((a as u64) as i64));
            }
            0xBC => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(a as f32));
            }
            0xBD => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(i32::from(a == 0)));
            }
            0xBE => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(i32::from(a == 0)));
            }
            0xBF => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(i32::from(a == 0.0)));
            }
            0xC0 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(i32::from(a == 0.0)));
            }
            0xC1 => {
                let a = pop_i32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F32(f32::from_bits(a as u32)));
            }
            0xC2 => {
                let a = pop_i64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::F64(f64::from_bits(a as u64)));
            }
            0xC3 => {
                let a = pop_f32(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I32(a.to_bits() as i32));
            }
            0xC4 => {
                let a = pop_f64(&mut pop, frame)?;
                push(&mut *frame, WasmValue::I64(a.to_bits() as i64));
            }
            other => return Err(format!("Unsupported numeric opcode 0x{other:02X}")),
        }
        Ok(())
    }
}

fn pop_i32(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<i32, String> {
    match pop(frame)? {
        WasmValue::I32(value) => Ok(value),
        other => Err(format!("Expected i32, got {other:?}")),
    }
}

fn pop_u32(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<u32, String> {
    pop_i32(pop, frame).map(|value| value as u32)
}

fn pop_i64(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<i64, String> {
    match pop(frame)? {
        WasmValue::I64(value) => Ok(value),
        other => Err(format!("Expected i64, got {other:?}")),
    }
}

fn pop_u64(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<u64, String> {
    pop_i64(pop, frame).map(|value| value as u64)
}

fn pop_f32(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<f32, String> {
    match pop(frame)? {
        WasmValue::F32(value) => Ok(value),
        other => Err(format!("Expected f32, got {other:?}")),
    }
}

fn pop_f64(
    pop: &mut impl FnMut(&mut Frame) -> Result<WasmValue, String>,
    frame: &mut Frame,
) -> Result<f64, String> {
    match pop(frame)? {
        WasmValue::F64(value) => Ok(value),
        other => Err(format!("Expected f64, got {other:?}")),
    }
}

/// Scan forward from `pc` for the `end` (0x0B) matching the block opened at
/// the current depth, honouring nested blocks. Returns the pc just past end.
fn find_end(code: &[u8], mut pc: usize) -> Result<usize, String> {
    let mut depth = 0usize;
    while pc < code.len() {
        match code[pc] {
            0x02..=0x04 => depth += 1,
            0x0B => {
                if depth == 0 {
                    return Ok(pc);
                }
                depth -= 1;
            }
            _ => {}
        }
        pc += 1;
    }
    Err("Unbalanced block: missing end".to_string())
}

/// Scan for the `else` or `end` of the block starting at `pc` (depth 0 at
/// the block itself), returning pc just past it.
fn find_else_or_end(code: &[u8], mut pc: usize) -> Result<usize, String> {
    let mut depth = 0usize;
    while pc < code.len() {
        match code[pc] {
            0x02..=0x04 => depth += 1,
            0x05 if depth == 0 => return Ok(pc + 1),
            0x0B if depth == 0 => return Ok(pc + 1),
            0x0B => depth -= 1,
            _ => {}
        }
        pc += 1;
    }
    Err("Unbalanced block: missing end".to_string())
}

#[derive(Debug)]
enum Action {
    Continue,
    Branch(u32),
    Call(u32),
    CallIndirect(u32),
    Return,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::parse_module;

    fn code_section(bodies: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0x0A];
        let mut content = vec![bodies.len() as u8];
        for body in bodies {
            content.push(body.len() as u8);
            content.extend_from_slice(body);
        }
        out.push(content.len() as u8);
        out.extend(content);
        out
    }

    fn body_no_locals(instructions: &[u8]) -> Vec<u8> {
        let mut body = vec![0x00];
        body.extend_from_slice(instructions);
        body.push(0x0B); // end
        body
    }

    fn add_module() -> WasmModule {
        let mut bytes = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        // type: (i32, i32) -> i32
        bytes.extend([0x01, 0x07, 0x01, 0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F]);
        // function: 1 fn type 0
        bytes.extend([0x03, 0x02, 0x01, 0x00]);
        // export "add"
        bytes.extend([0x07, 0x07, 0x01, 0x03]);
        bytes.extend(b"add");
        bytes.extend([0x00, 0x00]);
        // code: locals 0; local.get 0; local.get 1; i32.add; end
        let body = body_no_locals(&[0x20, 0x00, 0x20, 0x01, 0x67]); // i32.add
        bytes.extend(code_section(&[body]));
        parse_module(&bytes).expect("module must parse")
    }

    fn fact_module() -> WasmModule {
        // (i32) -> i32 factorial via recursion.
        let mut bytes = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        // type0: (i32)->i32 ; type1: (i32,i32)->i32
        bytes.extend([0x01, 0x0C, 0x02]);
        bytes.extend([0x60, 0x01, 0x7F, 0x01, 0x7F]);
        bytes.extend([0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F]);
        // function section: 1 fn, type 0
        bytes.extend([0x03, 0x02, 0x01, 0x00]);
        // export "fact"
        bytes.extend([0x07, 0x08, 0x01, 0x04]);
        bytes.extend(b"fact");
        bytes.extend([0x00, 0x00]);
        // code: locals 0; local.get 0; i32.const 2; i32.lt_s; if (result i32)
        //   i32.const 1
        // else
        //   local.get 0; local.get 0; i32.const 1; i32.sub; call 0; i32.mul
        // end
        let body = body_no_locals(&[
            0x20, 0x00, 0x41, 0x02, 0x47, 0x04, 0x7F, 0x41, 0x01, 0x05, 0x20, 0x00, 0x20, 0x00,
            0x41, 0x01, 0x68, 0x10, 0x00, 0x69, 0x0B, // sub=0x68, mul=0x69
        ]);
        bytes.extend(code_section(&[body]));
        parse_module(&bytes).expect("module must parse")
    }

    #[test]
    fn add_function_returns_sum() {
        let module = add_module();
        let mut instance = WasmInstance::instantiate(module).unwrap();
        let results = instance
            .invoke(0, &[WasmValue::I32(20), WasmValue::I32(22)])
            .unwrap();
        assert_eq!(results, vec![WasmValue::I32(42)]);
    }

    #[test]
    fn recursive_factorial_matches_expected_values() {
        let module = fact_module();
        let mut instance = WasmInstance::instantiate(module).unwrap();
        for (input, expected) in [(0, 1), (1, 1), (5, 120), (10, 3_628_800)] {
            let results = instance.invoke(0, &[WasmValue::I32(input)]).unwrap();
            assert_eq!(results, vec![WasmValue::I32(expected)]);
        }
    }

    #[test]
    fn wrong_argument_count_fails_closed() {
        let module = add_module();
        let mut instance = WasmInstance::instantiate(module).unwrap();
        assert!(instance.invoke(0, &[WasmValue::I32(1)]).is_err());
    }

    #[test]
    fn missing_exported_function_name_is_none() {
        let module = add_module();
        let instance = WasmInstance::instantiate(module).unwrap();
        assert_eq!(instance.exported_function("nope"), None);
        assert_eq!(instance.exported_function("add"), Some(0));
    }

    #[test]
    fn deep_recursion_fails_closed_without_panicking() {
        // fact(2000) recurses far past MAX_CALL_DEPTH_WASM.
        let module = fact_module();
        let mut instance = WasmInstance::instantiate(module).unwrap();
        let error = instance.invoke(0, &[WasmValue::I32(2_000)]).unwrap_err();
        assert!(error.contains("depth"), "got: {error}");
    }

    #[test]
    fn memory_grow_is_bounded_and_loads_stay_in_range() {
        // Module with memory (min 1 page) + export "store": (i32)->() that
        // writes 42 at the given address; export "load": ()->i32 reads addr 0.
        let mut full = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        full.extend([0x01, 0x09, 0x02]);
        full.extend([0x60, 0x01, 0x7F, 0x00]);
        full.extend([0x60, 0x00, 0x01, 0x7F]);
        full.extend([0x03, 0x03, 0x02, 0x00, 0x01]);
        full.extend([0x05, 0x03, 0x01, 0x00, 0x01]);
        full.extend([0x07, 0x10, 0x02]);
        full.extend([0x05]);
        full.extend(b"store");
        full.extend([0x00, 0x00]);
        full.extend([0x04]);
        full.extend(b"load");
        full.extend([0x00, 0x01]);
        let store_body = body_no_locals(&[0x20, 0x00, 0x41, 0x2A, 0x36, 0x02, 0x00]);
        let load_body = body_no_locals(&[0x41, 0x08, 0x28, 0x02, 0x00]); // i32.const 8; i32.load
        full.extend(code_section(&[store_body, load_body]));
        let module = parse_module(&full).expect("module must parse");
        let mut instance = WasmInstance::instantiate(module).unwrap();
        assert_eq!(instance.memory_bytes(), 65_536);
        instance.invoke(0, &[WasmValue::I32(8)]).unwrap();
        let results = instance.invoke(1, &[]).unwrap();
        assert_eq!(results, vec![WasmValue::I32(42)]);
        // Out-of-bounds store fails closed.
        let error = instance
            .invoke(0, &[WasmValue::I32(1_000_000)])
            .unwrap_err();
        assert!(error.contains("out of range"), "got: {error}");
    }
}

#[cfg(test)]
mod spec_regression_tests {
    use super::*;
    use crate::wasm::parse_module;

    fn code_section(bodies: &[Vec<u8>]) -> Vec<u8> {
        let mut section = vec![0x0A];
        let mut content = Vec::new();
        content.push(bodies.len() as u8);
        for body in bodies {
            // Body sizes here are far below 128, so a one-byte LEB suffices.
            content.push(body.len() as u8);
            content.extend_from_slice(body);
        }
        let count_len = 1;
        section.push((content.len()) as u8);
        section.extend(content);
        let _ = count_len;
        section
    }

    fn body_no_locals(instructions: &[u8]) -> Vec<u8> {
        let mut body = vec![0x00];
        body.extend_from_slice(instructions);
        body.push(0x0B);
        body
    }

    fn build_fn_module(name: &[u8], body_instructions: &[u8]) -> WasmModule {
        let mut bytes = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        // type: () -> i32
        bytes.extend([0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7F]);
        bytes.extend([0x03, 0x02, 0x01, 0x00]);
        // export
        let export_size = 1 + 1 + name.len() + 2;
        bytes.push(0x07);
        bytes.push(export_size as u8);
        bytes.push(0x01);
        bytes.push(name.len() as u8);
        bytes.extend_from_slice(name);
        bytes.extend([0x00, 0x00]);
        let body = body_no_locals(body_instructions);
        bytes.extend(code_section(&[body]));
        parse_module(&bytes).expect("module must parse")
    }

    #[test]
    fn loop_fall_through_exits_instead_of_spinning() {
        // (func (result i32) (loop (then i32.const 42)) ...): a loop whose
        // body falls through its `end` must EXIT; the old handler branched
        // back to the head and spun until the step budget.
        let module = build_fn_module(
            b"loopexit",
            &[0x03, 0x40, 0x0B, 0x41, 0x2A], // loop .. end; i32.const 42
        );
        let mut instance = WasmInstance::instantiate(module).unwrap();
        let results = instance.invoke(0, &[]).unwrap();
        assert_eq!(results, vec![WasmValue::I32(42)]);
    }

    #[test]
    fn branch_keeps_block_result_values() {
        // (block (result i32) (i32.const 7) (br 0)): the branch must keep the
        // block's one result value; truncating it made the invoke fail its
        // result-count check.
        let module = build_fn_module(
            b"brval",
            &[0x02, 0x7F, 0x41, 0x07, 0x0C, 0x00, 0x0B], // block i32 .. br 0 .. end
        );
        let mut instance = WasmInstance::instantiate(module).unwrap();
        let results = instance.invoke(0, &[]).unwrap();
        assert_eq!(results, vec![WasmValue::I32(7)]);
    }
}
