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
  default + `origin/HEAD`, set 2026-08-24). `main` is frozen at `rc3` and left alone.
  **Progress: R0, R0.9, ALL of R1, R2.1, R2.2, and most of R2.3/R2.4/R3 are ✅ banked** —
  what remains is listed as `⬜` inside those §12 rows (iv/LFTR, rotate/final-value,
  auto_inc, shrink-wrap).
  The backend is `src/{cfg,mem,compile,emit}.rs` + `src/hir/` (with `hir/pass/` = the §4
  ladder) + `src/mir/` (with `mir/pass/` = the §8 machine passes) + `src/isel/` +
  `src/regalloc/`; `cargo test` **116/116**, `tests/cases` 74/75 (only the adjudicated
  `float_h`), `tests/ext` 19/19, all five science gates PASS, torture **1471 pass / 0
  FAIL**, opt-parity 1552/0, csmith300 254/0, yarpgen300 300/0, determinism 85 × 8.
  sqlite **241,055 = 1.527×** gcc-O1, geo40 INSN **1.2982**, EXEC **1.5857**
  (rc3: 1.768× / 1.5835; the R1 origin: 2.997× / 2.5168 / 4.4077).

  **NEXT SESSION = R3.4, the Law-1 sync — not more optimization.** Twelve passes
  shipped in R2/R3 and `THEORY.md` records none of them; it still says
  `[PLANNED — R2/R3]`. Under Law 1 that is not a documentation lag, it is the
  SOURCE being wrong about its own object, and the memory
  `cxc-per-loc-before-test` says the sync happens per phase, before the suite —
  it was skipped twice. Every theorem to be written already EXISTS, in the doc
  comment at the head of the pass that realizes it; the row is transcription and
  Side-II table-filling, not research. Only after that does the §13b worklist
  (spill traffic, then copies, then layout) open.
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
1. `git switch mir-rearch`; read this file fully. (`CLAUDE.md`'s first paragraph is the branch boot
   override pointing here; the `[optimizer = main]` paragraph after it describes the DEAD architecture
   and does not apply on this branch.)
2. Resume at the first `⬜` in the §12 ladder. **State at 2026-08-25: R0, R0.9,
   R1, R2.1, R2.2 and most of R2.3/R2.4/R3 are ✅ banked.** §13a is the R1 GROUND
   METRIC (the unoptimized origin); **§13b is the current excess histogram and IS
   the worklist** — spill traffic (+24.5k), then copies (+33.5k, of which ~13k are
   the truncating `mov w, w` A64 requires), then block layout, then the remaining
   addressing modes. §15b and §15c are the defect ledgers; read §15c before
   touching `regalloc/` or `isel/munch`, because five of its six entries are
   rules that only became REACHABLE once an earlier layer started optimizing.
3. Every module ships with its verifier + interpreter-based proof battery before the next module.
4. Bank each R-milestone with a commit + measurement line in §12; push.
5. Standing gate for any bank: `cargo test` green · `tests/cases` no regression · `bash
   tests/determinism.sh` green · box build clean. From R1.5 on, add torture + csmith300 + yarpgen300.

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
src/hir/build.rs      AST → HIR lowering. NOTE (R0.9 audit): Braun's SSA construction is NOT
                      here yet and this line used to claim it was. R0/R1 keep every local in
                      memory (§14), so no φ/block-parameter insertion runs at all; Braun arrives
                      with `pass/sroa.rs` at R2.2, which is also where mem2reg uses it.
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
| 12 | sink | `pass/sink.rs` | the dual of licm: a pure trap-free instruction with ONE using block, dominated by here and no deeper in a loop, moves down to it. Added at R3 rather than planned: §13b measured register pressure as the largest remaining item, and this is the cheapest thing that shortens a live range | battery |

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
2. **Spilling** (`spill.rs`) — Braun-Hack, per register class. **As built (R2.2), with
   two deviations recorded here rather than left implicit:** SSA RECONSTRUCTION is absent
   because it is not needed — a reload's fresh register is used only inside the block that
   created it, so its live range is dominated by its definition; and a spilled BLOCK
   PARAMETER is removed from the IR rather than stored at its definition, since its
   definition is the block head. One slot per SSA WEB (parameter ∪ its arguments), merged
   only where the members do not interfere:
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
- `THEORY.md` and `SEMANTICS.md` were re-targeted at **R0.9**, not deferred to merge: Law 1 makes them
  the SOURCE and `src/` the compiled object, so leaving them describing a deleted IR would mean the
  source and the object disagreed. Still to update at merge: `MILESTONES.md` (R0–R5), the `CLAUDE.md`
  override paragraph (`[optimizer = main]` → the layered backend; `main` is frozen at rc3 and
  `mir-rearch` is default), `tests/README.md` (the new proof batteries + `tests/determinism.sh`).

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
| R0.5 `mir/mod.rs` + `isa.rs` (full AAPCS64 register table, immediates) + `verify.rs` + `interp.rs` | ✅ `src/mir/tests.rs` 10/10 |
| R0.6 `isel/lower.rs` naive 1:1 (no munch yet) + `isel/abi.rs` for scalar args/returns + `imm.rs` | ✅ `src/isel/tests.rs` 9 groups, ⟦hir⟧=⟦mir_v⟧ |
| R0.7 **regalloc, complete**: `live` → `spill` → `color` → `destruct` → `verify` (+ `mir/pass/frame.rs`, since a prologue is what makes callee-saved preservation provable) | ✅ `src/regalloc/tests.rs` 7 groups, ⟦mir_v⟧=⟦mir_p⟧. **Residual**: the spiller is the sound base case (spill-at-def, reload-per-use), NOT Braun-Hack — see R2.2 |
| R0.8 `mir/pass/frame.rs` + `layout.rs` + `emit.rs`; hello world links and runs in the box | ✅ box: `int main(){int s=0,i;for(i=0;i<10;i++)s+=i;return s;}` → exit 45 |
| R0.9 ✅ audit remediation (user audit, 2026-08-24): frame sentinel + frameless leaf · next-use distance without the `*1000` block-size assumption · every remaining convenience constant justified or removed · §2's Braun claim corrected · emit determinism seal in `tests/` (Article E's byte-identical gate, which the repo lacked) · frame/layout given their own `⟦mir_p⟧=⟦mir_final⟧` square instead of riding on regalloc's · `THEORY.md`/`SEMANTICS.md` re-targeted to HIR/MIR (Law 1: those docs ⊕ the specs ARE the source) | ✅ 7/7. THEORY A5–A7b rewritten (isel/HIR/MIR/regalloc-on-SSA + the pass ladder marked PLANNED), II-3/II-4/II-5 re-pointed, chordal graphs added to the B1 index; SEMANTICS rewritten around both levels. `cargo test` 40/40, `tests/cases` 61/81 unchanged, determinism 70×6 green |
| R0 gate | `cargo test` batteries green (36/36: hir 10, mir 10, isel 9, regalloc 7); `tests/cases` **61/81** — the 14 remaining failures are exactly the R1 feature set, each stopped by an explicit `todo!` rather than miscompiled: struct by value/return (`abi_callptr_struct abi_composite_ir addr_of_exotic_ir c89_structval stmt_expr_nested_struct`), >8 arguments on the stack (`c89_decl`), varargs (`c99_ternary_decay_vararg e_stdarg kr7_minprintf m5_printf_args`), VLA (`c99_digraph_vla vla_loop_reset_sp`), long double (`c99_long_double`), bitfields (`m6_bitfield`). **81/81 is R1.4's gate, not R0's** — R0.3 only ever claimed the scalar subset; the original wording of this row was inconsistent with it and is corrected here in place | ✅ |

