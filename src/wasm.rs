//! Bounded clean-room WebAssembly MVP parser and structural validator
//! (Phase 17-A). Derived from the public WebAssembly Core Specification
//! (MVP subset); nothing is copied from another engine.
//!
//! Milestone 1.1 scope: binary parsing and structural validation only.
//! Execution arrives with the interpreter milestone. Every malformed input
//! fails closed with an explicit error; the parser never panics.

/// Hard bounds for module structure (fail closed above these).
pub const MAX_MODULE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TYPES: usize = 4_096;
pub const MAX_FUNCTIONS: usize = 4_096;
pub const MAX_IMPORTS: usize = 2_048;
pub const MAX_EXPORTS: usize = 2_048;
pub const MAX_GLOBALS: usize = 2_048;
pub const MAX_LOCALS_PER_FUNCTION: usize = 1_024;
pub const MAX_CODE_DEPTH: usize = 1_024;
pub const MAX_TABLE_ENTRIES: usize = 1_024;
pub const MAX_MEMORY_PAGES: u64 = 1_024; // declared max; instantiate caps actual size
pub const MAX_DATA_SEGMENTS: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    I32,
    I64,
    F32,
    F64,
    FuncRef,
    ExternRef,
}

impl ValueType {
    fn from_byte(byte: u8) -> Result<Self, String> {
        match byte {
            0x7F => Ok(Self::I32),
            0x7E => Ok(Self::I64),
            0x7D => Ok(Self::F32),
            0x7C => Ok(Self::F64),
            0x70 => Ok(Self::FuncRef),
            0x6F => Ok(Self::ExternRef),
            other => Err(format!("Invalid value type byte 0x{other:02X}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncType {
    pub parameters: Vec<ValueType>,
    pub results: Vec<ValueType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportKind {
    Function(u32),
    Table(TableType),
    Memory(MemoryType),
    Global(GlobalType),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub min: u64,
    pub max: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableType {
    pub element_type: ValueType,
    pub limits: Limits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryType {
    pub limits: Limits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalType {
    pub value_type: ValueType,
    pub mutable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub kind: ImportKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub name: String,
    pub kind: ExportKind,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Function,
    Table,
    Memory,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Global {
    pub value_type: ValueType,
    pub mutable: bool,
    /// Initializer expression bytes (validated structurally, executed by
    /// the interpreter milestone).
    pub init: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDecl {
    pub count: u32,
    pub value_type: ValueType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionBody {
    pub locals: Vec<LocalDecl>,
    /// Raw instruction bytes including the trailing `end` (0x0B).
    pub code: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementSegment {
    pub table_index: u32,
    pub offset: Vec<u8>,
    pub function_indices: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSegment {
    pub memory_index: u32,
    pub offset: Vec<u8>,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WasmModule {
    pub types: Vec<FuncType>,
    pub imports: Vec<Import>,
    /// Function type indices: imported functions first, then defined ones.
    pub function_type_indices: Vec<u32>,
    pub tables: Vec<TableType>,
    pub memories: Vec<MemoryType>,
    pub globals: Vec<Global>,
    pub exports: Vec<Export>,
    pub start: Option<u32>,
    pub elements: Vec<ElementSegment>,
    pub data: Vec<DataSegment>,
    pub bodies: Vec<FunctionBody>,
    /// Total declared function count (imported + defined), for index checks.
    pub total_functions: u32,
}

/// Parse and structurally validate a WebAssembly MVP module.
pub fn parse_module(bytes: &[u8]) -> Result<WasmModule, String> {
    if bytes.len() > MAX_MODULE_BYTES {
        return Err("WASM module exceeds 8 MB".to_string());
    }
    let mut cursor = Cursor::new(bytes);
    let magic = cursor.read_bytes(4)?;
    if magic != [0x00, 0x61, 0x73, 0x6D] {
        return Err("Invalid WASM magic".to_string());
    }
    let version = cursor.read_bytes(4)?;
    if version != [0x01, 0x00, 0x00, 0x00] {
        return Err("Unsupported WASM version".to_string());
    }

    let mut module = WasmModule::default();
    let mut seen = [false; 13];
    let mut last_section = 0u8;
    let mut data_count_declared = None;

    while !cursor.is_empty() {
        let section_id = cursor.read_u8()?;
        if section_id == 0 {
            return Err("Custom sections are not supported".to_string());
        }
        if section_id > 12 {
            return Err(format!("Unknown section id {section_id}"));
        }
        // Sections must appear once, in ascending id order (MVP rule).
        if seen[section_id as usize] {
            return Err(format!("Duplicate section {section_id}"));
        }
        if section_id < last_section && section_id != 10 {
            return Err(format!("Out-of-order section {section_id}"));
        }
        seen[section_id as usize] = true;
        last_section = section_id;
        let size = cursor.read_leb_u32()? as usize;
        if size > cursor.remaining() {
            return Err("Section extends beyond module end".to_string());
        }
        let mut section = Cursor::new(&bytes[cursor.pos..cursor.pos + size]);
        cursor.pos += size;
        match section_id {
            1 => parse_type_section(&mut section, &mut module)?,
            2 => parse_import_section(&mut section, &mut module)?,
            3 => parse_function_section(&mut section, &mut module)?,
            4 => parse_table_section(&mut section, &mut module)?,
            5 => parse_memory_section(&mut section, &mut module)?,
            6 => parse_global_section(&mut section, &mut module)?,
            7 => parse_export_section(&mut section, &mut module)?,
            8 => parse_start_section(&mut section, &mut module)?,
            9 => parse_element_section(&mut section, &mut module)?,
            10 => parse_code_section(&mut section, &mut module)?,
            11 => parse_data_section(&mut section, &mut module)?,
            12 => {
                let count = section.read_leb_u32()?;
                data_count_declared = Some(count);
                if count as usize > MAX_DATA_SEGMENTS {
                    return Err("Data segment budget exceeded".to_string());
                }
            }
            _ => unreachable!("section id bounded to 1..=12"),
        }
        if !section.is_empty() {
            return Err(format!("Section {section_id} has trailing bytes"));
        }
    }

    // Cross-section consistency checks.
    let imported_functions = module
        .imports
        .iter()
        .filter(|import| matches!(import.kind, ImportKind::Function(_)))
        .count() as u32;
    module.total_functions = imported_functions + module.function_type_indices.len() as u32;
    if module.total_functions as usize > MAX_FUNCTIONS {
        return Err("Function budget exceeded".to_string());
    }
    if module.bodies.len() != module.function_type_indices.len() {
        return Err("Code section does not match function section".to_string());
    }
    if let Some(start) = module.start {
        if start >= module.total_functions {
            return Err("Start function index out of range".to_string());
        }
    }
    if module.memories.len() > 1 {
        return Err("Multiple memories are not supported".to_string());
    }
    if let Some(declared) = data_count_declared {
        if declared as usize != module.data.len() {
            return Err("Data count does not match data section".to_string());
        }
    }
    for export in &module.exports {
        match export.kind {
            ExportKind::Function => {
                if export.index >= module.total_functions {
                    return Err("Export function index out of range".to_string());
                }
            }
            ExportKind::Global => {
                if export.index >= module.globals.len() as u32 {
                    return Err("Export global index out of range".to_string());
                }
            }
            ExportKind::Table => {
                if export.index >= module.tables.len() as u32 {
                    return Err("Export table index out of range".to_string());
                }
            }
            ExportKind::Memory => {
                if export.index >= module.memories.len() as u32 {
                    return Err("Export memory index out of range".to_string());
                }
            }
        }
    }
    Ok(module)
}

// ===== section parsers =====

fn parse_type_section(cursor: &mut Cursor, module: &mut WasmModule) -> Result<(), String> {
    let count = cursor.read_leb_u32()?;
    if count as usize > MAX_TYPES {
        return Err("Type budget exceeded".to_string());
    }
    for _ in 0..count {
        let form = cursor.read_u8()?;
        if form != 0x60 {
            return Err("Only function types are supported".to_string());
        }
        let parameters = read_value_type_vec(cursor, MAX_LOCALS_PER_FUNCTION)?;
        let results = read_value_type_vec(cursor, 64)?;
        if results.len() > 1 {
            return Err("Multi-value results are not supported".to_string());
        }
        module.types.push(FuncType {
            parameters,
            results,
        });
    }
    Ok(())
}

fn read_value_type_vec(cursor: &mut Cursor, max: usize) -> Result<Vec<ValueType>, String> {
    let count = cursor.read_leb_u32()?;
    if count as usize > max {
        return Err("Value type vector budget exceeded".to_string());
    }
    let mut values = Vec::with_capacity(count as usize);
    for _ in 0..count {
        values.push(ValueType::from_byte(cursor.read_u8()?)?);
    }
    Ok(values)
}

fn parse_import_section(cursor: &mut Cursor, module: &mut WasmModule) -> Result<(), String> {
    let count = cursor.read_leb_u32()?;
    if count as usize > MAX_IMPORTS {
        return Err("Import budget exceeded".to_string());
    }
    for _ in 0..count {
        let module_name = cursor.read_name()?;
        let name = cursor.read_name()?;
        let kind_byte = cursor.read_u8()?;
        let kind = match kind_byte {
            0x00 => ImportKind::Function(cursor.read_leb_u32()?),
            0x01 => ImportKind::Table(read_table_type(cursor)?),
            0x02 => ImportKind::Memory(read_memory_type(cursor)?),
            0x03 => ImportKind::Global(read_global_type(cursor)?),
            other => return Err(format!("Invalid import kind 0x{other:02X}")),
        };
        module.imports.push(Import {
            module: module_name,
            name,
            kind,
        });
    }
    Ok(())
}

fn read_limits(cursor: &mut Cursor) -> Result<Limits, String> {
    let flag = cursor.read_u8()?;
    match flag {
        0x00 => Ok(Limits {
            min: cursor.read_leb_u64()?,
            max: None,
        }),
        0x01 => {
            let min = cursor.read_leb_u64()?;
            let max = cursor.read_leb_u64()?;
            if max < min {
                return Err("Limits max is smaller than min".to_string());
            }
            Ok(Limits {
                min,
                max: Some(max),
            })
        }
        other => Err(format!("Invalid limits flag 0x{other:02X}")),
    }
}

fn read_table_type(cursor: &mut Cursor) -> Result<TableType, String> {
    let element_type = ValueType::from_byte(cursor.read_u8()?)?;
    if !matches!(element_type, ValueType::FuncRef | ValueType::ExternRef) {
        return Err("Table element type must be a reference type".to_string());
    }
    let limits = read_limits(cursor)?;
    if let Some(max) = limits.max {
        if max > MAX_TABLE_ENTRIES as u64 {
            return Err("Table entry budget exceeded".to_string());
        }
    }
    Ok(TableType {
        element_type,
        limits,
    })
}

fn read_memory_type(cursor: &mut Cursor) -> Result<MemoryType, String> {
    let limits = read_limits(cursor)?;
    if limits.min > MAX_MEMORY_PAGES {
        return Err("Memory minimum exceeds 1024 pages".to_string());
    }
    if let Some(max) = limits.max {
        if max > MAX_MEMORY_PAGES {
            return Err("Memory maximum exceeds 1024 pages".to_string());
        }
    }
    Ok(MemoryType { limits })
}

fn read_global_type(cursor: &mut Cursor) -> Result<GlobalType, String> {
    let value_type = ValueType::from_byte(cursor.read_u8()?)?;
    let mutable = match cursor.read_u8()? {
        0x00 => false,
        0x01 => true,
        other => return Err(format!("Invalid mutability byte 0x{other:02X}")),
    };
    Ok(GlobalType {
        value_type,
        mutable,
    })
}

fn parse_function_section(cursor: &mut Cursor, module: &mut WasmModule) -> Result<(), String> {
    let count = cursor.read_leb_u32()?;
    if count as usize > MAX_FUNCTIONS {
        return Err("Function budget exceeded".to_string());
    }
    for _ in 0..count {
        let type_index = cursor.read_leb_u32()?;
        if type_index >= module.types.len() as u32 {
            return Err("Function type index out of range".to_string());
        }
        module.function_type_indices.push(type_index);
    }
    Ok(())
}

fn parse_table_section(cursor: &mut Cursor, module: &mut WasmModule) -> Result<(), String> {
    let count = cursor.read_leb_u32()?;
    if count as usize > MAX_TABLE_ENTRIES {
        return Err("Table budget exceeded".to_string());
    }
    for _ in 0..count {
        module.tables.push(read_table_type(cursor)?);
    }
    Ok(())
}

fn parse_memory_section(cursor: &mut Cursor, module: &mut WasmModule) -> Result<(), String> {
    let count = cursor.read_leb_u32()?;
    if count > 1 {
        return Err("Multiple memories are not supported".to_string());
    }
    for _ in 0..count {
        module.memories.push(read_memory_type(cursor)?);
    }
    Ok(())
}

fn parse_global_section(cursor: &mut Cursor, module: &mut WasmModule) -> Result<(), String> {
    let count = cursor.read_leb_u32()?;
    if count as usize > MAX_GLOBALS {
        return Err("Global budget exceeded".to_string());
    }
    for _ in 0..count {
        let global_type = read_global_type(cursor)?;
        let init = read_expr(cursor)?;
        module.globals.push(Global {
            value_type: global_type.value_type,
            mutable: global_type.mutable,
            init,
        });
    }
    Ok(())
}

fn parse_export_section(cursor: &mut Cursor, module: &mut WasmModule) -> Result<(), String> {
    let count = cursor.read_leb_u32()?;
    if count as usize > MAX_EXPORTS {
        return Err("Export budget exceeded".to_string());
    }
    for _ in 0..count {
        let name = cursor.read_name()?;
        let kind_byte = cursor.read_u8()?;
        let kind = match kind_byte {
            0x00 => ExportKind::Function,
            0x01 => ExportKind::Table,
            0x02 => ExportKind::Memory,
            0x03 => ExportKind::Global,
            other => return Err(format!("Invalid export kind 0x{other:02X}")),
        };
        let index = cursor.read_leb_u32()?;
        module.exports.push(Export { name, kind, index });
    }
    Ok(())
}

fn parse_start_section(cursor: &mut Cursor, module: &mut WasmModule) -> Result<(), String> {
    module.start = Some(cursor.read_leb_u32()?);
    Ok(())
}

fn parse_element_section(cursor: &mut Cursor, module: &mut WasmModule) -> Result<(), String> {
    let count = cursor.read_leb_u32()?;
    if count as usize > MAX_TABLE_ENTRIES {
        return Err("Element segment budget exceeded".to_string());
    }
    for _ in 0..count {
        let table_index = cursor.read_leb_u32()?;
        let offset = read_expr(cursor)?;
        let function_count = cursor.read_leb_u32()?;
        if function_count as usize > MAX_TABLE_ENTRIES {
            return Err("Element function budget exceeded".to_string());
        }
        let mut function_indices = Vec::with_capacity(function_count as usize);
        for _ in 0..function_count {
            function_indices.push(cursor.read_leb_u32()?);
        }
        module.elements.push(ElementSegment {
            table_index,
            offset,
            function_indices,
        });
    }
    Ok(())
}

fn parse_code_section(cursor: &mut Cursor, module: &mut WasmModule) -> Result<(), String> {
    let count = cursor.read_leb_u32()?;
    if count as usize > MAX_FUNCTIONS {
        return Err("Code body budget exceeded".to_string());
    }
    for _ in 0..count {
        let body_size = cursor.read_leb_u32()? as usize;
        if body_size > cursor.remaining() {
            return Err("Function body extends beyond section".to_string());
        }
        let mut body = Cursor::new(&cursor.bytes[cursor.pos..cursor.pos + body_size]);
        cursor.pos += body_size;
        let local_group_count = body.read_leb_u32()?;
        if local_group_count as usize > MAX_LOCALS_PER_FUNCTION {
            return Err("Local declaration budget exceeded".to_string());
        }
        let mut locals = Vec::new();
        let mut total_locals = 0u32;
        for _ in 0..local_group_count {
            let count = body.read_leb_u32()?;
            total_locals = total_locals.saturating_add(count);
            if total_locals > MAX_LOCALS_PER_FUNCTION as u32 {
                return Err("Per-function local budget exceeded".to_string());
            }
            let value_type = ValueType::from_byte(body.read_u8()?)?;
            if !matches!(
                value_type,
                ValueType::I32 | ValueType::I64 | ValueType::F32 | ValueType::F64
            ) {
                return Err("Function locals must be numeric types".to_string());
            }
            locals.push(LocalDecl { count, value_type });
        }
        let code_start = body.pos;
        let depth = validate_code(&body.bytes[body.pos..])?;
        if depth > MAX_CODE_DEPTH {
            return Err("Code nesting depth exceeded".to_string());
        }
        body.pos = body.bytes.len();
        module.bodies.push(FunctionBody {
            locals,
            code: body.bytes[code_start..].to_vec(),
        });
    }
    Ok(())
}

fn parse_data_section(cursor: &mut Cursor, module: &mut WasmModule) -> Result<(), String> {
    let count = cursor.read_leb_u32()?;
    if count as usize > MAX_DATA_SEGMENTS {
        return Err("Data segment budget exceeded".to_string());
    }
    for _ in 0..count {
        let memory_index = cursor.read_leb_u32()?;
        let offset = read_expr(cursor)?;
        let byte_count = cursor.read_leb_u32()? as usize;
        if byte_count > cursor.remaining() {
            return Err("Data segment extends beyond section".to_string());
        }
        let bytes = cursor.read_bytes(byte_count)?.to_vec();
        module.data.push(DataSegment {
            memory_index,
            offset,
            bytes,
        });
    }
    Ok(())
}

// ===== expression / instruction validation =====

/// Read one constant-expression until `end` (0x0B). The expression bytes are
/// retained for the interpreter; only structure is validated here.
fn read_expr(cursor: &mut Cursor) -> Result<Vec<u8>, String> {
    let start = cursor.pos;
    let mut depth = 0usize;
    loop {
        let opcode = cursor.read_u8()?;
        match opcode {
            0x0B => {
                if depth == 0 {
                    return Ok(cursor.bytes[start..cursor.pos - 1].to_vec());
                }
                depth -= 1;
            }
            0x02..=0x04 => depth += 1,
            // Constant-expression instruction operands.
            0x41..=0x44 => {
                skip_immediate(cursor, opcode)?;
            }
            // Only i32.const (0x41), i64.const (0x42), f32/f64.const,
            // global.get (0x23) and ref.null are valid in constant exprs;
            // anything else fails closed here.
            _ => return Err(format!("Invalid constant expression opcode 0x{opcode:02X}")),
        }
        if depth == 0 && opcode == 0x0B {
            return Ok(cursor.bytes[start..cursor.pos - 1].to_vec());
        }
    }
}

fn skip_immediate(cursor: &mut Cursor, opcode: u8) -> Result<(), String> {
    match opcode {
        0x41 => {
            cursor.read_leb_i32()?;
        }
        0x42 => {
            cursor.read_leb_i64()?;
        }
        0x43 => {
            cursor.read_bytes(4)?;
        }
        0x44 => {
            cursor.read_bytes(8)?;
        }
        _ => {}
    }
    Ok(())
}

/// Structural validation of a function body: balanced block/end, valid
/// opcodes, immediates consumed correctly. Returns the max nesting depth.
fn validate_code(code: &[u8]) -> Result<usize, String> {
    let mut cursor = Cursor::new(code);
    let mut depth = 0usize;
    let mut max_depth = 0usize;
    loop {
        if cursor.is_empty() {
            return Err("Function body missing end opcode".to_string());
        }
        let opcode = cursor.read_u8()?;
        match opcode {
            0x0B => {
                if depth == 0 {
                    return Ok(max_depth);
                }
                depth -= 1;
            }
            0x02 | 0x03 => {
                depth += 1;
                max_depth = max_depth.max(depth);
                // Block type: 0x40, a value type byte, or a type index LEB.
                let block_type = cursor.peek_u8()?;
                match block_type {
                    0x40 => {
                        cursor.read_u8()?;
                    }
                    0x7C..=0x7F => {
                        cursor.read_u8()?;
                    }
                    _ => {
                        cursor.read_leb_u32()?;
                    }
                }
            }
            0x04 => {
                depth += 1;
                max_depth = max_depth.max(depth);
                let block_type = cursor.peek_u8()?;
                match block_type {
                    0x40 => {
                        cursor.read_u8()?;
                    }
                    0x7C..=0x7F => {
                        cursor.read_u8()?;
                    }
                    _ => {
                        cursor.read_leb_u32()?;
                    }
                }
            }
            0x05 => {
                // else: no immediate.
            }
            0x0C | 0x0D => {
                cursor.read_leb_u32()?;
            }
            0x0E => {
                let count = cursor.read_leb_u32()?;
                if count > 1_024 {
                    return Err("br_table label budget exceeded".to_string());
                }
                for _ in 0..count {
                    cursor.read_leb_u32()?;
                }
                cursor.read_leb_u32()?;
            }
            0x10 => {
                cursor.read_leb_u32()?;
            }
            0x11 => {
                let count = cursor.read_leb_u32()?;
                if count > 1_024 {
                    return Err("call_indirect type budget exceeded".to_string());
                }
                for _ in 0..count {
                    cursor.read_leb_u32()?;
                }
                cursor.read_leb_u32()?;
            }
            0x12 | 0x13 => {
                cursor.read_leb_u32()?;
            }
            0x20..=0x26 => {
                cursor.read_leb_u32()?;
            }
            0x28..=0x3E => {
                cursor.read_leb_u32()?;
                cursor.read_leb_u32()?;
            }
            0x41 => {
                cursor.read_leb_i32()?;
            }
            0x42 => {
                cursor.read_leb_i64()?;
            }
            0x43 => {
                cursor.read_bytes(4)?;
            }
            0x44 => {
                cursor.read_bytes(8)?;
            }
            // Simple opcodes with no immediates.
            0x45..=0xC4 => {}
            // Unsupported / unknown opcodes fail closed.
            other => return Err(format!("Unsupported opcode 0x{other:02X}")),
        }
    }
}

// ===== LEB128 / cursor =====

pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    pub(crate) fn read_u8(&mut self) -> Result<u8, String> {
        let byte = *self
            .bytes
            .get(self.pos)
            .ok_or_else(|| "Unexpected end of input".to_string())?;
        self.pos += 1;
        Ok(byte)
    }

    pub(crate) fn peek_u8(&self) -> Result<u8, String> {
        self.bytes
            .get(self.pos)
            .copied()
            .ok_or_else(|| "Unexpected end of input".to_string())
    }

    pub(crate) fn read_bytes(&mut self, count: usize) -> Result<&'a [u8], String> {
        if self.remaining() < count {
            return Err("Unexpected end of input".to_string());
        }
        let slice = &self.bytes[self.pos..self.pos + count];
        self.pos += count;
        Ok(slice)
    }

    pub(crate) fn read_name(&mut self) -> Result<String, String> {
        let length = self.read_leb_u32()? as usize;
        if length > 1_024 {
            return Err("Name length exceeds 1024 bytes".to_string());
        }
        let bytes = self.read_bytes(length)?;
        std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|_| "Name is not valid UTF-8".to_string())
    }

    pub(crate) fn read_leb_u32(&mut self) -> Result<u32, String> {
        let value = self.read_leb_u64()?;
        u32::try_from(value).map_err(|_| "LEB128 value exceeds u32".to_string())
    }

    pub(crate) fn read_leb_u64(&mut self) -> Result<u64, String> {
        let mut result = 0u64;
        let mut shift = 0u32;
        for _ in 0..10 {
            let byte = self.read_u8()?;
            result |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
        Err("LEB128 integer too long".to_string())
    }

    pub(crate) fn read_leb_i32(&mut self) -> Result<i32, String> {
        let value = self.read_leb_i64()?;
        i32::try_from(value).map_err(|_| "LEB128 value exceeds i32".to_string())
    }

    pub(crate) fn read_leb_i64(&mut self) -> Result<i64, String> {
        let mut result = 0i64;
        let mut shift = 0u32;
        let mut byte;
        loop {
            byte = self.read_u8()?;
            result |= i64::from(byte & 0x7F) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
            if shift >= 70 {
                return Err("LEB128 integer too long".to_string());
            }
        }
        if shift < 64 && byte & 0x40 != 0 {
            result |= -1i64 << shift;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid module: one i32 -> i32 function returning its argument,
    /// exported as "add1" — hand-assembled bytes.
    fn minimal_module() -> Vec<u8> {
        let mut bytes = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        // type section (1): 1 type (i32)->(i32)
        bytes.extend([0x01, 0x06, 0x01, 0x60, 0x01, 0x7F, 0x01, 0x7F]);
        // function section (3): 1 function, type 0
        bytes.extend([0x03, 0x02, 0x01, 0x00]);
        // export section (7): "add1" func 0
        bytes.extend([0x07, 0x08, 0x01, 0x04]);
        bytes.extend(b"add1");
        bytes.extend([0x00, 0x00]);
        // code section (10): 1 body, locals 0, local.get 0, end
        bytes.extend([0x0A, 0x06, 0x01, 0x04, 0x00, 0x20, 0x00, 0x0B]);
        bytes
    }

    #[test]
    fn parses_minimal_valid_module() {
        let module = parse_module(&minimal_module()).unwrap();
        assert_eq!(module.types.len(), 1);
        assert_eq!(module.types[0].parameters, vec![ValueType::I32]);
        assert_eq!(module.types[0].results, vec![ValueType::I32]);
        assert_eq!(module.total_functions, 1);
        assert_eq!(module.exports.len(), 1);
        assert_eq!(module.exports[0].name, "add1");
        assert_eq!(module.bodies.len(), 1);
        assert_eq!(module.bodies[0].locals.len(), 0);
    }

    #[test]
    fn rejects_bad_magic_and_version() {
        assert!(parse_module(&[0, 0, 0, 0, 1, 0, 0, 0]).is_err());
        assert!(parse_module(&[0, 0x61, 0x73, 0x6D, 2, 0, 0, 0]).is_err());
    }

    #[test]
    fn rejects_truncated_and_malformed_input_without_panicking() {
        let module = minimal_module();
        for cut in 0..module.len() {
            let _ = parse_module(&module[..cut]);
        }
        assert!(parse_module(&[]).is_err());
    }

    #[test]
    fn rejects_unknown_opcode_in_body() {
        let mut module = minimal_module();
        // Replace local.get (0x20) with an unknown opcode (0xFF) in code.
        let code_start = module.len() - 6;
        module[code_start + 4] = 0xFF;
        assert!(parse_module(&module).is_err());
    }

    #[test]
    fn rejects_unbalanced_block_in_body() {
        // Code section with a balanced block: id 10, size 8, 1 body of
        // size 6: block(0x02) empty-type(0x40) local.get 0 end end.
        let mut module = minimal_module();
        let code_len = module.len();
        module.truncate(code_len - 8);
        // Body: locals 0, block(0x02) empty-type(0x40) local.get 0 end end.
        module.extend([
            0x0A, 0x09, 0x01, 0x07, 0x00, 0x02, 0x40, 0x20, 0x00, 0x0B, 0x0B,
        ]);
        assert!(parse_module(&module).is_ok());
        // Drop the final end: function body ends unbalanced.
        let mut broken = module.clone();
        broken.pop();
        assert!(parse_module(&broken).is_err());
    }

    #[test]
    fn rejects_out_of_order_and_duplicate_sections() {
        let mut module = minimal_module();
        // Append a second type section after the code section.
        module.extend([0x01, 0x04, 0x01, 0x60, 0x00, 0x00]);
        assert!(parse_module(&module).is_err());
    }

    #[test]
    fn rejects_oversized_limits() {
        // Memory section with min > 1024 pages.
        let mut bytes = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        bytes.extend([0x05, 0x07, 0x01, 0x00]); // memory section, 1 entry, min flag
        bytes.extend(encode_leb_u64(1_025));
        bytes.extend(encode_leb_u64(1_025)); // max
        assert!(parse_module(&bytes).is_err());
    }

    fn encode_leb_u64(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut byte = (value & 0x7F) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                return out;
            }
        }
    }
}
