# zcc — Reference Semantics ⟦·⟧ for HIR and MIR

**Status.** This is a *mechanized reference semantics*: a denotational semantics
for the two intermediate representations, realized by `src/hir/interp.rs` and
`src/mir/interp.rs` and exercised by the batteries in `src/*/tests.rs`. It is
**not** a machine-checked proof. By Rice's theorem semantic equivalence of
programs is undecidable in general, so every theorem below is quantified over a
*finite class of program shapes* and *checked mechanically*, never proved
universally. What it buys is Law 3: each layer is certified where the question is
still decidable, instead of deferring everything to the final binary and the
suite.

This document is the *mathematical definition of every instruction*. Each rule
maps one-to-one onto an arm of one interpreter — and, crucially, onto the SAME
arm a future constant folder will use, so the folder and the semantics are one
denotation function rather than two that must be kept in agreement.

Scope after the re-architecture (`REARCH.md`): the old single IR is gone. There
are now two levels, and the interpreters share ONE memory model
(`src/mem.rs`) — which is what makes `⟦hir⟧ = ⟦mir⟧` a meaningful equation
rather than a comparison of two different worlds.

See also: `THEORY.md` A6/A6b/A7 (the theorem catalogue), `REARCH.md` §10 (the
proof map), `tests/alg.sh` (the source-level fold-vs-runtime square this document
lifts to the IR level).

---

## 0. The two levels, and what each is for

```
HIR   target-independent SSA, block parameters, closed scalar type domain.
      ⟦hir⟧ is the meaning of the C program.               src/hir/interp.rs

MIR   AArch64 machine instructions, SSA over virtual registers (VIRTUAL phase)
      or physical registers (PHYSICAL phase). ONE interpreter for both, so
      instruction selection, register allocation and frame lowering are each an
      equality between two runs of the same function.      src/mir/interp.rs
```

---

## 1. Value domain

### 1.1 HIR

The type domain is CLOSED — six scalar types, and nothing else is ever a value:

```
Ty = I8 | I16 | I32 | I64 | F32 | F64          (pointers are I64; LP64)
```

Signedness is **not** part of the type. It lives in the opcode (`sdiv`/`udiv`,
`icmp.slt`/`icmp.ult`, `sext`/`zext`), which is what makes ⟦·⟧ a closed
definition needing no lookup into the frontend's type table.

A value is carried in a 64-bit word `Bits`:

- **Integers.** For `I32`/`I64` the carrier holds the value *sign-extended*, so a
  constant operand and a computed value of the same C value compare equal. For
  `I8`/`I16` only the low 8/16 bits are significant: `load` yields the raw bytes
  zero-extended, and every consumer of a narrow value reads exactly those bits
  (a `store`, a `zext`/`sext`, a narrow `trunc`). This is consistent because
  **HIR performs no arithmetic or comparison at I8/I16** — C99 6.3.1.1 promotes
  first, and `hir::build` makes that invariant explicit (`promote`).
- **Floats.** A value is the IEEE-754 bit pattern *of its own type*: `F32` is an
  `f32` pattern in the low 32 bits, `F64` an `f64` pattern. (The old IR widened
  every float to `f64` in registers; the closed type domain makes that
  unnecessary and removes the rounding hazard it created.) Bit patterns, not
  reals: NaN payloads and −0.0 are preserved, and no fold may reassociate.

### 1.2 MIR

Values are machine registers. The one deliberate difference from HIR:

> **A64 `w`-form results are ZERO-extended into the 64-bit register** (DDI 0487
> B1.2.1). ⟦mir⟧ models this exactly (`trunc(v, w)`), so a 32-bit value may
> differ bit-for-bit from ⟦hir⟧'s sign-extended carrier while denoting the same
> C value. Every square that crosses the boundary therefore compares **at the C
> return type**, not bitwise — this is not laxity, it is the statement that the
> two carriers are two encodings of one value.

Register classes: `Gpr` (x0–x30), `Fpr` (v0–v31), and `Flags` — NZCV as a class
of size k = 1, packed N=8, Z=4, C=2, V=1 as in PSTATE.

---

## 2. Machine state Σ

### 2.1 HIR