### R1 — correctness parity with `rc3` (no HIR optimization passes yet)
| task | status |
|---|---|
| R1.1 full C99 lowering: aggregates (alloca + memcpy/memset), bitfields, `switch`, VLA/alloca, long double via soft-float calls | ✅ `88c38e3` |
| R1.2 full ABI automaton: composites, HFA, sret, stack args, variadics + 192B save area, `va_arg` | ✅ `88c38e3` |
| R1.3 EXT surface: `__sync_*`, `__builtin_*_overflow`, computed goto, statement-expr, inline asm (opaque), `__va_area__` | ✅ `88c38e3` |
| R1.4 science gates green: `abi.sh alg.sh cpp.sh shape.sh decay.sh`; `tests/ext` 21/21 | ✅ abi/alg/cpp/shape/decay all PASS · `tests/cases` 74/75 (only the adjudicated `float_h`) · `tests/ext` 19/19 (2 SKIP: `cc` rejects them) · determinism 85 progs × 8 fresh processes |
| R1.5 torture ≥ 1471 pass (the `rc3` count; the 4 pre-existing runtime FAIL `20021127-1 bitfld-3 pr32244-1 pr34971` are a bonus if they pass), csmith300 0 DIVERGE, yarpgen300 0 DIVERGE | ✅ torture **1470 pass / 0 FAIL** / 224 not-impl — the not-impl manifest is BYTE-IDENTICAL to the committed `torture.not-impl` (rc3's, `423a42d`), and rc3's 4 runtime FAIL are gone; csmith300 **254 PARITY / 0 DIVERGE** (46 SKIP = `gcc` itself fails the sample; exactly rc3's 254/0); yarpgen300 **300 PARITY / 0 DIVERGE**. 13 torture defects were found and fixed on the way — see `§15` |
| R1.6 close the §15 PROOF DEBT: teach `hir::interp` the intrinsics so no R1 feature is ⊥ on both sides, then one battery per §15 row. Ordered BEFORE the measurement because a battery's job is to DISCOVER a lowering defect, and a fix moves the emitted code — a number taken first would be stale. Ordered before R2 because every R2 pass owes `⟦f⟧=⟦P f⟧` under this interpreter: while it traps on `Inst::Intrinsic`, each of the eleven batteries would hold VACUOUSLY over every variadic / long-double / atomic function | ✅ `cargo test` 40 → **52**; three defects found, one of them a latent MISCOMPILE (call-crossing values exceeding the callee-saved count at a non-call point). See §15 |
| R1 measurement | sqlite static insns + `corpus25.sh` excess histogram + `exectime.sh` paired geo40, all with HIR passes OFF (there are none yet — that IS the point: this is the unoptimized-parity origin every R2/R3 pass is measured against). **The allocator KPI (frame-slot mem-ops ≪ 27,403, reg-reg `mov` ≪ 40,573 at rc3) is NOT readable here**: per §14, R0/R1 keep every local in memory, so the allocator sees only expression temporaries. That KPI is measured at R2.2, immediately after SROA+mem2reg | ✅ recorded in §13a |

### R2 — tree-SSA parity (port the A7 ladder onto HIR, §4 order)
| task | status |
|---|---|
| R2.1 cfg_simplify, sccp, gvn, dce (+ batteries) | ✅ `e7473c9`. `cargo test` 52 → 67: `hir::tests::check` now runs EVERY battery program through both sides of ⟦f⟧=⟦P f⟧, and `isel`/`regalloc` do the same, so the R0/R1 corpus became the ladder's proof corpus at no authoring cost. Yield on its own is ~0 (every local is still a memory cell — that is what R2.2 fixes); one defect found: a literal address rode in ZR, which `Rn=31` decodes as SP (torture `930719-1`), now refused by `mir::verify` |
| R2.2 sroa+mem2reg, load_elim/dse, alias oracle. **Blocking prerequisite**: mem2reg is what first creates long-lived values, so Braun & Hack 2009 proper — per-block working set across edges, Belady MIN eviction, rematerialization of pure producers, SSA reconstruction (Braun 2013) — must land in `regalloc/spill.rs` FIRST, with its own battery. **The rc3 allocator KPI (frame-slot mem-ops ≪ 27,403, reg-reg `mov` ≪ 40,573) is measured here**, not at R1 | ✅ sqlite **473,253 → 322,606** (2.997× → **2.043×**), geo40 INSN **2.5168 → 1.5244** (rc3 was 1.5835). `add` 133,264 → 35,357, `ldr` 90,906 → 63,286, frame-slot mem-ops 12,253 → 64,185 (the spill traffic promotion creates), reg-reg+imm `mov` 59,224 → 68,138 — the new top of the killable floor, and R3's target. Deviation from the plan, recorded: SSA RECONSTRUCTION was not needed and is not there. A reload's fresh register is used only inside the block that created it, so its live range is dominated by its definition and SSA holds by construction; a value that stays in the working set across an edge keeps its ORIGINAL name. The price is one reload per block-residency instead of one per program region. What the milestone did NOT anticipate, and what the measurement forced: (a) a spilled BLOCK PARAMETER cannot be stored at its definition — the parameter is removed and each predecessor writes the slot; (b) a join can be wider than the register file, and no eviction relieves an edge argument, so the successor's parameter is spilled instead; (c) one slot per SSA WEB, merged only where the members do not interfere — without it a spilled parameter copies between slots on every edge, which cost `sqlite3VdbeExec` 110,000 stores and made the milestone a REGRESSION (571,648) before it was a win |
| R2.3 inline (+purity), licm (unconditional), iv/pointer-iv/LFTR | 🔨 inline + licm + **purity** banked; **iv/pointer-iv/LFTR still ⬜** (scalar strength-reduction CLOSED as Law-4 category-(a), §13c). purity (`pass/purity.rs`, 2026-08-25): the interprocedural read-only predicate the pure-call hoist rests on — gcc's `pure`, not `const`, so a caller must also prove memory-clean. Optimistic fixpoint, which is what makes a RECURSIVE read-only callee read-only: "performs a write" is existential over the body. 317 of sqlite's 2,528 functions qualify, so purity is NOT the binding constraint — the loop fences are (§13c residual). licm: EXEC geo40 1.9415 → 1.8374 (−5.4%) for +0.011 INSN and +0.75% sqlite — banked because §13a's directive makes EXEC the target and size the byproduct. inline: the bound is DERIVED, not tuned — a body no larger than the call sequence it replaces (`params + 2`: one instruction to place each argument, the `bl`, one to take the result) cannot grow the program — plus gcc-O1's own `-finline-functions-called-once`. Net EXEC 1.8374 → 1.7468, INSN 1.5357 → 1.5148, sqlite 315,665 → 317,285. A called-once callee must also be DELETED once its last call site is gone, or the rule is a pure size loss: sqlite grew 25% before that existed |
| R2.4 if_convert, rotate/final-value/pure-call hoist (+ sink, added) | 🔨 if_convert and sink banked (`pass/ifconv.rs`): a side-effect-free diamond becomes `select`, speculating at most two pure trap-free instructions per arm. Refuses a store, a load, a division whose divisor is not a non-zero literal, and — for now — a FLOAT diamond, since `fcsel` has no MIR form yet. `pass/sink.rs` is licm's dual and was added here rather than planned: §13b ranked register pressure as the largest remaining item, and sinking is the cheapest thing that shortens a live range. **invariant-pure-call hoist ✅ BANKED 2026-08-25** (`licm::hoist_call`, REARCH §13c row 1) — the bucket-emptier. Four fences, each checked: purity (`pass/purity.rs`) · invariant arguments (definitions dominate the preheader) · MEMORY-CLEAN over the whole body (a read-only callee is a function OF memory, so a single store anywhere in the loop breaks the equality — including one AFTER the call, which changes what the NEXT iteration would have read) · GUARANTEED EXECUTION on the first iteration (≥1-trip by evaluating the header test under the preheader's own edge arguments; the call's block dominates every latch AND every other block the loop exits from). Non-termination needed its own argument, since purity does not imply it: the memory-clean fence plus exit-dominance leave only a prior call or a nested loop as a way to diverge ahead of the hoisted call, and both are refused. A FAULT ahead of it needs no fence — a first iteration that faults is UB. **Result: the gcc-ZEROED bucket 6 → 0** (b4/c1/c2/c3/f3/j1, ~371 ms of zcc wall-time), and TWO new asymptotic wins over gcc-O1 in the mirror bucket (e1_recursion gcc 345 ms → zcc ≈0, g2_strlen gcc 161 ms → zcc ≈0). INSN geo40 1.3260 → **1.3043**, median 1.277 → 1.273. EXEC geomean on the COMMON timed set 1.6405 → 1.6232 (n=17, same box, same session) — the pass touches none of those 17 programs, so read that as unchanged, not as a win; the whole effect is in the two buckets and in INSN. sqlite **240,774, byte-for-byte the baseline** (it fires nowhere there — see the residual), compile time +0.5%. **rotate ✅ BANKED 2026-08-25 (shipped OFF, then ON — §13e → §13f)** (`pass/rotate.rs`, gcc's `-ftree-ch`): the square is argued by COUNTING EXECUTIONS — the guard IS the header's first execution — so a copied header need not be pure, it is relocated rather than speculated. It does not pay and §13e says why: rotation makes the back edge critical, the split block is where SSA destruction parks the loop-carried copy, and the branch removed is the branch that copy block adds back (10 instructions per iteration before AND after). Forced on: sqlite +2.7% and +1,732 BRANCHES, geo40 INSN 1.3043 → 1.3972, none of the five §13d loops improved. Quarantined with the gate NAMED — coalesce the loop-carried copy. **THE GATE WAS THEN OPENED (§13f) and rotation is ✅ ON**: `regalloc/color.rs` now frees a dying operand before placing the destination (six lines of `reads_before_writes`, replacing a conservative order that cost one `mov` in every counted loop), and `mir/pass/layout.rs` threads the block that is then empty. EXEC geomean 1.6232 → **1.4276** (≥30 ms only: 1.8423 → **1.4610**), INSN 1.3043 → **1.2437**, sqlite 240,774 → **236,886** with branches 19,151 → **14,711**; h1_popcount now beats gcc-O1 at 0.960, j2_histogram 1.017 and g3_reverse 1.000 reach parity. **final-value still ⬜** |
| R2 gate + measurement | opt-parity (passes off vs on) 0 DIVERGE; csmith/yarpgen 0 DIVERGE. KPI: INSN geo ≤ 1.58 (rc3), sqlite ≤ 1.5×. **Merge-to-main eligibility starts here** | ✅ **both KPIs met and passed**: opt-parity 1552/0, csmith300 254/0, yarpgen300 300/0, torture 1471/0. See the R3 measurement row for the numbers — R2 and R3 were measured together because the isel and MIR rows landed in the same session |

### R3 — machine passes (§8) + isel munch table complete (§6)
| task | status |
|---|---|
| R3.1 munch patterns: addressing modes, cmp-branch fusion, csel forms, madd/msub, bfx, extend folding, mul-by-const | ✅ `isel::munch` — one pre-pass deciding which producers each consumer absorbs, because the producer is emitted first and the consumer's choice has to be known before its turn comes. Two licences, not interchangeable: an ADDRESS folds when EVERY use of it is a memory operand (folding into some while still computing it for others only duplicates work); an ALU operand folds on a SINGLE use (the shift or extension happens inside the consumer). Rows: `[base, #off]`, `[slot, #off]`, `[base, idx, ext #shift]`, `add/sub … , sxtw`, `op … , lsl #k`, `madd`/`msub`, `cmp`+`b.cc`, `cbz`/`cbnz`, `cmp`+`csel`. `ubfx`/`sbfx`, `cbz`/`cbnz`, `tbz`/`tbnz` on the sign bit. `mul(x, 2^k) → shl` is an HIR canonicalization (`fold::canon`) because only the shift form folds into an address. A producer that has itself absorbed something may NOT be absorbed again — the value it swallowed would then be defined nowhere |
| R3.2 cmp_elim, auto_inc, ext_lattice, ldst_pair | 🔨 `ext_lattice` (`mir/pass/ext.rs`), `ldst_pair` (`mir/pass/ldstp.rs`) and `cmp_elim` (`mir/pass/cmpelim.rs`) banked — `uxtb` 3,918 → 344, `uxth` 1,357 → 142, 7,104 `ldp` + 3,224 `stp` where there were none, `cmp` 13,312 → 9,487. cmp_elim fuses only where the CONDITION CODE survives: `cmp d, #0` sets C=1 and V=0 by definition, `adds` sets them from the addition, so only the codes reading N and Z alone carry over — and `lt`/`ge` are rewritten to `mi`/`pl`. **auto_inc ✅** (`mir/pass/autoinc.rs`): `ldr r,[p]; add p2,p,#k → ldr r,[p],#k` (post-index, imm9). LOADS ONLY — a store post-index risks `t==n` UNPREDICTABLE; a load's transfer reg is live past the load so it interferes with the writeback reg by construction. The writeback reg IS the base (emit prints only the base), so `color.rs` TIES `wb` to `base` deterministically (hand base's dying colour to wb before the transfer reg is placed) and `color::check` asserts the tie — that assertion CAUGHT a real bug on `sqlite3IdListDup` (a call-crossing `wb` forced into base's caller-saved reg) which is now fenced: fold refused when `wb` crosses a call or a `Call` sits between load and add. **Yield ≈0 as the handoff predicted**: sqlite 241,055 → 241,046 (−9), geo40 INSN 1.2982 unchanged — sqlite's `add` excess is bfx/scaled-global + call-crossing loop pointers the pass safely refuses (R4/§13b, category-b). Fires+correct on clean array walks (battery `auto_inc_fires_and_preserves_meaning`, box pw.c → 21). Iterate gate green: cargo 117/0, opt-parity 1552/0, torture 1471/0, determinism 85×8; seal (csmith/yarpgen300) background |
| R3.3 switch jump tables, block layout, shrink-wrap | 🔨 jump tables banked: a switch with ≥4 cases occupying ≥half its span becomes `sub`/`cmp`/`b.hi` + `adrp`/`ldrsw`/`br` over a `.rodata` table of signed 32-bit offsets (position-independent, no run-time relocation). Block layout was already R0's; BRANCH RELAXATION was added to it, since `tbz` reaches ±32 KB and `b.cc`/`cbz` ±1 MB against `b`'s ±128 MB and the assembler cannot fix it — a far conditional gets a trampoline, placed AFTER the fall-through inversion so the inversion cannot undo it. **shrink-wrap ✅** (`mir/pass/shrink_wrap.rs`, runs after `frame`): moves the callee-saved SAVES to the nearest dominator of the blocks that use them and drops the RESTORES from the returns the fast path reaches. Four fences, each a miscompile if skipped: D≠entry; D executes at most once (no back-edge into it — else the save re-runs per loop iteration over a live value); R={b:D dom b} is a SINK REGION (leaves only by `ret`, else needs edge-split restores — deferred); and (falling out) no block before D uses a saved register. Moves only the callee-saved MIR — the sp adjust and x29 stay at entry (emit prints them); dynamic frames skipped. Square: `⟦mir_p⟧=⟦mir_final⟧` under the frame battery + the dedicated `shrink_wrap_moves_saves_off_the_fast_path` (fires + value 72 + saves-off-entry). RESIDUAL (category-b): when the value needing a callee-saved reg is a PARAMETER, the allocator homes it in that reg via a `mov` in the ENTRY block, so entry ∈ need and the pass cannot fire — param-copy sinking would lift it (R4). Saved ~272 static insns on sqlite (dropped fast-path reloads) |
| R3.4 **Law-1 sync — DO THIS NEXT.** `THEORY.md` ⊕ the specs IS the source of zcc and `src/*.rs` its compiled object; the docs currently describe a compiler that no longer exists. Re-derive them for everything R2/R3 shipped: **A6** (isel is no longer "the base case only, no munching" — `isel::munch` is the table, and there is no `isel/pattern.rs`) · **A7** (the spiller is Braun-Hack now, WITH its two recorded deviations: no SSA reconstruction, and a spilled parameter leaves the IR — both are theory, not implementation notes) · **A7b** (twelve passes are shipped, not `[PLANNED]`) · the MIR ladder (`ext_lattice`, `ldst_pair`, `cmp_elim` shipped) · rematerialization (shipped). **Side-II constants missing entirely**: `ldp`/`stp`'s scaled signed-7 offset · the branch reaches (`tbz` ±32 KB, `b.cc`/`cbz` ±1 MB, `b` ±128 MB) · `ubfx`/`sbfx` · the jump-table density rule · and above all **DDI 0487 B1.2.1, that every 32-bit write zeroes bits 63:32** — three of §15c's defects are that one line, and it appears in no table. `SEMANTICS.md` owes ⟦`Pair`⟧ and ⟦`Bfx`⟧ and the ⟦·⟧ obligation of each new pass | ✅ THEORY A5 (munch table, no `pattern.rs`), A7 (Braun-Hack spiller + its two deviations, remat shipped), A7b (`[IN USE — R2/R3]`, both ladders re-marked with the two ⬜ rows named), II-5 (new Side-II table: **DDI 0487 B1.2.1** 32-bit-write-zeroes-63:32, `ldp`/`stp` signed-7 scaled C6.2.130, branch reaches C6.2.26/42/375, `ubfx`/`sbfx`, jump-table density) rewritten. SEMANTICS §5.5 ⟦`Bfx`⟧/⟦`Pair`⟧ added, §6.2 flipped from "R0 ships no pass" to the shipped HIR+MIR square list. `isel/lower.rs` header corrected (named a dead `pattern.rs`). Doc + one comment only — cargo **116/0**, host build clean; no `.s` moves, so the f4aec0d gate stands |
| R3 measurement | `corpus25.sh` excess histogram per mnemonic; each class classified fundamental vs convenience (Law-4). Band: sqlite ≤ 1.3×, geo40 INSN/EXEC ≤ 1.2 | ✅ **all R3 rows banked (R3.1–R3.4)**; honest band report, NOT forced. sqlite **240,774 = 1.525×** (R1 origin 2.997×, rc3 1.768×; R3.4-start 241,055 → auto_inc −9 → shrink_wrap −272), geo40 INSN **1.2977** (origin 2.5168, rc3 1.5835), geo40 EXEC **1.5683** noisy/19 (origin 4.4077). **The ≤1.3×/≤1.2 band is NOT met and auto_inc+shrink_wrap were never going to meet it** — §13b shows the 83k excess is spill traffic (+24.5k), copies (+33.5k, ~13k truncating `mov w,w`), layout (+5.8k), addressing (+5.9k); closing it is R4/§13b, the largest-class-first worklist. Finishing R3 = the rows banked + this honest report, per the handoff. Gate: cargo 118/0, determinism 85×8, opt-parity 1552/0, torture 1471/0, csmith300 254/0, yarpgen300 300/0 |

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

## §15 R1 proof debt — CLOSED (R1.6), and what it cost to find out

R1 shipped its features validated by DIFFERENTIAL TESTING ALONE, which inverts
Law 3: csmith/yarpgen were DISCOVERING defects that the layer's own square
should have caught. `cargo test` stood at the 40 batteries R0.9 left; it now
stands at **52**, and no R1 feature is ⊥ on both sides of a square any more.

**What the interpreters gained.** `hir::interp` traps no longer on
`Inst::Intrinsic`: the exclusive pair and the barrier have their single-threaded
meaning, `LdLoad`/`LdStore` cross the binary128 bridge (`f64_to_f128` /
`f128_to_f64`, IEEE 754 §3.6 transcribed), and a variadic call materializes the
AAPCS64 save area and stack-argument area so `va_start`/`va_arg` execute. That
last one makes ⟦hir⟧ ABI-aware in exactly ONE place, and it is inherited rather
than chosen: `build::va_arg` is already lowered against the psABI `va_list`
layout, so a semantics refusing to model it could not run a variadic function at
all. (Removing the dependency means moving `va_arg` lowering into isel — an
option, not a debt.) `mir::interp` gained the soft-float externs and, more
importantly, DDI 0487 C6.2's rule that a scalar FP write zeroes bits 127:64.

**Inline asm stays ⊥ — by construction, not as debt.** An asm template's
meaning is the assembler's, not C's; ⟦·⟧ has nothing to say about it and
should not pretend otherwise.

**Three defects the new batteries found**, none of which the suites had caught:

| defect | why the suites missed it |
|---|---|
| `PTy::LDouble` meant a VALUE at a call site but an ADDRESS at the definition — HIR contradicting itself. isel implemented both conventions consistently, so the compiled code worked and only ⟦hir⟧ could see the contradiction | the code was RIGHT; only the semantics was unrunnable |
| A value crossing a call could exceed the callee-saved count AT A NON-CALL POINT. The spiller measured the ceiling only at calls, but two values may cross DIFFERENT calls and be live together in between — the colourer then needs more callee-saved colours than exist. LATENT MISCOMPILE on any FP-heavy function | needs ≥9 simultaneously-live call-crossing FP values; no suite program has that shape |
| ⟦mir⟧'s `FMov` never carried the upper 64 bits, and no FP write cleared them — so a `q` read could see a stale half | only a 128-bit value can observe it, and `Width::Q` arrived with R1 |

The second is the one that matters: it was a wrong-code bug reachable from
ordinary C, found because a battery asked a question the corpus never did.

Both ceilings are checked at every point the COLOURER assigns — each
instruction AND each block head, since block parameters are coloured there.
Checking only instructions left a hole a csmith program walked straight into.

**Residual, recorded rather than left implicit.** The spiller enforces both
ceilings — pressure ≤ k with a call's clobber set counted as fixed definitions,
and live call-crossing values ≤ the callee-saved count — but it is still
spill-at-def / reload-per-use, NOT Braun-Hack. R2.2's prerequisite is unchanged.
It is also not width-aware: a `q` value crossing a call has no legal colour at
all (AAPCS64 §6.1.2 preserves only the low half of v8–v15), which isel avoids by
parking every quad in memory and `color.rs` now reports as a named error rather
than silently truncating.

---

## §15b R2 defect ledger — what the batteries and the new verifiers found

Every entry below is a WRONG-CODE bug that the differential suites reported as
"binary aborts" and that a layer verifier now names at its own layer. The pattern
is the one Law 3 predicts: a rule becomes REACHABLE only once an earlier layer
optimizes, so R0/R1 could not have found any of them.

| defect | why it was latent, and what now catches it |
|---|---|
| A literal null address rode in the zero register. DDI 0487 C1.2.5: in the Rn field of a load/store, register 31 decodes as SP, not ZR — `strb wzr, [xzr]` is not an instruction | `*(char *)0 = 0` is a literal address only after the cast FOLDS. Caught by `mir::verify`, which now refuses a zero-register memory base (torture `930719-1`) |
| A promoted variable whose dominance frontier reaches a computed-goto target. `goto *e` names its successors without passing arguments, so the parameter placed there was never given a value — and neither `hir::verify` nor ⟦hir⟧ could see the hole, because both read arguments through `Term::targets`, which a computed goto has none of | R0/R1 created almost no block parameters. `sroa` now refuses to promote a variable whose frontier touches an argument-less edge (torture `comp-goto-1 920302-1 920501-3 20040302-1 20041214-1 20071210-1`) |
| Dead-store elimination across a TYPE PUN. `*(double*)p = x; l = *(long*)p; *(double*)p = y;` — the first store is not dead: a read of a different type saw it | The pass had no notion of a read making a store observable. A load that is not forwarded now clears the deletable mark of every store it may alias (torture `cbrt`) |
| A branch condition live across a call took a caller-saved register. `crosses_call` walked backwards from `live_out`, and a value used ONLY by the terminator is not in `live_out` — the branch consumes it here | While every local was a memory cell the condition was reloaded immediately before the branch and never spanned a call. `live::compute` now seeds the walk with the terminator's own operands (torture `pr36343`) |
| At an indirect call the callee pointer and the result share x0 — legally, since `blr` reads the target before the call writes the result — but `occupied` held one entry for the register, so freeing the dying pointer freed the result's register with it | Needs an indirect call whose pointer dies exactly at it. `occupied` is now a multiset, and `color::check` re-derives the whole colouring from `Liveness` INDEPENDENTLY of the incremental set that produced it (torture `pr34768-2`) |

**Two verifiers were added rather than two fixes**, because a fix that only
removes one occurrence of a class of bug is not Law 3:
`regalloc::spill::check_pressure` states the spiller's post-condition (pressure ≤
k, call-crossing ≤ callee-saved, at every point) and `regalloc::color::check`
states the colouring's (no two simultaneously-live values share a register).
Both run on every function in the shipped pipeline: a violation now names the
block, the instruction and the two values instead of surfacing as a wrong answer
in csmith.

