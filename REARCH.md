# REARCH.md — the layered-backend re-architecture of zcc (branch `mir-rearch`)

> **THIS FILE IS THE PLAN OF RECORD for branch `mir-rearch`.** It supersedes `OPT.md §0` spine row
> #25 (a user "re-plan", 2026-08-24). A new session opens by reading §0 (boot), then works the
> milestone ladder in §12. Chat is not memory; this file is. Edit it in place — never fork a second
> plan document (the anti-fragmentation law of `OPT.md` binds here too).

---

## §0 BOOT — read this first in every session

**State at the time of writing (2026-08-24, evening):**
- `main` @ `4fb7a0a` = the old optimizer, sqlite **1.768×** gcc-O1 (static insns), tagged **`rc3`** (pushed).
  It is the fallback. It is NOT to be touched, grinded, or referenced for structure.
- `mir-rearch` = this branch, created from `rc3`. **It is the repository's DEFAULT branch** (GitHub
  default + `origin/HEAD`, set 2026-08-24). `main` is frozen at `rc3` and left alone. Nothing
  implemented yet — only this document.
- The box: `docker exec zccbox …`, suites cached at `/suites` (`ZCC_SUITE_CACHE=/suites`), build with
  `CARGO_TARGET_DIR=/ltarget cargo build --release && cp /ltarget/release/zcc /usr/local/bin/zcc`.

