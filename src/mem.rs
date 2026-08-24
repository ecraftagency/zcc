// The memory model both reference interpreters share (`hir::interp`,
// `mir::interp`). Sharing it is what makes `⟦hir⟧ = ⟦mir_v⟧ = ⟦mir_p⟧` a
// meaningful equation: the two sides differ only in the instruction set, never
// in where an object lives or what a pointer means.
//
// μ is flat little-endian LP64 byte memory in two regions — the module's
// globals and string literals, materialized from their initializers, and a
// downward-growing stack. Address 0 is unmapped, so a null dereference traps
// instead of quietly reading a global.
use crate::ast::{Ast, GInit};

#[derive(Debug, Clone, PartialEq)]
pub enum Trap {
    DivZero,
    BadAddress(u64),
    NoSuchFunction(String),
    Unreachable,
    /// the step budget: a non-terminating run is ⊥ for proof purposes
    OutOfSteps,
}

pub const GLOBAL_BASE: u64 = 0x10_000;
pub const STACK_SIZE: u64 = 1 << 20;
pub const STACK_TOP: u64 = 0x8000_0000;
pub const STACK_BASE: u64 = STACK_TOP - STACK_SIZE;
/// tag bits distinguishing a function address and a block address from data
pub const FUNC_TAG: u64 = 1 << 40;
pub const LABEL_TAG: u64 = 1 << 41;

pub struct Mem {
    data: Vec<u8>,
    stack: Vec<u8>,
    pub sp: u64,
}

impl Mem {
    fn slice(&mut self, a: u64, n: u64) -> Result<&mut [u8], Trap> {
        let end = a.checked_add(n).ok_or(Trap::BadAddress(a))?;
        if a >= GLOBAL_BASE && end <= GLOBAL_BASE + self.data.len() as u64 {
            let o = (a - GLOBAL_BASE) as usize;
            return Ok(&mut self.data[o..o + n as usize]);
        }
        if a >= STACK_BASE && end <= STACK_TOP {
            let o = (a - STACK_BASE) as usize;
            return Ok(&mut self.stack[o..o + n as usize]);
        }
        Err(Trap::BadAddress(a))
    }
    pub fn load(&mut self, a: u64, n: u32) -> Result<u64, Trap> {
        let s = self.slice(a, n as u64)?;
        let mut v = 0u64;
        for (i, &b) in s.iter().enumerate() {
            v |= (b as u64) << (8 * i);
        }
        Ok(v)
    }
    pub fn store(&mut self, a: u64, n: u32, v: u64) -> Result<(), Trap> {
        let s = self.slice(a, n as u64)?;
        for (i, b) in s.iter_mut().enumerate() {
            *b = (v >> (8 * i)) as u8;
        }
        Ok(())
    }
    /// Reserve `bytes` (rounded to the 16-byte AAPCS64 stack alignment) and
    /// return the new stack pointer, which is the base of the new frame.
    pub fn push_frame(&mut self, bytes: u64) -> Result<u64, Trap> {
        let n = (bytes + 15) & !15;
        if self.sp < STACK_BASE + n {
            return Err(Trap::BadAddress(self.sp));
        }
        self.sp -= n;
        let o = (self.sp - STACK_BASE) as usize;
        for b in &mut self.stack[o..o + n as usize] {
            *b = 0;
        }
        Ok(self.sp)
    }
    pub fn pop_frame(&mut self, bytes: u64) {
        self.sp += (bytes + 15) & !15;
    }
}

/// Where each global and string literal was placed.
pub struct Layout {
    pub globals: Vec<u64>,
    pub strs: Vec<u64>,
}

pub fn build(ast: &Ast) -> (Mem, Layout) {
    let mut data: Vec<u8> = Vec::new();
    let align_to = |d: &mut Vec<u8>, a: usize| while d.len() % a != 0 { d.push(0) };
    let mut globals = Vec::with_capacity(ast.globals.len());
    for g in &ast.globals {
        let (size, align) = (ast.tt.size(g.ty) as usize, ast.tt.data_align(g.ty) as usize);
        align_to(&mut data, align.max(1));
        globals.push(GLOBAL_BASE + data.len() as u64);
        data.resize(data.len() + size.max(1), 0);
    }
    let mut strs = Vec::with_capacity(ast.strs.len());
    for s in &ast.strs {
        align_to(&mut data, 8);
        strs.push(GLOBAL_BASE + data.len() as u64);
        data.extend_from_slice(s);
    }
    let mut mem = Mem {
        data,
        stack: vec![0; STACK_SIZE as usize],
        sp: STACK_TOP,
    };
    let lay = Layout { globals, strs };
    for (i, g) in ast.globals.iter().enumerate() {
        let (base, size) = (lay.globals[i], ast.tt.size(g.ty));
        init(&mut mem, &lay, base, size, &g.init);
    }
    (mem, lay)
}

/// `size` is the width of THIS item: a `List` carries one per element, so a
/// scalar member never borrows the aggregate's size.
fn init(mem: &mut Mem, lay: &Layout, at: u64, size: u32, g: &GInit) {
    match g {
        GInit::None => {}
        GInit::Num(k) => {
            let _ = mem.store(at, size.clamp(1, 8), *k as u64);
        }
        GInit::Str(i) => {
            let _ = mem.store(at, 8, lay.strs[*i as usize]);
        }
        GInit::StrOff(i, off) => {
            let a = (lay.strs[*i as usize] as i64 + off) as u64;
            let _ = mem.store(at, 8, a);
        }
        GInit::Bytes(b) => {
            if let Ok(s) = mem.slice(at, b.len() as u64) {
                s.copy_from_slice(b);
            }
        }
        GInit::List(items) => {
            for (off, isz, it) in items {
                init(mem, lay, at + *off as u64, *isz, it);
            }
        }
        // A relocation against another symbol: the interpreter resolves symbols
        // by index, not by name, so these stay zero until a battery needs them.
        GInit::Addr(..) | GInit::Diff(..) => {}
    }
}