**Measurement integrity.** `tests/bench/corpus25.sh` reused `/tmp/corpus25` across
runs with stderr discarded, so a zcc that CRASHED on sqlite silently re-reported
the previous session's numbers — the Article E clean-input hole, and it hid this
milestone's first regression for a full cycle. A failed compile now voids the
measurement, and a program dropped from the per-program table is NAMED.

---

## §13a R1 GROUND METRIC (box, 2026-08-25, HEAD `7279b36`) — the origin for R2

The number every R2/R3 pass is measured against. It is *deliberately* the worst
this branch will ever look: R1 has NO optimization pass at all, and §14's
storage model keeps every C local in memory.

- **sqlite3.c static insns**: zcc **473,253** vs gcc-O1 **157,883** = **2.997×**
  (rc3, fully optimized, was 279,161 = 1.768×). gcc's count reproduces §13's
  157,883 exactly, which is what validates the counting method.
- **geo40 INSN** (deterministic, all 35): geomean **2.5168**, median 2.400,
  worst `c2_bitfield` 3.981, 35/35 above 1.1×.
- **geo40 EXEC** (noisy arbiter, 19 measurable): geomean **4.4077**, median
  6.133, worst `g3_reverse` 8.231. Nine programs in the **gcc-zeroed bucket**
  (gcc-O1 deletes the loop entirely) — asymptotic, kept out of the geomean.