```
Σ = ⟨ ν , μ , stack ⟩
ν : ValueId → Bits          the SSA value environment of the current call
μ : Addr → Byte             flat little-endian LP64 memory (src/mem.rs)
```

μ has two regions, and address 0 is unmapped so a null dereference **traps**
rather than silently reading a global:

- `[GLOBAL_BASE, …)` — globals and string literals, materialized from their
  initializers. A string literal's array includes its terminating null
  (C99 6.4.5p6), which the parser does not store and the layout adds back.
- a downward-growing stack; each call reserves its function's stack objects.

Block parameters replace φ: taking an edge assigns the target's parameters from
the edge's arguments, simultaneously, before the target's first instruction.

### 2.2 MIR

```
Σ = ⟨ ν , φ , nzcv , μ , sp ⟩
ν : VReg → Bits             per call (empty once the function is physical)
φ : PReg → Bits             the PHYSICAL register file — PERSISTENT across calls
```

φ is persistent on purpose: an AAPCS64 argument written by the caller is the
*same object* the callee reads, so the calling convention is executed rather than
assumed. `sp` is a real address; every stack object is addressed from it (there
is no frame pointer — `mir/pass/frame.rs`).

**The callee-saved contract.** AAPCS64 §6.1.1 promises x19–x28, v8–v15 and x30
survive a call. Before frame lowering nothing in a function preserves them — yet
the allocator already relied on the promise when it put long-lived values there.
So ⟦mir⟧ honors the promise on behalf of any function that has **not** been
through `pass/frame.rs`, and honors nothing for one that has, where real
`Spill`/`Reload` instructions keep it. The difference between those two runs is
exactly the obligation of frame lowering (§6.4).

---

## 3. Atomic denotations — the faithfulness keystone

These are the functions both the interpreter and (later) the constant folder
call, so there is one denotation and not two.

### 3.1 Carrier normalization
```
mask_τ(v)  =  sign-extend the low bits_τ bits into 64          (I8/I16/I32; identity at I64)
zext_τ(v)  =  the low bits_τ bits, zero-filled
sext_τ(v)  =  mask_τ(v) read as a signed integer
```

### 3.2 `⟦binop⟧_τ : Bits × Bits → Bits ∪ {⊥}`

Integer operations are arithmetic in ℤ/2^bits(τ), with the operands read signed
or unsigned according to the OPCODE:

| opcode | denotation | note |
|---|---|---|
| `add`, `sub`, `mul` | wrapping in ℤ/2^w | C99 leaves signed overflow undefined; ⟦·⟧ *defines* it as wrapping, which REFINES ⊥ and is therefore legal (§7) |
| `sdiv`, `srem` | truncation toward zero | C99 6.5.5p6. Divisor 0 ⟹ **⊥** |
| `udiv`, `urem` | unsigned | divisor 0 ⟹ **⊥** |
| `and`, `or`, `xor` | bitwise | |
| `shl`, `lshr`, `ashr` | shift by `count mod w` | C99 6.5.7p3 leaves `count ≥ w` undefined; the A64 shifts take it modulo the width, and that is what ⟦·⟧ defines |
| `fadd`, `fsub`, `fmul`, `fdiv` | IEEE-754 at the operand type | not associative; no fold may reassociate |

### 3.3 `⟦cmp⟧_τ : Bits × Bits → {0,1}`

Integer predicates read their operands signed (`slt/sle/sgt/sge`) or unsigned
(`ult/ule/ugt/uge`); `eq`/`ne` are signedness-free.

Floating predicates split on ordering, and the distinction is load-bearing:

| predicate | true when |
|---|---|
| `foeq`, `folt`, `fole`, `fogt`, `foge` | ORDERED: false if either operand is NaN |
| `fune` | `!(a == b)` — **true when unordered** |
| `funo` | either operand is NaN |

> C99 6.5.9: `a != b` is `!(a == b)`, so on NaN it is **true**. It is the
> UNORDERED not-equal (`fune`), not the ordered one. HIR spells both so the
> distinction cannot be lost in instruction selection — where it matters, because
> `fune` is a plain `ne` on the A64 flags while ordered `!=` has no single
> condition code at all.

### 3.4 `⟦cvt⟧_{σ→τ}` (C99 6.3.1.2 / 6.3.1.4 / 6.3.1.5)

