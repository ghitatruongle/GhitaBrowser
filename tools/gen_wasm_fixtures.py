#!/usr/bin/env python3
"""Generate WebAssembly binary fixtures for the interpreter conformance suite.

Each fixture is a spec-shaped MVP module (proper sections, LEB128 encoding,
data/element segments) exercising one opcode family. The Rust test
`tests/wasm_conformance_test.rs` loads these files, instantiates them through
`ghitabrowser::wasm_interp` and asserts spec-exact results.

Run from the repo root:  python tools/gen_wasm_fixtures.py
"""

import struct
from pathlib import Path

OUT_DIR = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "wasm"

I32, I64, F32, F64 = 0x7F, 0x7E, 0x7D, 0x7C


def uleb(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def sleb(value: int) -> bytes:
    out = bytearray()
    more = True
    while more:
        byte = value & 0x7F
        value >>= 7
        if (value == 0 and not byte & 0x40) or (value == -1 and byte & 0x40):
            more = False
        else:
            byte |= 0x80
        out.append(byte)
    return bytes(out)


def vec(items) -> bytes:
    return uleb(len(items)) + b"".join(items)


def section(sid: int, payload: bytes) -> bytes:
    return bytes([sid]) + uleb(len(payload)) + payload


class ModuleBuilder:
    def __init__(self):
        self.types = []          # (params, results) tuples
        self.functions = []      # type index per defined function
        self.bodies = []         # (local_groups, code_without_end)
        self.exports = []        # (name, kind, index)
        self.memory = None       # (min, max_or_None)
        self.table_size = None
        self.globals = []        # (type, mutable, init_expr_incl_end)
        self.elements = []       # (table_index, offset_i32, [func_indices])
        self.data = []           # (mem_index, offset_i32, bytes)

    def add_type(self, params, results) -> int:
        entry = (tuple(params), tuple(results))
        if entry in self.types:
            return self.types.index(entry)
        self.types.append(entry)
        return len(self.types) - 1

    def define(self, type_index, local_groups, code, export=None):
        self.functions.append(type_index)
        self.bodies.append((local_groups, code))
        index = len(self.functions) - 1
        if export:
            self.exports.append((export, 0x00, index))
        return index

    def emit(self) -> bytes:
        out = bytearray(b"\x00asm\x01\x00\x00\x00")
        if self.types:
            payloads = []
            for params, results in self.types:
                payloads.append(b"\x60"
                                + vec([bytes([p]) for p in params])
                                + vec([bytes([r]) for r in results]))
            out += section(1, vec(payloads))
        if self.functions:
            out += section(3, vec([uleb(t) for t in self.functions]))
        if self.table_size is not None:
            out += section(4, vec([b"\x70\x00" + uleb(self.table_size)]))
        if self.memory is not None:
            minimum, maximum = self.memory
            limit = (b"\x01" + uleb(minimum) + uleb(maximum)) if maximum is not None \
                else b"\x00" + uleb(minimum)
            out += section(5, vec([limit]))
        if self.globals:
            entries = [bytes([vtype, mutable]) + init
                       for vtype, mutable, init in self.globals]
            out += section(6, vec(entries))
        if self.exports:
            entries = []
            for name, kind, index in self.exports:
                encoded = name.encode("utf-8")
                entries.append(uleb(len(encoded)) + encoded
                               + bytes([kind]) + uleb(index))
            out += section(7, vec(entries))
        if self.elements:
            entries = []
            for table_index, offset, indices in self.elements:
                entries.append(uleb(table_index)
                               + b"\x41" + sleb(offset) + b"\x0b"
                               + vec([uleb(i) for i in indices]))
            out += section(9, vec(entries))
        if self.bodies:
            entries = []
            for local_groups, code in self.bodies:
                body = vec([uleb(count) + bytes([vtype])
                            for count, vtype in local_groups]) + code + b"\x0b"
                entries.append(uleb(len(body)) + body)
            out += section(10, vec(entries))
        if self.data:
            entries = []
            for mem_index, offset, blob in self.data:
                entries.append(uleb(mem_index)
                               + b"\x41" + sleb(offset) + b"\x0b"
                               + uleb(len(blob)) + blob)
            out += section(11, vec(entries))
        return bytes(out)


def bin_fn(mod, name, op, params=(I32, I32), result=(I32,)):
    """Exported `(p0, p1) -> r` computing p0 OP p1."""
    t = mod.add_type(params, result)
    mod.define(t, [], b"\x20\x00\x20\x01" + bytes([op]), export=name)


def un_fn(mod, name, op, param, result):
    """Exported `(p0) -> r` computing OP p0."""
    t = mod.add_type((param,), result)
    mod.define(t, [], b"\x20\x00" + bytes([op]), export=name)


def gen_i32():
    m = ModuleBuilder()
    for offset, name in enumerate(
            ["eq", "ne", "lt_s", "lt_u", "gt_s", "gt_u",
             "le_s", "le_u", "ge_s", "ge_u"]):
        bin_fn(m, name, 0x46 + offset)
    un_fn(m, "eqz", 0x45, I32, (I32,))
    for offset, name in enumerate(
            ["add", "sub", "mul", "div_s", "div_u", "rem_s", "rem_u",
             "and", "or", "xor", "shl", "shr_s", "shr_u", "rotl", "rotr"]):
        bin_fn(m, name, 0x6A + offset)
    for offset, name in enumerate(["clz", "ctz", "popcnt"]):
        un_fn(m, name, 0x67 + offset, I32, (I32,))
    un_fn(m, "extend8_s", 0xC0, I32, (I32,))
    un_fn(m, "extend16_s", 0xC1, I32, (I32,))
    return m.emit()


def gen_i64():
    m = ModuleBuilder()
    for offset, name in enumerate(
            ["eq", "ne", "lt_s", "lt_u", "gt_s", "gt_u",
             "le_s", "le_u", "ge_s", "ge_u"]):
        bin_fn(m, name, 0x51 + offset, (I64, I64), (I32,))
    un_fn(m, "eqz", 0x50, I64, (I32,))
    for offset, name in enumerate(
            ["add", "sub", "mul", "div_s", "div_u", "rem_s", "rem_u",
             "and", "or", "xor", "shl", "shr_s", "shr_u", "rotl", "rotr"]):
        bin_fn(m, name, 0x7C + offset, (I64, I64), (I64,))
    for offset, name in enumerate(["clz", "ctz", "popcnt"]):
        un_fn(m, name, 0x79 + offset, I64, (I64,))
    for offset, name in enumerate(["extend8_s", "extend16_s", "extend32_s"]):
        un_fn(m, name, 0xC2 + offset, I64, (I64,))
    return m.emit()


def _gen_float(width: int, prefix: str) -> bytes:
    m = ModuleBuilder()
    pair = (width, width)
    cmp_base = 0x5B if width == F32 else 0x61
    for offset, name in enumerate(["eq", "ne", "lt", "gt", "le", "ge"]):
        bin_fn(m, f"{prefix}.{name}", cmp_base + offset, pair, (I32,))
    unary_base = 0x8B if width == F32 else 0x99
    for offset, name in enumerate(
            ["abs", "neg", "ceil", "floor", "trunc", "nearest", "sqrt"]):
        un_fn(m, f"{prefix}.{name}", unary_base + offset, width, (width,))
    binary_base = 0x92 if width == F32 else 0xA0
    for offset, name in enumerate(
            ["add", "sub", "mul", "div", "min", "max", "copysign"]):
        bin_fn(m, f"{prefix}.{name}", binary_base + offset, pair, (width,))
    return m.emit()


def gen_f32():
    return _gen_float(F32, "f32")


def gen_f64():
    return _gen_float(F64, "f64")


def gen_conversions():
    m = ModuleBuilder()
    spec = [
        ("wrap_i64", 0xA7, I64, I32),
        ("trunc_f32_s", 0xA8, F32, I32), ("trunc_f32_u", 0xA9, F32, I32),
        ("trunc_f64_s", 0xAA, F64, I32), ("trunc_f64_u", 0xAB, F64, I32),
        ("extend_i32_s", 0xAC, I32, I64), ("extend_i32_u", 0xAD, I32, I64),
        ("trunc_f32_s_i64", 0xAE, F32, I64), ("trunc_f32_u_i64", 0xAF, F32, I64),
        ("trunc_f64_s_i64", 0xB0, F64, I64), ("trunc_f64_u_i64", 0xB1, F64, I64),
        ("convert_i32_s_f32", 0xB2, I32, F32), ("convert_i32_u_f32", 0xB3, I32, F32),
        ("convert_i64_s_f32", 0xB4, I64, F32), ("convert_i64_u_f32", 0xB5, I64, F32),
        ("demote_f64", 0xB6, F64, F32),
        ("convert_i32_s_f64", 0xB7, I32, F64), ("convert_i32_u_f64", 0xB8, I32, F64),
        ("convert_i64_s_f64", 0xB9, I64, F64), ("convert_i64_u_f64", 0xBA, I64, F64),
        ("promote_f32", 0xBB, F32, F64),
        ("reinterpret_f32_i32", 0xBC, F32, I32), ("reinterpret_f64_i64", 0xBD, F64, I64),
        ("reinterpret_i32_f32", 0xBE, I32, F32), ("reinterpret_i64_f64", 0xBF, I64, F64),
        ("extend8_s", 0xC0, I32, I32), ("extend16_s", 0xC1, I32, I32),
        ("iextend8_s", 0xC2, I64, I64), ("iextend16_s", 0xC3, I64, I64),
        ("iextend32_s", 0xC4, I64, I64),
    ]
    for name, opcode, param, result in spec:
        un_fn(m, name, opcode, param, (result,))
    return m.emit()


def gen_memory():
    m = ModuleBuilder()
    m.memory = (1, None)
    m.data.append((0, 16, bytes([0x89, 0xAB, 0xCD, 0xFE])))
    store_types = {I32: m.add_type((I32, I32), ()),
                   I64: m.add_type((I32, I64), ()),
                   F32: m.add_type((I32, F32), ()),
                   F64: m.add_type((I32, F64), ())}

    def loader(name, op, param, result):
        t = m.add_type((param,), (result,))
        m.define(t, [], b"\x20\x00" + bytes([op]) + b"\x00\x00", export=name)

    def storer(name, op, value_type):
        t = store_types[value_type]
        m.define(t, [], b"\x20\x00\x20\x01" + bytes([op]) + b"\x00\x00",
                 export=name)

    loader("load8_s", 0x2C, I32, I32)
    loader("load8_u", 0x2D, I32, I32)
    loader("load16_s", 0x2E, I32, I32)
    loader("load16_u", 0x2F, I32, I32)
    loader("load_i32", 0x28, I32, I32)
    loader("iload8_s", 0x30, I32, I64)
    loader("iload8_u", 0x31, I32, I64)
    loader("iload16_s", 0x32, I32, I64)
    loader("iload16_u", 0x33, I32, I64)
    loader("iload32_s", 0x34, I32, I64)
    loader("iload32_u", 0x35, I32, I64)
    storer("store8", 0x3A, I32)
    storer("store16", 0x3B, I32)
    storer("store_i32", 0x36, I32)
    storer("istore8", 0x3C, I64)
    storer("istore16", 0x3D, I64)
    storer("istore32", 0x3E, I64)
    storer("istore_i64", 0x37, I64)
    storer("store_f32", 0x38, F32)
    storer("store_f64", 0x39, F64)
    loader("load_f32", 0x2A, I32, F32)
    loader("load_f64", 0x2B, I32, F64)
    t_void_i32 = m.add_type((), (I32,))
    m.define(t_void_i32, [], b"\x3f\x00", export="size")
    return m.emit()


def gen_control():
    m = ModuleBuilder()
    void_to_i32 = m.add_type((), (I32,))
    pair_to_i32 = m.add_type((I32, I32), (I32,))
    one_to_i32 = m.add_type((I32,), (I32,))

    def const_i32(value: int) -> bytes:
        return b"\x41" + sleb(value)

    # sum10(): acc/i loop with a br_if back edge; returns 55. Locals:
    # slot 0 = acc, slot 1 = i (declared as 2 x i32).
    sum10 = (
        const_i32(0) + b"\x21\x00"                        # acc = 0
        + const_i32(1) + b"\x21\x01"                      # i   = 1
        + b"\x02\x40"                                     # block $exit
        + b"\x03\x40"                                     #   loop $top
        + b"\x20\x01" + const_i32(10) + b"\x4a"           #   i > 10 ?
        + b"\x0d\x01"                                     #   br_if $exit
        + b"\x20\x00\x20\x01\x6a\x21\x00"                 #   acc += i
        + b"\x20\x01" + const_i32(1) + b"\x6a\x21\x01"    #   i += 1
        + b"\x0c\x00"                                     #   br $top
        + b"\x0b"                                         # end loop
        + b"\x0b"                                         # end block
        + b"\x20\x00"                                     # acc
    )
    m.define(void_to_i32, [(2, I32)], sum10, export="sum10")

    # max(a, b): if/else with an else arm.
    m.define(pair_to_i32, [], (
        b"\x20\x00"
        + b"\x20\x01"
        + b"\x48"                                         # i32.lt_s (a < b)
        + b"\x04\x7f"                                     # if (result i32)
        + b"\x20\x01"                                     #   then b
        + b"\x05"                                         # else
        + b"\x20\x00"                                     #   a
        + b"\x0b"
    ), export="max")

    # bucket(n): br_table over three nested blocks. Slots: 0 = param n,
    # 1 = result local — they must stay distinct or the setter overwrites n.
    m.define(one_to_i32, [(1, I32)], (
        const_i32(300) + b"\x21\x01"                      # result = 300
        + b"\x02\x40"                                     # block $out
        + b"\x02\x40"                                     #   block $one
        + b"\x02\x40"                                     #     block $zero
        + b"\x20\x00"                                     #       n
        + b"\x0e\x02\x00\x01\x02"                         #       br_table 0 1 default 2
        + b"\x0b"                                         #     end $zero
        + const_i32(100) + b"\x21\x01"                    #     result = 100
        + b"\x0c\x01"                                     #     br $out (L1 from inside $one)
        + b"\x0b"                                         #   end $one
        + const_i32(200) + b"\x21\x01"                    #   result = 200
        + b"\x0c\x00"                                     #   br $out (L0 from inside $out)
        + b"\x0b"                                         # end $out
        + b"\x20\x01"                                     # result
    ), export="bucket")

    # select(a, b, cond): picks a when cond != 0 else b.
    m.define(m.add_type((I32, I32, I32), (I32,)), [], (
        b"\x20\x00\x20\x01\x20\x02\x1b"
    ), export="select")

    # after_block_work_continues(x): proves execution resumes AFTER an
    # outermost block exited via `br 0` (the old interpreter returned from
    # the whole function at that point, skipping everything after).
    m.define(one_to_i32, [], (
        b"\x02\x40"
        + b"\x0c\x00"                                     # br 0
        + b"\x0b"
        + b"\x20\x00" + const_i32(5) + b"\x6c"            # x * 5 — must still run
    ), export="after_block_work_continues")

    # call_indirect dispatch through an element-initialized table.
    even = m.define(one_to_i32, [], b"\x20\x00" + const_i32(1) + b"\x71")     # n & 1
    tripled = m.define(one_to_i32, [], b"\x20\x00" + const_i32(3) + b"\x6c")  # n * 3
    m.table_size = 2
    m.elements.append((0, 0, [even, tripled]))
    m.define(m.add_type((I32, I32), (I32,)), [], (
        b"\x20\x01"                                       # arg
        + b"\x20\x00"                                     # table index
        + b"\x11" + uleb(one_to_i32) + b"\x00"            # call_indirect (reserved 0)
    ), export="dispatch")
    return m.emit()


def gen_globals():
    m = ModuleBuilder()
    m.globals.append((I32, 1, b"\x41" + sleb(5) + b"\x0b"))  # mutable g = 5
    getter = m.add_type((), (I32,))
    setter = m.add_type((I32,), ())
    m.define(getter, [], b"\x23\x00", export="gread")
    m.define(setter, [], b"\x23\x00\x20\x00\x6a\x24\x00", export="gadd")
    return m.emit()


FIXTURES = {
    "i32_ops.wasm": gen_i32,
    "i64_ops.wasm": gen_i64,
    "f32_ops.wasm": gen_f32,
    "f64_ops.wasm": gen_f64,
    "conversions.wasm": gen_conversions,
    "memory_ops.wasm": gen_memory,
    "control_flow.wasm": gen_control,
    "globals.wasm": gen_globals,
}


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for filename, generator in FIXTURES.items():
        blob = generator()
        target = OUT_DIR / filename
        target.write_bytes(blob)
        print(f"{target.relative_to(target.parent.parent.parent)}: {len(blob)} bytes")


if __name__ == "__main__":
    main()
