#!/usr/bin/env python3
"""Lower a profiled subset of Thumb basic blocks to Rust, in Cargo's OUT_DIR."""

import argparse
import struct
from dataclasses import dataclass
from pathlib import Path


@dataclass
class Instruction:
    body: str
    cycles: int
    terminal: bool = False
    stack: tuple[int, int] | None = None
    sp_delta: int = 0
    taken: str | None = None


def transfer(pc, load, size, reg, base, offset, sp_delta):
    if base == 13:
        start = sp_delta + offset
        access = f"load_stack::<{size}>(stack, {{index}})" if load else (
            f"store_stack::<{size}>(stack, {{index}}, regs.r[{reg}])")
        body = f"regs.r[{reg}] = {access};" if load else f"{access};"
        return Instruction(body, 2, stack=(start, start + size))

    address = f"regs.r[{base}].wrapping_add({offset})"
    width = size * 8
    if load:
        body = (f"regs.r[{reg}] = u32::from(bus.read_u{width}"
                f"({address}, periph, &cpu.nvic));")
    else:
        body = (f"bus.write_u{width}({address}, regs.r[{reg}] as u{width}, "
                f"periph, &mut cpu.nvic);")
    return Instruction(f"bus.current_pc = {pc}; {body}", 2, terminal=True)


def decode(pc, first, second, sp_delta):
    if second is not None:
        reg = (second >> 8) & 15
        if first & 0xFBF0 in (0xF240, 0xF2C0) and second < 0x8000 and reg < 13:
            imm = ((first & 15) << 12) | ((first & 0x400) << 1)
            imm |= ((second >> 4) & 0x700) | (second & 255)
            if first & 0x80:
                return Instruction(f"regs.r[{reg}] = (regs.r[{reg}] & 65535) | {imm << 16};", 1)
            return Instruction(f"regs.r[{reg}] = {imm};", 1)
        if first == 0xF04F and second & 0xF000 == 0 and reg < 13:
            return Instruction(f"regs.r[{reg}] = {second & 255};", 1)
        if first & 0xFFF0 == 0xF000 and second & 0xF000 == 0 and reg < 13 and first & 15 < 13:
            return Instruction(f"regs.r[{reg}] = regs.r[{first & 15}] & {second & 255};", 1)
        if first & 0xF800 == 0xF000 and second & 0xD000 == 0xD000:
            sign = (first >> 10) & 1
            i1 = 1 ^ ((second >> 13) & 1) ^ sign
            i2 = 1 ^ ((second >> 11) & 1) ^ sign
            offset = (i1 << 23) | (i2 << 22) | ((first & 0x3FF) << 12) | ((second & 0x7FF) << 1)
            target = (pc + 4 + offset - (1 << 24 if sign else 0)) & 0xFFFFFFFF
            return Instruction(f"regs.lr = {pc + 5}; regs.pc = {target};", 3, terminal=True)
        if first & 0xFFC0 == 0xF880:
            base, reg = first & 15, second >> 12
            if base <= 13 and reg < 13:
                return transfer(pc, bool(first & 0x10), 1, reg, base, second & 0xFFF, sp_delta)
        return None

    if first & 0xF000 == 0x9000:
        return transfer(pc, bool(first & 0x800), 4, (first >> 8) & 7, 13,
                        (first & 255) * 4, sp_delta)
    if first & 0xF000 in (0x6000, 0x7000, 0x8000):
        size = {0x6000: 4, 0x7000: 1, 0x8000: 2}[first & 0xF000]
        return transfer(pc, bool(first & 0x800), size, first & 7, (first >> 3) & 7,
                        ((first >> 6) & 31) * size, sp_delta)
    if first & 0xFF00 == 0xB000:
        delta = (first & 0x7F) * (-4 if first & 0x80 else 4)
        return Instruction("", 1, sp_delta=delta)
    if first & 0xF800 == 0xA800:
        reg, offset = (first >> 8) & 7, (first & 255) * 4 + sp_delta
        return Instruction(f"regs.r[{reg}] = initial_sp.wrapping_add_signed({offset});", 1)
    if first & 0xE000 == 0x2000:
        op, reg, imm = (first >> 11) & 3, (first >> 8) & 7, first & 255
        if op == 0:
            return Instruction(f"regs.r[{reg}] = {imm}; regs.set_nz({imm});", 1)
        return Instruction(f"arith(regs, {reg}, {imm}, {str(op != 2).lower()}, {str(op != 1).lower()});", 1)
    if first == 0xBF00:
        return Instruction("", 1)
    if first == 0x4770:
        return Instruction("regs.pc = regs.lr & !1;", 2, terminal=True)
    if first & 0xF800 == 0xE000:
        imm = (first & 0x7FF) << 1
        target = pc + 4 + imm - (4096 if imm & 2048 else 0)
        if target == pc + 2:
            return Instruction("", 2)
        return Instruction(f"regs.pc = {target & 0xFFFFFFFF};", 2, terminal=True)
    if first & 0xF500 == 0xB100:
        target = pc + 4 + (((first >> 9) & 1) << 6) + (((first >> 3) & 31) << 1)
        op = "!=" if first & 0x800 else "=="
        return Instruction("", 1, terminal=True, taken=f"if regs.r[{first & 7}] {op} 0 {{ regs.pc = {target}; return {{cycles}}; }}")
    if first & 0xF000 == 0xD000 and (first >> 8) & 15 < 14:
        offset = (first & 255) * 2 - (512 if first & 128 else 0)
        target = (pc + 4 + offset) & 0xFFFFFFFF
        condition = (first >> 8) & 15
        return Instruction("", 1, terminal=True,
                           taken=f"if Thumb16::eval_condition({condition}, regs) {{ regs.pc = {target}; return {{cycles}}; }}")
    return None


