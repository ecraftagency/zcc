// The memory model both reference interpreters share (`hir::interp`,
// THEORY A6 — the HIR layer's state; SEMANTICS 2.1 — the machine state Σ
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

/// THEORY A8 — the reference interpreter's address map, not a target fact
// The interpreter's address space. These are choices of the SEMANTICS, not of
// the target: nothing here reaches emitted code, and no compiled program can
// observe them. What they must satisfy is only that the regions are disjoint,
// that address 0 is unmapped (so a null dereference is ⊥ rather than a silent
// read), and that the tag bits lie above any real address.
/// first data address; everything below is unmapped, so null traps
pub const GLOBAL_BASE: u64 = 0x10_000;
/// THEORY A8 — the reference interpreter's address map
/// Stack available to an interpreted call chain. A battery that exhausts it
/// gets `Trap::BadAddress`, i.e. ⊥ — which is sound but useless as a proof, so
/// a battery needing deeper recursion raises this rather than working around it.
pub const STACK_SIZE: u64 = 1 << 20;
/// THEORY A8 — the reference interpreter's address map
pub const STACK_TOP: u64 = 0x8000_0000;
/// THEORY A8 — the reference interpreter's address map
pub const STACK_BASE: u64 = STACK_TOP - STACK_SIZE;
/// THEORY A8 — the reference interpreter's pointer tagging
/// Tag bits marking a function address and a block address. Above STACK_TOP, so
/// they can never collide with a data or stack address.
pub const FUNC_TAG: u64 = 1 << 40;
/// THEORY A8 — the reference interpreter's pointer tagging
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
    /// `n` is at most 8: a `u64` is the widest scalar μ hands back, and a
    /// 16-byte access (`Width::Q`) is two of these by construction. Asserting it
    /// here is Law 3 at the layer that owns the invariant — the alternative is a
    /// shift that overflows in debug and silently loses the top half in release.
    pub fn load(&mut self, a: u64, n: u32) -> Result<u64, Trap> {
        debug_assert!(n <= 8, "μ load of {} bytes: a scalar access is at most 8", n);
        let s = self.slice(a, n as u64)?;
        let mut v = 0u64;
        for (i, &b) in s.iter().enumerate() {
            v |= (b as u64) << (8 * i);
        }
        Ok(v)
    }
    pub fn store(&mut self, a: u64, n: u32, v: u64) -> Result<(), Trap> {
        debug_assert!(n <= 8, "μ store of {} bytes: a scalar access is at most 8", n);
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
        // C99 6.4.5p6: the terminating null is part of the literal's array but
        // is not stored in `strs`.
        data.push(0);
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