`sext`/`zext`/`trunc` are the carrier operations of §3.1. `fpext`/`fptrunc`
convert between `F32` and `F64` with IEEE rounding. `sitofp`/`uitofp` read the
source as signed/unsigned. `fptosi`/`fptoui` truncate **toward zero**; a value
out of range is undefined in C and A64 `fcvtzs`/`fcvtzu` SATURATE, so ⟦·⟧
defines saturation (again a refinement of ⊥). `bitcast` reinterprets the bits.

`(_Bool)x` is **not** a truncation: it is `x != 0` (C99 6.3.1.2), and on a
floating operand it uses `fune`, so `(_Bool)NaN` is 1.

---

## 4. HIR instruction semantics ⟦Inst⟧ : Σ → Σ

Big-step, one rule per instruction; `↓` reads an operand (a value from ν, or a
constant, which takes its type from the enclosing instruction).

```
⟦bin  d, op, τ, a, b⟧      ν' = ν[d ↦ ⟦op⟧_τ(a↓, b↓)]
⟦un   d, op, τ, a⟧         neg = 0−a in ℤ/2^w · not = bitwise complement ·
                           fneg = IEEE SIGN FLIP (not 0−x: defined on NaN and −0.0)
⟦cmp  d, p, τ, a, b⟧       ν' = ν[d ↦ ⟦p⟧_τ(a↓, b↓)]                      d : I32 ∈ {0,1}
⟦cvt  d, c, σ→τ, a⟧        ν' = ν[d ↦ ⟦c⟧_{σ→τ}(a↓)]
⟦load d, τ, p⟧             ν' = ν[d ↦ μ[p↓ .. p↓+bytes(τ)]]               unmapped ⟹ ⊥
⟦store τ, p, v⟧            μ' = μ[p↓ .. ↦ v↓]                             unmapped ⟹ ⊥
⟦slotaddr d, k, c⟧         ν' = ν[d ↦ base(slot k) + c]
⟦symaddr d, s⟧             ν' = ν[d ↦ address of s]
⟦select d, τ, c, a, b⟧     ν' = ν[d ↦ (c↓ ≠ 0 ? a↓ : b↓)]    both arms already evaluated
⟦call d, sig, f, args⟧     run ⟦f⟧ on the argument values; ν' = ν[d ↦ result]
⟦alloca d, n, α⟧           reserve n bytes on the stack; ν' = ν[d ↦ base]
⟦memcpy p, q, n⟧           μ' = μ with n bytes copied
⟦memset p, b, n⟧           μ' = μ with n bytes set
⟦intrinsic …⟧              the EXT surface — opaque to every pass (Effect::Call)
```

An instruction's **effect class** (`Pure | Read | Write | Call`) is a property of
the instruction, consulted by every pass instead of a hand-written opcode list. A
*volatile* access is `Call`-class: C99 6.7.3 forbids removing, duplicating or
reordering it, which is exactly the discipline applied to a call.

## 4b. Terminator semantics ⟦Term⟧ : Σ → (BlockId ⊎ Halt)

```
⟦jmp  T⟧               take edge T
⟦br   c, T, F⟧         c↓ ≠ 0 ? take T : take F                (c : I32)
⟦switch v, τ, ks, D⟧   the arm whose key equals sext_τ(v↓), else D
⟦ret  v?⟧              Halt with v↓ (or none)
⟦unreachable⟧          ⊥
⟦goto_ptr v, targets⟧  the block whose address is v↓            EXT(gcc)
```

*Taking an edge T = (b, args)* assigns b's parameters from `args`,
**simultaneously**. That single sentence is the whole of the φ semantics the old
IR needed a predecessor-tracking rule for.

## 4c. Function big-step ⟦Func⟧ : Bits* → Bits ∪ {⊥}

Reserve the function's stack objects; seed the incoming parameters; run blocks
from the entry until a terminator halts; release the stack. The observable is the
return value. External symbols resolve to a small builtin table
(`memcpy`/`memset`/`strlen`/…); anything else is ⊥ — a function that reaches it
is simply not usable as a proof witness.

---

## 5. MIR semantics

### 5.1 ALU and NZCV