def generate(pram, ranges):
    functions = []
    dispatch = []
    for lo, hi in ranges:
        starts = []
        pc = lo
        while pc < hi:
            first = struct.unpack_from("<H", pram, pc)[0]
            wide = first >> 11 in (0x1D, 0x1E, 0x1F)
            after = pc + (4 if wide else 2)
            if after > hi:
                break
            second = struct.unpack_from("<H", pram, pc + 2)[0] if wide else None
            starts.append((pc, after, first, second))
            pc = after
        for index, (start, _, _, _) in enumerate(starts):
            if start == 0:
                continue
            instructions = []
            delta = 0
            for pc, after, first, second in starts[index:index + 8]:
                inst = decode(pc, first, second, delta)
                if inst is None:
                    break
                instructions.append((pc, after, inst))
                delta += inst.sp_delta
                if inst.terminal:
                    break
            if len(instructions) < 3:
                continue

            last_pc, end, last = instructions[-1]
            windows = [inst.stack for _, _, inst in instructions if inst.stack]
            low = min(low for low, _ in windows) if windows else 0
            high = max(high for _, high in windows) if windows else 0
            count = len(instructions)
            cycles = sum(inst.cycles for _, _, inst in instructions)
            worst = cycles + bool(last.taken)
            name = f"block_{start:04x}"
            original = ", ".join(f"0x{byte:02x}" for byte in pram[start:end])
            dispatch.append(
                f"    {start} => Some(Block {{ source: &[{original}], steps: {count}, "
                f"max_cycles: {worst}, stack_low: {low}, stack_high: {high}, run: {name} }}),")
            lines = [f"fn {name}(cpu: &mut Cpu, bus: &mut MemoryBus, periph: &mut Peripherals, stack_index: usize) -> u32 {{",
                     "    let regs = &mut cpu.regs;"]
            if delta or any("initial_sp" in inst.body for _, _, inst in instructions):
                lines.append("    let initial_sp = regs.get_sp();")
            if windows:
                lines.append(f"    let stack = &mut bus.sram.data[stack_index..stack_index + {high - low}];")
            lines.append(f"    regs.pc = {end};")
            for _, _, inst in instructions:
                if inst.body:
                    body = inst.body.format(index=inst.stack[0] - low) if inst.stack else inst.body
                    lines.append(f"    {body}")
            if delta:
                lines.append(f"    regs.set_sp(initial_sp.wrapping_add_signed({delta}));")
            lines.append(f"    bus.current_pc = {last_pc};")
            if last.taken:
                lines.append("    " + last.taken.replace("{cycles}", str(worst)))
            lines.extend([f"    {cycles}", "}"])
            functions.extend(lines)
    if not dispatch:
        raise ValueError("the requested ranges contain no supported blocks")
    return "\n".join([
        "pub(super) fn find_block(pc: u32) -> Option<Block> { match pc {",
        *dispatch,
        "    _ => None,",
        "} }",
        *functions,
    ]) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pram", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("ranges", nargs="+", help="nonoverlapping hexadecimal START:END pairs")
    args = parser.parse_args()
    pram = args.pram.read_bytes()
    if len(pram) != 65536:
        parser.error("expected a 64 KiB PRAM image")
    try:
        ranges = sorted(tuple(int(value, 16) for value in entry.split(":"))
                        for entry in args.ranges)
        previous = 0
        for lo, hi in ranges:
            if not previous <= lo < hi <= len(pram) or lo % 2 or hi % 2:
                raise ValueError("ranges must be aligned, nonoverlapping, and inside PRAM")
            previous = hi
        args.output.write_text(generate(pram, ranges))
    except ValueError as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