**The directive (user, verbatim intent): "NO HALF MEASURE, MUST FULL MEASURE."**
Big-bang rewrite of everything below the AST boundary. Old code may be consulted ONLY for theorems,
commuting-square proofs, oracles, and test suites — NEVER for architecture ("it taints: not
correctness, but it tied our hands"). If the AST prevents the design, erase it too. No toggle-gated
"alongside" migration, no incremental port of the old backend, no peephole grinding. Plan the whole
architecture from first principles; implement from zero LOC.

**Why (the post-mortem, so it is never repeated):** week 1 built a tcc-class compiler in ~5 hours and
C99 + gcc extensions in days; then FIVE days of grinding 0.1–0.4% levers. The nuclear allocator (#25)
collapsed on entry: the allocator was already Chaitin-Briggs + coalescing + a wide budget. The real
disease is architectural and was proven by measurement:
1. `optimize_ssa = to_ssa ▸ passes ▸ out_of_ssa ▸ abi_alloc` — **allocation ran AFTER SSA
   destruction**, on a non-chordal graph (NP-hard → heuristic → one whole-life home per temp →
   live-range splitting structurally impossible → 27k frame-slot mem-ops on sqlite).
2. **The backend was text**: instruction selection fused into a 1.9k-LOC emitter, 2k LOC of
   string-matching peepholes compensating for it. Addressing modes, flags, post-index — none
   expressible in the IR, so every machine optimization was a fragile `.s` hack.
3. The IR had no `switch`, no machine operands, consulted the frontend's TyTab from inside passes.

QBE reaches ~gcc-O1 with ONE SSA IR and allocation ON SSA (chordal → polynomial, optimal), ~11k LOC.
zcc solved the hard version of the problem. This document adopts the right version.

**Next-session checklist:**
1. `git switch mir-rearch`; read this file fully.
2. Start §12 **R0** at its first unchecked task. Follow the order; the allocator lands FIRST, not last.
3. Every module ships with its verifier + interpreter-based proof battery before the next module.
4. Bank each R-milestone with a commit + measurement line in §12; push.

---

## §1 Ground rules — what dies, what survives

| category | items | rule |
|---|---|---|
| **DIES** (written from zero) | `src/ir.rs` (1,858 LOC), `src/opt/*` (8,683), `src/codegen/*` (6,614) = **17,155 LOC, 70% of zcc** | delete at R0 start (`git rm`); never copy code from them |
| **SURVIVES as INPUT** | `src/lexer.rs`, `src/preprocess.rs`, `src/parser.rs`, `src/ast.rs`, `src/ext.rs`, `src/main.rs` (~7.1k) | the AST + TyTab boundary (charter Article B). The failure is entirely below it. `main.rs` loses its call into `codegen`; `ext.rs` keeps its frontend hooks; IR-side `EXT(...)` lowerings are re-implemented in the new layers |
| **SURVIVES as REFERENCE ONLY** | `THEORY.md` (A7 pass ladder = the list of theorems to re-realize; II-3 AAPCS64; II-4 ELF; II-5 arch), `SEMANTICS.md` (`⟦·⟧` definitions, re-targeted to HIR + extended to MIR), `tests/` (all suites, oracles, science gates, `bench/`, `corpus25.sh`, `exectime.sh`), `tests/README.md` | theorems and measurement, never structure |
| **ARCHIVED** | `OPT.md` | its own charter says it is transient; keep on the branch until merge, then delete. Scoreboard baselines (§0 numbers) are quoted in §13 here |

Constraints that do not change: Rust, edition 2024, **zero external crates**, single crate, AArch64-ELF
only (macOS = clang oracle only), strict C99 + marked `EXT(...)`, Laws 1–3 + Article E gates.

---

## §2 Pipeline and module layout

```
C ──cpp/lex/parse──► AST + TyTab
   ──lower (Braun on-the-fly SSA)──► HIR (SSA, target-independent, block parameters)
   ──HIR passes (tree-SSA half)──► HIR
   ──isel (maximal munch + AAPCS64 automaton)──► MIR (SSA, virtual registers, arm64 ops)
   ──MIR-SSA passes (cmp-elim, auto-inc, sxtw-lattice, ldp/stp)──► MIR
   ──regalloc (Braun-Hack spill → chordal color → biased coalesce → SSA destruct)──► MIR (physical)
   ──frame lowering / shrink-wrap / block layout / structured peephole──► MIR (final)
   ──emit (1:1 print from the ISA table)──► .s
```

MIR before and after allocation is ONE type in two lifecycle states (virtual/SSA vs physical/ordered),
exactly as LLVM's "MIR". MIR is arm64-specific by design (Article B: one module per target); a
target-independent middle layer is deferred until a second target exists.

```
src/hir/mod.rs        HIR types: Func, Block, Inst, Term, Value, Ty, Effect
src/hir/build.rs      AST → HIR lowering; Braun SSA construction (also used by mem2reg)
src/hir/verify.rs     SSA dominance property, arity, typing
src/hir/interp.rs     ⟦hir⟧ — the executable reference semantics (SEMANTICS.md)
src/hir/dom.rs        preds/succs, dominator tree (Cooper-Harvey-Kennedy), loop forest + depth
src/hir/alias.rs      the memory oracle (THEORY A7 "ALIAS ANALYSIS", re-derived)
src/hir/pass/*.rs     one file per theorem family (see §4)
src/mir/mod.rs        MIR types: MFunc, MBlock, MInst, Reg, Operand, AddrMode, constraints
src/mir/isa.rs        Side-II tables: register files, classes, encodable-immediate predicates, ISA shapes
src/mir/verify.rs     SSA (virtual phase) / constraint satisfaction (physical phase)
src/mir/interp.rs     ⟦mir⟧ — one interpreter for virtual and physical MIR
src/mir/pass/*.rs     cmp_elim, auto_inc, ext_lattice, ldst_pair, frame, shrink_wrap, layout, peephole
src/isel/lower.rs     HIR → MIR, per-block bottom-up munch over single-use trees
src/isel/pattern.rs   the pattern table (each row = one theorem, one battery test)
src/isel/abi.rs       AAPCS64 C.1–C.15 automaton, varargs, HFA, sret, stack args
src/isel/imm.rs       immediate legalization (imm12 / logical / movz-movk / shifts)
src/regalloc/live.rs  liveness on SSA with block parameters
src/regalloc/spill.rs Braun-Hack Belady spilling + rematerialization + SSA reconstruction
src/regalloc/color.rs chordal greedy coloring in dominance preorder, biased coalescing
src/regalloc/destruct.rs  block-arg → parallel copies → sequentialized moves
src/regalloc/verify.rs    interference / constraints / slot dataflow / clobber safety
src/emit.rs           MIR(final) → text (+ ELF directives, Side-II II-4)
```

---

## §3 HIR — target-independent SSA

### 3.1 Design decisions
- **SSA from birth.** `build.rs` lowers the AST straight into SSA with Braun et al. 2013 ("Simple and
  Efficient Construction of Static Single Assignment Form", on-the-fly, no dominance frontiers — already
  a proven theorem in this project). Scalar locals become values; aggregates and address-taken locals
  become `alloca` stack objects accessed by typed load/store. SROA + mem2reg (Braun again, on allocas)
  promote the split pieces later. **There is no `out_of_ssa` anywhere in HIR.**
- **Block parameters instead of φ instructions** (Cranelift/MLIR/Swift style):
  `br %c, bb1(%a, %b), bb2(%c)`. Edge semantics are explicit; the verifier and the interpreter are
  simpler; SSA destruction (in MIR) becomes literally "one parallel copy per edge". HIR and MIR share
  this model — one mental model.
- **Closed scalar types**: `Ty = I8 | I16 | I32 | I64 | F32 | F64`; pointers are `I64`. Signedness and
  width live in the **opcode**, not in a TyTab lookup (`sdiv/udiv`, `srem/urem`, `sext/zext/trunc`,
  `icmp.slt/ult`, `ashr/lshr`). After lowering, HIR is independent of the frontend's `TyTab`.
  SEMANTICS.md §3 (`canon_τ`, `⟦op⟧_τ`, `⟦cast⟧`) becomes a closed definition over this `Ty`.
- **Effect class per instruction** — `Effect = Pure | Read | Write | Call | Control`. DCE, CSE, GVN,
  LICM, sinking legality are a table lookup, never a per-pass hand-list.
- **Calls carry the C signature** (param types incl. composites by (size, align, class-hint), return
  type, `nfix` for variadics). ABI classification is NOT an HIR concern — it is isel's Side-II job.

### 3.2 Instruction set
```
Value operands: %v (SSA value) | const (iconst ty k | fconst ty bits) | sym (global/function address)
Arithmetic  : add sub mul  sdiv udiv srem urem   and or xor  shl lshr ashr      (ty ∈ I*)
Float       : fadd fsub fmul fdiv fneg                                          (ty ∈ F*)
Compare     : icmp.{eq,ne,slt,sle,sgt,sge,ult,ule,ugt,uge}  fcmp.{oeq,one,olt,ole,ogt,oge,uno}  → I32 0/1
Convert     : sext zext trunc (int↔int)  fptosi fptoui sitofp uitofp fpext fptrunc  bitcast(I64↔F64, I32↔F32)
Memory      : load ty %addr [aclass]   store ty %addr %val [aclass]   alloca size align → I64   memcpy %dst %src n   memset %dst n
              (aclass = the C effective type's alias class, assigned by the frontend lowering: the hook for
               type-based alias analysis (TBAA, O2 `-fstrict-aliasing`) — cheap to carry from day one, expensive to retrofit)
Address     : addr_global sym  addr_func sym  addr_label sym (EXT computed goto)   (address arithmetic = plain add/mul)
Select      : select ty %c %a %b
Call        : call sig callee(args…) → %r?      (callee = sym | %fnptr)
Intrinsics  : va_start %ap  va_arg ty %ap → %r  va_area → %r  overflow.{add,sub,mul}.{s,u}.ty %a %b %rp → %flag
              sync.{fetch_add,…} …  asm "tmpl" operands   (each = Effect::Call-class, opaque to passes)
Terminators : jmp bb(args)  br %c, bb(args), bb(args)  switch %v, [(k, bb(args))…], default bb(args)
              ret %v?  unreachable  goto_ptr %v (EXT)
```
`switch` is NEW (the old IR had none — hence no jump tables, hence the "5.1 switch quarantined" note).

### 3.3 Data structures (sketch)
```rust
pub struct Func { name, sig: Sig, blocks: Vec<Block>, values: Vec<ValueInfo>, allocas: Vec<Alloca>, entry: BlockId }
pub struct Block { params: Vec<Value>, insts: Vec<Inst>, term: Term, weight: Freq /* static branch-probability estimate (Ball-Larus heuristics); drives layout + spill next-use weighting; PGO hook */ }
pub struct ValueInfo { ty: Ty, def: Def /* Inst(bi, ii) | Param(bi, k) | FuncParam(k) */ }
pub enum Inst { Bin{dst, op: BinOp, ty, a, b}, Un{..}, Cmp{..}, Cvt{..}, Load{..}, Store{..}, Alloca{..},
                Addr{..}, Select{..}, Call{..}, Intrinsic{..} }
pub enum Term { Jmp(Target), Br(Operand, Target, Target), Switch(Operand, Vec<(i64, Target)>, Target),
                Ret(Option<Operand>), Unreachable, GotoPtr(Operand) }
pub struct Target { block: BlockId, args: Vec<Operand> }
```
Analyses (cached on `Func`, invalidated by any CFG edit): `Cfg{preds,succs}`, `DomTree` (Cooper-Harvey-
Kennedy iterative — simpler than Lengauer-Tarjan, adequate), `LoopForest{header, body, depth, latch,
preheader}`, `Alias` (the B1 oracle: allocas non-escaping ⟹ disjoint; globals by symbol; TBAA-free
otherwise = may-alias).

### 3.4 Interpreter `⟦hir⟧` and verifier
`interp.rs`: Σ = ⟨values: Vec<Bits>, memory: flat byte array (LP64 layout, globals materialized),
call stack⟩; big-step per SEMANTICS.md §4, block-argument transfer replaces φ-select. Returns
`Result<Bits, Trap>` — a trap (UB: div-by-zero, misaligned/OOB access) is `⊥` and any transform may
refine `⊥` (commuting squares compare only on non-⊥ inputs). Externals: a small builtin table
(memcpy/memset/strlen/printf-subset) so corpus functions run under the interpreter.
`verify.rs`: every use dominated by its def; block-arg arity and types match every incoming edge;
opcode/type consistency; exactly one terminator; entry has no params.

---

## §4 HIR passes — the tree-SSA half (re-realized from THEORY A7)

Order mirrors gcc -O1 (`-ftree-*`). Bounded fixpoint over the sequence, max 3 rounds.

| # | pass | file | theorem (THEORY A7 row) | proof |
|---|---|---|---|---|
| 1 | cfg_simplify | `pass/cfg.rs` | block merge, unreachable elim, jump threading of trivial blocks | `⟦f⟧=⟦P f⟧` battery |
| 2 | sroa + mem2reg | `pass/sroa.rs` | non-escaping aggregate → scalar allocas → Braun promotion | battery + alias oracle |
| 3 | sccp | `pass/sccp.rs` | Wegman-Zadeck lattice over reachability | battery |
| 4 | gvn | `pass/gvn.rs` | dominator-based value numbering; absorbs CSE, copy-prop, constant folding, algebraic normalization (`⟦L⟧=⟦R⟧` rewrite table) | battery + rewrite table exhaustively checked |
| 5 | load_elim / dse | `pass/mem.rs` | store→load forwarding, dead store, gated by the alias oracle | battery |
| 6 | dce | `pass/dce.rs` | Effect table: `Pure` with no uses is dead | battery |
| 7 | inline | `pass/inline.rs` | β-reduction; gcc -O1 = called-once + small (size threshold = dated policy constant) + interprocedural purity (the #24 `pure_functions` theorem) | battery on caller |
| 8 | licm | `pass/licm.rs` | pure, trap-free, invariant → preheader. **Unconditional at O1** — no register-pressure guard; the allocator owns pressure | battery |
| 9 | iv / strength-reduce / pointer-iv / LFTR | `pass/iv.rs` | derived IV rewrite, address recurrence, linear-function test replacement | battery |
| 10 | if_convert | `pass/ifconv.rs` | side-effect-free diamond → `select` | battery |
| 11 | rotate / final-value / invariant-pure-call hoist | `pass/loop.rs` | loop rotation; the #24 4-fence theorem; SCEV closed forms for counted loops | battery |

Battery = the existing method: small-domain-exhaustive inputs + boundary values, `⟦f⟧ ≡ ⟦P f⟧` on every
corpus function, run under `cargo test`. Ported in spirit from `opt/tests.rs`; the *tests* are theorems
and may be re-derived from the old file — the *pass code* may not.

---

## §5 MIR — the load-bearing layer

### 5.1 Registers and classes (Side-II, AAPCS64 §6.1.1 — the full table, no convenience truncation)
```
Class GPR : x0–x30 minus reserved {sp, x29 (fp when a frame pointer is required), x30 (lr), x16, x17 (IP0/IP1 — scratch for parallel-copy cycles and veneers), x18 (platform)}
            allocatable order: caller-saved first x0–x15 (x0–x7 are also argument regs), then callee-saved x19–x28
Class FPR : v0–v31 minus reserved {v31 (FP scratch for copy cycles)}; caller v0–v7,v16–v30; callee v8–v15 (low 64 bits)
Class FLAGS: k=1, the NZCV register. `cmp/cmn/tst/adds/subs/ands/fcmp` define it; `b.cc/csel/cset/cinc/ccmp` use it.
Reg = V(VReg) | P(PReg);  VRegInfo { class, width: W32|W64|S|D }
```
Modeling NZCV as a k=1 class makes compare-elimination a GVN over flag definitions and makes "two flag
values live at once" an ordinary interference the allocator resolves by rematerializing the `cmp`
(flags are always rematerializable: their producer is pure).

### 5.2 Operands, constraints, addressing modes
```
Operand   = Reg(Reg) | Imm(i64) | FImm(bits) | Mem(AddrMode) | Sym(Symbol, Reloc) | Cond(CC) | Slot(StackSlot)
AddrMode  = BaseImm{base: Reg, off: i32 /*scaled-unsigned or signed-9*/}
          | BaseReg{base, idx: Reg, ext: None|Uxtw|Sxtw|Lsl, shift: u8}
          | PreIdx{base, off} | PostIdx{base, off}       (both DEFINE a new base vreg in SSA phase)
          | PcRel{sym, page: bool} | Slot{id, off} | SpArg{off}
Constraint on each register operand (regalloc2 model):
   Use | Def | UseFixed(PReg) | DefFixed(PReg) | Clobber(RegSet /*on Call*/) | Reuse(def = use k) (rare on arm64)
```
Every instruction exposes `operands(&self) -> impl Iterator<(OperandRef, Constraint)>` and
`operands_mut`, plus `effects(&self) -> MemEffect` (`None | Read(aclass) | Write(aclass) | Barrier`).
The allocator, liveness, verifier and interpreter use ONLY these visitors — no per-opcode special
cases outside `isa.rs`. `effects()` is also the dependence oracle a list scheduler needs (O2 shelf, §16),
so scheduling costs no new IR surface later.

### 5.3 Instruction families (enum by arm64 shape; `isa.rs` owns encodability)
```
AluRR{op, w, dst, a, b}  AluRI{op, w, dst, a, imm12<<sh}  AluRRS{op, w, dst, a, b, shift, amt}  AluRRX{op, w, dst, a, b, ext, amt}
Mul{w,dst,a,b}  Madd/Msub{w,dst,a,b,c}  Smull/Umull  Div{s/u,w,dst,a,b}  Logic{op,w,dst,a,b|logimm}  Shift{op,w,dst,a,b|imm}
MovZ/MovK/MovN{w,dst,imm16,shift}  Mov{w,dst,src}  Ext{sxtb/sxth/sxtw/uxtb/uxth}  Bfx{u/s,dst,src,lsb,width}  Bfi/Bfxil
Ld{width,ext,dst,mem}  St{width,src,mem}  LdP/StP{w,r1,r2,mem}  Adrp{dst,sym}  AddLo12{dst,base,sym}  LdrGot
Cmp/Cmn/Tst{w,a,b|imm}→FLAGS  AddS/SubS/AndS  Csel/Csinc/Csinv/Csneg{w,dst,a,b,cc}  Cset/Cinc  Ccmp
B(target)  Bcc{cc,target}  Cbz/Cbnz{w,reg,target}  Tbz/Tbnz{reg,bit,target}  Br(reg)  Ret
Bl{sym}  Blr{reg}                           (always wrapped by the Call pseudo below)
FP: Fmov(rr, r↔g, imm8)  Fadd/Fsub/Fmul/Fdiv/Fneg/Fabs/Fsqrt  Fcmp→FLAGS  Fcsel  Fcvt{s↔d}  Scvtf/Ucvtf  Fcvtzs/Fcvtzu  LdF/StF
Sync: Ldaxr/Stlxr/Ldar/Stlr/Dmb            (EXT __sync_*)
Pseudo (exist only before their lowering pass):
  Call{callee, args: fixed uses, rets: fixed defs, clobbers: caller-saved set, stack_bytes, tail: bool /* sibling call → `b` after epilogue (O2 -foptimize-sibling-calls) */}
  Copy{dst,src}  ParallelCopy{pairs}  Spill{slot,src}  Reload{dst,slot}  FrameAddr{dst,slot}  Asm{tmpl,ops}
  JumpTable{index, table: Vec<BlockId>}     (lowered to adr+ldr+br in layout)
```
### 5.4 SSA, interpreter, verifier
Virtual phase: every VReg defined once; block parameters carry values across edges; `PreIdx/PostIdx`
define a fresh base vreg. `interp.rs`: machine state ⟨regs (a map for V, an array for P), NZCV,
memory, sp/frame⟩; one interpreter for both phases so `⟦hir⟧ = ⟦mir_v⟧ = ⟦mir_p⟧ = ⟦mir_final⟧` is
checkable end-to-end per function. `verify.rs`: virtual phase = SSA + arity + width consistency +
FLAGS def-before-use; physical phase = no V left, every Fixed constraint met, clobbered regs not live
across the clobber, every Slot resolved.

---

## §6 isel — HIR → MIR

Per block, bottom-up over the SSA use-def graph, **maximal munch on single-use trees** (a value with
one use may be folded into its user; multi-use values are materialized once). The pattern table
(`isel/pattern.rs`) is the theorem table — each row = one `⟦hir-tree⟧ = ⟦mir-seq⟧` battery test:

| HIR tree | MIR | note |
|---|---|---|
| `load(add(b, shl(i, k)))`, k∈{0..3} matching width | `ldr [b, i, lsl #k]` | also `sxtw/uxtw` extend when `i` is I32 |
| `load(add(b, c))`, c encodable | `ldr [b, #c]` | scaled-unsigned or signed-9 per width |
| `load(addr_global s)` | `adrp; ldr [x, :lo12:s]` | GOT form for externs |
| `br(icmp.cc a b)` | `cmp a, b; b.cc` | `cbz/cbnz` when b=0 and cc∈{eq,ne}; `tbz/tbnz` for single-bit tests |
| `select(icmp…, a, b)` | `cmp; csel` | `csinc/csinv/csneg/cset` special forms |
| `add(mul(a,b), c)` / `sub(c, mul(a,b))` | `madd` / `msub` | `smull/umull` for widened products |
| `and(lshr(a,s), mask)` / `shl+ashr` | `ubfx` / `sbfx` | bit-field extract |
| `add(a, sext(b))` etc. | `add a, b, sxtw` | operand-extend folding |
| `mul(a, const)` | shift/add sequence | Side-II cost table, otherwise `mov+mul` |
| immediates | `imm12`, `imm12<<12`, logical-imm, `movz/movk` chain, `mov wzr`, `movn` | `isel/imm.rs` predicates |
| `switch` | jump table (`adr+ldr+br`) when density ≥ threshold else balanced compare tree | thresholds = gcc defaults, dated policy constants |

**ABI (`isel/abi.rs`)** = the AAPCS64 §6.4–6.8 C.1–C.15 automaton over the call's C signature (THEORY
II-3): NGRN/NSRN/NSAA state, composites ≤16B in registers, HFA/HVA, >16B by reference (caller copy),
sret in x8, C.11 lock, variadics (nfix; the 192-byte register save area; `va_start/va_arg` lowering),
long double via soft-float calls. Emits fixed constraints on the `Call` pseudo + explicit `str` to
`SpArg` for stack args; function entry materializes params from `DefFixed` or `SpArg` loads.

---

## §7 Regalloc on MIR-SSA — the core

References: Hack 2007 (thesis: SSA interference graphs are chordal; dominance preorder is a perfect
elimination order), Braun & Hack 2009 ("Register Spilling and Live-Range Splitting for SSA-Form
Programs"), Boissinot et al. 2009 (fast liveness / out-of-SSA), Braun et al. 2013 (SSA reconstruction).

1. **Liveness** (`live.rs`): iterative backward dataflow on SSA with block parameters (a target's
   argument is a use on the edge). Cheap enough; Boissinot's dominance-based variant is an optional
   later optimization.
2. **Spilling** (`spill.rs`) — Braun-Hack, per register class:
   - Walk blocks in dominance order. For each block compute the entry set `W_entry` (≤ k values) from
     the predecessors' exit sets, preferring values with the nearest next use (loop-aware next-use
     distance: uses outside the current loop count as "far").
   - Walk instructions applying Belady MIN: for each use not in `W`, insert `Reload` (a **new vreg**);
     when `|W| + defs > k`, evict the value with the furthest next use, inserting a `Spill` at its
     definition (once per value, lazily). Values marked **rematerializable** (`iconst`, `adrp+add`,
     `mov`-of-immediate, extends of a value still in W) are recomputed instead of reloaded.
   - At block boundaries reconcile `W_exit(pred)` with `W_entry(succ)`: insert reload/spill on the edge
     (critical edges are split first, in `dom.rs`).
   - Fixed constraints (`UseFixed/DefFixed`, `Clobber`) count against `k` at that instruction; Hack's
     method: a `ParallelCopy` is inserted before a constrained instruction so the constraint is local.
   - Reloads create new definitions ⟹ **SSA reconstruction** (Braun 2013 again) rewires uses to the
     nearest reaching definition, inserting block parameters as needed.
   - Post-condition (verified): register pressure ≤ k(class) at every program point. **This is
     live-range splitting** — a value lives in a register where hot and in its slot elsewhere.
3. **Coloring** (`color.rs`): dom-tree preorder over blocks, instructions in order; maintain the live
   set incrementally; at each definition assign the lowest free color respecting the constraint and
   the class order (caller-saved first, callee-saved last, so values not live across a call avoid
   prologue saves). Block parameters are colored at the block head (after the predecessors' copies
   are accounted for). **Theorem: never fails after step 2** (chordality + pressure ≤ k). A call's
   `Clobber` set is treated as fixed definitions live across the instruction, so a value live across a
   call cannot receive a caller-saved color — **no special "crossing" logic exists.**
4. **Coalescing**: biased coloring — prefer a copy partner's color (`Copy`, block-argument pairs,
   `PostIdx` base pairs) when free. Never merges nodes, never breaks the pressure guarantee. Upgrade to
   Boissinot merging only if the measured residual copies (Law-4 residual check, §13) justify it.
5. **SSA destruction** (`destruct.rs`): each edge's parameters become a `ParallelCopy` (already colored);
   sequentialize with the standard windmill algorithm; cycles broken with the reserved scratch
   (x16 for GPR, v31 for FPR). Spill/Reload become `str/ldr` to `Slot` operands.
6. **Verify** (`verify.rs`, run in debug builds on every function + in the battery): (a) no two values
   simultaneously live (pre-destruction liveness) share a color; (b) every Fixed constraint met; (c)
   every `Reload` slot is dominated by a `Spill` of the same value; (d) no value in a caller-saved
   register is live across a `Call`; (e) `⟦mir_v⟧ = ⟦mir_p⟧` on the corpus.

---

## §8 MIR passes — the O1 back-half (gcc -O1 has NO instruction scheduler; none here)

Pre-allocation (on SSA):
- `cmp_elim`: GVN over FLAGS definitions; `sub`+`cmp` → `subs`, `and`+`tst` → `ands` when the flags
  consumer is the only other user. (gcc `-fcompare-elim`.)
- `auto_inc`: `ldr [p]; add p', p, #k` → `ldr [p], #k` defining `p'` (post-index), pre-index dual.
  (gcc `-fauto-inc-dec`.)
- `ext_lattice`: known-width dataflow ("value is already sign/zero-canonical in its low 32 bits")
  eliminating redundant `sxtw/uxtb/uxth`. Replaces the five old text sxtw levers with one pass.
- `ldst_pair`: adjacent same-base accesses → `ldp/stp` (THEORY A7 "LDP/STP PAIRING").
Post-allocation (physical):
- `frame`: assign slots (spills, allocas, outgoing-arg area, callee-saved save area, vararg save
  area); one frame adjust (`-fcombine-stack-adjustments` by construction); frame pointer only when a
  VLA/alloca exists (`-fomit-frame-pointer`); prologue/epilogue with exactly the callee-saved set used.
- `shrink_wrap` (R3): place prologue/epilogue at the nearest common dominator of the blocks that need
  callee-saved registers or the frame (`-fshrink-wrap`).
- `layout`: block order = RPO with loop bodies contiguous; invert conditions for fall-through; drop
  `b .next`; lower `JumpTable`.
- `peephole` (structured, on MIR — never on text): self-move elimination, `mov wzr`, dead defs.

---

## §9 Emit

`emit.rs`: `fn fmt(inst: &MInst) -> String` driven by `isa.rs`; sections, symbols, relocations,
TLS per THEORY II-4 (`adrp/:lo12:`, `:got:`, `:tprel_*`, no `_` prefix). Determinism seal: identical
MIR ⟹ identical bytes. Confirmation: `as` accepts every emitted file; the suites confirm.

---

## §10 Proof map (Law 3 — certify at the middle) and the cost model

| layer | obligation | mechanism | where it runs |
|---|---|---|---|
| AST → HIR | faithful lowering | HIR verifier + differential suites (c99 referee) | `cargo test` + box gates |
| HIR pass P | `⟦f⟧ = ⟦P f⟧` | exhaustive small-domain battery on the corpus under `hir::interp` | `cargo test` |
| isel | `⟦tree⟧ = ⟦seq⟧` per pattern; `⟦hir⟧ = ⟦mir_v⟧` per function | pattern battery + whole-function translation validation with generated inputs | `cargo test` |
| MIR-SSA pass | `⟦m⟧ = ⟦P m⟧` | battery under `mir::interp` | `cargo test` |
| regalloc | renaming bisimulation | mechanical verifier (§7.6 a–d) + `⟦mir_v⟧ = ⟦mir_p⟧` | debug builds + `cargo test` |
| frame / layout | `⟦mir_p⟧ = ⟦mir_final⟧` | interpreter with sp/frame semantics + verifier | `cargo test` |
| emit | determinism; assembler acceptance | md5 seal; `as` | box |
| whole compiler | CONFIRMS, never discovers | opt-parity (HIR passes off vs on), torture, csmith300, yarpgen300, cts, musl | box |

**Cost-square exact by construction:** one `MInst` = one machine instruction after `frame/layout`, so
`cost(f) = |MIR_final(f)|` needs no separate model. Δinsn of any transform is computed on MIR before
emitting anything: **predict → apply → confirm** becomes cheap. (The lesson of lever ㉕.2 — a build
without a prior prediction — is fixed structurally, not by discipline.)

---

## §11 Charter reconciliation

- Laws 1–3, Articles A–G unchanged. This document *is* Article G's "refactor/optimize obey the Laws"
  applied at architecture scale: every layer ships its square before the next layer is built.
- `OPT.md §0` spine: row #25 is superseded by §12 here (user "re-plan"). Rows 1–24 remain history on
  `main`. The scoreboard method (paired INSN+EXEC, distribution, gcc-zeroed bucket, corpus25 excess
  histogram) survives verbatim — it is measurement, not architecture.
- Docs to update when merging to `main`: `THEORY.md` A5/A6/A7 (rewrite around HIR/MIR; add the Hack /
  Braun-Hack / Boissinot theorems), `SEMANTICS.md` (HIR semantics re-targeted; new MIR semantics
  section), `MILESTONES.md` (R0–R4), `CLAUDE.md` override paragraph (`[optimizer = main]` block → the
  layered backend), `src/codegen/arm64_elf.md` deleted, `tests/README.md` (new proof batteries).

---

## §12 Milestone ladder — the spine of this branch (edit IN PLACE; status is memory)

Legend: ⬜ todo · 🔨 in progress · ✅ banked (commit + measurement recorded) · ⚠️ quarantined.

### R0 — skeleton, hello world (the "tcc in 5 hours" moment on the new architecture)
| task | status |
|---|---|
| R0.1 `git rm -r src/ir.rs src/opt src/codegen`; `main.rs` compiles against a stub `hir::build` + `emit` | ✅ |
| R0.2 `hir/mod.rs` types + `verify.rs` + `dom.rs` (cfg, dom tree, loop forest, critical-edge split) | ✅ |
| R0.3 `hir/build.rs`: AST → HIR with Braun SSA; scalar C subset (int/ptr arithmetic, if/while/for, calls, globals, strings) | ✅ |
| R0.4 `hir/interp.rs` + first battery (`equiv` harness) proving `build` on the science-gate programs | ✅ `src/hir/tests.rs` 10/10 |
| R0.5 `mir/mod.rs` + `isa.rs` (full AAPCS64 register table, immediates) + `verify.rs` + `interp.rs` | ⬜ |
| R0.6 `isel/lower.rs` naive 1:1 (no munch yet) + `isel/abi.rs` for scalar args/returns + `imm.rs` | ⬜ |
| R0.7 **regalloc, complete**: `live` → `spill` (Braun-Hack + remat + SSA reconstruction) → `color` → `destruct` → `verify` | ⬜ |
| R0.8 `mir/pass/frame.rs` + `layout.rs` + `emit.rs`; hello world links and runs in the box | ⬜ |
| R0 gate | `tests/cases` 81/81, `tests/ext` scalar subset; `cargo test` batteries green | ⬜ |

### R1 — correctness parity with `rc3` (no HIR optimization passes yet)
| task | status |
|---|---|
| R1.1 full C99 lowering: aggregates (alloca + memcpy/memset), bitfields, `switch`, VLA/alloca, long double via soft-float calls | ⬜ |
| R1.2 full ABI automaton: composites, HFA, sret, stack args, variadics + 192B save area, `va_arg` | ⬜ |
| R1.3 EXT surface: `__sync_*`, `__builtin_*_overflow`, computed goto, statement-expr, inline asm (opaque), `__va_area__` | ⬜ |
| R1.4 science gates green: `abi.sh alg.sh cpp.sh shape.sh decay.sh`; `tests/ext` 21/21 | ⬜ |
| R1.5 torture ≥ 1471 pass (the `rc3` count; the 4 pre-existing runtime FAIL `20021127-1 bitfld-3 pr32244-1 pr34971` are a bonus if they pass), csmith300 0 DIVERGE, yarpgen300 0 DIVERGE | ⬜ |
| R1 measurement | sqlite static insns + `corpus25.sh` excess histogram + `exectime.sh` paired geo40, all with HIR passes OFF. Record here as the correctness-parity data point. **The allocator KPI (frame-slot mem-ops ≪ 27,403, reg-reg `mov` ≪ 40,573 at rc3) is NOT readable here**: per §14, R0/R1 keep every local in memory, so the allocator sees only expression temporaries. That KPI is measured at R2.2, immediately after SROA+mem2reg | ⬜ |

### R2 — tree-SSA parity (port the A7 ladder onto HIR, §4 order)
| task | status |
|---|---|
| R2.1 cfg_simplify, sccp, gvn, dce (+ batteries) | ⬜ |
| R2.2 sroa+mem2reg, load_elim/dse, alias oracle | ⬜ |
| R2.3 inline (+purity), licm (unconditional), iv/pointer-iv/LFTR | ⬜ |
| R2.4 if_convert, rotate/final-value/pure-call hoist | ⬜ |
| R2 gate + measurement | opt-parity (passes off vs on) 0 DIVERGE; csmith/yarpgen 0 DIVERGE. KPI: INSN geo ≤ 1.58 (rc3), sqlite ≤ 1.5×. **Merge-to-main eligibility starts here** | ⬜ |

### R3 — machine passes (§8) + isel munch table complete (§6)
| task | status |
|---|---|
| R3.1 munch patterns: addressing modes, cmp-branch fusion, csel forms, madd/msub, bfx, extend folding, mul-by-const | ⬜ |
| R3.2 cmp_elim, auto_inc, ext_lattice, ldst_pair | ⬜ |
| R3.3 switch jump tables, block layout, shrink-wrap | ⬜ |
| R3 measurement | `corpus25.sh` excess histogram per mnemonic; each class classified fundamental vs convenience (Law-4). Band: sqlite ≤ 1.3×, geo40 INSN/EXEC ≤ 1.2 | ⬜ |

### R4 — exhaustion toward 1×
The excess histogram is the worklist: attack the largest class with one proof-carrying lever, re-measure,
repeat until every class is residual-fundamental. Predict on MIR before building — always.

### R5 — the O2 headroom stack (§16)
User principle (2026-08-24): "to reach 1× we must stack enough technique to reach 0.5×, and keep 0.5×
as headroom" — O1 parity must be reached with margin, not asymptotically. R5 pulls the §16 shelf in
rank order (effect on the measured arm64 gap ÷ proof cost) until the paired scoreboard sits at ≤ 1.0 on
BOTH axes with the distribution flat. Items marked ★ in §16 are cheap enough to ship inside R2/R3.

**Rules of the ladder.** One commit per task, push after each R-gate. A red gate after one bounded
Law-2 attempt quarantines the task (⚠️ + reason), never the milestone. `main` is never touched; merge
`mir-rearch` → `main` only at ≥ R2 KPI with the full gate green. Estimated backend size 13–17k LOC
(today 17.2k) — a taste, not a budget.

---

## §13 Baselines to beat (from `rc3`, box, 2026-08-24) — measurement only

- sqlite3.c static insns: zcc **279,161** vs gcc-O1 **157,883** = **1.768×** (pre-㉕.1: 286,129 = 1.812×).
- mnemonic excess (post-㉕.1): reg-reg `mov` 40,573; imm `mov` 22,248; `ldr` 32,159; `str` 13,544;
  frame-slot mem-ops 27,403; `add` 17,033; `cmp` 12,428; `ldp` 13,766.
- geo40: INSN geomean **1.5835** over 34 (median 1.554, worst f3_float_minmax 2.358); EXEC median 1.552
  over 19 (the EXEC geomean line prints 0.0000 — reducer bug: `log(0)` from a zero sample; fix in
  `exectime.sh` early in R1: drop zero samples).
- Gate at rc3: cargo 184/0, opt-parity 1552/0, csmith300 254/0, yarpgen300 300/0, torture 1471 pass +
  4 pre-existing runtime FAIL.
- Harnesses: `tests/bench/corpus25.sh` (excess histogram), `tests/bench/exectime.sh` (paired geo40),
  `tests/opt-parity.sh`, `tests/suites/{torture,csmith,yarpgen}.sh`, `tests/gate.sh all`.

---

## §14 Decision log (settled; reopen only with a stated reason)

| decision | choice | why |
|---|---|---|
| frontend | keep as input | failure is entirely below AST; parser is an independent proven artifact |
| SSA representation | block parameters (HIR and MIR) | explicit edges, trivial destruction, one model |
| HIR types | closed `Ty` enum, signedness in opcodes | passes independent of TyTab; closed semantics |
| allocation | on SSA, Braun-Hack spill first, chordal greedy color | polynomial + optimal for the spill set; splitting free |
| coalescing | biased coloring first; Boissinot merge only on measured residual | never breaks the pressure guarantee |
| call-crossing values | modeled as `Clobber` constraints, no special logic | falls out of constraint-respecting greedy coloring |
| flags | k=1 register class | compare-elim = GVN; conflicts = liveness |
| scheduler | none | gcc -O1 has none; YAGNI |
| middle target-independent IR | deferred | one target |
| migration | big-bang on `mir-rearch`; `rc3` is the fallback | user directive; incremental rejected |
| scratch registers | x16, x17 (GPR), v31 (FPR) reserved | AAPCS64 IP0/IP1; parallel-copy cycle breaking |
| R0/R1 local storage | every C local stays in ONE frame slot (memory); promotion is R2.2 SROA+mem2reg | the parser reports `Var(off)`, not variable identity — two locals in disjoint scopes may share an offset, so promotion at build time would rest on an unproven disambiguation. Consequence: R0/R1 exercise the allocator on expression temporaries only, and the R1 allocator KPI is re-measured at R2.2 (noted in §12 R1) |

---

## §16 The O2 headroom shelf — techniques beyond O1, ranked, with the names behind them

Every row is a theorem to realize on THIS architecture (HIR or MIR as noted) under Law 3 — each ships
its commuting square. Rank = expected effect on the measured arm64 gap ÷ proof cost. ★ = cheap enough
to ship inside R2/R3 because the §3/§5 hooks (aclass, weight, effects(), tail) already exist.

| rank | technique | layer | gcc flag (level) | origin / big name | proof shape |
|---|---|---|---|---|---|
| ★1 | type-based alias analysis (TBAA) feeding load-elim/DSE/LICM | HIR alias oracle | `-fstrict-aliasing` (O2) | Diwan, McKinley, Moss 1998; C99 6.5p7 effective-type rule | oracle soundness vs C99 6.5p7 + battery |
| ★2 | value-range propagation + branch folding + unsigned-shift/`udiv` narrowing | HIR | `-ftree-vrp` (O2) | Patterson 1995 (range analysis); Harrison 1977 | lattice pass, `⟦f⟧=⟦Pf⟧` |
| ★3 | sibling/tail-call optimization | MIR `Call.tail` | `-foptimize-sibling-calls` (O2) | Steele 1977 ("Lambda: the ultimate GOTO"); Clinger 1998 | ABI condition table + TV |
| ★4 | static branch prediction → block weights → layout + spill weighting | HIR `Block.weight` | `-fguess-branch-probability` (O1), `-freorder-blocks` (O1/O2) | Ball & Larus 1993; Pettis & Hansen 1990 (code positioning) | layout TV; weights are advisory (no semantic obligation) |
| 5 | partial redundancy elimination / lazy code motion (global CSE across paths, loop-invariant load hoisting) | HIR | `-ftree-pre`, `-fgcse`, `-fcode-hoisting` (O2) | Morel & Renvoise 1979; Knoop, Rüthing, Steffen 1992 (LCM); Chow, Chan, Kennedy et al. 1997 (SSAPRE); Kennedy et al. 1999 | availability/anticipability dataflow + battery |
| 6 | global code motion + GVN unified (schedule pure ops to the cheapest dominating block) | HIR | (GCC has no single flag; LLVM GVN+hoist/sink) | Cliff Click 1995 ("Global Code Motion / Global Value Numbering") | dominance + loop-depth placement, `⟦f⟧=⟦Pf⟧` |
| 7 | interprocedural constant propagation + scalar-replacement of args + identical-code folding | HIR module | `-fipa-cp`, `-fipa-sra`, `-fipa-icf` (O2) | Callahan, Cooper, Kennedy, Torczon 1986 (IPCP); Jan Hubička (GCC IPA) | call-graph lattice; per-callee `⟦f⟧` equality |
| 8 | inlining policy beyond called-once: small-function + partial + indirect | HIR | `-finline-small-functions`, `-fpartial-inlining`, `-findirect-inlining` (O2) | Ayers, Gottlieb, Schooler 1997; Hubička | β-reduction battery (existing) |
| 9 | instruction scheduling (list scheduling on the basic block; critical-path priority; generic in-order latency table) | MIR (post-RA) | `-fschedule-insns2` (O2) | Gibbons & Muchnick 1986; Rau & Fisher (VLIW era); Muchnick 1997 ch.17 | dependence preservation via `effects()` + `⟦m⟧=⟦Pm⟧` |
| 10 | store merging + strlen/memcpy idiom recognition | HIR | `-fstore-merging`, `-foptimize-strlen` (O2) | GCC (Jakub Jelínek); Muchnick | battery |
| 11 | switch conversion (dense case bodies → table loads) + tail-merge/cross-jumping | HIR / MIR layout | `-ftree-switch-conversion`, `-fcrossjumping`, `-ftree-tail-merge` (O2) | GCC | battery / TV |
| 12 | loop unrolling (counted, small bodies) + peeling + unswitching | HIR | `-funroll-loops` (not default), `-fpeel-loops`, `-funswitch-loops` (O3) | Dongarra & Hinds 1979; Allen & Kennedy 2001 | SCEV trip count + battery |
| 13 | SLP / loop vectorization (NEON) — needs `Ty::V128` in HIR + vector ops in MIR (FPR class already holds v-regs) | HIR + MIR | `-ftree-vectorize` (O2 since GCC 12, very-cheap cost model) | Allen & Kennedy 1987/2001; Larsen & Amarasinghe 2000 (SLP); Nuzman & Zaks (GCC) | dependence proof + battery; largest surface |
| 14 | alignment of loops/functions/jumps (a SIZE cost gcc pays at O2 — track, do not copy blindly) | MIR layout | `-falign-loops` etc. (O2) | Pettis & Hansen; microarchitecture SWOGs | none (layout only) |
| 15 | rematerialization + live-range splitting refinements in RA (already structural in §7; the O2 delta is spill-slot coloring + region splitting) | MIR RA | `-flra-remat`, IRA regions (O2) | Briggs 1992 (remat); Cooper & Simpson 1998 (live-range splitting); Makarov 2007 (IRA); Olesen 2011 (LLVM greedy); Wimmer & Franz 2010 (SSA linear scan) | RA verifier (§7.6) |
| 16 | superoptimization / e-graph peephole search for the isel pattern table | isel | — | Massalin 1987; Bansal & Aiken 2006; Tate et al. 2009 (equality saturation) | each found pattern = a battery row |

Books that cover the whole shelf: Rastello & Bouchez Tichadou (eds.) 2022 *SSA-based Compiler Design*
(the single best reference for §3–§7); Muchnick 1997 *Advanced Compiler Design and Implementation*;
Cooper & Torczon *Engineering a Compiler*; Allen & Kennedy 2001 *Optimizing Compilers for Modern
Architectures*; Appel *Modern Compiler Implementation*.

Out of scope even for headroom: polyhedral loop-nest optimization (Graphite/Pluto — Bondhugula 2008),
LTO, PGO instrumentation. They would not move the arm64 O1 gap and their proof surface is enormous.

---

## §17 arm64 leverage table — the isel exhaustion checklist (Law-4 applied to instruction selection)

The A64 ISA (ARM DDI 0487) is the Side-II ultimate fact for isel. The pattern table (§6) is **exhausted**
only when every ISA feature below that removes an instruction has a pattern row with its battery proof,
and the corpus excess histogram shows the corresponding mnemonic at gcc parity. Each row = one isel
lever; ✔ marks features gcc -O1 uses routinely on sqlite (the ones the old backend measurably lacked).

| feature | ISA form | HIR tree it absorbs | saves |
|---|---|---|---|
| ✔ shifted-register operands | `add/sub/and/orr/eor/cmp x, x, x, lsl/lsr/asr #n` | `op(a, shl(b,n))` | the shift |
| ✔ extended-register operands | `add/sub/cmp x, x, w, sxtw/uxtw/sxtb… #n` | `op(a, sext/zext(b))`, with shift | the extend (+shift) |
| ✔ register-offset addressing | `ldr/str [x, x, lsl #k]`, `[x, w, sxtw/uxtw #k]` | `load(add(b, shl(i,k)))`, `i` I32 | `lsl` + `add` (+extend) |
| ✔ immediate addressing | `[x, #imm12·size]`, `[x, #simm9]` | `load(add(b, c))` | the `add` |
| ✔ pre/post-index | `ldr x, [p], #k` / `[p, #k]!` | load + pointer bump in loops | the `add` |
| ✔ load/store pair | `ldp/stp x, x, [base, #imm7·8]` | two adjacent accesses (struct fields, spills, prologue) | one mem op |
| ✔ extending loads | `ldrb/ldrh/ldrsb/ldrsh/ldrsw` | `sext/zext(load narrow)` | the extend |
| ✔ 32-bit ops zero-extend for free | `w`-form ALU | `zext32(op32)` | `uxtw` |
| ✔ zero register | `xzr/wzr` as operand or dest | `iconst 0`, discarded results, `cmp x, #0` via `cmp`/`cbz` | a `mov` |
| ✔ flag-setting ALU | `adds/subs/ands/adcs/sbcs`, `cmn`, `tst` | `op` + `cmp op 0` / `cmp a, -b` / `and`+`cmp` | the `cmp` |
| ✔ compare-and-branch | `cbz/cbnz`, `tbz/tbnz` | `br(icmp eq/ne x 0)`, sign-bit / single-bit tests | the `cmp` |
| ✔ conditional select family | `csel/csinc/csinv/csneg/cset/csetm/cinc/cinv/cneg` | `select`, `c?a+1:a`, `c?-a:a`, `c?1:0`, `c?~a:a`, min/max, abs | branches |
| ✔ conditional compare chains | `ccmp/ccmn` | `&&`/`||` of relations feeding one branch/select | branch + extra `cmp` |
| ✔ multiply-accumulate | `madd/msub/mneg`, `smull/umull/smaddl/umaddl/smulh/umulh` | `add(mul)`, `sub(mul)`, `neg(mul)`, widened products | the `add`/`sext` |
| ✔ mul/div by constant | shifts+adds, `umulh/smulh` magic (Granlund & Montgomery 1994), `lsr` for pow2 | `mul/udiv/sdiv/urem/srem` by const | the `mul`/`div` |
| ✔ bit-field ops | `ubfx/sbfx/ubfiz/sbfiz/bfi/bfxil`, `extr` (funnel shift), `rbit/clz/cls/rev/rev16/rev32` | `and(lshr)`, `shl(and)`, insert masks, rotates, `__builtin_clz/bswap` | 1–3 ops each |
| ✔ inverted-operand logic | `bic/orn/eon` | `and(a, not b)`, `or(a, not b)`, `xor(a, not b)` | the `mvn` |
| ✔ logical immediates | bitmask-imm encoding (`and/orr/eor/tst #imm`) | masks, `x & ~0x7`, alignment ops | `mov` of the mask |
| ✔ constant materialization | `movz/movk/movn`, `orr #logimm`, `adr`, `ldr literal`, `fmov #imm8` | any constant | 1–3 `mov`s |
| ✔ symbol addressing | `adrp` + `:lo12:` folded into `ldr/str/add`, `:got:` | globals | one `add` |
| ✔ frame | omit frame pointer (x29 allocatable), single `sub sp`, `stp x29,x30,[sp,#-N]!` pre-index prologue | prologue/epilogue | 1–2 insns per function |
| FP | `fmadd/fmsub/fnmadd/fnmsub`, `fmin/fmax/fminnm`, `fcsel`, `fabs/fneg/fsqrt`, `scvtf/ucvtf/fcvtzs/fcvtzu` with int operands, `fmov` reg-reg free width switch | FP trees | 1 each |
| register file | 31 GPR + 32 FPR (§5.1 full table) | — | the spill floor → ~0 for most functions |
| LSE atomics (armv8.1+) | `ldadd/swp/cas` | `__sync_*` | LL/SC loops — only under `-march`, off by default |
| NEON | `ld1/st1`, vector ALU, `addp`, `cnt`, `uaddlv` | §16 row 13 (vectorization), `__builtin_popcount`, memcpy/memset inline | many — the last shelf |

Method for exhaustion: (1) after R3, run `corpus25.sh`; (2) for each mnemonic where zcc > gcc, diff a
sample of functions and name the missing row above; (3) add the pattern + battery row; (4) re-measure.
Discovery aid: superoptimization / equality saturation over the pattern table (§16 row 16) finds rows
a human misses. The table is complete when every remaining excess is category (a) fundamental.

---

## §18 Reading list (theorems to realize — cite in code comments as `THEORY <ref>`)

- Braun, Buchwald, Hack, Leißa, Mallon, Zwinkau 2013 — Simple and Efficient Construction of SSA Form (CC'13).
- Hack 2007 — Register Allocation for Programs in SSA Form (PhD, Karlsruhe): chordality, dominance
  preorder as perfect elimination order, constraint handling by local parallel copies.
- Braun & Hack 2009 — Register Spilling and Live-Range Splitting for SSA-Form Programs (CC'09): the
  Belady-based spiller used in §7.2.
- Boissinot, Darte, Rastello, Dinechin, Guillon 2009 — Revisiting Out-of-SSA Translation (CGO'09).
- Boissinot, Hack, Grund, Dinechin, Rastello 2008 — Fast Liveness Checking for SSA-Form Programs.
- Cooper, Harvey, Kennedy 2001 — A Simple, Fast Dominance Algorithm.
- Wegman & Zadeck 1991 — SCCP. Rosen/Wegman/Zadeck 1988 + Briggs/Cooper/Simpson — value numbering.
- Aho/Ganapathi/Tjiang 1989 — BURS/maximal munch instruction selection.
- QBE (`qbe/*.c`: `ssa.c`, `spill.c`, `rega.c`, `isel.c`, `abi.c`) and Cranelift `regalloc2` — the
  two reference implementations of exactly this architecture (read for algorithms, not structure).
- ARM AAPCS64 (IHI 0055) §6.1.1 register use, §6.4–6.8 parameter passing; ARMv8-A ARM (DDI 0487) A64
  instruction encodings; AArch64 ELF (IHI 0056) relocations.