- **Mnemonic composition of sqlite** — the fact that steers R2/R3, because it
  says WHERE the mass is rather than that there is mass:

  | mnemonic | count | share |
  |---|---|---|
  | `add` | 133,264 | **28.2%** |
  | `ldr` | 90,906 | 19.2% |
  | `mov` | 59,224 | 12.5% |
  | `str` | 31,128 | 6.6% |
  | `cmp` | 14,470 | 3.1% |
  | `cset` | 13,161 | 2.8% |
  | `bl` | 13,078 | 2.8% |
  | `sxtw` | 11,477 | 2.4% |
  | `ldp`/`stp`/`csel`/`ldrsw` | 0 | — |

  `add` at 28% is one fact, not a diffuse cost: every local access is a
  `SlotAddr` (one `add`, two when the frame exceeds imm12) followed by a `ldr`,
  because R0/R1 address every local through the parser's frame block. Two
  scheduled passes remove almost all of it — **R2.2 SROA+mem2reg** (the local
  stops being memory) and **R3.1 addressing modes** (`Slot{s,off}` folds into
  the load, deleting the `add` even where the local stays). The four zeros are
  the R3.2/R3.1 rows that have not been written yet, not a measurement gap.
- frame-slot mem-ops 12,253 — recorded but NOT the rc3-comparable KPI (see the
  R1 measurement row).
- Gate at this commit: cargo **52/0** · cases 74/75 (only the adjudicated
  `float_h`) · ext 19/19 · abi/alg/cpp/shape/decay PASS · determinism 85 × 8 ·
  torture **1470 pass / 0 FAIL** / 224 not-impl · csmith300 **254 PARITY / 0
  DIVERGE** · yarpgen300 **300 PARITY / 0 DIVERGE**.
- Reproduce: `ZCC=/usr/local/bin/zcc GCC=gcc SQLITE=/suites/sqlite/sqlite3.c sh
  tests/bench/corpus25.sh` and `ZCC=/usr/local/bin/zcc sh tests/bench/exectime.sh`.

---

## §15c R2.4/R3 defect ledger — what the new layers got wrong

The same pattern as §15b, one layer down: every entry is a wrong-code bug that
only became REACHABLE once an earlier layer optimized, and each is now refused
at the layer that owns the rule.

| defect | the rule that was missing |
|---|---|
| `select c, 1, 0 → c`. A select tests its condition ≠ 0 exactly as a branch does, so the rewrite holds only when `c` is ALREADY 0 or 1 — and `x && y` supplies a whole value. The row is deleted, not narrowed: `fold_inst` sees an operand, never the instruction that produced it (torture `pr10352-1`) | a fold may only use what its own arguments say |
| `orr x0, x1, w3, uxtw`. DDI 0487 C6.2: only ADD/SUB take an EXTENDED register operand; the logical instructions take a shifted one and nothing else (torture `bswap-1`, `cbrt`, thirteen others) | the munch table is a table of the ISA's forms, not of shapes that look plausible |
| An arm's instructions moved into the head of a diamond whose join parameter had a NARROWER type than the argument. `build` occasionally hands an edge a value wider than the parameter it feeds; that is tolerable while the parameter exists to narrow it and ill-typed the moment `select` or a block merge removes it | a substitution is a renaming only when the types agree — now checked in both `ifconv` and `cfg_simplify` |
| LICM hoisting an instruction whose operand was "outside the loop" but did not DOMINATE the preheader. The two are equivalent for a reducible loop and not in general (csmith `c0019`) | the property the verifier checks is dominance, so dominance is the property to test |
| `sxtb w0, w1` recorded as "sign-extended from 8 bits". A `w`-form instruction sign-extends inside the low 32 bits and ZEROES bits 63:32 (DDI 0487 B1.2.1), so a later `sxtw` looked redundant when it was precisely the instruction that would fill the upper half — wrong on every negative value (yarpgen: 45 of 300 diverged) | the extension lattice states its fact about the 64-bit register, and a fact that only holds below bit 32 is restated before an `x`-form consumer reads it |
| A self-move deleted when it was a TRUNCATION. `mov w0, w0` zeroes bits 63:32 (DDI 0487 B1.2.1), so it is only redundant when nobody reads the register wider — and that question is not local: in `t1 = (int)x; t2 = t1; use64(t2)`, `t1` looks 32-bit-only until `t2`'s copy is deleted and `t1` inherits its 64-bit reader. Latent until biased colouring began handing a copy its source's own register on purpose; yarpgen then diverged on 45 of 300 (`s0131` and nine others after the first fix) | the decision moved into `destruct::apply_colors`, the one place that has BOTH the virtual identity (how wide the value is ever read) and the colour (whether the move is a self-move at all), and it is computed as a FIXPOINT over the chain |
| Biased colouring taking a copy partner's register when the partner was `Reg::P(ZR)`. An integer constant zero IS the zero register, so an edge argument holding one handed x31 to a real value — `cmp wzr, #5` for a loop counter | a hint is filtered through `alloc_mask`, and `color::check` now refuses a non-allocatable colour outright |

