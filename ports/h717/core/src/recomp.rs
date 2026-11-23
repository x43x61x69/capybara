use crate::emulator::{
    cpu::{Cpu, Registers},
    mmu::{map::SRAM_BASE, MemoryBus},
    peripherals::Peripherals,
};

struct Block {
    source: &'static [u8],
    steps: u32,
    max_cycles: u32,
    stack_low: i32,
    stack_high: i32,
    run: fn(&mut Cpu, &mut MemoryBus, &mut Peripherals, usize) -> u32,
}

// The generated subset is selected at build time. Some profiles use only
// register operations and have no calls to these memory/ALU helpers.
#[allow(dead_code)]
fn load_stack<const N: usize>(stack: &[u8], offset: usize) -> u32 {
    let mut word = [0u8; 4];
    word[..N].copy_from_slice(&stack[offset..offset + N]);
    u32::from_le_bytes(word)
}

#[allow(dead_code)]
fn store_stack<const N: usize>(stack: &mut [u8], offset: usize, value: u32) {
    stack[offset..offset + N].copy_from_slice(&value.to_le_bytes()[..N]);
}

#[allow(dead_code)]
fn arith(regs: &mut Registers, reg: usize, imm: u32, subtract: bool, write: bool) {
    let lhs = regs.r[reg];
    let (result, carry, overflow) = if subtract {
        let (result, borrow) = lhs.overflowing_sub(imm);
        (result, !borrow, (((lhs ^ imm) & (lhs ^ result)) as i32) < 0)
    } else {
        let (result, carry) = lhs.overflowing_add(imm);
        (result, carry, ((!(lhs ^ imm) & (lhs ^ result)) as i32) < 0)
    };
    if write {
        regs.r[reg] = result;
    }
    regs.set_nz(result);
    regs.set_flag_c(carry);
    regs.set_flag_v(overflow);
}

#[allow(unused_imports, unused_variables)]
mod generated {
    use super::{arith, load_stack, store_stack, Block, Cpu, MemoryBus, Peripherals};
    use crate::emulator::cpu::Thumb16;
    include!(concat!(env!("OUT_DIR"), "/recompiled.rs"));
}

pub(crate) fn step(
    cpu: &mut Cpu,
    bus: &mut MemoryBus,
    periph: &mut Peripherals,
    budget: u32,
) -> u32 {
    let Some(block) = generated::find_block(cpu.regs.pc) else {
        return 0;
    };
    execute(block, cpu, bus, periph, budget)
}

#[inline]
fn execute(
    block: Block,
    cpu: &mut Cpu,
    bus: &mut MemoryBus,
    periph: &mut Peripherals,
    budget: u32,
) -> u32 {
    let pc = cpu.regs.pc as usize;
    if block.steps > budget
        || !cpu.can_run_block(block.max_cycles, periph)
        || bus.pram.data.get(pc..pc + block.source.len()) != Some(block.source)
    {
        return 0;
    }
    let mut stack_index = 0;
    if block.stack_low != block.stack_high {
        let start = cpu
            .regs
            .get_sp()
            .wrapping_add_signed(block.stack_low)
            .wrapping_sub(SRAM_BASE) as usize;
        let size = (block.stack_high - block.stack_low) as usize;
        let Some(last_start) = bus.sram.data.len().checked_sub(size) else {
            return 0;
        };
        if start > last_start {
            return 0;
        }
        stack_index = start;
    }
    let cycles = (block.run)(cpu, bus, periph, stack_index);
    cpu.finish_execution(cycles, periph);
    block.steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator::cpu::StepResult;

    const PC: usize = 0x100;
    const BYTES: &[u8] = &[7, 0x20, 0, 0x90, 0, 0x99]; // MOVS r0, 7; STR r0, [sp]; LDR r1, [sp].

    fn example() -> Block {
        Block {
            source: BYTES,
            steps: 3,
            max_cycles: 5,
            stack_low: 0,
            stack_high: 4,
            run: |cpu, bus, _, stack_index| {
                let stack = &mut bus.sram.data[stack_index..stack_index + 4];
                cpu.regs.r[0] = 7;
                cpu.regs.set_nz(7);
                store_stack::<4>(stack, 0, cpu.regs.r[0]);
                cpu.regs.r[1] = load_stack::<4>(stack, 0);
                cpu.regs.pc += 6;
                bus.current_pc = PC as u32 + 4;
                5
            },
        }
    }

    fn machine() -> (Cpu, MemoryBus, Peripherals) {
        let mut cpu = Cpu::new();
        let mut bus = MemoryBus::default();
        cpu.regs.pc = PC as u32;
        cpu.regs.msp = SRAM_BASE;
        cpu.nvic.en_attente = false;
        bus.pram.data[PC..PC + BYTES.len()].copy_from_slice(BYTES);
        (cpu, bus, Peripherals::default())
    }

    #[test]
    fn native_stack_block_matches_the_interpreter() {
        let (mut slow, mut slow_bus, mut slow_periph) = machine();
        let (mut fast, mut fast_bus, mut fast_periph) = machine();
        assert_eq!(
            execute(example(), &mut fast, &mut fast_bus, &mut fast_periph, 3),
            3
        );
        for _ in 0..3 {
            assert!(matches!(
                slow.step(&mut slow_bus, &mut slow_periph),
                StepResult::Ok(_)
            ));
        }
        assert_eq!(
            (fast.cycles, fast.regs.pc, fast.regs.xpsr, fast.regs.r),
            (slow.cycles, slow.regs.pc, slow.regs.xpsr, slow.regs.r)
        );
        assert_eq!(fast_bus.sram.data, slow_bus.sram.data);
        assert_eq!(fast_bus.current_pc, slow_bus.current_pc);
    }

    #[test]
    fn native_block_falls_back_for_short_budgets_it_and_modified_code() {
        let (mut cpu, mut bus, mut periph) = machine();
        assert_eq!(execute(example(), &mut cpu, &mut bus, &mut periph, 2), 0);
        cpu.regs.itstate = 8;
        assert_eq!(execute(example(), &mut cpu, &mut bus, &mut periph, 3), 0);
        cpu.regs.itstate = 0;
        cpu.nvic.en_attente = true;
        assert_eq!(execute(example(), &mut cpu, &mut bus, &mut periph, 3), 0);
        cpu.nvic.en_attente = false;
        cpu.regs.msp = SRAM_BASE - 1;
        assert_eq!(execute(example(), &mut cpu, &mut bus, &mut periph, 3), 0);
        cpu.regs.msp = SRAM_BASE;
        bus.pram.data[PC] ^= 1;
        assert_eq!(execute(example(), &mut cpu, &mut bus, &mut periph, 3), 0);
        assert_eq!(cpu.cycles, 0);
        assert_eq!(cpu.regs.pc, PC as u32);
        assert_eq!(&bus.sram.data[..4], &[0; 4]);
    }
}