`⟦alu⟧` is `⟦binop⟧` at the instruction's width, with `w`-form results
zero-extended (§1.2). The flag-setting forms additionally produce NZCV by the
manual's `AddWithCarry` (DDI 0487 C6.2): N = sign of the result, Z = result is
zero, C = carry out (for `sub`, of `a + ¬b + 1`), V = signed overflow. `cmp`,
`cmn` and `tst` are `subs`, `adds` and `ands` discarding the result.

`⟦cc⟧(nzcv)` is the manual's condition table: `eq`=Z, `ne`=¬Z, `hs`=C, `lo`=¬C,
`mi`=N, `pl`=¬N, `vs`=V, `vc`=¬V, `hi`=C∧¬Z, `ls`=¬C∨Z, `ge`=N=V, `lt`=N≠V,
`gt`=¬Z∧N=V, `le`=Z∨N≠V.

`fcmp` sets NZCV per C6.2: **unordered sets C and V and clears N and Z**; equal
sets Z and C; less sets N; greater sets C. Each ordered predicate of §3.3 has a
single condition that is false when unordered, which is why `folt` maps to `mi`
and not `lt`.

### 5.2 Operands

An `Rhs` is a register, a shifted register, an extended register, or an
immediate; an `AddrMode` is `[base,#off]`, `[base,idx,ext #k]`, pre/post-index
(which DEFINE a new base register, so the SSA property survives), a stack slot,
or a symbol's low-12 offset. All of them denote by the manual's definition, and
`mir/isa.rs` decides which immediates exist at all.

> One encoding fact that is easy to lose and changes meaning: in the ADD/SUB
> **immediate** form, register 31 encodes **SP**, not ZR. `add w0, wzr, #5` is
> not an instruction. The shifted-register and logical-immediate forms do read 31
> as ZR.

### 5.3 Calls

`Call` carries the ABI as fixed operand constraints plus a clobber set. Executing
it moves each constrained operand into the register the ABI named, runs the
callee against the same physical file, and moves the results back. Argument
placement is a `ParallelCopy` immediately before — simultaneous assignment,
sequentialized after allocation (§6.3).

### 5.4 Stack objects

`Spill`/`Reload`/`SlotAddr` address a stack object by id. Before
`pass/frame.rs` each object is its own region; after, all of them live at
assigned offsets inside one frame and `frame_size` is its size — which may
legitimately be **zero** for a leaf that needs no stack.

### 5.5 Bit-field extract and the paired forms

These are the denotations the R3 machine passes (`isel` munch, `ldst_pair`)
introduce; each is the atom its commuting-square battery compares against.

```
⟦Bfx u  w, d, s, lsb, wid⟧   f = zext of bits [lsb+wid-1 .. lsb] of s↓;   ν' = ν[d ↦ f]   (ubfx)
⟦Bfx s  w, d, s, lsb, wid⟧   f = sext of that same field (bit lsb+wid-1 replicated);      (sbfx)
                             then the `w`-form rule of §1.2: a W32 result ZEROES bits 63:32
⟦Pair load  w, a, b, m⟧      addr = ⟦m⟧;  ν' = ν[a ↦ μ[addr .. addr+w], b ↦ μ[addr+w .. addr+2w]]
⟦Pair store w, a, b, m⟧      addr = ⟦m⟧;  μ' = μ[addr .. ↦ a↓][addr+w .. ↦ b↓]
```

`Bfx` denotes exactly `and(lshr(s, lsb), (1<<wid)−1)` for the unsigned form and
its sign-extended twin for the signed one — which is *why* the munch table may
fold `and(lshr(a,s), mask)` and `shl+ashr` into it. `Pair` names the FIRST of
two registers; the second is at `+w.bytes()` (DDI 0487 C6.2.130), so a pair is
two independent single accesses with one caveat the fusing pass must honor and
⟦·⟧ makes visible: a **load** must not name a destination twice and must not
clobber a base it is still addressing through — otherwise the two-access
denotation and the one-instruction form diverge.

---

## 6. The commuting squares (REARCH §10 made concrete)

Each is an equality between two runs, with no assembler, linker or hardware in
the loop. All are quantified over the battery's program shapes, and all compare
only inputs on which neither side is ⊥ (§7).