The last two are the argument for the verifiers of §15b in one line: neither was
found by reading the pass, and both were named by a checker at the layer that
owns the invariant rather than by a suite three layers away.

---

## §13b R3 EXCESS HISTOGRAM (box, 2026-08-25) — what is left, and what each class is

sqlite, zcc **241,055** vs gcc-O1 **157,883** = **1.527×**. The excess is 83,172
instructions, and it is NOT diffuse — four classes carry 80% of it. Law-4
demands each be classified *fundamental* (a real ISA/ABI boundary) or
*convenience* (an incomplete realization); only the second kind is work.

| class | zcc | gcc | excess | classification |
|---|---|---|---|---|
| `ldr` + `str` (frame mem-ops 44,394 of them) | 58,218 | 33,653 | **+24,565** | **convenience** — spill traffic. The allocator keeps one reload per block-residency and has no cross-block SSA reconstruction (see §7's recorded deviation), so a value used in five blocks is reloaded five times. This is the single largest item and the one with a named fix |
| `mov` + `movz` (immediates and copies) | 67,552 | 34,065 | **+33,487** | **mixed, and the largest single named residual**. Part is rematerialization, a WIN traded against the reload it replaces. Part is copies biased colouring did not manage — aggressive (Boissinot) coalescing is the named fix. And about 13,000 are the TRUNCATION `mov w, w` that A64 requires whenever a 64-bit value is narrowed and then read wide again: `Cvt::Trunc` emits a real instruction because the width-typed virtual register cannot express "the same register under a narrower name". Making truncation a rename is the fix, and it needs the block-parameter width rule relaxed first |
| `b` | 14,290 | 8,459 | +5,831 | **convenience** — block layout only inverts a conditional whose taken target is next; it does not choose the order to maximize fall-through, and it does not tail-duplicate |
| `add` | 17,567 | 11,654 | +5,913 | **convenience** — address arithmetic the munch table does not yet reach: `bfx`, pre/post-index (`auto_inc`), and a scaled index whose base is a global |
| `csel` | 4,400 | 570 | +3,830 | **judgement, not excess** — zcc if-converts more than gcc -O1 does. It buys the `b` and the misprediction; whether it is a win is an EXEC question, and the exec number says yes |
| `cmp` | 10,150 | 6,999 | +3,151 | **convenience** — `cmp_elim` (`subs`/`ands` instead of a separate compare) is not written yet |
| everything else | | | +5,608 | small; `mul` +623 is a strength-reduction gap, `sxtw` +797 the residue `ext_lattice` cannot see across blocks |

The ranking is the R4 worklist, in order: spill traffic, then copies, then
layout, then the remaining addressing modes.

---

## §13c R2-LOOP BASELINE + OPPORTUNITY (box, 2026-08-25, HEAD `02db900`) — the "before" for the remaining loop passes; MEASUREMENT collected, implementation is next session

The only work still pending in R2+R3 is the six loop items in R2.3/R2.4 (iv/strength-
reduce/pointer-iv/LFTR, rotate/final-value/invariant-pure-call hoist). This section is the
baseline every one of them is measured against, and — the point of collecting it FIRST — a
measurement that **re-orders the plan**.

### Baseline dashboard (numbers to beat)
- sqlite **240,774 = 1.525×** gcc-O1. geo40 **INSN 1.2977** (determ, 35), **EXEC 1.5822**
  (noisy, 19; median 1.833; worst j5 2.868).
- **HARNESS FIX, 2026-08-25 (row 1).** `exectime.sh` printed `EXEC geomean 0.0000` the moment zcc
  produced a 0 ms run — `log(0)`, the reducer bug §13 had already flagged. Fixed SYMMETRICALLY, as
  the mirror of the gcc-ZEROED rule rather than as a clamp: a side below `GCC_FAST` is unmeasurable
  at ms granularity, so a program where ZCC is the fast side leaves the constant-factor geomean and
  is reported by name in a **zcc-ZEROED** bucket. Consequence for reading the table: the timed pool
  CHANGES SIZE as programs enter and leave the buckets, so two geomeans over different pools are not
  comparable — every A/B below is quoted over the COMMON timed set, re-measured in the same session
  with `ZCC_NOPASS=purecall` as the "before".
- sqlite mnemonic excess for the loop-pass targets (zcc − gcc): `mul` **+623**, `msub` +105,
  `madd` −9 · `bl` **+448** · `b` **+5,833** · `cmp` +2,488.

### The gcc-ZEROED bucket — the biggest EXEC gap vs gcc-O1
| prog | zcc ms | gcc ms | INSN ratio |
|---|---|---|---|
| b4_ptr_diff | 25 | ~0 | 1.255 |
| c1_struct_sum | 31 | ~0 | 1.281 |
| c2_bitfield | 30 | ~0 | 1.463 |
| c3_nested_struct | 29 | ~0 | 1.418 |
| f3_float_minmax | 106 | ~0 | 1.679 |
| j1_reduction | 152 | ~0 | 1.457 |

**~373 ms of zcc wall-time that gcc spends ≈0 on.** Emptying this bucket is the single
largest EXEC win available.

### THE FINDING that re-orders the plan (read the SOURCES, not just the numbers)
All six have ONE shape: `main` runs `for(k=0;k<K;k++) s += work(array, n)`, where `work`
reads the array — UNCHANGED across the k-loop — and returns the SAME value every call. gcc
zeros them by **loop-invariant PURE-CALL hoisting**: hoist `work(array,n)` out of the k-loop
(compute once), leaving `for(k){s+=const}`, which final-value then closes to `s=const*K`.
So the bucket is emptied by **invariant-pure-call hoist (R2.4, the old OPT.md #24)**, with
**final-value** finishing the leftover k-loop — NOT by final-value on the inner loop. This is
exactly the #24 shape (`2af2702` on `main`: j1 152→~1 ms).

### Infra MISSING on this branch (what next session must build FIRST)
- **Interprocedural purity** — for pure-call-hoist. Per-instruction `Effect` (Pure|Read|
  Write|Call, `hir/mod.rs:361`) EXISTS; the FUNCTION-level fixpoint (`is work() pure?`) does
  NOT — it was `inline.rs::pure_functions` on `main` and was lost in the big-bang. Hoisting
  `work(a,n)` also needs the outer loop proven MEMORY-CLEAN (nothing in it writes memory the
  callee may read) — the #24 four-fence: purity · invariant args · memory-clean · ≥1-trip.
- **IV/SCEV analysis** — for final-value, LFTR, pointer-iv. None exists (`hir/scev.rs` to be
  written). Target-independent (HIR) → x64 inherits it.

### CORRECTED execution order (edit R2.3/R2.4 IN PLACE when banking; anti-fragmentation)
1. ✅ **BANKED 2026-08-25 — interprocedural purity + invariant-pure-call hoist.** KPI met: bucket
   **6 → 0**. Shipped as `pass/purity.rs` + `licm::hoist_call`, NOT as a new `pass/loop.rs`: the
   hoist is licm's own theorem with a call as the hoisted term (same preheader, same invariance
   rule, same dominance argument), and a second file would have split one theorem across two
   seams for no proof gained. Full row + numbers in §12 R2.3/R2.4.
2. **loop rotation** (`pass/loop.rs`) — unblocks licm read-hoist (licm's own recorded residual);
   gives clean trip count for final-value. KPI: `b` count, EXEC on hot loops.
3. **final-value / SCEV** (`pass/loop.rs`) — needs IV/SCEV; closes the leftover k-loop after
   hoist and any counted-loop-final-scalar. KPI: finishes bucket→0, sqlite INSN.
4. **LFTR** (`pass/iv.rs`) — needs IV; kills the original counter (DCE). KPI: small INSN.
5. **pointer-IV** (`pass/iv.rs`) — non-pow2 stride → running pointer; x64-relevant (arm64
   post-index half is `auto_inc`, shipped). KPI: `mul`/`add` in loops.
- **SKIP scalar mul→add strength-reduction** — `mul` excess is only +623 and mul→add is 1:1
  static, and A64 (like any OoO core) pipelines `mul` at ~add cost, so the rewrite is ≈0 on
  every target. Classify as Law-4 category-(a) FUNDAMENTAL (OoO makes it null), not pending.
  The IV *analysis* under it is still built (for rows 3–5).

### Non-bucket high-EXEC (rotation + licm-read-hoist targets, real O(n) work each call)
g1_memcpy 2.02 · g2_strlen 2.00 · g3_reverse 1.96 · j3_prefix_sum 1.94 · h1_popcount 1.85 ·
h2_revbits 1.84 · j2_histogram 1.74. These are hot inner loops; the INSN gap (1.1–1.4×) plus
one branch/iteration is where rotation + stronger licm pay, not final-value.

### Row-1 LAW-4 RESIDUAL (measured, not reasoned — `ZCC_RESIDUAL=1`, sqlite, 2026-08-25)
`readonly` = **317 of 2,528** functions, so PURITY is not the binding constraint. 1,816 loops hold
a read-only call the hoist could have moved and did not; every one is classified:

| fence that refused | count | class | what would close it |
|---|---|---|---|
| memory-clean | 1,414 (78%) | **(a) fundamental for this predicate** | the loop really does write memory the callee may read. It only becomes (b) with an ALIAS ORACLE proving the writes cannot reach what the callee reads — TBAA, §16 ★1, unbuilt |
| trip-count (≥1-trip undecidable) | 352 (19%) | **(b) convenience** | **row 2, loop rotation** — a rotated loop is a do-while, and ≥1 trip becomes structural instead of arithmetic. This is precisely why §13c ordered rotation second |
| conditional-call | 37 (2%) | (b) | a GUARDED hoist (duplicate the call under the condition) — O2 territory, not O1 |
| variant-args | 13 (0.7%) | **(a) fundamental** | nothing: the arguments genuinely differ per iteration |

The first cut of the fence was "the header is the only exit", which refused **1,123** of those 1,816 —
every loop with a `break` or an early `return` after the call. All 1,123 were category (b), and the
fence was replaced by exit-DOMINANCE (a `break` reached THROUGH the call proves the call ran) before
this row was banked: Law 4, `cấm dừng ở green đầu tiên`. The batteries
`a_break_after_the_call_still_lets_it_out` / `a_break_before_the_call_keeps_it_in` pin both sides.
A second residual lives inside the analysis rather than the transform: a pointer carried round a loop
as a block PARAMETER is never proven frame-local, so a helper writing only into its own buffer through
such a pointer is not read-only (category (b); needs the optimistic-cycle treatment). Recorded in
`pass/purity.rs`.

### Per-row process + dashboard (fixed)
predict Δ on the model → implement + inline commuting-square battery → cargo (0.4s) →
iterate gate opt-parity+torture (~75s) → seal csmith300+yarpgen300 (~3min) → re-measure
`corpus25.sh` (size + mnemonic) and `exectime.sh` (INSN+EXEC geomean, distribution, **bucket
count**) → bank (commit+push, edit R2.3/R2.4 in place) or quarantine → advance. Reproduce the
baseline: `ZCC=/usr/local/bin/zcc GCC=gcc SQLITE=/suites/sqlite/sqlite3.c sh
tests/bench/corpus25.sh` and `ZCC=/usr/local/bin/zcc sh tests/bench/exectime.sh`.

### Zero-pending accounting
R3 is fully ✅. R0, R1, R2.1, R2.2 are ✅. The ONLY non-✅ items in all of R0–R3 are the six
loop rows above. When rows 1–5 are banked (strength-reduce classified category-(a)), R2.3 and
R2.4 flip ✅ and **R2+R3 have zero pending**.

---

## §13d `main` vs `mir-rearch`, SAME harness SAME box SAME session (2026-08-25, after row 1)

Prompted by a claim that `main` reached geo40 EXEC **1.55×** while this branch sits at ~1.7×, i.e.
that the re-architecture went BACKWARDS on the axis that matters. Both compilers were therefore
built for the box and run through the SAME `exectime.sh` (`main` = `4fb7a0a`, this branch =
`09403df`). The 1.55 does not reproduce, and the reason is the one §13c had already written down:
it was a geomean over a DIFFERENT POOL of programs measured by the pre-patch harness. Same pool,
same session:

| metric | `main` 4fb7a0a | `mir-rearch` 09403df |
|---|---|---|
| EXEC geomean, common 17 | 1.6642 | **1.6232** |
| INSN geomean, common 34 | 1.6191 | **1.3007** |
| sqlite static insns | ~285,899 (1.83×) | **240,774 (1.53×)** |
| DIVERGE | 1 (`f1_float_sum`) | **0** |

So the branch is not behind. But the honest reading of the same table is the finding, and it is
not comfortable: **`main` is faster on 10 of the 17 individually timed programs.** This branch wins
the geomean only by killing `main`'s outliers (i1_global_acc 4.316→1.300, f2_double_poly
3.800→1.400, j5 3.820→2.869). Five hot loops went the other way, every one of them emitting FEWER
instructions and running SLOWER:

| program | EXEC main → rearch | INSN main → rearch |
|---|---|---|
| g1_memcpy_loop | 1.000 → **2.021** | 1.167 → 1.100 |
| j3_prefix_sum | 1.245 → **1.920** | 1.382 → 1.255 |
| h2_revbits | 1.222 → **1.865** | 1.594 → 1.438 |
| h1_popcount | 1.531 → **1.859** | 1.429 → 1.314 |
| g3_reverse | 1.692 → **1.962** | 1.830 → 1.277 |

Fewer instructions and slower is a Law-2 signal, not a mystery: the count fell where it does not
execute, and rose where it does. Read at the instruction level (`mycopy`, g1), the inner loop says
it plainly — `main` 3 instructions per iteration plus the test, this branch 7 plus the test:

```
main:      ldrb w1, [x20], #1 ; strb w1, [x19], #1 ; b .L1     (+ cmp/b.hs)
rearch:    sxtw x1, w0 ; add x2, x3, x1 ; add x6, x4, x1 ;
           ldrb w1, [x6] ; strb w1, [x2] ; add w1, w0, #1 ;
           mov x0, x1 ; b .L1                                  (+ cmp/b.lt)
```

Four named causes, and three of them are ROWS ALREADY ON THE §13c LIST:
1. **two branches per iteration** — the loop is top-tested. §13c **row 2, rotation**.
2. **the index is rebuilt every iteration** (`sxtw` of a 32-bit counter, then address arithmetic
   from scratch) where `main` walked a POINTER with post-increment. §13c **row 5, pointer-IV** —
   and note `auto_inc` (R3.2, shipped) cannot fire until the pointer IV exists to attach it to.
3. **un-coalesced edge copies** (`mov x0, x1 ; mov x1, x7` in j3's body) — the SSA-destruction
   residual §13b sized at `mov` +33,487. **R4.**
4. `main` INLINED `mycopy` into the hot caller and this branch does not — inline policy, and
   `-finline-functions-called-once` does not cover it because the callee is not static.

**This CORRECTS the projection given earlier in this session**, which called rows 2–5 modest and
put the money in R4. That was inferred from the sqlite static histogram, where these loops barely
appear; measured on the hot-loop programs, rows 2 and 5 attack roughly half the per-iteration
instructions of the worst regressions. The plan order in §13c stands unchanged — rotation second,
pointer-IV fifth — but their EXPECTED VALUE is revised UP, and the five programs above are their
KPI, not sqlite.

---

## §13f ROW 3 — the gate opened. Coalescing + block threading, and rotation turned ON.

§13e quarantined rotation with a named gate: coalesce the loop-carried copy. Opening it took TWO
fixes, and the second was not predicted — which is the point of naming a gate rather than guessing
at one.

**Fix 1 — the allocator frees a DYING operand before placing the destination** (`regalloc/color.rs`).
The colourer already did this for a plain `Copy`, with the conservative order kept "for everything
else" to avoid the case analysis. The case analysis is `reads_before_writes`, it is six lines, and
the convenience cost exactly one instruction in every counted loop: `add w1,w0,#1 ; mov x0,w1`,
because w1 was placed while the dying w0 still held its register. On A64 an instruction reads every
source before it writes any destination, so the register is free. The exceptions are rules, not
cautions: `ParallelCopy` (simultaneous), `StlXr` (DDI 0487 — Ws may not be Xt or Xn), `Pair`, `Asm`,
`Call`, `StackAlloc`, and a pre/post-index access whose writeback is TIED to the base (R3.2).

**Fix 2 — MIR layout threads empty blocks** (`mir/pass/layout.rs`). With the copy gone, the block
holding it is empty — and an empty block is not an accident: critical edges are split before
allocation precisely so destruction has somewhere to put a copy, and when coalescing succeeds there
is no copy to put. What remained was a branch to a branch, which is the difference between one
branch per iteration and two.

Only with BOTH does the loop become what the theorem promised:

```
row 1      .L1: cmp w0,w5 ; b.lt .L2
           .L2: sxtw ; add ; add ; ldrb ; strb ; add w1,w0,#1 ; mov x0,x1 ; b .L1   10, two branches
row 3      .L2: sxtw ; add ; add ; ldrb ; strb ; add w0,w0,#1 ; cmp w0,w5 ; b.lt .L2  8, one branch
```

### Numbers (paired, same box, same session, vs the row-1 bank `09403df`)
| metric | row 1 | row 3 |
|---|---|---|
| **EXEC geomean, common 17** | 1.6232 | **1.4276** |
| **EXEC geomean, ≥30 ms only** | 1.8423 | **1.4610** |
| INSN geomean (35) | 1.3043 | **1.2437** (26 of 35 over 1.1×, from 32) |
| sqlite insns | 240,774 | **236,886** |
| sqlite branches | 19,151 | **14,711** (−23%) |

| hot loop | row 1 | row 3 |
|---|---|---|
| h1_popcount | 184 ms, 1.859 | 95 ms, **0.960** — faster than gcc-O1 |
| j2_histogram | 103 ms, 1.746 | 60 ms, **1.017** |
| g3_reverse | 51 ms, 1.962 | 26 ms, **1.000** |
| h2_revbits | 69 ms, 1.865 | 45 ms, **1.216** |
| g1_memcpy_loop | 97 ms, 2.021 | 74 ms, **1.542** (INSN 1.000) |
| j3_prefix_sum | 96 ms, 1.920 | 99 ms, 1.980 — unmoved |
| j5_insertion_sort | 2869 ms, 2.869 | 2855 ms, 2.864 — unmoved |

Gate: cargo 131/0, opt-parity 1552/0, torture 1378 pass / 0 FAIL, csmith300 254/0, yarpgen300 300/0,
determinism 85 × 8 fresh processes.

### The defect the idempotence battery caught, recorded because it is a trap
Rotation's termination argument was "a rotated loop's LATCH exits, and the pass refuses those". It
is false the moment the ladder is re-entered: `split_critical_edges` runs at the top of every entry
and puts an EMPTY block on the back edge, that block becomes the latch, the latch no longer exits,
and the rotated loop presents itself as top-tested again — peeling its whole body into a second
guard. `ladder_is_idempotent_at_the_fixpoint` is what found it, by the instruction count going UP on
a second run. The guard is now phrased about the BODY — a loop with no work outside its header has
nothing for the test to move past — which no later pass can invalidate. **A termination argument a
later pass can quietly falsify is not a termination argument.**

Two licm batteries also had to be re-pointed rather than repaired: rotation legitimately REMOVES the
condition the ≥1-trip fence and the trap-freedom fence refuse on, because a rotated loop reaches its
preheader only through the guard and has therefore already run once. licm's own header predicted
exactly this ("proving ≥1 iteration is the rotation theorem's job"). The fences are now proven on the
unrotated shape they were written against (`unrotated()`), and the interaction has its own battery
(`rotation_licences_the_hoist_the_trip_count_fence_refused`) whose square is the proof: the bound is
0, the guard fails, the hoisted call is never made.

### What is left, and what the numbers now say about it
j3_prefix_sum (1.980), d3_early_exit (2.000) and j5_insertion_sort (2.864) did not move, and
g1_memcpy sits at INSN **1.000** — instruction-count parity with gcc-O1 — while still costing 1.542
on the clock. That is the clearest statement yet that what remains in these loops is NOT instruction
count. It is the DEPENDENCE CHAIN: `sxtw x1,w0 ; add x1,x4,x1 ; ldrb w1,[x1]` makes every load wait
on three dependent operations, where gcc walks a pointer and issues `ldrb w1,[x20],#1` immediately.
That is §13d cause #2 and it is the next row — pointer-IV, on top of the IV/SCEV analysis.

---

## §13g SCEV — the affine analysis the last three rows sit on (2026-08-25)

`pass/scev.rs`. An ANALYSIS, so no commuting square; what it owes is that every
recurrence it reports is TRUE, because pointer-IV, final-value and LFTR each turn a false one into
a miscompile. A value's evolution is `{base + off, +, step}` — one loop-invariant symbolic term,
constant step, no nesting. That is the affine fragment of chains of recurrences, and it covers
every shape the three consumers need (`i`, `p + i*4`, `n - i`). Outside it, `None` — never an
approximation. Seven batteries, including two that pin REFUSALS.

**The one deep fact, and it is a consequence of this compiler's own semantics.** `sext(i)` for a
32-bit counter is affine only while `i` does not wrap. Every other compiler gets that free from
"signed overflow is undefined"; **SEMANTICS.md §7 defines it as WRAPPING here**, so the shortcut is
unavailable and the no-wrap fact has to be PROVEN — from the trip count, in `stays_in_range`.
Consequence, pinned by `scev_refuses_the_widening_it_cannot_bound`: `p[i]` in a loop whose bound is
a parameter has NO evolution. With a literal bound it reports `{p + 0, +, 4}`. This is the
analysis's largest Law-4 residual and it is category (b) — a value-range analysis on `i` (§16 ★2)
closes it without any trip count.

**The trip count counts BODY executions, and where the test sits is part of the answer.** Let `k` be
the number of times the test says "stay in". A top-tested loop asks before the body, so the body
runs `k`; a bottom-tested one — which is now every counted loop, since rotation ships — asks after,
so it runs `k + 1`. Both shapes are pinned by battery. A test in the MIDDLE of the body is refused
outright: the two halves run different numbers of times and there is no single count to report.
`i += 3` to a bound of 10 is four trips, not three, and that is a battery too, because an
off-by-one here is written straight into the program by final-value.

Shipped UNWIRED — no pipeline row calls it yet, and the tree is byte-identical to `5118da0` over 56
programs. That is a deliberate exception to Article A's "no feature before a real `.c` demands it",
taken because the three demanders are named rows of this plan and because the analysis is the part
that has to be right BEFORE any of them is written. It is dead weight if the next row is not built.

---

## §13h ROW 5 — pointer induction variables, and the miscompile the bench suite caught

`pass/iv.rs`, on `pass/scev.rs`. For a load whose address has an affine evolution, a header
parameter walks the address itself: entry edges pass `base + off`, latch edges `q + step`, the load
reads `q`, and `auto_inc` (R3.2) then folds the bump into the access. `mycopy`'s inner loop becomes
`ldrb w1, [x4], #1` — the exact instruction §13d recorded gcc emitting and this branch not.

| metric | row 3 (`5118da0`) | row 5 |
|---|---|---|
| **EXEC geomean, ≥30 ms** | 1.4610 | **1.4125** |
| EXEC geomean, common 18 | 1.3996 | 1.3788 |
| INSN geomean (35) | 1.2437 | 1.2460 |
| sqlite insns | 236,886 | 238,730 (+0.8%) |
| **g1_memcpy_loop** | 74 ms, 1.542 | **48 ms, 1.000** — parity, INSN **0.950** |
| j3_prefix_sum | 99 ms, 1.980 | 96 ms, 1.920 |
| d2_nested_loops | 2.000 | 1.900 |
| **j2_histogram** | 60 ms, 1.017 | **68 ms, 1.153** — a real regression, below |

Gate: cargo 139/0, opt-parity 1552/0, torture 1378 pass / 0 FAIL, csmith300 254/0, yarpgen300
300/0, determinism 85 × 8.

### THE MISCOMPILE, and why it is the most valuable thing in this section
`j5_insertion_sort` DIVERGED — 24,356,600 against gcc's 24,577,970. Bisected mechanically
(`ZCC_NOPASS`) to iv × rotate, then minimized, then read at the instruction level: the pointer
walk started at `p` where it should have started at `p + 4`.

The defect was in `scev.rs`, not in the transform. The widening rule computed the recurrence of
`sext(x)` and returned `{base: None, off: x.off, step: x.step}` — **silently DISCARDING a symbolic
base**. For `for (i = k; …) p[i]` that reports `{p + 0, +, 4}` instead of `{p + k*4, +, 4}`, and
insertion sort duly sorted the wrong window. Every other rule in the file refuses a base it cannot
carry (`scale` does, `Sub` does); the conversion rule dropped it. **A rule that returns a WRONG
answer where its neighbours return `None` is the shape to hunt for in an analysis** — it is
invisible to every battery that only asks whether the analysis fires.

Fixing it exposed the second half: `licm::preheaders` builds a preheader that FORWARDS the header's
parameters, which turns the literal start `0` into a symbolic one — so the now-correct refusal
killed every ordinary `for (i = 0; …)`. `const_through` resolves a parameter to the constant every
predecessor passes, which is an equality rather than an approximation.

### LOADS ONLY, and it is a COST argument, not a safety one
A64 addresses `p[i]` with a scaled index — `ldr w, [base, w, sxtw #2]`, one instruction, the
arithmetic free. Strength-reducing that trades a free addressing mode for an explicit `add`, so it
only pays when the add then disappears into a post-index — and A64 offers post-index safely for
LOADS alone (`STR Xt, [Xn], #imm` with t == n is CONSTRAINED UNPREDICTABLE, which is why `auto_inc`
is loads-only). Measured with stores included: `j2_histogram`'s zeroing loop went from four
instructions per iteration to five and the program lost 60 → 69 ms. The step must also fit the
post-index imm9, or the fold cannot happen and the pointer is pure cost.

### RESIDUAL, measured with 9 samples rather than reasoned about
| program | without iv | with iv |
|---|---|---|
| g1_memcpy_loop | 73 ms | **47 ms** |
| j3_prefix_sum | 96 ms | 95 ms |
| **j2_histogram** | 59 ms | **67 ms** |

j2 regresses at IDENTICAL instruction count (INSN 1.047 both ways): the same eight instructions,
with `ldr w4,[x2],#4` in place of `ldr w4,[x2,w1,sxtw #2]`. So the cost is microarchitectural, not
static — the writeback form plausibly issuing as a second µop where the scaled-index form does not.
That is a HYPOTHESIS and it is written here as one: it has not been proven, and proving it needs a
cycle-level measurement this harness does not take. Until then the honest statement is that
post-index is not free, and that the profitability rule above ("does the add disappear") is
necessary but not sufficient. Closing it is a cost-model question, category (b).

---

## §13i ROW 5 re-judged on the DISTRIBUTION, and gated off. The winner was paying an isel debt.

§13h banked pointer-IV on a geomean. Asked the sharper question — is the win BROAD? — the answer is
no, and the row does not survive it.

| set (≥30 ms, best-of-5) | row 3 | row 5 |
|---|---|---|
| all 8 | 1.4610 | 1.4125 (−3.3%) |
| **minus g1_memcpy** | 1.4498 | **1.4840 (+2.4% WORSE)** |
| all timed, minus g1_memcpy | 1.3917 | 1.4051 (+1.0% worse) |

**1 win / 1 loss / 6 flat**: g1_memcpy −35%, j2_histogram +13%, the other six inside ±3%. Take the
single winner away and the row is a net loss for +0.8% on sqlite. That is not a row that earns its
place, and a geomean that says otherwise is the "single number flatters" trap this branch already
wrote down twice.

**Where the winner actually comes from.** With pointer-IV OFF, `d[i] = s[i]` compiles to

```
    zcc     sxtw x1,w0 ; add x2,x3,x1 ; add x1,x4,x1 ; ldr w1,[x1] ; str w1,[x2]
    gcc     ldrb w3,[x1,x2] ; strb w3,[x0,x2]
```

isel does NOT fold the add into the addressing mode — for `char` OR `int` — when TWO accesses share
one index. It does fold when there is one (`j2`: `ldr w4,[x2,w1,sxtw #2]`). So most of the 35% is
pointer-IV paying off an ISEL debt, and the addressing mode should pay it directly: no extra
parameter, no extra register, no size, and no post-index µop to lose on. `d[i] = s[i]` — two
accesses sharing an index — is about the commonest loop shape there is.

So the pass ships `ENABLED = false` (`ZCC_IV=1` forces it), batteries and square intact, and the
gate is: **fix the addressing-mode fold, then re-measure.** What remains of the row afterwards is
the post-index form alone, and j2 — regressing at IDENTICAL instruction count — is standing evidence
that that is not reliably a win.

This is the second time a row measured worthless because a LOWER layer was leaving instructions on
the table (§13e → §13f was the first, and there the fix made rotation a large win). The lesson is
about ordering, and it is now written twice: when a transform's number disappoints, look DOWN before
looking sideways.

---

## §13j THE ISEL DEBT — one condition in the munch table, and it was worth more than the row above it

§13i said pointer-IV's only win was paying an isel debt, and named the debt: `add` + load is not
folded into an addressing mode when TWO accesses share one index. It is one line.

```rust
let pick = |i, base| { if uses[i] != 1 { return None; }  … };
```

The index had to be single-use. But single use is what licences PEELING the `sext`/shift INTO the
addressing mode; it is not what licences folding the `add`. A multiply-used index is simply read as
a register — `[base, idx]`, the plain 64-bit register-offset form — and the `add` still disappears.
Conflating the two cost one `add` PER ACCESS in `d[i] = s[i]`, the commonest loop shape there is.

```
    before   sxtw x1,w0 ; lsl x1,x1,#2 ; add x2,x3,x1 ; add x1,x4,x1 ; ldr w2,[x1] ; str w2,[x2]
    after    sxtw x1,w0 ; lsl x1,x1,#2 ;                               ldr w2,[x4,x1] ; str w2,[x3,x1]
```

### Numbers — and note the DISTRIBUTION, which is the whole point of §13i
| set | row 3 | isel fix |
|---|---|---|
| EXEC ≥30 ms | 1.4610 | **1.3789** |
| EXEC all timed | 1.3996 | 1.3654 |
| INSN (35) | 1.2437 | **1.2419** |
| sqlite | 236,886 | 237,026 (+140, flat) |

**8 of 8 programs improve, 0 regress.** g1_memcpy 74 → 47 ms (−36.5%) — the SAME win pointer-IV
bought, without the pass, without the parameter, without the register, without the +0.8% size, and
WITHOUT j2_histogram going backwards (it improves, 60 → 59 ms). Compare §13i's row 5: 1 win, 1 loss,
6 flat, ≥30 ms 1.4125. The lower layer is strictly better on every axis.

### Two defects found on the way, both of the same family
`yarpgen s0096` failed to compile — `use of undefined v2659` — the moment the fold began reaching
these adds. Two orphaned-value bugs, both pre-existing and both made REACHABLE by the wider fold:

1. **The dead-marking guessed which operand was the index**, testing each side for `scaled`. An
   `add` whose BASE happened to be a single-use sign-extension had the base marked dead — and `dead`
   means "not emitted", so its register was never defined. `Folded::Indexed` now RECORDS which
   operand it chose (`src`) instead of re-deriving it.
2. **The ALU stage absorbed operands into instructions that were already dead** from address
   folding. The absorbed value is marked dead too, and then nothing emits it. Fixed by skipping a
   `Bin` whose destination is already dead — absorbing into an instruction that is never emitted
   was meaningless anyway.

Both are the same shape: a value marked "rides inside its consumer" when the consumer no longer
exists. Worth naming, because address folding will keep widening.

**ORDERING, now the third instance.** §13e→§13f: rotation worthless until coalescing and block
threading were fixed below it. §13i→§13j: pointer-IV worthless, and the layer below it was the whole
story. When a transform's number disappoints, look DOWN before looking sideways.

---

## §13e ROW 2 (rotation) — measured worthless, quarantined with a named gate. **Superseded by §13f: the gate was opened and rotation is ON.** Kept because the diagnosis is the reason row 3 existed.

§13d named rotation as cause #1 of every hot-loop regression: `mycopy`'s inner loop pays a
conditional branch at the header AND an unconditional one at the latch, where `main` and gcc pay
one. `pass/rotate.rs` implements it (gcc's `-ftree-ch`, header copying, enabled at its -O1), with
the commuting square argued by COUNTING EXECUTIONS — the guard IS the header's first execution, so
a loop entered n times still runs the header n+1 times, in the same order, on the same values.
That is why a copied header need not be pure: it is relocated, not speculated. Batteries in
`pass/tests.rs` pin the square, the zero-trip case, the store refusal and idempotence.

**It does not pay, and the measurement says why.** Rotation makes the back edge CRITICAL — the
header gains a second successor and the new header a second predecessor — so it is split, and the
split block is precisely where SSA destruction parks the loop-carried copy. The branch rotation
removes is the branch the copy block adds back:

```
before      .L1: cmp w0,w5 ; b.lt .L2
            .L2: ...work... ; add w1,w0,#1 ; mov x0,x1 ; b .L1         = 10 / iteration
after       .L2: ...work... ; add w1,w0,#1 ; cmp w1,w5 ; b.lt .L13
            .L13: mov x0,x1 ; b .L2                                    = 10 / iteration
```

| metric | row 1 (banked) | rotation forced on |
|---|---|---|
| sqlite insns | 240,774 | 247,202 (+2.7%) |
| sqlite branches | 19,151 | **20,883 (+1,732)** — the target metric, wrong way |
| geo40 INSN | 1.3043 (32 of 35 > 1.1×) | **1.3972 (35 of 35)** |
| geo40 EXEC | 1.6232, median 1.746 | 1.6269, median 1.800 |
| the five §13d loops | — | none improved (g1 2.021→2.021, j3 1.920→1.939, g3 1.962→1.923, h1 1.859→1.806, h2 1.865→1.833) |

Per the no-pivot contract this is a BLOCKER quarantined to its own row: one bounded attempt was
made and it is recorded — the first fence refused any loop whose header value is read after it
(`for(...) s+=...; return s;`, the commonest shape there is), and building loop-closed SSA for the
exit removed that refusal. It made rotation fire MORE and the numbers WORSE, which is itself the
confirmation: the transform is not being throttled, it is being cancelled.

So the pass ships `ENABLED = false` with `ZCC_ROTATE=1` to force it for re-measurement, and the
tree is **byte-identical to the row-1 bank over 56 programs** (`tests/refactor_gate.sh`) plus
sqlite at exactly 240,774 — so row 1's gate carries over unchanged and nothing was re-run to claim
it. Precedent: `main` reached the same verdict at OPT.md #17 and shipped rotation off too; the
difference is that the cause is now named rather than observed.

**THE GATE TO TURN IT ON, and it is one thing: coalesce the loop-carried copy** (§13b's `mov`
+33,487, R4). With `mov x0, x1` gone the latch block is empty, cfg_simplify merges it, and the
loop becomes the `...work... ; cmp ; b.lt` the theorem promised. The theorem and its batteries are
already proven and waiting; only the number needs re-taking.

### What this does to the plan (edit in place; §13c order revised, not replaced)
Rotation was row 2 because it was believed to be independently valuable AND an enabler. It is
neither, YET — it unlocked only 36 of row 1's 352 trip-count refusals (336 → 300). Both halves of
its value are downstream of the same R4 item. The revised order:

1. ✅ row 1 — purity + invariant pure-call hoist (banked, `09403df`)
2. ⏸️ row 2 — rotation: PROVEN, SHIPPED OFF, gated on coalescing (this section)
3. ⬜ **coalescing** — was R4's second item; the measurement has PROMOTED it, because it is now
   the gate on rotation, and §13d named it independently as cause #3 of the hot-loop regressions
   (`mov x0,x1 ; mov x1,x7` in j3's body). It is the one item two other rows are waiting on
4. ✅ IV/SCEV analysis (`pass/scev.rs`, §13g) — shipped unwired, seven batteries
5. ⏸️ **pointer-IV** (`pass/iv.rs`, §13h) — built and proven; re-judged on the distribution and
   GATED OFF (§13i): 1 win / 1 loss / 6 flat, and the winner is paying an isel debt
5b. ✅ **isel addressing-mode fold** (§13j) — EXEC ≥30 ms 1.4610 → **1.3789**, 8 of 8 improve, 0
   regress, sqlite flat. Strictly better than row 5 was on every axis
6. ⬜ final-value, then LFTR — cheap now that the analysis is wired. LFTR is also what would let
   `mycopy` drop its separate counter and test the pointer instead, which is the last instruction
   between that loop and gcc's

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