### 6.1 `⟦build(parse(src))⟧ = the value C99 assigns src`
The lowering battery (`src/hir/tests.rs`). The oracle is the STANDARD,
transcribed by hand — never "what zcc currently prints".

### 6.2 `⟦f⟧ = ⟦P f⟧` for an HIR pass P, and `⟦m⟧ = ⟦P m⟧` for a MIR pass
Shipped and exercised (R2/R3). The harness is the one above, run on both sides of
each pass. HIR: `cfg_simplify`, `sroa+mem2reg`, `sccp`, `gvn`, `load_elim/dse`,
`dce`, `inline`, `licm`, `if_convert`, `sink`. MIR-SSA (under `mir::interp`):
`cmp_elim` — a fused `subs`/`ands` denotes the same result AND the same NZCV as
the separate op-then-`cmp` only for the condition codes reading N/Z alone (§5.1),
which is the square's whole content; `ext_lattice` — an extension whose source
already satisfies the width fact (§1.2) is the identity, and the fact is
established only by instructions whose architectural definition establishes it;
`ldst_pair` — §5.5. Each `⬜` row of §A7b (iv/LFTR, rotate, `auto_inc`,
`shrink_wrap`) owes the same equality when it lands.

### 6.3 `⟦hir⟧ = ⟦mir_v⟧` (instruction selection) and `⟦mir_v⟧ = ⟦mir_p⟧` (allocation)
`src/isel/tests.rs` and `src/regalloc/tests.rs`. The second is a **renaming
bisimulation**: allocation renames values and may route some through memory, but
must not change what the function computes. Alongside it, structural obligations
the interpreter cannot see — no virtual register survives, every ABI-fixed
operand is satisfied, every `Reload` is DOMINATED by a `Spill` of its slot, no
`ParallelCopy` remains.

### 6.4 `⟦mir_p⟧ = ⟦mir_final⟧` (frame lowering and block layout)
`src/mir/pass/tests.rs`. Because ⟦mir⟧ honors the callee-saved contract for a
function that has no prologue and not for one that has (§2.2), this equality
states exactly: *frame lowering realizes, in instructions, the ABI assumption the
allocator made* — plus *layout reorders blocks and inverts conditions without
changing an edge*.

### 6.5 Emission
Identical MIR ⟹ identical bytes, sealed across FRESH processes so a per-process
hash seed cannot leak into the output (`tests/determinism.sh`). Assembler
acceptance and end-to-end behaviour are CONFIRMED by the suites — never
discovered there.

---

## 7. ⊥ and refinement

A trap — C99 undefined behavior reached (division by zero, an out-of-range
access), a missing external, `unreachable`, or exhausting the step budget — is
**⊥**. ⊥ is not a wrong answer; it is the absence of one.

A transform may **refine** ⊥ into anything. That is why ⟦·⟧ is free to *define*
signed overflow as wrapping, shifts as modulo the width, and float-to-int
conversion as saturating: each refines a ⊥ the standard left open, and each
matches what the machine does, so the compiler and the semantics agree. It is
also why every square compares only inputs on which neither side traps —
comparing at ⊥ would reject a legal transform.

The step budget deserves naming: it makes non-termination ⊥. This is sound (an
infinite loop has no observable value) but it means a square is silent about
programs that do not terminate, which is one of the reasons the corpus suites
still exist.

---

## 8. Limitations, honestly

- **Not machine-checked.** Everything here is validated by execution over a
  finite class of shapes. The next rung is per-pass machine-checked proof; the
  rung after is a proof-carrying pipeline.
- **The modelled space is smaller than the real one.** rc3's history is explicit
  about this: passes that were *proven* on the commuting-square space still
  regressed on real programs (inlining, strength reduction), and the box torture
  caught what the model was blind to. The square is a *discovery* instrument; the
  suites *confirm*.
- **Floating point is modelled by the host's `f32`/`f64`.** Correct for the
  operations defined here, but it says nothing about contraction (`fma`), about
  excess precision, or about rounding modes.
- **Concurrency is not modelled.** `volatile` is honored structurally (a
  `Call`-class effect: never removed, duplicated or reordered), and the
  `__sync_*` intrinsics are opaque, but there is no memory model.
- **No I/O trace.** The observable is the return value. A function whose meaning
  is its output stream is checked by the differential suites, not here.
