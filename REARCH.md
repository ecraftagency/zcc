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
2. **Spilling** (`spill.rs`) — Braun-Hack, per register class. **As built (R2.2, entry sets
   widened at R4.1), with two deviations recorded here rather than left implicit:** SSA
   RECONSTRUCTION is absent because it is not needed. Through R3 that held because a reload's
   register never left the block that created it. Since **R4.1** a reload copy is carried into
   the successors, but only where EVERY predecessor holds that same copy — and a copy has one
   definition, so that condition says every path to the use runs through the definition, which
   IS dominance. The use is dominated by its definition for the same reason as before, with no
   block parameter and no renaming; `mir::verify` re-derives it after every spill in debug
   builds. The second deviation is unchanged: a spilled BLOCK PARAMETER is removed from the IR
   rather than stored at its definition, since its definition is the block head. One slot per SSA WEB (parameter ∪ its arguments), merged
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
| R2.3 inline (+purity), licm (unconditional), iv/pointer-iv/LFTR | ✅ **COMPLETE**. inline + licm + **purity** + **scev** (`pass/scev.rs`, §13g) + **IV widening** (§13l — five instructions against gcc's five on the array-copy loop; subsumes LFTR for that shape, the narrow counter and its test both gone). **pointer-IV BUILT, PROVEN, and OFF** — gate discharged NEGATIVE on the fixed isel baseline (§13i/§13k): A64's free scaled index makes its premise false. Scalar strength-reduction CLOSED as Law-4 category-(a) (§13c). purity (`pass/purity.rs`, 2026-08-25): the interprocedural read-only predicate the pure-call hoist rests on — gcc's `pure`, not `const`, so a caller must also prove memory-clean. Optimistic fixpoint, which is what makes a RECURSIVE read-only callee read-only: "performs a write" is existential over the body. 317 of sqlite's 2,528 functions qualify, so purity is NOT the binding constraint — the loop fences are (§13c residual). licm: EXEC geo40 1.9415 → 1.8374 (−5.4%) for +0.011 INSN and +0.75% sqlite — banked because §13a's directive makes EXEC the target and size the byproduct. inline: the bound is DERIVED, not tuned — a body no larger than the call sequence it replaces (`params + 2`: one instruction to place each argument, the `bl`, one to take the result) cannot grow the program — plus gcc-O1's own `-finline-functions-called-once`. Net EXEC 1.8374 → 1.7468, INSN 1.5357 → 1.5148, sqlite 315,665 → 317,285. A called-once callee must also be DELETED once its last call site is gone, or the rule is a pure size loss: sqlite grew 25% before that existed |
| R2.4 if_convert, rotate/final-value/pure-call hoist (+ sink, added) | ✅ **COMPLETE**. if_convert and sink banked (`pass/ifconv.rs`): a side-effect-free diamond becomes `select`, speculating at most two pure trap-free instructions per arm. Refuses a store, a load, a division whose divisor is not a non-zero literal, and — for now — a FLOAT diamond, since `fcsel` has no MIR form yet. `pass/sink.rs` is licm's dual and was added here rather than planned: §13b ranked register pressure as the largest remaining item, and sinking is the cheapest thing that shortens a live range. **invariant-pure-call hoist ✅ BANKED 2026-08-25** (`licm::hoist_call`, REARCH §13c row 1) — the bucket-emptier. Four fences, each checked: purity (`pass/purity.rs`) · invariant arguments (definitions dominate the preheader) · MEMORY-CLEAN over the whole body (a read-only callee is a function OF memory, so a single store anywhere in the loop breaks the equality — including one AFTER the call, which changes what the NEXT iteration would have read) · GUARANTEED EXECUTION on the first iteration (≥1-trip by evaluating the header test under the preheader's own edge arguments; the call's block dominates every latch AND every other block the loop exits from). Non-termination needed its own argument, since purity does not imply it: the memory-clean fence plus exit-dominance leave only a prior call or a nested loop as a way to diverge ahead of the hoisted call, and both are refused. A FAULT ahead of it needs no fence — a first iteration that faults is UB. **Result: the gcc-ZEROED bucket 6 → 0** (b4/c1/c2/c3/f3/j1, ~371 ms of zcc wall-time), and TWO new asymptotic wins over gcc-O1 in the mirror bucket (e1_recursion gcc 345 ms → zcc ≈0, g2_strlen gcc 161 ms → zcc ≈0). INSN geo40 1.3260 → **1.3043**, median 1.277 → 1.273. EXEC geomean on the COMMON timed set 1.6405 → 1.6232 (n=17, same box, same session) — the pass touches none of those 17 programs, so read that as unchanged, not as a win; the whole effect is in the two buckets and in INSN. sqlite **240,774, byte-for-byte the baseline** (it fires nowhere there — see the residual), compile time +0.5%. **rotate ✅ BANKED 2026-08-25 (shipped OFF, then ON — §13e → §13f)** (`pass/rotate.rs`, gcc's `-ftree-ch`): the square is argued by COUNTING EXECUTIONS — the guard IS the header's first execution — so a copied header need not be pure, it is relocated rather than speculated. It does not pay and §13e says why: rotation makes the back edge critical, the split block is where SSA destruction parks the loop-carried copy, and the branch removed is the branch that copy block adds back (10 instructions per iteration before AND after). Forced on: sqlite +2.7% and +1,732 BRANCHES, geo40 INSN 1.3043 → 1.3972, none of the five §13d loops improved. Quarantined with the gate NAMED — coalesce the loop-carried copy. **THE GATE WAS THEN OPENED (§13f) and rotation is ✅ ON**: `regalloc/color.rs` now frees a dying operand before placing the destination (six lines of `reads_before_writes`, replacing a conservative order that cost one `mov` in every counted loop), and `mir/pass/layout.rs` threads the block that is then empty. EXEC geomean 1.6232 → **1.4276** (≥30 ms only: 1.8423 → **1.4610**), INSN 1.3043 → **1.2437**, sqlite 240,774 → **236,886** with branches 19,151 → **14,711**; h1_popcount now beats gcc-O1 at 0.960, j2_histogram 1.017 and g3_reverse 1.000 reach parity. **final-value CLOSED on measured absence of demand** (§13m: 0 closable loops in sqlite, 0 in geo40). **R2.4 COMPLETE** |
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

**Planned in full at §13n** (2026-08-25, on the zero-pending compiler): four steps — R4.1 SSA
reconstruction after spilling, R4.2 Boissinot coalescing, R4.3 truncation-as-rename, R4.4 re-judge
`csel` — each with its execution site, the prediction to take BEFORE building, its KPI and its gate.
**R4.1 ✅ BANKED** · **R4.2 ✅ BANKED** (§13n): reload copies carried across edges (no SSA
reconstruction needed after all), then the ABI-boundary truncation no-op — sqlite
237,025 → 232,214 → **218,776**, 1.509× → **1.3928×**, full gate green (musl included, for the
first time).
**RE-PLANNED 2026-08-25 (user)** after the R4.2 prediction inverted the row's premise: the excess was
re-decomposed by ROOT CAUSE on the R4.1 compiler and is now twelve rows R4.1–R4.12 in §13n, each with
its evidence, site, the prediction to take first and its KPI. The two largest are the ABI family the
first plan had no row for: truncation self-moves at ABI boundaries (12,570 insns, 16.7% of the gap,
every one a no-op under AAPCS64 §6.4.2) and a colourer rule that forbids a parallel-copy destination
from taking its own dying source (2.22 movs per call vs gcc 1.00).
**RE-PLANNED AGAIN 2026-08-25 (user, "re-plan") on the hot-loop inspection**, which measured the exec
side of §13n for the first time: R4.2 REOPENED for its FPR twin (1,558 insns), R4.6/R4.7 amended,
**R4.13** (the IV family, R2's residual) and **R4.14** (three orphans) opened, and the ORDER changed
from size-weighted to measured-programs-owned.
**R4.2's FPR half ✅ BANKED 2026-08-25** (`fmov` windmill 783 → 4 pairs, −1,616 insns, 1.3928× →
**1.3826×**; residual 4 = fundamental double-swaps, exhausted; full gate green).
**§13o + R4.8 ⚠️ (2026-08-26)** — the excess re-decomposed on this compiler, and
R4.8 REFUTED by it: frame slot-touches are 35,550 against gcc's 34,931 (+1.8%),
so the spiller is at parity on VOLUME and the whole +5,044 frame excess is
PAIRING DENSITY (39.6% of touches paired against gcc's 65.6%). The pairing half
shipped — spills-first frame layout and a window that looks past disjoint frame
accesses — for sqlite **186,262 = 1.1858×**. R4.15 (below) then made the frame
adjust a real MIR instruction and folded it into the first save pair, ≈3,300
instructions — the largest named card, now banked.
**R4.6 ✅ + R4.10 ✅ (2026-08-26)** — constants value-numbered (not copied, and
not across a call) and the copy-partner graph followed transitively: sqlite
**186,705 = 1.1886×**, EXEC unchanged at ≈1.05, INSN 1.0677.
**R4.15 ✅ + R4.12 ✅ (2026-08-26)** — R4 CLOSED. The frame adjust is now an
ordinary `SpAdj` MIR instruction (the "emit invents two instructions" wart gone,
`cost = |MIR|` exact) that `frame_fold` fuses into the first save pair as a
pre-index `stp x19,x20,[sp,#-N]!` and the last restore as a post-index `ldp …,
[sp],#N` (DDI 0487 C6.2.130). Guards: ordinary frame (`!dyn_stack`, `outgoing=0`
so offset 0 is free — `frame` places the callee-saves there under that
condition), in the writeback's POSITIVE reach (pair N≤504, single N≤255 — the
post-index end binds, was the `ldr x30,[sp],#256` reject), and the offset-0 save
present at the prologue head / epilogue tail (the reloads commute, so it is moved
to the tail to free last). sqlite **186,705 → 183,253 = 1.1886× → 1.1667×**
(−3,452), geo40 **INSN 1.0677 → 1.0272**, EXEC **1.0576 → 1.0513** (0 DIVERGE) —
a per-function −2 across the whole suite. Commuting square `frame_fold_folds_the_
adjust_into_the_save_pair` (effect: writeback present, no `SpAdj`, pre-index
leads; fallback: big frame + VLA keep `SpAdj`/printed adjust) + `frame_fold_
preserves_meaning`; full gate 15/15, provenance non-vacuous.
**R4.12** (`csel` re-judged): paired A/B `ZCC_NOPASS=ifconv`, ON = EXEC 1.0576 /
INSN 1.0677 vs OFF 1.0732 / 1.0694 — ON wins both axes, same distribution.
**KEEP as-is.**

**R4.16 ✅ (2026-08-26) — region-resident spill (the allocator road to 1× re-opened).**
R4 is NOT closed: 1× is the goal and sqlite stood at 1.1667×. Re-decomposed the
excess (§13p): **ldr+str = 15,025 of the 26,179 gap (57%), and 11,532 of that is
FRAME (spill) traffic** — `sqlite3VdbeExec` reloads the `Vdbe *p` parameter
([sp,#96]) **116 times, stored once, never modified, while x28 sits UNUSED**;
gcc-O1 keeps it in a callee-saved register (0 frame ops in 6,041 insns). The
allocator's own passes UNDER-utilize the register file — the user's diagnosis,
confirmed. `regalloc/promote.rs` puts such a value back: a spill slot with one
store that DOMINATES its ≥3 reloads, when a callee-saved register is wholly free
in the function, is bound to that register (store → `mov r,src`, reloads → reads
of `r`, added to `saved`). Local copy-propagation deletes a reload whose
destination dies in-block; a FIXED (ABI-argument) use keeps its `mov` and is
never propagated into the call — the miscompile the guard fixes (torture
20180921-1 segfault). sqlite **182,956 = 1.1648×** (−297 static), but the true
win is **−2,146 memory loads** ([sp,#96] 116→1) — an EXEC/cache win geo40 CANNOT
see (nothing in the suite spills, §19), so geo40 stays EXEC 1.049 / INSN 1.027.
Square `promote_moves_a_spilled_value_out_of_memory` (same_all meaning + hand-built
effect: reloads→0, callee-saved copy, saved updated); full gate 15/15.
**STILL NOT 1×.** The dominant remaining class is spill VOLUME — VdbeExec stores
2,810 frame slots to gcc's 0. `promote` only rescues values a FREE register can
hold; matching gcc needs the spiller to keep values register-resident across the
switch fan-out (lift the block-local residency truncation, §7.2) — the next lever,
still O1, still R4.
**R4.17 ✅ (2026-08-26) — the allocator-splitting restructure: live-range
splitting with SSA reconstruction, banked (§13p has the full table).** R4.16
named the lever precisely — the block-local residency truncation — and this
row lifts it, big-bang, per the committed design/plan (Braun & Hack 2009 +
Braun 2013 reconstruction). Four mechanisms land together, each proof-carrying
in `regalloc/tests.rs::same()`: (1) a **generalized cross-edge carry**
(`generalized_carry_cuts_switch_reloads`) inserts a block-parameter `P_V`
wherever a spilled value is register-resident at SOME (not necessarily all)
predecessors — a register edge-arg where held, a minted reload on the edge
otherwise, R4.1's dominance-only carry now the special case where every arg is
the same register; (2) the same reconstruction reaches **loop headers**
(`loop_header_carry_keeps_the_accumulator_in_a_register`), seeded from the
prior round's latch-exit residency; (3) `Sim::More` eviction becomes
**regional, not whole-web** (`eviction_splits_regionally_not_whole_web`) — a
value leaves the working set only in the pressure region that forced it out,
re-entering a register at its next use, instead of the whole SSA web going to
memory for its entire life; (4) Braun §2.3 **trivial-parameter elimination**
(a parameter every edge reaches with the same reaching definition IS that
definition) runs to a fixpoint before cold-edge reloads are minted, and the
dead-parameter sweep is a worklist, O(phis×preds), not a re-sweep
(`reconstruction_is_pruned_and_pressure_is_counted`,
`the_carry_budget_reaches_a_doubly_nested_header` — the loop-nesting-depth+1
carry budget reaches a doubly-nested header, both levels get a parameter). A
**prediction instrument** (`ceiling_report`'s new `split`/`web-split`
columns) was built and measured BEFORE Task 5: ≤2,084 of 11,520 reloads
removable, 4,370 of 4,549 spilled values register-resident somewhere — the
whole-web model wrong for 96% of them — against an independent pass's claim
(from fixtures with no block boundaries, which structurally cannot show the
effect) that the effect was unmeasurable; the instrument's own prediction was
refuted-in-favor: actual yield 2,052 reloads removed, 1.5% off 2,084.
sqlite frame `ldr`+`str` **21,991 → 22,208 (interim, carry landed ahead of the
split that pays for it — 2,684 phis fired, all-preds and loop-header both) →
21,048** (gcc 12,721). sqlite total instructions **182,956 = 1.1648× →
181,609 = 1.1562×** (−1,347 vs pre-restructure). `mov` 37,828 → 37,689 (139
BELOW baseline — the 607 uncoalesced phi edge copies the carry added are
fully paid off by pruning). Compile time 11 s → **10.1 s** (faster, not
slower — the worklist and 1,883 fewer parameters). geo40 **EXEC 1.0523 (18
progs ≥30 ms, noisy) / INSN 1.0272 (all 35, deterministic, bit-identical to
R4.15/R4.16 and to an independent mid-session read at 1.0272/1.0540)** — the
phi machinery never fires on those 35 small programs (none spill under
pressure), so speed is **UNCHANGED**; the ULTIMATUM's speed axis (~1.05×)
stands undisturbed. Full gate 15/15 green: cargo 171/0, provenance PASS (58
modules / 64 constants / 23 passes, every new test non-vacuous), determinism
88×8, torture 0 FAIL, opt-parity 1552/0 DIVERGE, csmith300 254/0 DIVERGE/
TIMEOUT, yarpgen300 300/0 DIVERGE / 0 TIMEOUT / 0 CTIMEOUT, musl PASS.
**Judge this row against the spec's stated floor, ~1.10×, not 1.0×**: of the
25,882-instruction gap only the spill-traffic front (9,270) was in scope —
reg-reg `mov`/coalescing (6,813), constant materialization (4,861) and misc
(~4,800) are enabled-not-done by this restructure (perfect elimination of the
whole spill front alone gives 182,956−9,270 = 173,686 = 1.106×), so 1.0× was
never reachable by construction here; this row makes it reachable by opening
the other three fronts' headroom.
**Scope correction against the spec §1 motivating example, proven in code by
two independent readers, not a failed row**: `sqlite3VdbeExec`'s `[sp,#600]`
stays at **243 stores** (227 pre-restructure, unmoved by this row).
`evict_params` evicts every spilled block parameter unconditionally, and phi
candidacy requires `spilled[v] AND has_def[v]`; a loop-carried accumulator's
ONLY definition IS its header parameter, so spilling it strips `has_def`
permanently — header phis therefore carry only loop-INVARIANT values, never
accumulators. **[sp,#600] is unreachable by this lever by construction.**
Named next lever: regional split of the PARAMETER at the terminator (keep the
parameter, spill the argument on the edge) — §4.3 applied to a block
parameter instead of an instruction definition.
**Residual, Law-4 classified** (§13p has the table): (i) `[sp,#600]` above —
convenience truncation, lever named; (ii) `some-preds` 1,284 reloads
untouched (1,266 at the interim checkpoint) — all yield came from
`all-preds` (817→184), the cold-edge depth fence refuses the rest,
convenience truncation, needs a real profitability model not a depth test;
(iii) frame `str` ROSE 148 over the pre-restructure baseline while `ldr` fell
1,091 — net good (−943 frame ops overall) but stores are the larger half of
the gcc gap and are drifting the wrong way; (iv) uncoalesced phi edge-copies
— `mov` is net-positive (−139) but the underlying gap remains, `destruct`
emits one parallel copy per edge and only biased colouring removes any;
`THEORY.md` A7 names Boissinot value-based merging as the upgrade, gated on
exactly this measured residual; (v) the trivial-phi elimination fixpoint's
near-linearity is asserted, not proven — PARKED on the gate's 0 TIMEOUT / 0
CTIMEOUT over 600 fuzzer programs as empirical evidence, follow-up is to
bound the round count or convert it to a worklist keyed on newly-aliased
phis' consumers; (vi) a phi-insertion **cost fence was built, measured (513
frame loads traded for ~790 `mov`s) and REFUSED under Law 0** — purity over a
number, the refusal itself is evidence the ordering was honoured.
**Pre-existing defect found and fixed**: `regalloc::verify` obligation (b)
inherited its "already stored" set from the immediate dominator (sound but
incomplete — it can only see a store that DOMINATES the reload) and
false-alarmed on the `evict_params` shape, where every incoming edge stores
and none of the dominators do; replaced with the forward MUST dataflow
(`in[b] = ∩_preds out[p]`, iterated to the greatest fixed point) the
obligation always meant, which strictly subsumes the old dominance check
(anything dominance could prove, the MUST analysis proves too). Two
independent readers reproduced the A/B showing the false alarm PRE-DATES this
restructure — it fails identically with the split forced off. `mir::verify`
untouched.
Commits `9dc8455`..`650e521` (Tasks 1–6: fixpoint round-cap, `insert_phi`,
generalized carry, loop-header carry, prediction instrument, regional split,
prune + pressure + near-linear fixpoint). **STILL NOT 1×** — the three
enabled-not-done fronts above are the next lever, and `[sp,#600]` needs the
terminator-level split named above.
**R4.3 ✅ + R4.4 ✅ (2026-08-25)** — a parallel-copy destination takes its own
dying source, and one epilogue per shape instead of one per return path (plus
dropping frame slots nothing names): sqlite **189,279 = 1.2050×**, EXEC geomean
**1.0357** (median 1.000, only 3 programs above 1.1×), INSN **1.0690**. Both
rows were filed "size only, the suite cannot see it" and both moved it: e2
1.500 → **1.000**, f2 1.200 → **1.000**. **Next ⬜ is R4.6.**
**R4.11 ✅ + R4.14 ⚠️ (2026-08-25)** — rotation over EVERY loop exit (its
residual print refuted §13n's guess and named the real reason), and one of
R4.14's three orphans: **EXEC geomean 1.0777, median 1.001, only 5 programs
above 1.1×**; d3 **1.969 → 0.969** and j5 **1.940 → 1.002**; sqlite 201,727 =
**1.2842×**. R4.14 (3) REFUTED by A/B (16% INSN for 7% EXEC) and reverted;
R4.14 (2) measured and left open — the case count is not the variable.
**Next ⬜ is R4.3**, and the five remaining exec losers are now owned by the
SIZE rows (e2→R4.3, h2→R4.4, f2→R4.6), so the two axes have converged.
**R4.5 + R4.9 ✅ BANKED together (2026-08-25)** — booleans stay flags (cfg
threading identities (e)/(f)) and memory crosses one edge (`mem.rs` seeded from
a single predecessor): sqlite **199,979 = 1.2731×**, EXEC geomean **1.1490**,
median **1.000**, j5 **2.857 → 1.940**. **Next ⬜ is R4.11** (rotation), which
owns d3 (2.000×, now pure rotation) and closes j5's remaining count gap.
**R4.7 ✅ BANKED (2026-08-25)** — the eight §17 rows verified one by one:
sqlite **212,066 = 1.3501×**, EXEC geomean **1.3386 → 1.2044** (median 1.225 →
1.073), and the plan's first CYCLE prediction validated to the third decimal
(j3 1.940 → **1.000** at identical instruction count). **R4.13 ⚠️ DISCHARGED by
its own residual print** — two of its three shapes are category (a) on this
target, the third's bug half banked under R4.7 (d2 2.111 → 1.500). R4.5 was
put first because j5 (2.857×, 81% of the suite's wall time) is its program, and
it delivered.
Fourteen rows now; §13n holds the table, the evidence and the order.
Measured worklist: `ldr`+`str` are 34% of the excess and `mov` 24%, so **58% of the gap is one
subsystem, the allocator**; the loop rows have reached everything they can.

### R5 — the O2 headroom stack (§16)
User principle (2026-08-24): "to reach 1× we must stack enough technique to reach 0.5×, and keep 0.5×
as headroom" — O1 parity must be reached with margin, not asymptotically. R5 pulls the §16 shelf until
the paired scoreboard sits at ≤ 1.0 on BOTH axes with the distribution flat. Items marked ★ in §16 are
cheap enough to ship inside R2/R3.

**R5 IS FOR BROADENING SPEED — NOT for driving sqlite to 1× (user, 2026-08-27).** The 2026-08-27
localization (`MEASURED M21`) settled that sqlite's residual gap is register RESIDENCY — keep p/pOp/pC
resident across the mispredicting VdbeExec dispatch — and that chasing it to 1× is a **grind trap**, for
four measured reasons: (a) the ceiling is ~2–4% (one reload = +0.9% on the realprog geomean, full
residency est. +2–4%), so it lands at ~1.12× and stops, ~12% short of 1×; (b) the win sits at the
realprog run-to-run **noise floor** (±1%), so levers past the first few cannot be told from noise;
(c) any live-range-splitting change alters what EVERY function spills (the §5 `c04804` class), so each
step needs a full 10k AWS seal — grind-war iteration economics; (d) it may hit the §4b wall — chordal-SSA
in dominance order cannot revisit, so true global recoloring is a DIFFERENT allocator, a REARCH decision
not a row. A hand-edit proved residency is real and correct; it is banked as a Tier-2 row below, NOT the
R5 goal. **The R5 goal is broad sub-1× margin across the spectrum, ranked by broad-speed ÷ effort:**

**Tier 1 — cheap, broad, ship early (the R5 opening hand, in order). ALL FIVE ROWS ARE IMPLEMENTED
(2026-08-27), each behind its own default-OFF toggle, each squared, none measured on a machine yet.**
- ✅ **R5.1 = ★4** static branch prediction → block weights → **layout + spill-weighting**. Three
  commits: `acc572c` computes the weights (`freq::annotate` — the field had nine writers and no
  readers), `f576f57` lays the heavy successor out as the fall-through (and deletes the dead
  `(rpo_num, depth)` sort that could never affect the order), `d5e0b49` scales the spiller's Belady
  distance by the frequency of the block where the reload would be paid. Toggle `ZCC_WEIGHTS`.
- ✅ **R5.2 = ★1** TBAA → **load-elim / DSE**. `8d08396` stamps the C99 6.5p7 class at the access and
  teaches `mem.rs::disjoint` to answer on types before addresses; `713e6d3` honours the GNU opt-outs
  the BOX found (`may_alias`, `optimize("-fno-strict-aliasing")`, and the driver flag). LICM does not
  hoist loads, so the row's reach is `mem.rs`, not the three passes the plan named. Toggle `ZCC_TBAA`.
- ✅ **R5.3 = #13** **SLP-SIMD**, the straight-line half. `f08aa3a`, and NOT where the plan put it:
  built as a MIR pass with one new instruction (`MInst::VAlu`) rather than a HIR pass with `Ty::V128`,
  because `Width::Q`/`MemOp::Q`/the FPR class already carry the whole vector data path (§14 decision
  ㉚). One shape — two adjacent `double` pairs, one op, two adjacent stores → four instructions — with
  an alias oracle over `Adrp`/`SlotAddr` origins, because the merge moves a store past loads. Toggle
  `ZCC_SLP`.
- ✅ **R5.4 = #9** **BB list-scheduling**, `4d5af69` + the fix in `f08aa3a`. Dependence DAG (RAW/WAR/WAW
  + memory + barriers from `MInst::effect()`), list-scheduled by longest latency-weighted path from
  `MEASURED M10`. Runs BEFORE frame lowering: the first cut ran after it and the box answered with
  corpus-wide segmentation faults, because an epilogue's sp-restoring load is a memory READ and two
  reads are unordered. Toggle `ZCC_SCHED`.
- ✅ **R5.5 = ★2** VRP + branch-folding + division narrowing, `6e1e862`. Intervals with Cousot
  widening, guards inherited down the dominator tree from single-predecessor edges, folding a decided
  comparison and reducing `x / 2^k` / `x % 2^k` on a proven non-negative dividend. Toggle `ZCC_VRP`.

**TIER 1 IS MEASURED, AND IT DOES NOT PAY (2026-08-28).** Paired INSN+EXEC over the 42-program
taxonomy suite, one toggle at a time, same session, same machine. Seal gate green first (15 PASS / 0
RED at FUZZ_N=1000 with all five on), so these are numbers about speed and not about correctness.

| toggle | EXEC | INSN | verdict |
|---|---|---|---|
| baseline | 1.0206 | 1.0719 | — |
| `ZCC_WEIGHTS` (R5.1) | 1.0873 | 1.0880 | **QUARANTINED** — the whole batch regression is this row |
| ⤷ `ZCC_WEIGHTS_LAYOUT` | 1.0925 → 1.0712 after two fixes | 1.0774 | still worse than baseline; stays OFF |
| ⤷ `ZCC_WEIGHTS_SPILL` | 1.0199 | 1.0818 | EXEC-neutral, costs instructions (`k2_live_pressure` 1.468 → 1.621); stays OFF |
| `ZCC_TBAA` (R5.2) | 1.0276 | 1.0718 | flat on this suite — measure on sqlite before judging |
| `ZCC_VRP` (R5.5) | 1.0204 | **1.0673** | INSN −0.46%, the only positive; at the noise floor |
| `ZCC_SCHED` (R5.4) | **1.0152** | 1.0718 | EXEC −0.5%, at the noise floor |
| `ZCC_SLP` (R5.3) | 1.0213 | 1.0719 | no pack fires on this suite at all |

⚠️ **The absolute geomean here is NOT the rc5 instrument.** `rc5` recorded geo40 EXEC 0.9494 on the
box; this baseline reads 1.0206 on a laptop under Docker. Only the A/B deltas within the session are
trustworthy — which is exactly what Law 3c says about reading one number.

**THE ROWS ARE ADDITIVE, NOT CLASHING.** Sum of the individual INSN deltas predicts 1.0832; measured
all-on is 1.0839. Phase-ordering interference is not what is wrong here — one row is.

**WHAT R5.1 TAUGHT, and it is worth more than the row.** Two defects were found by reading the emitted
code rather than the pass: `freq` scored a loop's EXIT edge at even odds (a loop that runs `TRIPS`
times leaves ONCE), and `chain_by_weight` broke ties toward the block reverse postorder visits first,
making a character-dispatch chain pay a taken branch per comparison. Both are fixed in `b878483`;
neither made the row pay. The weights themselves are now trustworthy — which matters, because the use
they were originally justified by is still unbuilt: **gating `rotate`/`licm`/`iv`, which add 7,728
instructions to sqlite for no measurable speed (`MEASURED M17`) because they fire on cold loops.** That
is the R5.1 consumer worth building, and it is not layout.

**WHAT TIER 1 STILL OWES.** A measurement on sqlite and the real-program set, where TBAA and the
spiller have something to bite on that 42 kernels do not. Until then every row stays default-OFF, which
is the only honest state for a row whose whole claim is speed. The gate itself was nearly a liar here:
`fullsuite.sh` forwarded only `ZCC_IN_BOX` before `4d5af69`, so a toggled run tested the untoggled
compiler and reported green.

**Tier 2 — medium, broad.**
- **R5.6 = #15** remat + live-range-splitting refinements (**the residency lever — now one broad row for
  every register-pressured program, NOT the sqlite-1× target**; the proven pOp hand-edit is its seed,
  `MEASURED M21`). Bounded scope only; a full allocator revisit is out of R5 (§4b).
- **R5.7 = ★3** tail-call · **R5.8 = #5** PRE/LCM · **R5.9 = #12** unroll/peel/unswitch (code-size watch).

**Tier 3 — big radius / grind / research (defer until Tiers 1–2 are banked and the surface is widened).**
- #6 GCM+GVN · #7 IPCP/SRA/ICF · #8 inlining policy · #10 store-merge/strlen · #11 switch-conv/cross-jump
  · #16 superopt/e-graph · #14 alignment (size-only, track don't copy).

**Widen the surface in parallel (Law 3c standing order).** The 35 kernels + sqlite are <10% of the
spectrum; a broad-speed claim needs 100–200 real programs. Rank Tier-1 rows by their effect on the WIDE
suite, not on sqlite.

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

### Zero-pending accounting — REACHED 2026-08-25
**R0, R1, R2.1, R2.2, R2.3, R2.4 and all of R3 are ✅. R2 and R3 have ZERO PENDING ROWS.**

How the six loop rows closed, each by measurement rather than by fiat:
| row | outcome |
|---|---|
| invariant pure-call hoist | ✅ shipped — gcc-ZEROED bucket 6 → 0 (§13c row 1) |
| rotation | ✅ shipped ON, after §13f fixed the two things below it that were cancelling it |
| iv/scev | ✅ shipped — `pass/scev.rs`, and **IV widening** reaches instruction-for-instruction parity with gcc-O1 on the array-copy loop (§13l) |
| LFTR | ✅ subsumed by widening for the shape that matters: the narrow counter and its test are both gone |
| pointer-IV | ⛔ built, proven, and OFF — gate discharged NEGATIVE (§13k). A64's free scaled index makes the premise false |
| scalar strength-reduction | ⛔ Law-4 category-(a): an out-of-order core pipelines `mul` at ≈`add` cost (§13c) |
| final-value | ⛔ closed on MEASURED absence of demand — 0 closable loops in sqlite, 0 in geo40 (§13m) |

Two rows were closed by turning them OFF and one by not building it, and that is the point: three of
the seven outcomes are negative, every one of them backed by a number, and none of them was decided
by how the transform looked on paper.

### Where the compiler stands at zero-pending
geo40 EXEC **1.3654** (over 18, median 1.269, worst j5 2.856) · INSN **1.2410** (over 35, median
1.217) · sqlite **237,025 = 1.51× gcc-O1** · gcc-ZEROED bucket EMPTY · zcc-ZEROED bucket holds
e1_recursion (gcc 335 ms → ≈0) and g2_strlen (gcc 156 ms → ≈0). Session arc: EXEC 1.6232 → 1.3654,
INSN 1.3043 → 1.2410. Against `main` measured through the same harness (1.6642 EXEC / 1.6191 INSN),
this branch is ~20% ahead on exec and ~24% on instruction count.

The R4 band (≤1.3× / ≤1.2) is still not met, and §13b already says where the remaining mass is:
spill traffic and copies, not loops.

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

## §13k ROW 5 RE-MEASURED on the fixed baseline — the gate is discharged, NEGATIVE. It stays off.

§13i gated pointer-IV with one condition: fix the addressing-mode fold, then re-measure. §13j fixed
it. The re-measurement (`ZCC_IV=1` against the same tree) settles the row:

| | iv OFF | iv ON |
|---|---|---|
| EXEC ≥30 ms | **1.3789** | 1.4087 |
| EXEC all timed | **1.3654** | 1.3690 |
| INSN (35) | **1.2419** | 1.2454 |
| sqlite | **237,026** | 238,302 (+1,276) |
| g1_memcpy_loop | 47 ms | 47 ms — **the win is gone; isel already had it** |
| j2_histogram | 59 ms | 67 ms — **the loss remains** |

**0 win / 1 loss / 7 flat.** Worse on every axis. The row's premise — that rebuilding an address
from a counter is expensive — is simply FALSE on A64, where the scaled-index form makes it free.
What was left after §13j is the post-index form alone, and j2 is the standing counter-example:
identical instruction count, 13% slower, because the writeback is not free either.

So `ENABLED = false` stands, and the gate is now marked DISCHARGED rather than open. Re-opening it
needs something this branch does not have: a cost model that can say WHEN a post-index pays, which
is a cycle-level question the `-S` harness cannot answer. Until then the pass is a proven theorem
with no profitable instance, kept for its batteries — which encode two real ISA facts (loads-only
post-index, and the free scaled index) — and for `scev`, whose next consumer is item 6.

The three-line summary of rows 5 / 5b, because it is the most transferable thing in this file:
**a transform that looks like a win can be a lower layer's bug wearing a costume.** Measure the
distribution, not the geomean; and when one program carries the whole number, go find out why.

---

## §13l IV WIDENING — instruction-for-instruction parity with gcc -O1 on the array-copy loop

After §13j, `mycopy`'s inner loop was six instructions against gcc's five, and the extra one was a
`sxtw`: every iteration, widening a 32-bit counter so it can index memory. gcc runs the counter in
64 bits and sign-extends the BOUND once, outside the loop. `pass/iv.rs::widen` does the same, and
the fact that licences it was already proven — `scev::find_nowrap` shows the loop's own exit test
keeps the counter inside its type, so `sext(i)` is the identity and a 64-bit counter takes the same
values.

```
    zcc   ldrb w2,[x4,x1] ; strb w2,[x3,x1] ; add x1,x1,#1 ; cmp x1,x0 ; b.lt
    gcc   ldrb w3,[x1,x2] ; strb w3,[x0,x2] ; add x2,x2,1  ; cmp x4,x2 ; bne
```

Five instructions each, one for one. The narrow counter is deleted BY CONSTRUCTION rather than left
to `dce`, because what remains of it is a CYCLE — the parameter feeds its own step and the step
feeds the parameter back along the edge — so a use-count sweep sees both as live and the loop keeps
a second counter for nothing. (That is how the first cut was caught: the loop stayed at six, the
`add` landing exactly where the `sxtw` had been.)

### Measured: EXEC-NEUTRAL, INSN flat, and the parity is structural
Same box, same session, `ZCC_NOPASS=widen` for the other arm, best of nine:

| program | widen ON | widen OFF |
|---|---|---|
| g1_memcpy_loop | 48 ms | 47 ms |
| j3_prefix_sum · h1_popcount · h2_revbits · h3_fnv · j2_histogram · d3_early_exit | identical | identical |

geo40 EXEC **1.3654 → 1.3670**, INSN **1.2419 → 1.2410**, sqlite 237,026 → **237,025**.

So the honest statement is: **this buys instruction parity, not time.** The `sxtw` it removes was
already free — it hides in the load shadow, which is the same fact OPT.md #18 recorded on `main`
when a similar rewrite measured exec-neutral. It ships ON because it is correct, gated green, and
strictly better on the INSN axis THE ULTIMATUM also names; it does not ship as a speed win, and a
cross-run comparison that appeared to show ±3% was machine drift, caught by re-measuring both arms
back to back.

### The defect, and the distinction that caused it
sqlite failed to compile: `%79 used in bb18 but defined in bb6`. The bound is sign-extended into the
ENTRY block, so it must be defined OUTSIDE the loop — and the check asked `AddRec::is_invariant()`,
which only says the recurrence has STEP ZERO. That is also true of values computed INSIDE the loop:
the sum of two invariants has step zero and is defined in the header. `scev` now offers
`is_loop_invariant` for the question a code-motion caller actually has, with both spelled out at the
definition so the next caller does not repeat it.

A second hole, found earlier the same way: when the exit test reads the counter AFTER the step, the
value it reads is the one that LATCH produced — so a test outside a latch has no such value to name.
The first cut silently skipped the rewrite in that case and deleted the counter anyway, which is
what `use of undefined` meant on 100 csmith programs. Refused now before anything is mutated.

Both are the same lesson as §13j's pair: **a predicate that is ALMOST the one you want is worse than
an absent one.** `is_invariant` vs `is_loop_invariant`; "the add folds" vs "the index peels".

---

## §13m FINAL-VALUE — closed on MEASURED absence of demand. R2 and R3 have zero pending.

The last open row. gcc -O1 does perform it (`for (i=0;i<100;i++) s+=i;` returns a literal 4950 there
and runs an emptied countdown), so the question was never whether the transform is real — it was
whether this corpus contains one.

`iv::fv_opportunity` (`ZCC_FVDBG=1`) counts loops a final-value pass could close: every value the
body defines has an affine evolution, so the whole loop reduces to a closed form. The answer:

| corpus | closable loops |
|---|---|
| sqlite (2,046 functions) | **0** |
| geo40 (35 programs) | **0** |

**The oracle was validated before the verdict was believed, and it needed to be.** Its first cut
reported 0 everywhere INCLUDING on a hand-written known-positive, because it demanded that the
loop's own exit COMPARE be affine — control, not data, and never affine. Believing that run would
have closed this row on a lie. It now finds `s += 1` against a symbolic bound and correctly misses
`s += i`, which is quadratic and outside the affine fragment `scev` claims.

So the row closes under Article A — **no feature before a real `.c` demands it** — with the demand
measured rather than assumed, and the measurement reproducible. Two things make the absence
unsurprising in hindsight: row 1's invariant pure-call hoist already emptied the gcc-ZEROED bucket,
which was the asymptotic gap final-value was meant to finish; and a WEAKER form that closes only the
accumulator while the loop keeps running would save one `add` per loop, not an order of growth.

**R2 and R3 now have zero pending rows.**

---

## §13n R4 PLAN — measured worklist and the steps, each with its execution / measurement / gate (re-planned 2026-08-25)

Re-taken on the zero-pending compiler (`10fc699`), because §13b's histogram predates rotation,
coalescing, block threading and the isel fold — and this session twice showed that acting on a stale
baseline aims at the wrong layer.

### The excess, today. sqlite zcc **237,025** vs gcc-O1 **157,074** = **1.509×**, excess **79,951**
> Taken on the pre-R4.1 compiler and kept as the plan's premise. Where a row has since banked, its
> own section below carries the re-taken numbers: after R4.2 (both halves) the module is **217,160**
> (**1.3826×**, excess **60,086**) — GPR half 218,776, then the FPR twin −1,616. `mov` is **40,237**
> against gcc's 33,608, so the `mov` row's excess is now **+6,629**, not +19,147, and rows (b)/(i) are
> aimed at what is left of it.
| class | zcc | gcc | excess | share |
|---|---|---|---|---|
| `mov` | 52,755 | 33,608 | **+19,147** | 24% |
| `ldr` | 41,322 | 22,332 | **+18,990** | 24% |
| `str` | 17,569 | 9,462 | **+8,107** | 10% |
| `csel` | 4,498 | 542 | +3,956 | 5% — JUDGEMENT, not excess: zcc if-converts more, and the exec number says yes |
| `cmp` | 10,200 | 6,973 | +3,227 | 4% |
| `sub` · `sxtw` · `cset` | | | +2,376 | 3% |

`ldr` + `str` = **+27,097, 34% of all excess**. `mov` = 24%. **Together 58%, and both are the
allocator.** Everything the loop rows could reach is now reached; what is left is one subsystem.

Two decompositions that make the steps concrete:
- **spill traffic** — frame `[sp]` accesses: `ldr` **22,421** + `str` **12,298** + `ldp`/`stp` 9,899.
  So over half of all `ldr` is a RELOAD.
- **copies** — of 52,755 `mov`: `x,x` **27,425** · `w,w` **16,870** · `,zr` 8,302 · other 158. The
  `w,w` half is where the TRUNCATION movs live (a 64-bit value narrowed then read wide again — the
  width-typed vreg cannot say "same register, narrower name").

### The steps — RE-PLANNED 2026-08-25 (user: "re-plan R4 carefully, we are still far from 1×")

The first plan had four steps aimed at "the allocator". R4.1 banked and the R4.2 prediction inverted
its premise (below), so the whole excess was re-decomposed by ROOT CAUSE rather than by mnemonic,
on the R4.1 compiler (`bb7ae52`), sqlite **232,214** vs gcc-O1 **157,074**, gap **75,140**, and the
35-program exec suite read program by program. Every number here is from the `.s` or from a
read-only instrument (`ZCC_SPILLCEIL`, `ZCC_MOVKIND`, `ZCC_COALESCE`), never reasoned.

**What the re-decomposition found, in one table.** The gap is BROAD, not concentrated: the top 10
functions hold 27.9% of it, the top 80 only 53.9%, and the bulk of gcc's instruction mass sits at
1.3–1.5×. So it is a per-function tax paid by several independent mechanisms, and no single lever
reaches 1×. Ranked by measured size × certainty ÷ cost:

| # | root cause | evidence on sqlite | site | exec programs it owns |
|---|---|---|---|---|
| a | **ABI-boundary truncation self-moves** — a 64-bit name copied into a fixed argument/return register at 32 bits, or a call result read back narrow. `dst == src` makes the windmill see a cycle and route it through x16/v31 | `mov x16,xN ; mov wN,w16` **4,208 pairs, all before a `bl`** (8,416 insns) + `mov wN,wN` 4,906: 3,462 after a call, 692 before `ret`, 720 other. **12,570 insns = 16.7% of the gap**, every one a no-op: AAPCS64 §6.4.2/§6.8.2 leave the bits above an argument's or result's type width UNSPECIFIED, so the reader reads `wN` and the truncation has no observer | `regalloc/destruct.rs` `nop()` + `isel` call-result width | f2 (`fmov d31,d0; fmov d0,d31`, the FP twin), every `bl` in the suite |
| b | **Parallel-copy destinations may not take their own dying source's register.** `color.rs` frees dying operands only AFTER an instruction's definitions are placed — correct for an ordinary instruction, and deliberately extended to `ParallelCopy` "at the cost of one register". The real cost is one `mov` per pair: a destination may not take ANOTHER pair's dying source, but taking ITS OWN is exactly the copy disappearing | every function copies its arguments out of x0–x7 at entry (`mov x3,x0; mov w4,w1; mov w5,w2`, ≈3 per function, ≈5.9k); **2.22 register movs per call vs gcc 1.00** (27,501 before 12,378 `bl` vs 11,949 before 11,901); ABI copies 36,911 of the 55,338 surviving | `regalloc/color.rs` walk (the "Free colours only AFTER" rule), `live.rs` is already right | e2 (8 entry movs in `mix`), e3, every call site |
| c | **Constants materialized per use.** HIR carries constants as `Operand::Imm`, not values, so isel mints a `MovImm` at every use that does not fold; nothing shares them across blocks. NOT the spiller: only 891 of 12,370 reloads are rematerializations (7.2%) | `movz/movn` 14,393 of which **9,035 repeat** an immediate already materialized in the same function; `mov ,zr` 8,282 of which **7,050 repeat**. zcc 22,675 immediates vs gcc ≈12,720: **+9,955** | new MIR pass `const_share` (dominator-scoped GVN of `MovImm`/`Adrp`; the spiller's remat already handles pressure) | f2 (1024.0 re-built every iteration, gcc loads it once), j5 (`movz #1; cmp; movz #1`) |
| d | **Booleans materialized, then branched.** `&&`/`\|\|` in a condition become a VALUE (`select`), then `br(value)`; and `select c,1,0` is emitted as `movz #1 ; csel` instead of `cset` | pure-boolean `csel w,w,wzr` **3,707** (each with its `movz #1`); `csel/cset → cbnz` 669 vs gcc 9; `cbnz` 10,878 vs 7,066 while `b.cond` 5,463 vs 6,822 and `ccmp` 0 vs 621 — zcc branches on bools, gcc on flags | `hir/pass/cfg.rs` thread `br(select c,k1,k2)` → `br c`; `isel` `cset`/`csinc`/`csneg` rows; `ccmp` chains (§17) | **j5 2.87×** (`cmp; movz; csel; cbnz` per iteration vs `cmp; ble`), f5-shape loops, d4 (`csneg`) |
| e | **Returns not merged, frames not folded, dead slots kept.** Every return path duplicates the epilogue; every function carries a frame even when every local was promoted; `sub sp` / `add sp` are separate instructions where gcc folds them into `stp x29,x30,[sp,-N]!` / `ldp …,[sp],N` | extra `ret` per function **1,928 vs gcc 317**; `add sp,sp` 3,807; **289 leaf functions with a frame and zero `[sp]` references** (promoted allocas keep their slot: `f1` has 4 locals → `sub sp,#32`); 1,879 of 1,892 functions adjust sp vs 1,616 of 1,883 for gcc | HIR `unify_ret` (single exit block, one parameter); `mir/pass/frame.rs` drop unreferenced slots + pre/post-index fold of the adjust into the first/last save pair | every leaf in the suite (`isort`, `find`, `prefix`, `revbits`: 2 insns each) |
| f | **§17's ✔ marks are claims, not measurements.** Rows marked "exhausted" are measurably not firing | `ldrsh` **0 vs 492**, `ldrsb` 0 vs 111, `ldrsw` 29 vs 329, standalone `sxth` 687 vs 90, `uxtb/uxth` 516 vs 0 (a `w`-form load already zero-extends); `tst` 0 vs 489 (`cmpelim` fuses only when the `cmp` is the NEXT instruction, and only `and`+`cmp #0`); `fmov #imm8` never emitted (`isa::fp_imm8` is dead code — the build warns); `csneg/csinc` absent; shifted-operand fold not commuted (`lsl w1; orr w1,w1,w3` where gcc `orr w0,w3,w0,lsl 1`); `cmp` with a constant LEFT operand materializes it (`movz w0,#1; cmp w0,w3`); `tbz` 326 vs 1,721; `sbfiz` 0 vs 477 | `isel/lower.rs` munch table, `mir/pass/cmpelim.rs`, `ext_lattice` | **j3 1.92×** (`add x1,x1,w5,sxtw` on the loop-carried chain: 2-cycle latency where `ldrsw` + `add` is 1 — identical instruction COUNT, double the time), **f2 1.8×** (`movz; fmov d,x` GPR→FPR transfer per constant), h2 1.28×, d4 |
| g | **Spill traffic, what R4.1 left.** Residency restarts at every loop header because the latch is unsimulated when the header is walked; spill stores sit at the definition whether or not any path reloads | frame `ldr` 17,052 vs 7,765 (**+9,287**), frame `str` 12,239 vs 4,956 (**+7,283**) = 22%; 12,370 reloads of which **7,607 (61.5%) inside loops**; dom-ceiling 3,485; `sqlite3VdbeExec` alone 15,484 vs 6,041 (2.56×) | `regalloc/spill.rs` entry-set fixpoint across back edges; spill-store placement at the eviction frontier rather than the definition | the suite cannot see this row at all (no function spills); sqlite only |
| h | **Redundant loads across blocks.** HIR GVN numbers pure expressions only; a load repeated in a dominated block with no intervening store is loaded again | non-frame `ldr` 18,964 vs 14,567 (**+4,397**); j5 loads `p[j]` in the condition and again in the body | `hir/pass/gvn.rs` memory-aware value numbering (dominator walk, kill on store/call by alias class) — gcc's FRE | **j5** (second load on the hot path) |
| i | **Edge-copy coalescing** (the old R4.2, now measured) | 16,778 param/arg pairs: 11,848 already one colour, **4,782 FREE**, 148 BOUND | `regalloc/color.rs` Boissinot merge on the FREE residual | — |
| j | **Loops that refuse rotation, and a store that stays in the loop.** A header carrying ANY C label is refused (`rotate.rs:129` tests `labels`, where only ADDRESS-TAKEN labels pin a block — `pinned()` already knows the difference); the early-`return`-in-body shape is refused for a reason not yet traced (the pass prints no residual — a Law-4 gap in itself); a loop-invariant global read+written every iteration is not promoted to a register | d3/`f1` not rotated (2 branches per iteration), d4/`f4` not rotated; i1 `ldr x5,[x0]; add; str x4,[x0]` per iteration where gcc keeps `gsum` in x2 | `hir/pass/rotate.rs` (labels → `pin`, residual print, then the early-return shape); LICM store motion (`-ftree-loop-im`, an O1 feature) | **d3 1.97×**, d4 1.40×, i1 1.30× |
| k | `csel` re-judge | 4,498 vs 542 — re-ask AFTER (d), which removes the boolean `csel`s | `hir/pass/ifconv.rs` | — |

**Arithmetic, stated honestly.** Predicted ceilings, lower…upper: (a) 12,570 certain · (b) 8k…20k
(measure first: pairs whose source dies at the copy and whose colours differ) · (c) 6k…12k · (d)
5k…7k · (e) 4k…6k · (f) 3k…5k · (g) 4k…8k · (h) 3k…4.4k · (i) 2k…4.8k · (j)/(k) exec rows, size
small. Lower bounds sum to ≈47k of the 75,140 gap; upper bounds to ≈84k — but the rows OVERLAP (an
x16 pair is also a "mov before `bl`"; a `movz #1` is both a repeated constant and a boolean), so the
sum is not a forecast. The forecast is made one row at a time, on the model, before each build, and
the histogram is re-taken after each bank. What the table does establish: **the gap is explained
by named mechanisms with named sites, none of them fundamental**, and the suite's every loss above
1.3× has a row that owns it. The user's R5 principle (reach 1× with 0.5× of headroom) still needs the
§16 shelf on top of this; R4 alone is not expected to reach 1× on sqlite with margin.

**The two axes come apart here, and the plan says so.** The 35-program suite has no function that
spills, so rows (g) and (i) are invisible to it, and R4.1 moved its geomean by exactly nothing. The
suite's losses are (d), (f), (h), (j) — isel, control-flow and memory shapes — and j3 is the proof
that instruction count and time are different quantities on this machine. Both are reported after
every row, paired, in one session, as a distribution.

| # | step | execution | prediction to take FIRST | KPI | status |
|---|---|---|---|---|---|
| **R4.1** | reload copies carried across edges | `regalloc/spill.rs` | ceiling band [3,425 , 9,849] | frame `ldr` ≪ 22,421 | ✅ 232,214, frame `ldr` 17,052 |
| **R4.2** | **ABI-boundary truncation is a no-op** (a). A truncating copy whose destination is a fixed argument register at a call, or whose source is a fixed result register after one, or the return register before `ret`, is dropped: the reader reads the declared width. Cite AAPCS64 §6.4.2 in the code; the same rule for `fmov` with v31. The `mir::verify` width rule must still hold, so the rule lives in `destruct::nop` where the physical register's reader is known, or the `Copy` is never emitted by isel | `regalloc/destruct.rs`, `isel` call result width | the 4,208 + 3,462 + 692 counts ARE the prediction: −12,570 | `mov x16` before `bl` = 0; `mov wN,wN` after `bl` = 0; sqlite ≤ 220k; **`fmov v31` pairs = 0** | ✅ **BANKED (both halves).** GPR half 218,776; **FPR twin** `fmov d31` windmill **783 → 4 pairs** (−779 = **−1,558 `fmov`**), sqlite **218,776 → 217,160** (−1,616), **1.3928× → 1.3826×**. Residual 4 = genuine `d8↔d9` swaps (real 2-cycles, scratch mandatory) → category (a) fundamental, **exhausted**. See "R4.2 IS NOT EXHAUSTED" below |
| **R4.3** | **A parallel-copy destination takes its own dying source** (b). Amend the "free AFTER" rule: at a `ParallelCopy`, a destination may take the register of the source of ITS OWN pair when that source dies here; the simultaneity argument forbids only the other pairs' sources. `color::check` already asserts no two live values share a colour, so the proof obligation is met by the existing checker | `regalloc/color.rs` walk | count pairs `(d ← s)` with `s` dying at the copy and `colour(d) ≠ colour(s)` — that number is the ceiling; report it split entry / call / other | movs before `bl` per call → ≈1.0 (gcc 1.00); entry copies ≈ 0 | ⬜ |
| **R4.4** | **returns merged, dead slots dropped, frame adjust folded** (e) | HIR `unify_ret`; `mir/pass/frame.rs` | extra `ret` 1,928 → ≤ 400; 289 leaf frames → 0; `sub sp` count → ≈ number of functions with no save pair | sqlite −4k…−6k | ⬜ |
| **R4.5** | **booleans stay flags** (d): `br(select c,k1,k2)` → `br c` threading in `cfg.rs`; `cset`/`csinc`/`csneg` rows; then `ccmp` for `&&`/`\|\|` chains | `hir/pass/cfg.rs`, `isel/lower.rs` | pure-boolean `csel` 3,707 and `csel→cbnz` 669 are the ceiling | `csel` ≈ gcc's 542 + real if-conversions; **j5 exec** | ✅ **BANKED with R4.9** (cfg identities (e) threading-a-known-condition + (f) branch-on-select-of-literals; the `cset`/`csinc`/`csneg` rows shipped early under R4.7). **Residual: `ccmp` 0 vs 612 — category (b), needs a `CCmp` MIR instruction; named, unshipped.** See "R4.5 + R4.9" below |
| **R4.6** | **constants shared** (c): dominator-scoped `MovImm`/`Adrp` sharing on MIR, before spilling; the spiller's remat decides pressure. **Amended 2026-08-25:** the inspection found the INTEGER loop bound rebuilt every iteration too (`movz w0,#2304; movk w0,#61,lsl 16` inside f2's and e2's loops, where gcc holds it in a callee-saved register), not only f2's FP constant — a loop-invariant immediate must be hoisted to the preheader, and the row's ceiling counts those | new `mir/pass/const_share.rs` | repeats 16,085 minus what R4.5 removes = the ceiling; realistic target zcc immediates → ≈ gcc's 12.7k; **plus** count of `MovImm`/`Adrp` inside a loop whose value is loop-invariant | `movz+movn+mov,zr` ≤ 14k; **f2, e2 exec** | ⬜ |
| **R4.7** | **§17 verified, row by row** (f): extending loads (`ldrsb/ldrsh/ldrsw`, and prefer the extension in the LOAD over the ALU operand when the value feeds a loop-carried chain — the j3 latency fact, recorded as a cost-model caveat since `cost = \|MIR\|` cannot see it), `cmpelim` across non-flag-writing instructions and `and`+`cmp` → `tst`, `fmov #imm8`, `csneg/csinc`, shifted-operand commute, constant-LHS `cmp` commute, `tbz` for sign/bit tests, `sbfiz`. Each row: its count vs gcc BEFORE, its battery, its count AFTER — the ✔ becomes a number | `isel/lower.rs`, `cmpelim.rs`, `ext_lattice`, **`mir/isa.rs` latency table (Side II — MEASURED, see "THE MISSING DUAL")** | the per-mnemonic table above, **and the first CYCLE prediction the plan has ever carried**: j3's loop-carried chain is `add xN,xN,wM,sxtw` (2 cyc) → `ldrsw`+`add` (1 cyc), bound 2.0 → 1.0; measured today 1.940, so the row predicts **j3 ≈ 1.0×** and, on the same table, **i1 and e2's `sxtw` chains**. **Amended 2026-08-25:** the KPI below WAS a size KPI ("±10% per mnemonic"); the inspection showed this row's value is on the clock and predictable before the build | **j3 exec 1.94 → ≤ 1.1** (the cycle prediction, validated against the clock); each mnemonic within ±10% of gcc; **i1, e2, h2, d4, f2 exec** — 5 of the 10 programs above 1.2× | ✅ **BANKED.** j3 **1.940 → 1.000** (the cycle prediction, to the third decimal), d4 **1.400 → 1.000**, i1 **1.333 → 0.750**, d2 **2.111 → 1.500**; sqlite **217,160 → 212,066** (1.3826× → **1.3501×**); EXEC geomean **1.3386 → 1.2044**, median **1.225 → 1.073**. Eight rows, each with a square AND a count. Residual: `sbfiz`/`ubfiz`/`fmov #imm8`/`mul`-by-constant = category (b), named; `tst` = category (a). See "R4.7 — BANKED" below |
| **R4.8** | **spill, second pass** (g): entry-set fixpoint across back edges (carry into loop headers); spill-store placement at the eviction frontier | `regalloc/spill.rs` | `ZCC_SPILLCEIL` in-loop 7,607 and dom-ceiling 3,485 are the reload ceiling; for stores, count spilled definitions with a path that never reloads | frame `ldr` ≪ 17,052, frame `str` ≪ 12,239 | ⬜ |
| **R4.9** | **memory-aware GVN** (h) — FRE | `hir/pass/gvn.rs` + alias classes from `effects()` | count loads whose address was loaded in a dominating block with no clobbering store/call between — measured before a line is written, like R4.1 | non-frame `ldr` → ≈ gcc's 14,567; **j5 exec** | ✅ **BANKED with R4.5.** Shipped in `mem.rs`, not `gvn.rs`: `mem.rs` already had the alias oracle, and what it lacked was a block big enough to see across. A block whose ONLY predecessor is P is seeded with P's exit table — sound with no dataflow. **Residual: the fully general FRE over arbitrary control flow still needs a memory SSA — category (b), §12** |
| **R4.10** | **Boissinot merge** on the FREE residual (i) | `regalloc/color.rs` | 4,782 | edge copies ≪ 9,332 | ⬜ |
| **R4.11** | **rotation residual + store motion** (j): `rotate.rs` tests `pin`, not `labels`; add the refusal-reason residual print; trace and lift the early-return shape; LICM store motion for a loop-invariant address with no aliasing access | `hir/pass/rotate.rs`, `hir/pass/licm.rs` | residual print first — the count of refused loops per reason IS the prediction | **d3, d4, i1 exec**; sqlite branch count | ⬜ |
| **R4.12** | **`csel` re-judged** (k) after R4.5 | `hir/pass/ifconv.rs` | paired A/B with `ZCC_NOPASS=ifconv`, ≥30 ms subset | keep or narrow, on the number | ✅ **KEEP.** ON = EXEC 1.0576 / INSN 1.0677; OFF = 1.0732 / 1.0694 — ON wins both axes, identical distribution (median 1.000, worst d2 1.556). No program regresses; no narrowing warranted |
| **R4.15** | **the frame adjust becomes an ordinary MIR instruction** (§13o), which `frame_fold` fuses into the first save pair (pre-index `stp …,[sp,#-N]!`) and the last restore (post-index `ldp …,[sp],#N`), DDI 0487 C6.2.130 | new `MInst::SpAdj` + `AddrMode::FrameWb`; `mir/pass/frame_fold.rs`; `frame` places callee-saves at offset 0; `emit` stops inventing the adjust for an ordinary frame | 1,588 `sub sp` + 1,726 `add sp` = ≈3,300 the pre/post-index forms fold for free | sqlite ≪ 186,705; the standalone adjust gone on every ordinary frame | ✅ **BANKED.** sqlite **183,253 = 1.1667×** (−3,452); geo40 INSN 1.0677 → **1.0272**, EXEC 1.0576 → **1.0513**, 0 DIVERGE. Guards: `!dyn_stack`, `outgoing=0`, pair N≤504 / single N≤255 (post-index end binds — the `ldr x30,[sp],#256` reject), offset-0 save at prologue head / epilogue tail. Square `frame_fold_folds_the_adjust_into_the_save_pair` + `_preserves_meaning`; full gate 15/15 |
| **R4.13** | **the IV family — R2's exhaustion residual** (l, opened 2026-08-25 on the hot-loop inspection). Three shapes SCEV does not fire on, all one theorem over `scev.rs`/`iv.rs`: (1) a **pointer / 64-bit IV** where the source has a 32-bit counter — zcc recomputes `[xB, wI, sxtw #2]` every iteration, gcc walks a pointer with post-index writeback (`str w3, [x2], -4`; j5, d3); (2) a **count-down IV** whose decrement SETS THE FLAGS so the exit test is free (`subs w1,w1,#1; bpl` — j5, h2), where zcc keeps a separate `cmp`/`tbz`; (3) **strength reduction of `i*j+k` in a nested loop** to an add-IV (d2: `madd` every inner iteration vs gcc's `add`). Every one is `-O1` (`-fauto-inc-dec`, `-ftree-slsr`, IV canonicalization). NOT folded into R4.11 — that row owns rotation refusals and store motion, and Article G forbids blurring a theorem seam. §13n missed this family because it was decomposed from a STATIC sqlite histogram, where an IV shape costs zero instructions | `hir/pass/scev.rs`, `hir/pass/iv.rs`; the post-index fold is `isel/lower.rs` | **residual print FIRST**, as R4.11 requires of rotation: for every loop, is there an IV `scev` recognizes but `iv` refuses to widen/rewrite, and WHY — the count per refusal reason IS the prediction. Then per shape: (1) loops with an address `[base, w, sxtw]` on a recognized IV; (2) counted loops whose exit compare is against 0 or a hoistable bound; (3) inner-loop `mul`/`madd` on two IVs | **j5 exec** (11 → ~7 insns/iter), **d3, d2, h2 exec**; each shape's residual classified (a)/(b) | ⚠️ **RESIDUAL TAKEN → 2 of 3 shapes REFUTED on this target.** (1) pointer/64-bit IV: `ZCC_IV=1` re-measured post-R4.7 is still NEGATIVE (INSN 1.1493 → 1.1538, EXEC 1.2044 → 1.2140) — **category (a)**, the pass stays off. **RE-ENTRY TRIGGER: the time-dual cost model shipping** — that is §13k's own named gate ("re-opening needs a cost model that can say WHEN a writeback pays"), and R4.7's validated j3 cycle prediction is the argument that may open it. Re-entry is one command, no code. (2) count-down IV: zcc's `j>=0` is ALREADY one `tbz`, so `sub`+`tbz` = gcc's `subs`+`bpl` — **category (a)**, nothing to win; the general form shipped as R4.7's `cmp_elim` window. (3) `i*j+k`: the BUG half (cross-block `madd` undoing LICM) banked under R4.7, d2 **2.111 → 1.500**; the add-IV/exit-rewrite half is the ONE **category (b)** left, owning one program. **RE-ENTRY TRIGGER: after R4.5 → R4.9 → R4.11, re-measure d2** — it also carries R4.10's edge copies in the loop nest, so what is left of its gap by then may not be the add-IV at all. **The row is NOT ✅: a (b) residual means Law 4 is not satisfied, only that nothing downstream waits on it.** **AMENDED 2026-08-26 (§13q) — shape (1) was RE-OPENED and half of it BANKED.** The category-(a) verdict was over-broad: it was taken on a UNIT-STRIDE address, the only case A64's scaled index reaches for free. A ROW-STRIDED address (`B[k][j]`, step 1920) has no addressing mode and is rebuilt with a MULTIPLY every iteration; walking a pointer replaces that with an `add` at the SAME instruction count. `matmul` **1.638× → 1.000×**, hand-validated before a line was written (MEASURED M9). Two defects fixed: the default-off gate moved down into `strengthen` so it covers the unit-stride half alone, and `iv::affine` now splits an address with TWO invariant symbolic terms around one recurrence, which `scev::AddRec`'s single base could not hold. The unit-stride half stays category (a) behind `ZCC_IV`, its re-entry trigger unchanged. Residual print shipped: `ZCC_IVDBG=1`. See "R4.13" below and §13q |
| **R4.18** | **the TIME dual of the cost model** (§13q; the row "THE MISSING DUAL" reserved and gated). `cost = |MIR|` is exact for SIZE by construction and blind to TIME by the same construction — matmul moved 1.638× → 1.000× with the instruction count UNCHANGED at seven. Two independent validations now exist for the premise, which is the condition §13 set for opening this row: R4.7's j3 cycle prediction (2.0 → 1.0 predicted, 1.940 → 1.000 measured, 3% error) and §13q's matmul. Shape: a latency/pipe column in `mir/isa.rs` (Side II — MEASURED, no vendor guide exists for this core), a `mir/cost.rs` that scores a loop by its CRITICAL RECURRENCE rather than its length, and the square `time_model ≡ cycles(interp)` proven per loop over the corpus, exactly as `cost ≡ len∘codegen` is proven per function. Then a transform's Δcycles is predicted BEFORE any build, the way Δinsn already is | `mir/isa.rs`, new `mir/cost.rs`, `mir/interp.rs` | re-predict the two validated cases FIRST — the model must reproduce j3's 2.0 → 1.0 and matmul's `madd`-vs-`add` gap from the table alone, with no clock; a model that cannot re-derive what is already measured is not shipped | every remaining program above 1.1× EXEC that sits at INSN parity — the set the size model provably cannot see; and the RE-ENTRY of M2's unit-stride half, whose own gate is "a cost model that can say WHEN a writeback pays" | ✅ **BANKED.** Latencies MEASURED (`tests/bench/latency.sh`, `MEASURED M10`) rather than transcribed — dependent chains self-calibrated against a plain `add`, so the clock cancels; `nop` control at 0.12. **The ship condition was met before the model shipped**: from the table alone, `loops.c` 4/3 = **1.333×** against a measured 1.365 (2.3%), j3 **2.00×** against 1.940 (3%), matmul addr **3 → 0**. `Bound` carries TWO axes because matmul forced it — `madd`'s accumulator forwards in **1** cycle, not 3, so the accumulator recurrence is 1 EITHER WAY and a recurrence-only model reproduces `cost=|MIR|`'s exact blindness; the address bound separates them. Correctness note recorded: the first cut seeded every header parameter at zero in ONE distance array and reported 16 cyc/iter for a loop that runs in ~5 — a recurrence is a CYCLE, so it needs one longest-path pass per loop-carried value. **The worklist it produced**: h2_revbits recurrence **2** (actionable, the next lever) · j4_binsearch recurrence **9** = category-(a) floor, its 0.917 INSN is irrelevant · j5/g1/j2 recurrence 1 = not latency-bound at all · **d1_switch recurrence 1, which is why all four of its latency-flavoured hand-edits failed — the model would have said so first** · e2 UNSCORED (call) |
| **R4.14** | **three orphans, one row so they stay tracked** (m, opened 2026-08-25): (1) **`x / 2^k` → `x · 2^−k`** — exact under IEEE 754 since the reciprocal of a power of two is representable, so the commuting square is an identity on every input including ±0/∞/NaN (f2: `fdiv` 10+ cyc → `fmul` 3); (2) **small dense `switch` → compare tree, not a jump table** — R3.3's density constant ("≥4 cases, ≥½ span") is exactly Article E's "the spec's number or my convenience's number?": gcc-O1 builds a `cmp`/`tbnz`/`csel`/`csinc` tree for d1's 8 cases and wins 1.33× on it, so the constant is re-judged against a measured crossover, not cited; (3) **inline a called-once function that is not `static`** — `inline.rs` requires `is_static` for the called-once rule; gcc's `-finline-functions-called-once` does not, and keeps the out-of-line body (e2: `mix` marshals 10 arguments per call). Three sites, three proofs, each a few lines; grouped only so none is lost | `hir/pass/fold.rs`; `isel/lower.rs::jump_table` (the policy constant); `hir/pass/inline.rs` | (1) count `fdiv` by a constant power of two on sqlite and the suite; (2) measure the crossover: compare-tree vs table exec at 4, 6, 8, 12, 16 cases, on the clock; (3) count non-`static` functions with exactly one call site in the module | **f2, d1, e2 exec**; each with its own square | ⬜ |

### Order, and why — RE-PLANNED 2026-08-25 (user), on the hot-loop inspection

The first order was **certainty × size ÷ cost** — a SIZE-weighted criterion, chosen before any
exec prediction had been taken. After R4.2 the axes stand at INSN **1.179** and EXEC **1.336**:
size is ahead, time is behind, and R4.3 — the row that order put next — is one §13n itself says
"the suite cannot see at all". The inspection (below) measured the exec side for the first time and
found that R4.7 owns 5 of the 10 programs above 1.2×, with a cycle prediction already validated to
3% on j3; a whole theorem family (now R4.13) owned 4 and had no row; and R4.2 was closed with its
FPR half unmeasured. So the criterion becomes **measured programs owned × certainty ÷ cost**, and the
order is:

```
R4.2 ✅ → R4.7 ✅ → R4.13 ⚠️ → R4.5 ✅ → R4.9 ✅ → R4.11 ✅ → R4.14 ⚠️(1 of 3)
        → R4.3 ✅ → R4.4 ✅ → R4.6 ✅ → R4.10 ✅ → R4.8 ⚠️(refuted; pairing half shipped)
        → R4.15 ✅ (frame adjust an `SpAdj`, folded into save pair, −3,452) → R4.12 ✅ (ifconv: KEEP)
```

**AMENDED 2026-08-25, on R4.13's own residual print — the spine is edited IN
PLACE, not renumbered.** R4.13 was placed second because it "touches four (j5,
d3, d2, h2) and is where j5 is decided". Its residual, taken first as the row
itself requires, refuted that: two of its three shapes are category (a) on this
target and the third owns d2 alone. What actually decides j5 — 2.857× and **81%
of the suite's absolute wall time** — is the `cmp; movz; csel; cbnz` per
iteration that R4.5 owns, with R4.9's repeated `p[j]` load behind it. So **R4.5
moves to the front**, R4.9 follows it (both are j5's, and R4.9's prediction
should be taken on the loop as R4.5 leaves it), then R4.11 (d3 2.000× is now
PURE rotation — its IV half was the refuted shape). Nothing was renumbered and
no row was invented: only the order changed, on a measurement.

**Why each sits where it does.** R4.2's FPR half is not a re-plan item at all — it is an unfinished
row under Law 4, 1,558 instructions on a theorem already proven, and the R4.1 precedent says a
follow-up belongs to the row that owns the theorem. R4.7 is first because it is the only row with a
quantitative exec prediction and it touches five of the ten (j3, i1, e2, h2, d4); its Side-II half —
the measured latency table — is also the cheap validation of the time-dual premise, so it is done
here before any infrastructure is bet on it. R4.13 touches four (j5, d3, d2, h2) and is where j5 —
81% of the suite's absolute wall time — is decided; it follows R4.7 because both rewrite what isel
sees and the table should be in place first. R4.11 touches three (d3, d4, i1). R4.14 is three small
proofs that ride cheaply once the rows around them have moved. R4.5 and R4.9 are j5's remaining
mechanisms and are taken once R4.13 has reshaped that loop, so their predictions are made on the
loop as it will be, not as it is. R4.6 then R4.3/R4.4 are the size rows the suite cannot see;
R4.8/R4.10 the allocator's second pass; R4.12 a measurement.

**What does not change.** One commit per row, the full gate each, the histogram re-taken each,
**both axes reported paired in one session as a distribution**, `.s` confirms and never discovers.
A row whose measured prediction comes in under 20% of its stated ceiling gets one push, then the
quarantine mark and the next row — the no-pivot contract binds. **Every exec-bearing row now takes
its prediction on the inspection table first**, the way the size rows take theirs on an instrument.

**The time dual is NOT a row yet, by decision.** "THE MISSING DUAL" below records the shape
(`mir/cost.rs` + an interpreter scoreboard + the square `time_model ≡ cycles(interp)`). It is opened
as a row only if R4.7's latency table — its cheap half — proves the premise on j3, i1, e2. If the
table alone closes j3 from 1.94× to ≈1.0×, that is the argument for the rest; if it does not, the
premise was wrong and nothing was built on it.

### R4.1 — BANKED. sqlite **237,025 → 232,214**, 1.509× → **1.478×**

**The ceiling was measured first, and it is the reason the step is small.** `ZCC_SPILLCEIL=1`
(read-only, in `spill.rs`) reported on the pre-change compiler: 18,764 planned reloads in 189
functions, of which **9,849 (52.5%)** had the same value already reloaded in a STRICTLY DOMINATING
block — the loose bound, which ignores whether the register file can hold the copy that far — and
**3,425 (18.3%)** were still resident at the exit of EVERY predecessor, the bound that a
block-boundary reconciliation can actually reach. The prediction was therefore a band,
`[3,425 , 9,849]` fewer reloads, not a number. **Measured: −5,288 frame `ldr`, net −4,811
instructions** — inside the band, above its floor.

**No SSA reconstruction was built, and none is needed.** §13n named Braun 2013 reconstruction as the
execution, and the measurement made it unnecessary. A reload copy is carried into a block only where
EVERY predecessor is holding that same copy; a copy has exactly one definition; so the condition says
every path from the entry to the use runs through that definition — which is dominance. The use is
dominated by its definition for the same reason it was when the copy could not leave its own block,
so there is no φ, no block parameter and no renaming. The §14 row is amended rather than discharged.

| | base | R4.1 | gcc-O1 |
|---|---|---|---|
| total | 237,025 | **232,214** | 157,074 |
| frame `ldr` | 22,340 | **17,052** | 7,765 |
| frame `str` | 12,065 | 12,239 | 4,956 |
| `mov` | 52,755 | 53,518 | 33,608 |

Excess 79,951 → 75,140: **6.0% of the gap closed**. `str` and `mov` moved the wrong way by 174 and
763 — longer residencies are more to copy — which is R4.2's subject, not a regression to chase here.

**Gate** — cargo 142/0 · sci-gate shape/cpp/decay/alg/abi PASS · cases/ext/torture/cts PASS ·
opt-parity **1552 PARITY / 0 DIVERGE** · csmith300 **254/0** · yarpgen300 **300/0** · determinism
**56 programs × 8 fresh processes**. `musl` is RED and was **RED on `10fc699` too** — pre-existing,
re-checked on the parent commit before this step was allowed to proceed.

**The verifier check that caution #2 demands.** The invariant "a reload copy is used only in the
block that made it" is exactly what this step removes, so `spill_with` now runs
`mir::verify::verify` — one definition per vreg, every use dominated by it — after every spill in
debug builds. `apply` also had to mint every copy's register BEFORE rewriting any block: block INDEX
order is not dominance order, so a use could otherwise read a register not yet assigned.

**EXEC: unchanged, and that is the honest reading.** Paired in one session, geo40 INSN geomean
**1.2410 both sides, identical**; EXEC 1.3634 → 1.3567, inside noise. The 35-program taxonomy suite
has no function under enough pressure to spill, so R4.1 cannot fire there at all. The win is in large
real functions — `sqlite3VdbeExec` alone planned 2,330 reloads — and sqlite is the only corpus in the
harness that contains them. Caution #1 said to expect exec and insn to come apart; here the suite
simply cannot see the axis.

**Compile time: −11% and kept.** sqlite compile 10.46s → 11.57s, all of it in `spill`
(4.93s → 6.09s). Measured, it is NOT extra rounds (2,087 → 2,117 simulate calls, +1.4%) and NOT the
predecessor-intersection scan (replacing it with a sorted binary search moved nothing). It is that
the working sets now genuinely hold more live values, and every per-instruction scan in `simulate` is
O(|W|). That is the price of keeping values in registers rather than a defect. Making those scans
incremental is a pure refactor under Article G and is not this commit.

**LAW-4 RESIDUAL (measured, `ZCC_SPILLCEIL=1` on the shipped compiler).** 18,764 → **12,370** planned
reloads. Of what remains: dom-ceiling 9,849 → **3,485**, all-preds 3,425 → **1,120**, and **61.5% sit
inside loops**. The residual is dominated by ONE named category-(b) truncation, not by fundamentals:
blocks are walked in reverse postorder, so a loop header's latch has not been simulated when the
header is, and nothing is carried across a back edge — residency restarts every iteration. Lifting it
needs a fixpoint over the loop. **R4.1 is therefore NOT exhausted**, and the follow-up belongs in this
row rather than in a new one.

### R4.2 — BANKED. sqlite **232,214 → 218,776**, 1.478× → **1.3928×**

**The prediction was re-taken on this compiler before a line was written** and it
reproduced §13n's counts exactly: 4,208 `mov x16,xN ; mov wN,w16` pairs (8,416
instructions) and 4,906 `mov wN,wN`, **13,322 no-ops, 5.7% of the module**.

**Where they came from.** `apply_colors` already deleted the copies biased colouring
turned into self-moves — but only where BOTH ends were virtual, so every ABI copy, whose
one end is a physical register by construction, was invisible to it. The x16 pairs are
the second-order cost of that blindness: a narrow identity pair left inside a
`ParallelCopy` reads to the windmill as a one-element CYCLE, which it dutifully breaks
through the scratch register — two instructions where the right answer is none.

The candidate set is now every copy whose two ends land on the same physical register,
and the drop question splits by who can answer it:

| shape | who answers | rule |
|---|---|---|
| `V ← P` (call result read back, incoming parameter) | `max_read`, already here | no reader of the destination looks past `w` |
| `P ← V`, `P ← P` (argument setup, return value) | `abi_reader`, new | the register is an argument of the very next `Call` (§6.4.2), or nothing else in the block mentions it and the block RETURNS (§6.8.2) |

Everything else is refused — `Asm` in particular, whose template chooses its own operand
width and may name `%x0` for an `int` operand. Doing it BEFORE `destruct` is what makes
the second question answerable at all: every `ParallelCopy` in the function is still
isel's ABI marshalling, since SSA destruction has not yet created an edge copy.

**The one that miscompiled, recorded because the error is the interesting part.** The
first version also read an edge ARGUMENT at the width of the parameter it is copied
into. That is true only while the edge copy SURVIVES: when argument and parameter share
a colour the copy is deleted, and the argument then IS the parameter and inherits every
reader it has. yarpgen **s0188** is exactly that shape — `mov w1,w1` narrowing a value
the successor stores with `str x1` — and it was the ONE diverge in an otherwise 300/0
run, on the `-O0` side, which is why only a full gate could find it. The parameter's
width stays as the floor; the parameter's READERS now propagate back to the argument
through the same fixpoint that already handled a deleted copy. Cost of the correction:
11 instructions.

| | R4.1 | R4.2 | gcc-O1 |
|---|---|---|---|
| total | 232,214 | **218,776** | 157,074 |
| `mov` | 53,518 | **40,237** | 33,608 |
| every other mnemonic | | identical | |

Excess 75,140 → **61,702**: **17.9% of the gap closed**, above the row's own 12,570
prediction. `ldr`/`str`/`csel`/`cmp` are unchanged to the instruction, which is what a
pure copy-removal row must look like.

**EXEC and INSN come apart, exactly as caution #1 said.** Paired in one session:
INSN geomean **1.2410 → 1.1786** (median 1.217 → 1.173, programs above 1.1× 26 → 20);
EXEC 1.3691 → 1.3603, median 1.309 on both sides — inside noise. A register-rename
no-op costs a modern core nothing to retire, so this row moves the deterministic axis
and not the clock. j5's insn ratio crosses below gcc (1.000 → 0.970).

**LAW-4 RESIDUAL** (`ZCC_R42RES=1`, read-only, on the shipped compiler): **41 of 13,322
survive, 0.31%** — **35 category (a)** (a reader genuinely looks past `w`, so the
truncation has an observer and the instruction is doing work), **6 category (b)** (an
`MInst` form `max_read` does not list, charging its operand a full 8-byte read),
**0** refused for want of an ABI reader. The row is exhausted but for six instructions
behind two unlisted instruction forms.

**Gate** — cargo 144/0 over 30 runs · sci-gate shape/cpp/decay/alg/abi PASS ·
cases/ext/torture/cts PASS · **musl PASS** · opt-parity **1552 PARITY / 0 DIVERGE** ·
csmith300 **254/0** · yarpgen300 **300/0** · determinism **85 programs × 8 fresh
processes**.

**THE COMPILER STATE IS NOW WHOLLY GREEN, and that took five defects of its own**
(commit `f99ca66`, banked underneath this row because the R4.2 gate could not be read
until they were gone). `musl` had been RED since before R4.1; it is PASS for the first
time. Side I: `parser::cond_expr` implemented two of C99 6.5.15p6's rows and not the
rest, so a null pointer constant against a pointer either narrowed the pointer to `int`
or left the constant an `int` (musl `getpass`, `return l < 0 ? 0 : password;`);
`mir::interp`'s `Pair` arm passed 16 bytes to a `u64` accessor for a `q`-form
`ldp`/`stp`; `hir::interp::f64_to_f128` reassociated `exp - 1023 + 16383` into an
unsigned underflow for every value below 1.0. Side II: `pinned_symbols` did not pin an
`__attribute__((alias))` TARGET, so the inliner deleted musl's `static void
dummy(void){}` behind `weak_alias(dummy,_init)`; a weak EXTERN object emitted no
`.weak`, making the reference to `_DYNAMIC` a strong undefined one. The measurement
exception, once: `testutil::frontend` named its temp file after the source hash "so
concurrent threads cannot collide", which guarantees they do when two batteries quote
the same program — `fs::write` truncates first, so a reader preprocessed half a file.
That was the whole of the ~20% battery flake; it is write-private-then-rename now, and
30 consecutive runs are green.

### The prediction that forced the re-plan (taken as "R4.2" under the first plan; its findings are rows R4.2, R4.3 and R4.10 above)

§13n's R4.2 row says the prediction to take first is "classify the 44,295 reg-reg movs into edge
copies / call-argument setup / truncation; only the first is Boissinot's". Taken (`ZCC_MOVKIND=1` in
`destruct.rs`, `ZCC_COALESCE=1` in `regalloc/mod.rs`, both read-only), the classification says the
row was aimed at the smallest of the three.

Of **55,338** copies surviving to the emitter on sqlite:

| kind | count | share | whose problem |
|---|---|---|---|
| **ABI** — call arguments, returns, fixed operands | **36,911** | **66.7%** | argument TARGETING — no row exists |
| EDGE — SSA destruction | 9,332 | 16.9% | R4.2, the coalescer |
| NARROW — truncation | 7,527 | 13.6% | R4.3 |
| WIDE — standalone 64-bit | 1,514 | 2.7% | mixed |
| FP | 54 | 0.1% | — |

And of the **16,778** parameter/argument pairs the coalescer could act on, biased colouring **already
merges 11,848 (70.6%)**; **4,782 (28.5%)** are FREE — different colours although the argument dies on
the edge, so a merge was available and greedy colouring missed it — and 148 are BOUND, genuinely
coexisting names that no coalescer removes. **R4.2's ceiling is therefore ~4,782 copies, not the
+19,910 the row's KPI names.**

**The first version of this measurement was WRONG and is recorded because the error is instructive.**
It counted every `ParallelCopy` as an edge copy and reported 46,243 — 2.8× the truth. `isel` emits the
SAME instruction for ABI marshalling, so at the emitter all of them are the letters `mov` and the two
are indistinguishable unless the count is taken where each is still structurally what it is. Acting on
that number would have aimed a coalescer at a category it cannot touch.

**Independent confirmation on the `.s`, because one instrument is not evidence.** Counting register
movs immediately preceding a `bl`:

| | calls | movs before a call | per call |
|---|---|---|---|
| zcc | 12,378 | 27,501 | **2.22** |
| gcc -O1 | 11,901 | 11,949 | **1.00** |

**≈15,552 excess movs are call-argument setup — 78% of the whole `mov` excess (+19,910), and ~21% of
the remaining instruction gap.** zcc's ABI marshalling alone (36,911 copies) is larger than gcc's
ENTIRE copy budget (33,608 movs). Two instruments, one at MIR and one on the emitted text, agree.

**Consequence for the ladder, stated but NOT acted on.** The largest measured copy class has no row in
§13n, and R4's own definition is "attack the largest class". Adding a row is a re-plan and belongs to
the user, so R4.2 stands as written and this measurement is banked underneath it. What the numbers say
if it is opened: the lever is argument TARGETING (place the value in its AAPCS64 register at its
definition rather than copying it there at the call), which is a `regalloc`/`isel` question and NOT a
coalescing one — `#21` on `main` did the same thing for the >8-argument hazard path and cut sqlite by
906 instructions.

### THE HOT-LOOP INSPECTION (2026-08-25, after R4.2) — the exec side of §13n, measured for the first time

**Why it was taken.** §13n's SIZE side was measured with instruments (`ZCC_MOVKIND`, `ZCC_COALESCE`,
`ZCC_SPILLCEIL`), each printing a verdict before a line was written. Its EXEC side was assigned by
INFERENCE from the same static sqlite histogram — "the suite's losses are (d), (f), (h), (j)" — with
no per-program measurement. The plan's own iteration process says to take a row's prediction FIRST;
for R4.5, R4.7, R4.9 and R4.11 nobody ever had. This is that step: every program above 1.2× exec, hot
loop diffed against gcc-O1, every mechanism named and attributed to a row or marked UNOWNED.

**Result: 16 mechanisms. 10 owned by a §13n row, 6 with no row.** Only d4, i1 and j3 are fully
explained by the plan as written. The plan's CONTENT held up well; what it lacked was the exec
prediction, the order, and one whole theorem family.

| program | exec | insn | zcc hot loop | gcc | mechanisms (row) |
|---|---|---|---|---|---|
| j5_insertion_sort | 2.850 | 0.970 | **11 insns** | **6** | bool-then-branch (R4.5) · `p[j]` loaded twice (R4.9) · index recompute vs post-index (**NEW**) · count-down IV (**NEW**) · hot body after `ret` (R4.11) · dead frame (R4.4) |
| d3_early_exit | 1.969 | 1.055 | 7, 3 branches | 6, 2 | not rotated: top test + unconditional back-branch (R4.11) · 32-bit IV forces `sxtw` in the address where gcc widens to `x` (**NEW**) · dead frame (R4.4) |
| j3_prefix_sum | 1.940 | 1.091 | **6** | **6** | `ldr`+`add …,sxtw` (2 cyc) vs `ldrsw`+`add` (1 cyc) on the loop-carried chain — **the ONLY difference** (R4.7) |
| d2_nested_loops | 1.900 | 1.308 | 6 | 5 | `i*j+k` not strength-reduced to an add-IV, `madd` every iteration (**NEW**) · edge copies in the loop nest (R4.10) |
| f2_double_poly | 1.800 | 1.375 | 15 (incl. `fdiv`) | 7 | `x/1024.0` not `x*2^-10` (**NEW**) · FP constant rebuilt per iteration (R4.6) · **integer loop bound** rebuilt per iteration (R4.6) · `fmov d31,d8; fmov d8,d31` (**R4.2 RESIDUAL**) |
| e2_many_args | 1.500 | 1.283 | 23 in `mix` | 13 | **8** entry `mov`s out of x0–x7 (R4.3) · `ldr`+`sxtw` vs `ldrsw` (R4.7) · gcc inlines the called-once non-`static` callee, zcc requires `is_static` (**NEW**) |
| d4_goto | 1.400 | 1.292 | 9, 2 branches | 6, 1 | `and`+`cmp #0` not `tst` (R4.7) · `sub wzr`+`csel` not `csneg` (R4.7) · not rotated (R4.11) |
| i1_global_acc | **1.333** | 1.089 | 8, 2 mem-ops | 6, 0 | `gsum` reloaded+stored every iteration, gcc keeps it in a register (R4.11 store motion) · `ldr`+`add …,sxtw` vs `ldrsw` (R4.7) · `adrp/add` per global vs one `.LANCHOR` (unowned, cold) |
| d1_switch | **1.326** | 1.058 | 40 | 40 | jump table + indirect branch where gcc uses a compare tree with `csel`/`csinc`/`tbnz` — R3.3's density policy constant is an Article-E "spec's number or convenience's number?" question (**NEW**) · `sub w3,w3,#0` |
| h2_revbits | 1.250 | 1.219 | 7 | 5 | `lsl`+`orr` not commuted into `orr …, lsl 1` (R4.7) · count-down IV with `subs` setting the flags (**NEW**) · dead frame (R4.4) |

**d1 and i1 were BELOW the harness's own ~30ms trust floor and are re-measured here** at 25× and 30×
the work: d1 1.500 → **1.326**, i1 1.368 → **1.333**. geo40 EXEC geomean 1.3467 → **1.3356**.

**The six unowned mechanisms, and what they are.**

| mechanism | gcc flag (all **-O1**) | programs |
|---|---|---|
| pointer / 64-bit IV instead of a recomputed `sxtw` index | `-fauto-inc-dec`, `-ftree-slsr` | j5, d3 |
| count-down IV: `subs`+`bne`, the decrement sets the flags | IV canonicalization | j5, h2 |
| strength-reduce `i*j+k` to an add-IV in a nested loop | `-ftree-slsr` | d2 |
| `x/2^k` → `x*2^-k` (exact in IEEE, the reciprocal is representable) | -O1 algebraic | f2 |
| small dense switch → compare tree, not a jump table | -O1 heuristic | d1 |
| inline a called-once function that is not `static` | `-finline-functions-called-once` | e2 |

The first three are ONE theorem family: **zcc shipped SCEV in R2 and it does not fire on these
shapes.** §13n could not see them because it was decomposed from a STATIC sqlite histogram, where an
induction-variable shape costs zero instructions. That is the honest reason the plan missed them.

### R4.2 IS NOT EXHAUSTED → NOW EXHAUSTED — the FPR twin, 1,558 instructions ✅ BANKED 2026-08-25

`fmov d31, d8 ; fmov d8, d31` in f2's loop is the exact windmill pair R4.2 removed on the GPR side.
On sqlite: **783 pairs = 1,566 instructions** (measured; §13n's 779/1,558 was the pre-R4.1 estimate),
0.7% of the module. R4.2's banked Law-4 residual ("41, of which 35 fundamental") was **GPR-ONLY** —
the count script grepped `mov x16`/`mov wN,wN` and never looked at `fmov`, and `residual_report`
counts only CANDIDATES, which an edge copy is not.

The cause is structural, not a missed grep. `sequentialize`'s `nop` treated **every** narrow physical
self-move as truncating (`Reg::P(_) => false`), correct for a GPR ABI register but wrong for an FPR:
`fmov d8,d8` was refused and windmilled through v31. For an FPR the pair's OWN width is the value's
width — a 128-bit value carries `Width::Q` — so at `s`/`d` no `q`-form reader can observe the bits
`fmov d,d` zeroes; `d` IS the full useful width, the same argument R4.2 already makes one register
class over (AAPCS64 §6.8.2). §13n row (a) predicted it ("f2 — the FP twin"). **Category (b), same
theorem, banked under R4.2's own number** (R4.1 precedent).

**THE FIX** — one arm in `destruct::sequentialize::nop`: a physical self-move at `Width::S | Width::D`
is a no-op. drop_self_moves needed no change; the FPR windmills are edge copies, born after `destruct`
and killed at `nop`. **Measured 783 → 4 pairs**; the 4 survivors are a genuine `fmov d31,d9 ; fmov
d9,d8 ; fmov d8,d31` **swap of two doubles** (a real 2-cycle, scratch mandatory) — category (a),
fundamental. Residual is (a) entirely ⟹ **exhausted**. sqlite **218,776 → 217,160** (−1,616 insns,
**1.3928× → 1.3826×** vs gcc-O1 157,074).

**Proof (Law 3).** Inline test `a_double_self_move_across_an_edge_leaves_no_instruction`
(`regalloc/tests.rs`): a two-double loop's edge copies leave **zero** `SCRATCH_FPR` writes
(the count assertion), and a genuine double-swap keeps its scratch and stays correct under the
`same` differential (the commuting square). **GATE:** cargo 145/0, torture 0 FAIL, opt-parity
1552/0 DIVERGE, csmith300 254/0 DIVERGE, yarpgen300 294/0 DIVERGE (the 6 CTIMEOUT are a **pre-existing**
optimizer/backend slow-compile on pathological yarpgen functions, not R4.2: proven with `ZCC_O0=1`
s0007 12 s vs opt-on 259 s, and this change lives in `destruct`, after the optimizer — now the subject
of **§CP, the compile-speed campaign**), determinism 86×8.

**Banked WITH R4.2 (byte-identical compile-speed, §CP):** two O(n²)→O(n) fixes that leave codegen
untouched — `sroa`'s iterated-frontier `ever`/`seen` Vec-scans became bitmaps, and `licm`'s
per-hoist full-`Func` `refresh_defs` became a scoped `refresh_block_defs` over the two blocks a hoist
changes. sqlite 217,160 insns UNCHANGED; the rest of the campaign (backend regalloc first) is planned
in §CP, not started.

### §CP — THE COMPILE-SPEED CAMPAIGN (opened 2026-08-25; a side campaign, orthogonal to R4)

**Moved to `CP.md`** (transient working doc, the compile-speed twin of `OPT.md`). The full campaign —
why, the debug-vs-release build fact, the measured phase profile, the shipped 3894fb5 fixes, and the
CP2.x algorithm ladder (spiller-first) — lives there and is edited in place there while it runs.
`CP.md` is DELETED when the campaign closes; its durable results cook back here (final baseline) and
into `THEORY.md` (any load-bearing algorithm). One-line status: **Phases 0–1 DONE (profiled, ranked);
Phase 2 = the CP2.x ladder, NOT started, spiller `spill_with` is target #1 at 51–64 % of compile.**

### R4.7 — BANKED. The §17 rows, verified one by one. sqlite **217,160 → 212,066** (1.3826× → **1.3501×**); EXEC geomean **1.3386 → 1.2044**, median **1.225 → 1.073**, count>1.1× **10 → 8**

**The row's whole content was that §13n finding (f) is true: §17's ✔ marks were
claims.** Every number below is a `grep` over the emitted `.s` of the sqlite
amalgamation, taken before and after on the same box in the same session.

| mnemonic | before | after | gcc-O1 | what closed it |
|---|---|---|---|---|
| `ldrsb` | 0 | 106 | 111 | extending loads |
| `ldrsh` | 0 | 541 | 492 | extending loads |
| `ldrsw` | 29 | 438 | 329 | extending loads |
| `sxtb` | 120 | 14 | 11 | …their consequence |
| `sxth` | 687 | 146 | 90 | …their consequence |
| `sxtw` | 1,488 | ~1,180 | 641 | …their consequence + the no-op operand row |
| `tbz` | 156 | 449 | 749 | single-bit tests |
| `tbnz` | 170 | 714 | 972 | single-bit tests |
| `cmn` | 0 | 116 | 134 | negative compare immediate |
| `csneg` | 0 | 8 | 25 | conditional-select family |
| `csinc` | 0 | 39 | 39 | conditional-select family |
| `csinv` | 0 | 1 | 30 | conditional-select family |
| shifted ALU operand (`lsl #`) | 480 | 613 | — | commuted from either side |
| **module total** | **217,160** | **212,066** | 157,074 | **−5,094** |

**The eight rows shipped, each with its square in `isel/tests.rs` (a square AND
a count — a square alone stays green with nothing selected, which is exactly how
§17 acquired eight false ✔ marks).**

1. **Extending loads** — a `sext` whose only source is a narrow load is
   performed BY the load, into the extension's own register. Nothing moves; the
   load has one use, checked. This is the row R4.7 was ordered first for, and
   the reason is NOT the instruction it removes.
2. **The extension width belongs to the OPCODE.** `ldrsb Wt` and `ldrsb Xt` are
   different instructions — the `w` form zeroes bits 63:32 — and after
   allocation the destination is physical and carries no width at all. `emit.rs`
   inferred it from the register and printed `ldrsb x0` for a 32-bit extension:
   torture `pr19606` went RED, `(unsigned)(signed char)-4` computing −4 instead
   of 4,294,967,292. **A Law-2 Side-II defect** (a spec fact — the two forms —
   applied wrongly), found by the gate in one run, fixed by splitting `MemOp`
   into `SB`/`SBX`/`SH`/`SHX`/`SW`, which is what DDI 0487 C6.2.192 actually
   lists. Battery row: `the_extension_width_belongs_to_the_opcode_not_the_register`.
3. **Single-bit tests** — `tbz`/`tbnz` for `x & (1<<k)`, in both spellings C
   uses (the bare truth value, which HIR carries as `br(value)`, and the
   explicit `!= 0`, which arrives as a fused compare). A multi-bit mask is
   refused: `tst` + `b.cc` is two instructions exactly like `and` + `cbz`, so
   gcc's 489 `tst` against zcc's 0 is **category (a) — a naming difference, not
   an excess**, and the `tst` row of §17 is closed on that argument.
4. **The conditional-select family** — `csneg`/`csinv`/`csinc` absorb the
   negation, complement or increment on the arm the other one does not name; and
   `c ? 1 : 0` is `cset` alone, where materializing the 1 and selecting it was
   two instructions. Every `&&`/`||` that is not directly a branch condition
   reaches that shape.
5. **A constant operand reaches the immediate field from either side** —
   A64's immediate field is on the second source only, so `7 < x` is read as
   `x > 7` (the condition table is symmetric); `Imm(0)` is left alone because
   the zero register serves either side free.
6. **`cmn` for a negative compare immediate** — `cmp x,#-1` has no encoding
   (the add/sub imm12 field is unsigned) and was materialized into a register;
   `cmn x,#1` is bit-for-bit the same arithmetic and therefore bit-for-bit the
   same NZCV.
7. **A shift folds into a commutative operation from either side** — C writes
   the shifted side wherever it likes; `t = x<<1; y|t` is `orr w0,w3,w0,lsl #1`.
   Subtraction keeps the single order.
8. **`cmp_elim` searches to the next flag-touching instruction**, not only to
   the next instruction. The side condition is the whole content: moving the
   flag definition back is legal exactly when nothing in between reads or writes
   NZCV (which the allocator would reject outright — NZCV is a class of size
   one) and no `Call` clobbers it.

**THE CYCLE PREDICTION, VALIDATED.** §13n predicted, from a latency table and
with no build, that j3's loop-carried `add xN,xN,wM,sxtw` (2 cycles) becoming
`ldrsw` + `add` (1 cycle) would take j3 from a measured **1.940** to **≈1.0**.
Measured after: **1.000**. The instruction COUNT is unchanged — six against six,
before and after — so `cost = |MIR|` scored the two loops identically and always
would have. This is the first quantitative time prediction the plan has made and
it came in at the third decimal. It is the argument "THE MISSING DUAL" was
waiting for, and the time-dual row is now opened on evidence rather than on
taste.

**Its Law-4 residual, taken and closed.** `ext_lattice` removed the standalone
`sxtw`; an extension riding INSIDE an operand (`add x1,x1,w0,sxtw`) is a
different instruction — 2 cycles against 1 — and the lattice never looked at
one. `s += (i*j+k)&31` put exactly that on d2's loop-carried recurrence for an
extension that provably does nothing (`and w,#31` leaves bits 63:32 zero and
bit 31 clear, which is what `sxtw` would write). `mir/pass/ext.rs::plain_operand`
proves it on the same lattice and rewrites the operand to the plain register.
d2 **2.111 → 1.500**. Battery row in `mir/pass/tests.rs`.

**And a second residual, which was a de-optimization dressed as a munch row.**
The `madd` fold absorbed a multiply from ANOTHER block. For a shift or an
extension that is free — the producer rides inside the consumer's encoding — but
a multiply is a multiply, and the one it kept pulling back into d2's inner loop
had just been HOISTED OUT of it by LICM. A fold may now absorb a `Mul3` producer
only from its own block. Same instruction count, one multiply per iteration
gone.

**Per-program exec, before → after** (same box, paired with the deterministic
insn ratio). **The wall-clock noise is stated, not hidden:** two runs of the
final tree region gave EXEC geomeans of **1.1870** and **1.2044** — the second
run's absolute times were ~2% higher across every program, box load — so the
honest reading of the row is "EXEC 1.34 → 1.19…1.20", and the DETERMINISTIC
column (sqlite −5,094 instructions, INSN geomean 1.1568 → 1.1493) is the one
with no error bar.

| program | before | after | what moved it |
|---|---|---|---|
| j3_prefix_sum | 1.940 | **1.000** | `ldrsw` on the loop-carried chain (the prediction) |
| d4_goto | 1.400 | **1.000** | `csneg` + `tbz` |
| i1_global_acc | 1.333 | **0.750** | `ldrsw`; zcc now FASTER than gcc-O1 |
| d2_nested_loops | 2.111 | **1.500** | no-op operand extension + the cross-block `madd` |
| d3_early_exit | 2.065 | 1.969 | the rest is rotation — R4.11 |
| e2_many_args | 1.500 | 1.500 | `ldrsw` banked; the rest is R4.3 + R4.14 |
| h2_revbits | 1.250 | 1.237 | the shift commute; the rest is R4.4's dead frame |
| j5_insertion_sort | 2.850 | 2.857 | untouched — see R4.13 below |
| d1_switch · f2_double_poly | 1.326 · 1.800 | 1.500 · 1.200 | not this row's (R4.14, R4.6) |

**What R4.7 did NOT close, classified.** `sbfiz` 0 vs 477 and `ubfiz` 0 vs 94 —
**category (b)**, a convenience truncation: `Bfx` is UBFM-as-extract and the
insert form needs a MIR variant of its own; 571 instructions, no measured exec
target, left as a named residual. `fmov #imm8` — **category (b)**, needs an
`FMovImm` variant; f2's row. `ccmp` 0 vs 612 and `csel` 4,450 vs 542 — **not
this row's**: they are R4.5's boolean-vs-flags theorem. `mul` 736 vs 108 —
**category (b)**, the `x*k → shift+add` half of §17's mul-by-constant row is
unshipped; SPEED-positive and SIZE-neutral-to-negative, so it needs its own
paired measurement and is left as a named residual rather than guessed at.
`uxtb`/`uxth` 516 vs 0 — the `ext_lattice` residual, sources that are not loads.

### R4.13 — the IV family: the residual was taken FIRST, and it refuted two of the three shapes on THIS compiler

The row's own instruction is "residual print FIRST … the count per refusal
reason IS the prediction". Taken on the post-R4.7 compiler:

**Shape (1), the pointer / 64-bit IV.** `hir/pass/iv.rs` already implements it
and is shipped default-OFF on a measurement (§13k). Re-measured on today's
compiler with `ZCC_IV=1`, the whole suite: **INSN 1.1493 → 1.1538 and sqlite
grows** — still negative, and now for a sharper reason than in §13k. A64's
scaled-index addressing makes rebuilding an address from a counter free, and
R4.7 has just removed the last thing that was NOT free about it: the `sxtw` in
the ALU that fed the chain. **Category (a) — fundamental on this target**, and
the gate §13k named ("re-opening needs a cost model that can say WHEN a
writeback pays") is not discharged. The pass stays off.

**Shape (2), the count-down IV whose decrement sets the flags.** The premise was
that gcc's `subs w1,w1,#1 ; bpl` beats zcc's separate compare. Counted on the
emitted code: zcc's `j >= 0` test is ALREADY one instruction — `tbz` on the sign
bit, the §17 row that has been shipped since R3 — so `sub` + `tbz` is two
instructions against gcc's `subs` + `bpl`, also two. **There is nothing here to
win**; the shape was read off gcc's output and assumed to be a gap without
counting zcc's. Category (a). What the row's *general* form does buy — a compare
that the arithmetic before it has already performed, when the two are not
adjacent — shipped in R4.7 as the `cmp_elim` window.

**Shape (3), strength-reducing `i*j+k` in a nested loop.** Half of it was a
BUG, not a missing theorem: LICM had hoisted `i*j` out of the inner loop and
isel's `madd` row was pulling it back in, every iteration. Fixed under R4.7
(cross-block `Mul3` refused) and d2 went **2.111 → 1.500** with the multiply
gone from the loop. The remaining half is real and is what gcc's `-ftree-slsr`
does: gcc's inner loop is **5** instructions to zcc's **6** because its loop
COUNTER *is* `i*j+k` — one add-IV serving both the value and the exit test,
where zcc keeps a counter and computes the value from it. That needs a new IV to
be created, the exit condition rewritten in terms of it, and the old counter
proven dead — a genuine SCEV/`iv.rs` theorem with its own commuting square.
**Category (b), and it is the ONLY (b) left in this row**; it owns one program
(d2, now 1.500×) and is re-ranked accordingly.

**So R4.13 as written is discharged**: two of its three shapes are category (a)
on this target — measured, not argued — and the third has had its bug half
banked. The residual add-IV theorem stays a row, but it no longer sits second in
the order: **the hot-loop inspection's own numbers now put j5 (2.857×, and 81%
of the suite's absolute wall time) squarely on R4.5** — `cmp; movz; csel; cbnz`
per iteration where gcc branches on flags — with R4.9's repeated `p[j]` load and
R4.11's block layout behind it. The order below is amended on that measurement.

### R4.5 + R4.9 — BANKED together. sqlite **212,066 → 199,979** (1.3501× → **1.2731×**); EXEC geomean **1.2044 → 1.1490**, median **1.000**; **j5 2.857 → 1.940**

Taken as one batch because they are the same program's two mechanisms: §13n's
hot-loop inspection attributed j5 — 2.85× and the largest absolute wall time in
the suite — to bool-then-branch (R4.5) and a `p[j]` loaded twice (R4.9), and
neither can be judged with the other still in place.

**R4.5 — identities (e) and (f) in `cfg.rs`.** C's `&&`/`||` build a VALUE: one
arm computes a relation, the other passes a literal, and the merge block is
branched on. §13n row (d) measured what that costs — 3,707 pure-boolean `csel`
each with its `movz`, and 669 `csel → cbnz`, against gcc's 9.

* **(e) THREADING A KNOWN CONDITION.** If S is instruction-free and ends in
  `br p, X, Y` with `p` one of S's OWN parameters, a predecessor passing a
  literal for `p` already knows the answer — `⟦br k,X,Y⟧` is `⟦jmp X⟧` for k≠0.
  It names X directly, carrying S's arguments with S's parameters substituted by
  what that edge passed. Identity (d) then merges what is left of the merge
  block into the block that computed the relation, and isel's existing
  compare-branch fusion does the rest: j5's `cmp; movz; csel; cbnz` becomes
  `cmp; b.gt`.
* **(f) A BRANCH ON A SELECT OF TWO LITERALS** is a branch on its condition —
  the same shape, reached when the merge has already been if-converted.

**THE SIDE CONDITION, and the defect that found it.** Skipping S skips the
DEFINITION of S's parameters, and SSA licences a use anywhere S dominates —
arbitrarily far below the successor the substitution reaches. Shipped without
it, a loop header whose induction parameter the body read directly threaded into
that body and left the parameter undefined: `hir::verify` reported `t: %24 used
in bb6 but defined in bb2` on torture pr54937, `main: %17 used before its
definition` on pr109925, `bar: use of undefined %93` on pr116799, and
`unixLock: %298 used in bb73 but defined in bb66` on sqlite. **Four cases, one
run, caught at the layer that owns the invariant** — Law 3 exactly as written,
and the reason the rule now reads: S is threadable only when every parameter it
defines is used NOWHERE but its own terminator, so the substitution is total.
(Nothing else can lose dominance: a strict dominator D of S dominates every
predecessor P of S — extend any path entry→P by the edge P→S, and D must lie on
it with D ≠ S — so every value the threaded edge still names is defined above P.)

**R4.9 — one edge, no dataflow (`mem.rs`).** `mem.rs` already had the alias
oracle and the three memory transforms; what it lacked was a block big enough to
see across. A block C whose ONLY predecessor is P is entered exactly once per
execution of P, immediately after it, by no other route — so the memory state at
C's entry IS the state at P's exit, which is the same statement the block-local
walk already makes about two adjacent INSTRUCTIONS, applied to two adjacent
BLOCKS. C's table is seeded with P's, under three conditions: `preds(C) = {P}`;
P is visited first (reverse postorder, so a back edge seeds nothing); and a
carried entry loses its store-deletion candidacy, because forwarding a store to
a later load is a fact about memory while DELETING it would need C to always
follow P, and P may have other successors. The value a carried entry names is
defined in P or above it, and P dominates its only successor's every use.

That is the whole of j5's second `p[j]`: the body's only predecessor is the
condition block that just loaded it.

**Result on j5's inner loop — 11 instructions before this session, 8 now:**

```
zcc, now (8)                            gcc -O1 (6)
.L5: tbnz w4,#31, exit                  .L3: ldr  w3,[x2,-4]
     ldr  w5,[x2,w4,sxtw #2]                 cmp  w3,w4
     cmp  w5,w1                              ble  .L4
     b.gt body                               str  w3,[x2],-4
body:add  w6,w4,#1                           subs w1,w1,#1
     str  w5,[x2,w6,sxtw #2]                 bpl  .L3
     sub  w4,w4,#1
     b    .L5
```

**What is left on j5, named — and it is NOT size.** Its insn ratio is **0.881**:
zcc emits FEWER static instructions than gcc for the whole program and still
runs 1.94×. Two causes, both control-flow and latency:
1. **Not rotated (R4.11).** The `j>=0` test sits at the TOP and needs an
   unconditional `b` to return; gcc rotated the loop so that test IS the
   back-branch (`bpl`). Two instructions, two branches per iteration against
   one — and that alone is the entire 8-against-6 count gap.
2. **The address recurrence.** zcc's loop-carried chain is `sub w4,w4,#1` →
   `tbnz w4` → `ldr [x2, w4, sxtw #2]`: the index is sign-extended and scaled
   INSIDE the address every iteration. gcc's `str w3,[x2],-4` makes the store's
   own writeback the decrement, so the next address is already in a register —
   a shorter recurrence, and `add w6,w4,#1` disappears with it.

Cause 2 is **R4.13 shape 1**, which this session measured negative on the
whole-suite aggregate. That measurement stands and j5 does not overturn it: it
identifies WHERE the shape would pay, which is the per-loop cost question §13k
named and the global on/off switch cannot answer. It is also a STORE post-index,
which `autoinc.rs` refuses by design (`STR Xt,[Xn],#imm` with t == n is
CONSTRAINED UNPREDICTABLE, so the pass takes loads only and buys a side
condition it never has to discharge). **R4.11 goes first**, since it owns d3
(2.000×, now pure rotation) as well and closes j5's count gap; j5 is re-measured
after it, before shape 1 is re-opened.

**GATE (all green):** cargo 155/0 · fullsuite 10 PASS / 0 RED (shape, cpp,
decay, alg, abi, cases, ext, torture 1378 pass / 0 FAIL, cts, musl) ·
opt-parity 1552 / 0 DIVERGE · csmith300 254 / 0 DIVERGE · determinism 87×8
fresh processes. Batteries: `a_short_circuit_condition_reaches_the_branch_as_flags`,
`threading_refuses_a_block_whose_parameter_is_read_below_it`,
`a_load_survives_one_edge_into_a_single_predecessor_block`.

**Law-4 residual.** `ccmp` 0 vs gcc's 612 is NOT closed: threading turns
`a && b` into two branches, where gcc turns it into one `cmp` + one `ccmp` + one
`b.cc`. That is a further row of R4.5 needing a `CCmp` MIR instruction and a
two-block isel pattern; category (b), named, unshipped. `csel` and `cset` after
threading are re-measured under R4.12 as that row already says.

### R4.11 + R4.14 — BANKED (partly REFUTED). EXEC geomean **1.1490 → 1.0777**, median **1.001**, only **5** programs above 1.1×; sqlite 199,979 → **201,727** (1.2731× → **1.2842×**)

**R4.11's residual print came FIRST, as the row demands, and it named a reason
nobody had guessed.** §13n attributed d3 and d4 to `rotate.rs:129`'s refusal of
any labelled header. The instrument (`ZCC_RESIDUAL=1`, the same one `licm.rs`
carries) counted every refusal on sqlite:

| reason | before | after |
|---|---|---|
| header stores, calls or allocas | 4,070 | 4,610 | 
| **a header value is read after the loop, and the exit block is a MERGE** | **1,837** | **0** |
| header holds body work (copying it would be peeling) | 358 | 365 |
| header larger than `max-loop-header-insns` | 280 | 306 |
| **a header value is read after the loop, outside the exit's dominance** | **221** | **0** |
| header's branch has both arms inside the loop | 194 | 196 |
| an exit block carrying a header value has a predecessor outside the loop | — | 286 |
| a labelled header | 0 measured — **the guess was wrong** | — |

The labelled-header refusal was fixed anyway (it now tests `pin`, which is what
address-taken means, keeping the old rule only for a VLA function where `emit`
writes `mov sp, x29` at a label). But it refused nothing measurable. **The two
reasons that mattered were both the same thing: the loop-closed-SSA construction
demanded a SINGLE DOOR** — one exit block, with one predecessor, dominating
every reader of a header value. A loop with an early `return` has two exits; a
`while (a && b)` reaches one exit from two different in-loop blocks. Both are
ordinary C.

**The general construction.** For each EXIT BLOCK e — outside the loop, with a
predecessor inside it — a header value read below e leaves through e as a
parameter, and every predecessor of e supplies the name IT can see: the old
header passes the value (it still defines it), the guard passes its clone, and a
body block passes the parameter the NEW header was given — which is why a value
needed only at an exit must get that parameter whether or not the body reads it.
Refused when an exit has a predecessor from outside the loop, which owes an
argument it has no value for (286 loops, category (a) for this construction).

**d3_early_exit 1.969 → 0.969** — zcc now beats gcc-O1 on it — and **j5 1.940 →
1.002**, its loop reaching 7 instructions against gcc's 6 with 2 branches
against 2. The 8-vs-6 gap R4.5's section predicted rotation would close, closed.

**A LATENT BUG IN A SHIPPED ANALYSIS, exposed by the battery.**
`scev::compute_trips` classified top- vs bottom-testing by PLACEMENT — "the
exiting block is the header and the header is not a latch". That is true of a
top-tested loop and ALSO true of the shape rotation now produces, because
rotation puts the body INTO the header and `cfg::merge` then absorbs the latch,
leaving one block that does the work and then tests. The trip count came out one
short (10 reported as 9; `scev_counts_the_trips_of_a_literal_loop`), and trip
counts are what the IV-widening overflow proof rests on. Fixed on DATA FLOW, the
same lesson `rotate.rs` records about its own termination argument: a top test
computes NOTHING but its condition, so every instruction in its block lies in
the condition's transitive cone. **This is the third time in this project that a
placement-based side condition was defeated by a later pass, and it is the
strongest argument yet for the rule "phrase it about data flow".**

### R4.14 — one row of three shipped, two REFUTED BY MEASUREMENT

**(1) `x / 2^k` → `x · 2^−k` — SHIPPED.** A power of two has an all-zero
significand, so its reciprocal is representable, and IEEE 754 §5.4 makes both
operations the correctly-rounded result of the same exact real number: they
agree bit for bit on every finite input, on ±0, on ±∞ and on every NaN payload.
Two exclusions, both about representability rather than rounding — a zero,
subnormal or infinite exponent field, and an exponent whose reciprocal would
land on the subnormal boundary. This is the ONE float row `fold.rs`'s rule 2
admits, and it is admitted because it is not an approximation. `fdiv` is 10+
cycles here and `fmul` is 3.

**(2) small dense `switch` → compare tree — MEASURED, INCONCLUSIVE, NOT
SHIPPED.** The row's content was that R3.3's density constant (≥4 cases) is an
Article-E "the spec's number or my convenience's number?" question, to be
settled on a measured crossover rather than cited. It was measured, and the
measurement refuses to settle it:

* On d1_switch (8 cases), directly and repeatedly: the jump table is **15 ms**
  and the compare tree **12 ms** — the tree wins by 20% while emitting **12 MORE
  instructions** (95 against 83). The table's indirect branch is unpredictable.
* On a synthetic sweep at 4, 6, 8, 12, 16, 24 and 32 cases, with a
  pseudorandom (unpredictable) index: **table and tree are within 1 ms of each
  other at every case count.**

The two disagree, so **the case count is not the variable**, and no constant
derived from it would be honest. The whole-suite A/B says the same: `ZCC_JT=9`
moved the EXEC geomean 1.0899 → 1.0639, but d1 alone moves only 13% and the
geomean would need 35% from it — the rest is cross-program noise. **MIN_CASES
stays 4, `ZCC_JT` stays as the instrument, and the row is recorded as measured
and open**: what distinguishes d1 is something about its switch that a case
count does not name.

**(3) inline a called-once function that is not `static` — REFUTED, REVERTED.**
§13n read e2's inlined `mix` as evidence of a rule gcc has. gcc's
`-finline-functions-called-once` says "all **STATIC** functions called once", and
the reason is structural: a callee with EXTERNAL linkage may be called from
another translation unit, so its out-of-line body can never be deleted and
inlining it duplicates the body permanently. Measured over the whole suite by
A/B:

| config | EXEC | INSN | sqlite |
|---|---|---|---|
| neither | 1.1627 | 1.1493 | 199,984 |
| **R4.11 only** | **1.0782** | **1.1499** | 201,727 |
| R4.14 (3) only | 1.0841 | **1.3326** | 201,078 |
| both | 1.0371 | 1.3332 | 202,875 |

A **16% INSN regression for a 7% EXEC one** fails THE ULTIMATUM's "both axes"
outright, and the A/B is also the cleanest statement yet of what R4.11 costs:
**EXEC −7.3% for an INSN geomean that does not move (1.1493 → 1.1499) and sqlite
+0.87%.** Reverted, with the reason written into `inline.rs` so the row is not
re-proposed.

**GATE (all green):** cargo 157/0 · fullsuite 10 PASS / 0 RED · opt-parity
1552 / 0 DIVERGE · csmith300 254 / 0 DIVERGE · determinism 87×8. Batteries:
`a_loop_with_an_early_return_rotates`,
`a_division_by_a_power_of_two_becomes_a_multiplication`, and the corrected
`scev_counts_the_trips_of_a_literal_loop`.

**Where the suite stands.** Five programs above 1.1×, and every one is owned by a
row the suite was said to be blind to: **d1 1.500** (R4.14 (2), open), **d2
1.500** (R4.13's add-IV), **e2 1.500** (R4.3's entry copies), **h2 1.222**
(R4.4's dead frame), **f2 1.200** (R4.6's rebuilt constants). The exec and size
sides have converged onto the same rows.

### R4.3 + R4.4 — BANKED. sqlite **201,727 → 189,279** (1.2842× → **1.2050×**); EXEC geomean **1.0777 → 1.0357**, INSN **1.1499 → 1.0690**, only **3** programs above 1.1×

**R4.3 — a parallel-copy destination may take its OWN dying source's register.**
`color.rs`'s "free colours only AFTER the definitions are placed" rule is right
about a `ParallelCopy` and for the right reason: the assignments are
SIMULTANEOUS, so a destination taking ANOTHER pair's dying source would destroy
a value that pair still has to read. It says nothing about a destination taking
the source of ITS OWN pair — that assignment makes the pair a self-move, which
writes nothing, so every other pair reads exactly what it read before. The
colour is freed for that one destination and re-occupied the instant the bias
declines it, so nothing else can slip into it, and `check` still asserts that no
two live values share a colour. sqlite **−2,046**.

Its ceiling was §13n's one UNMEASURED band — "(b) 8k…20k (measure first)" — and
the realized number is a quarter of the low end. That is the third row this
session whose stated ceiling and delivered number disagreed by more than 2×, and
it is why a ceiling is written as an upper bound and never as a forecast.

**R4.4, in two halves, and the second is the one that mattered.**

*(i) An object nothing names occupies nothing.* `sroa`/mem2reg promote a local
into registers and leave its SLOT behind, so a leaf function whose every local
was promoted still carried a frame and paid `sub sp` + `add sp` for it — §13n
counted 289 such functions, and f1's four promoted locals still bought
`sub sp, #32`. A slot no instruction names (no `AddrMode::Slot`, no
`Spill`/`Reload`, no `SlotAddr`) is never read and never written, so giving it
zero bytes changes no access and no other object's address beyond moving it
down. `sub sp` 1,879 → 1,588; sqlite **−1,623**.

*(ii) One epilogue per SHAPE, not one per return path.* `frame::run` gave every
`Ret` block its own copy of the callee-saved reloads and `emit` added `add sp`
and `ret` to each: sqlite paid **3,815 `ret` against gcc's 317**. Those tails are
identical — physical registers, fixed slots — so all but one copy of each
distinct tail is duplication. The return VALUE is already in its ABI register
when a `Ret` block is reached, so the shared block needs no parameter and cannot
observe which path arrived. sqlite **−8,779**, `ret` 3,815 → 2,160.

**AND IT HAD TO BE A SEPARATE PASS, WHICH THE BATTERY PROVED.** Written inside
`frame::run`, it silenced `shrink_wrap` outright: that pass requires the region
below its save point to be a SINK — no successor outside it — and a shared
epilogue is exactly such a successor. `shrink_wrap_moves_saves_off_the_fast_path`
went red on the first build. Moved to its own pass running AFTER shrink-wrapping,
the two compose: shrink-wrapping leaves some returns holding the reloads and
some not, and grouping by the EXACT tail keeps those apart. Two optimizations
that each look local, one ordering constraint, and only the battery could see it.

**The two axes have collapsed together.** Every row of this batch was filed under
"size only — the suite cannot see it", and the suite moved more than it has for
any row this session:

| program | before | after |
|---|---|---|
| **e2_many_args** | 1.500 | **1.000** — R4.3's own program, the entry copies |
| **f2_double_poly** | 1.200 | **1.000** |
| e4_leaf_calls | 1.000 | 1.103 (short program, at the noise floor) |
| h2_revbits | 1.222 | 1.189 |
| INSN geomean | 1.1499 | **1.0690** (median 1.140 → 1.060, 20 → 12 above 1.1×) |
| EXEC geomean | 1.0777 | **1.0357** (3 programs above 1.1×) |

**What is left above 1.1×, all three already owned:** d1 1.500 (R4.14 (2), open —
the case count is not the variable), d2 1.500 (R4.13's add-IV, category (b)),
h2 1.189.

**The mnemonic histogram, after the whole R4 run so far** (zcc vs gcc-O1):
`csel` **599 vs 542** — collapsed from 4,450 by R4.5's threading; `cbz` **4,631
vs 4,631**, exact parity; `cmn` 118 vs 134; `tbz`/`tbnz` 650/855 vs 749/972.
The named remainders: `mul` 730 vs 108 (the shift-and-add half of §17's
mul-by-constant row), `ccmp` 0 vs 612 (R4.5's residual), `sbfiz`/`ubfiz` 0 vs
571, `cbnz` 4,441 vs 2,435, `sxtw` 1,210 vs 641, `uxtb`/`uxth` 577 vs 0.

**GATE (all green):** cargo 157/0 · fullsuite 10 PASS / 0 RED · opt-parity
1552 / 0 DIVERGE · csmith300 254 / 0 DIVERGE · determinism 87×8.

### R4.6 + R4.10 — BANKED. sqlite **189,205 → 186,705** (1.2046× → **1.1886×**); EXEC unchanged at **≈1.05**, INSN **1.0677**

Both are SIZE rows and both behaved like it: the exec geomean read 1.0357,
1.0503, 1.0547 and 1.0535 across four runs of essentially this compiler, which
is one ±1.5% noise band. The number that moved is the deterministic one.

**R4.6 — the constant that was already materialized (`mir/pass/const_share.rs`).**
HIR carries a constant as an OPERAND, not a value: no definition point, no
interference, no value number — which is exactly what lets isel fold it into an
immediate field without first proving single use. The price appears at the one
place it does not fold, where isel mints a fresh `MovImm` per use and nothing
shares them. The fix is what HIR already does for every other expression —
dominator-scoped value numbering — applied to the two instructions that have no
HIR value to be numbered as, `MovImm` and `Adrp`. Both are pure and constant, so
a dominating one has already produced the same bits on every run that reaches
the later.

**THREE MEASUREMENTS, and each changed the shipped pass.**

1. *A copy is not a merge.* Written the obvious way — replace the redundant
   definition with `Copy` from the dominating one — it was a **+338 REGRESSION**:
   `movz` fell by 1,678 and `mov` ROSE by 1,478 with 262 extra reloads, because a
   copy's two ends are two live ranges and colouring merged them only sometimes.
   Rewriting the USES instead, and deleting the definition, has no second range:
   **−546**.
2. *Not across a call.* AAPCS64 §6.1.1 leaves ten callee-saved GPRs, so a value
   live across a call competes for a file a fifth the size. A constant is the one
   value for which entering that competition is never worth it — re-materializing
   costs ONE instruction, holding it costs one of ten. csmith proved it is not
   merely a bad trade: **thirteen programs failed to allocate**, the allocator
   reporting "11 call-crossing Gpr values live but only 10 callee-saved". Cutting
   the dominator scope at every `Call` fixed all thirteen **and improved sqlite
   by a further 392** — the across-call shares had been paying for themselves in
   spills.
3. *The ceiling had already been spent.* §13n's ceiling for this row was
   6k…12k, taken from "14,393 `movz`/`movn` of which 9,035 repeat". By the time
   the row was reached, `movz` was 8,823 — **R4.5's threading had removed the
   `movz #1` boolean constants that were most of those repeats.** A ceiling
   measured before three other rows ran is a ceiling about a different compiler.
   Realized: 938. Recorded rather than rationalized.

**R4.10 — the partner graph is FOLLOWED, not read one hop (`regalloc/color.rs`).**
Biased colouring hints from a copy partner's colour, and a direct partner is
often not coloured yet: colouring walks the DOMINATOR tree, while a block
parameter's argument comes from a PREDECESSOR, which need not be a dominator at
all — a back edge is coloured strictly later. The hint then finds nothing and the
parameter takes an arbitrary register, which is one `mov` on that edge for ever.
Following the partner graph transitively reaches a coloured member through the
chain the copies form. **Nothing about correctness changes**: this only proposes
a colour, and `free` still refuses one that is occupied, conflicting or in the
wrong half of the partition — a wrong hint costs a `mov`, never a value.

The depth is MEASURED, not chosen (Article E). Sweeping it over sqlite: depth 1
(the old one-hop behaviour) 188,659 · 2 = 187,260 · **3 = 187,097** · 5 = 187,081
· 8 and 16 = 187,104. It saturates at three; `ZCC_CODEPTH` re-runs the sweep.

`ZCC_COALESCE`, which is Boissinot's own ceiling for this row: **FREE 5,070 →
4,143**, SAME 10,409 → 11,521, and `mov` 37,669 → 36,380. Realized −1,562
against a 2k…4.8k ceiling — the first row this session to land INSIDE its band.

**GATE (all green):** cargo 159/0 · fullsuite 10 PASS / 0 RED · opt-parity
1552 / 0 DIVERGE · csmith300 254 / 0 DIVERGE / **0 NOT-IMPL** · determinism 87×8.
Battery: `a_constant_already_materialized_is_not_materialized_again`, whose
second half asserts the across-call refusal.

### §13o — THE EXCESS, RE-DECOMPOSED (2026-08-26, on the R4.10 compiler). sqlite **186,705** vs gcc-O1 **157,074**, gap **29,631**

§13n's class table was taken at **237,025 instructions** and every number in it
now describes a compiler that no longer exists: `mov` excess went +19,147 →
+6,813, `csel` 4,498 → 599. §13n itself was born from re-taking a stale
histogram; this is the same step, and the tool is `tests/bench/excess.sh` —
EVERY mnemonic in either output, ranked by `zcc − gcc`, rather than a hand-picked
list that can only find classes someone already suspected.

**THE INSTRUMENT LIED TWICE BEFORE IT WAS TRUSTED**, and both corrections are in
the script so the next reader does not repeat them. The two assemblers do not
spell the same instruction the same way:
1. A constant materialization is `movz`/`movn` in zcc's output and `mov wN, …`
   in gcc's, so the first ranking put **`movz` 8,020 vs 0** at the top — pure
   dialect — while gcc's `mov` column silently carried its own immediates.
2. A conditional branch is `b.eq` in one dialect and `beq` in the other, which
   put ~5,000 of phantom excess in eight rows and ~5,000 of phantom deficit in
   eight more.
3. And after fixing (1) by matching `, #`, gcc turned out to OMIT the sigil —
   it writes `mov w0, 0` — so the immediates were STILL hiding. Match the
   operand, not the punctuation.
A ranking that cannot survive being read literally is not evidence (Article E,
applied to the instrument itself).

| class | zcc | gcc-O1 | excess | % of gap |
|---|---|---|---|---|
| **`ldr` + `str`** | 48,233 | 31,794 | **+16,439** | **55.5%** |
| — of which **frame `[sp,#…]`** | 22,819 | 12,456 | **+10,363** | **35.0%** |
| — of which non-frame | 25,414 | 19,338 | +6,076 | 20.5% |
| **`ldp` + `stp`** | 6,772 | 12,637 | **−5,865** | −19.8% |
| register `mov` (reg←reg) | 29,281 | 22,468 | +6,813 | 23.0% |
| constant materialization | 16,485 | 11,624 | +4,861 | 16.4% |
| `add` + `sub` + `cmp` | 25,710 | 20,804 | +4,906 | 16.6% |
| `cbnz` against gcc's `tst` + `ccmp` | 4,414 | 3,536 | +878 | 3.0% |
| `mul` + `sdiv` (by a constant) | 1,021 | 201 | +820 | 2.8% |
| `sxtw`/`uxtb`/`uxth` against `sbfiz`/`uxtw`/`rev` | 1,787 | 1,448 | +339 | 1.1% |

**THE FINDING: over half the remaining gap is ONE SUBSYSTEM, and it is two rows,
one of which nobody had counted.**

* **Frame traffic, +10,363 (35% of the gap on its own).** `ldr [sp,#…]` 13,507
  against 7,976 and `str [sp,#…]` 9,312 against 4,480. This is R4.8, and it is
  larger than every other named row left combined.
* **PAIRING, 5,865 instructions, and it is NOT IN §13n AT ALL.** gcc emits
  **12,637** `ldp`/`stp` to zcc's **6,772**. Every pair replaces two singles, so
  gcc's frame traffic is cheaper partly BECAUSE it pairs and we do not — the
  `ldstp` pass exists but its coverage was never measured against the reference.
  This is a genuinely new card, found only because the ranking included the rows
  where zcc emits FEWER of something.

**They compound, so they are taken together.** Pairing the spills is exactly how
gcc gets both numbers down at once: a spill slot pair `stp x19, x20, [sp, #16]`
is one instruction where zcc writes two. Sequencing R4.8 before pair coverage
would measure the spiller against a frame layout that cannot pair, and then
measure pairing against a spill set already shrunk — neither number would be the
row's own.

**What the tail says.** Register `mov` +6,813 is the coalescing residual
(`ZCC_COALESCE` prints FREE 4,143 of it); constants +4,861 is R4.6's residual
after the across-call refusal; `add`/`sub`/`cmp` +4,906 is address arithmetic and
loop bookkeeping with no row of its own. Those three are where R4.3/R4.6/R4.10
have already been grinding, and they are at diminishing returns — which is the
honest reason the next step is the frame, not another peephole.

### R4.8 — REFUTED BY ITS OWN CEILING. The frame excess is PAIRING DENSITY, not spill volume

The row said "spill, second pass: entry-set fixpoint across back edges; spill
stores at the eviction frontier", ceiling 4k…8k, on §13n's reading that frame
`ldr` was 17,052 against gcc's 7,765. Taken on THIS compiler, counting what those
instructions actually move rather than how many there are:

| | zcc | gcc-O1 | excess |
|---|---|---|---|
| frame slot-TOUCHES | 35,550 | 34,931 | **+619 (1.8%)** |
| frame INSTRUCTIONS | 28,519 | 23,475 | **+5,044 (21.5%)** |
| of which paired | 7,031 | 11,456 | — |
| share of touches paired | **39.6%** | **65.6%** | — |

**The spiller is at parity with gcc on how much it spills.** §13n compared
SINGLES against SINGLES and never counted gcc's `ldp`/`stp`, each of which moves
two values — so a compiler that spills the same amount and pairs it twice as
often looked like a compiler that spills twice as much. R4.1 and R4.2 had already
closed the volume; nobody re-measured. Building "spill less" on that number would
have chased 619 touches out of 35,550.

**The excess is entirely how many instructions the same traffic takes**, and
gcc's 11,456 frame pairs over 1,883 functions is ≈6 per function — the
prologue and epilogue, not body spills.

**SHIPPED, the pairing half.** sqlite 186,705 → **186,262**.

* **The frame is laid out spills-first.** `ldp`/`stp` take a SCALED SIGNED 7-BIT
  displacement (DDI 0487 C6.2.130), so a paired 64-bit access reaches 504 bytes —
  one eighth of a single access's reach. Slots were laid out in CREATION order,
  putting C locals first and the callee-saved saves and allocator spills above
  them, so in any function with a kilobyte of locals the prologue, the epilogue
  and every spill run sat out of range. Measured before: of 2,598 near-adjacent
  pairable frame accesses, **1,903 were refused for the offset alone**, 1,170 of
  them ADJACENT. After: 761 and 417. **+479 pairs.**
* **The pairing window looks past what is between.** Two accesses may be
  reordered when they cannot observe each other, and there are two ways to know
  it: both only READ (loads never conflict with loads), or both name frame
  objects whose byte ranges are DISJOINT — decidable, not an alias guess, because
  after `frame` every slot has a number. Refusing every memory instruction in
  between made the window useless (+14 pairs), since in a spill RUN the things in
  between are the other spills. **+139 pairs.**

**WHAT IS LEFT, MEASURED, and why the batch stops here.**
* ~1,700 pairing opportunities remain inside a ten-instruction window: 939
  encodable (blocked by the transfer register being rewritten in between — real)
  and 761 still beyond imm7 in the largest frames.
* **≈3,300 instructions in the frame adjust**: 1,588 `sub sp` and 1,726 `add sp`
  that `stp x19,x20,[sp,#-N]!` and `ldp x19,x20,[sp],#N` fold away for free
  (DDI 0487 C6.2.130's pre/post-indexed forms). ✅ **BANKED as R4.15** — see the
  §13n table row and the R4 status block. The adjust is now an ordinary
  `MInst::SpAdj` (not synthesised by `emit`, so Article B is honoured and
  `cost = |MIR|` is exact) that `mir/pass/frame_fold.rs` fuses into the first
  save pair and last restore pair. sqlite −3,452 → **183,253 = 1.1667×**.

**GATE (all green):** cargo 159/0 · fullsuite 10 PASS / 0 RED · opt-parity
1552 / 0 DIVERGE · csmith300 254 / 0 DIVERGE / 0 NOT-IMPL · determinism 88×8.

### THE MISSING DUAL — why a row can be right about size and blind about time

**j3 is the cleanest evidence in the project.** Six instructions against six, and 1.940× slower:

```
zcc   ldr   w5, [x3, x2, lsl #2]      gcc   ldrsw x4, [x0, x2, lsl 2]
      add   x1, x1, w5, sxtw   2 cyc        add   x3, x3, x4          1 cyc
      str / add / cmp / b.lt                str / add / cmp / bne
```

The loop-carried recurrence is `acc += ext(load)`. Extended-register ALU is 2 cycles, plain `add` is
1, so the recurrence bound predicts **2.0** and the measurement is **1.940** — a 3% error, computed
from a latency table with no build.

Law 3 gives every pass a CORRECTNESS dual (`⟦f⟧=⟦opt f⟧`) and a SIZE dual (`cost ≡ len∘codegen`,
exact by construction here). **There is no TIME dual**, so `cost = |MIR|` scores those two loops
identically and always will. The proposed shape, in the same grammar:

* **Side II — the latency/port table** in `mir/isa.rs`. NOTE (Article E, "the spec's number or my
  convenience's number?"): the box runs implementer `0x61` — **Apple M1 Pro cores, natively** — and
  **Apple publishes no Software Optimization Guide.** There is no spec to cite. Either cite ARM's
  published SOG and declare that the deployment target is a documented ARM core while the measuring
  machine is an M1, or MEASURE the table with a latency micro-benchmark and record it as a measured
  Side-II constant with its method. The second is honest; the first invents a citation.
* **Side I — the cost theorem** in a new `mir/cost.rs`, written independently of the lowering:
  `time(f) = Σ_b weight(b)·max(ResII(b), CritPath(b))`, and for a loop
  `max(RecII, ResII)` per iteration, `RecII = max over loop-carried cycles C of Σ lat(i)/dist(C)`.
  `weight` already exists on `MBlock`.
* **The square** — `time_model(f) ≡ cycles(mir::interp + scoreboard)`. Both sides from the SAME
  table, one structurally over the dependence graph, one dynamically by operand-ready times. Neither
  is a physical clock, so the equation is EXACTLY checkable — the property that makes
  `cost ≡ len∘codegen` a proof and not a benchmark. A disagreement is a Law-2 defect localized to one
  construct. `mir/interp.rs` already runs every battery.
* **Physical validation, ONCE** (not per pass, or patch-then-measure is back): correlate
  `cycles_interp` against wall time over the 35 programs; report the correlation and classify every
  outlier.

**What the model will NOT see, stated up front:** branch misprediction (d1's indirect branch, j5's
data-dependent `cbnz` — the model would predict ~1.8× for j5 against a measured 2.85×) and cache
behaviour. Both are category (a) FOR THE MODEL and stay the suite's job.

**A premise nobody had written down:** every exec number in this document was measured on Apple M1
Pro cores under Docker, while the notional target is generic AArch64-Linux.

### Two standing cautions, both earned this session
1. **Expect exec and insn to come apart.** j2_histogram regressed at IDENTICAL instruction count, and
   IV widening removed an instruction for exactly zero time. Spill traffic is memory ops in the hot
   path so it SHOULD move both — but that is a prediction to test, not to assume.
2. **The allocator is where the nastiest defects live** (§15b: the truncating self-move, the zero
   register as a copy partner). Both were found by a checker at the layer that owns the invariant,
   not by a suite three layers away. Any R4 step that weakens an allocator invariant adds its
   verifier check in the same commit.

### §13p — R4.17 CAPSTONE: the allocator-splitting restructure, before/after (2026-08-26, HEAD `650e521`)

Executed from `docs/superpowers/specs/2026-08-26-allocator-splitting-restructure-design.md`
against the plan in `docs/superpowers/plans/2026-08-26-allocator-splitting-restructure.md`
(Tasks 1–7). Baseline `761bbd7` is R4.16's closing number; `5c93a76` is the
interim checkpoint after generalized + loop-header carry landed but before the
regional split that pays for them (Batch B); `650e521` is the final tree
(Batch C: prediction instrument, regional split, prune).

| metric | baseline `761bbd7` | interim `5c93a76` (carry, no split) | **final `650e521`** | gcc-O1 |
|---|---|---|---|---|
| frame `ldr` | 10,675 | 10,827 | **9,584** | — |
| frame `str` | 11,316 | 11,381 | **11,464** | — |
| frame `ldr`+`str` | **21,991** | 22,208 | **21,048** | 12,721 |
| sqlite static insn | **182,956 = 1.1648×** | 183,682 = 1.1694× | **181,609 = 1.1562×** | 157,074 |
| `mov` | **37,828** | +607 (edge copies) | **37,689** | — |
| VdbeExec `[sp,#600]` stores | **227** | 243 | **243 (unchanged)** | 0 |
| compile time (release, in box) | ~11 s | ~11 s | **10.1 s** | — |
| non-loop join / loop-header phis | 0 / 0 | 982 / 1,702 (2,684 fired) | 545 / 411 (956, after pruning 1,883 trivial) | — |
| `ZCC_SPILLCEIL` total reloads | 12,479 | (dirty tree, not sampled) | **9,468** | — |
| `all-preds` reloads (no phi needed) | RC4 = 1,576 | 817 | **184** | — |
| `some-preds` reloads (needs a cold-edge phi) | — | 1,266 | **1,284 (untouched)** | — |
| `web-split` (spilled values register-resident somewhere) | — | 4,370 of 4,549 (96%) | consistent | — |
| `web-none` (over-pressured, no register anywhere) | — | 179 | **193** | — |

The regression at the interim checkpoint and its full recovery are both real
and both measured — the phi machinery fired (2,684 phis) before the mechanism
that makes it pay (regional eviction) existed, exactly the "residency without
headroom" signature the plan's own reading rule predicted before Batch C ran.
Batch C recovered AND overshot it: −1,347 instructions / −943 frame ops
against the `761bbd7` baseline, −2,073 / −1,160 against the interim.

**geo40 — the mandatory regression check for this task, run independently of
every number above:**

| | EXEC (arbiter, ≥30 ms subset, noisy) | INSN (deterministic, all 35) |
|---|---|---|
| RC4 / R4.15 / R4.16 baseline | ≈1.0517 (R4.15 banked 1.0513) | 1.0272 |
| mid-session independent read, `5c93a76` | 1.0540 (flat/noise) | **1.0272 (bit-identical)** |
| **this measurement, `650e521`** (`N=7`, `tests/bench/exectime.sh`) | **1.0523**, median 1.000, worst d2_nested_loops 1.556, 18 progs timed | **1.0272**, median 1.022, worst e3_struct_byval 1.759, 11 of 35 > 1.1× |

**No regression on either axis.** INSN is bit-identical across all three
independent readings taken over this whole restructure — none of the 35 geo40
programs spill under register pressure, so the phi/carry/split machinery
never fires on them; the restructure is, correctly, invisible to INSN there.
EXEC's ±0.002 spread across three independent reads sits inside this
project's own stated noise band for sub-30 ms wall-clock programs (±25% per
program; the geomean itself is far tighter) — read as flat, not as drift.
zcc-ZEROED bucket unchanged (e1_recursion, g2_strlen — asymptotic wins kept
out of the geomean, per the standing metric rule). The ULTIMATUM's speed axis
(~1.05× vs gcc-O1) is undisturbed by this restructure.

**Proof / commuting-square names**, all in `src/regalloc/tests.rs`, run
inside the `same()` battery (interpreter on both sides of
`⟦mir_before_alloc⟧ = ⟦mir_after_alloc⟧`):
`reconstruct_reconciles_a_join_with_a_phi`,
`generalized_carry_cuts_switch_reloads`,
`loop_header_carry_keeps_the_accumulator_in_a_register`,
`eviction_splits_regionally_not_whole_web`,
`reconstruction_is_pruned_and_pressure_is_counted`,
`the_carry_budget_reaches_a_doubly_nested_header` — plus the Task-1/2
round-cap and `insert_phi` meaning-guard tests. Every one defines its callees
(the codebase-wide vacuous-test trap this session found: `same()`'s
`(Err(_), _) => {}` arm silently accepts a trap on the BEFORE side, so a test
calling an undefined function proves nothing — worked around in every new
test here, and flagged as a pre-existing casualty elsewhere,
`abi_boundary_truncation_leaves_no_instruction`, not fixed in this scope).

**Full gate at `650e521`** (`.superpowers/sdd/2026-08-26-allocator-splitting-restructure/gate-650e521.txt`):
`== 15 PASS / 0 RED ==` — provenance (58 modules / 64 constants / 23 passes,
every pass squared and non-vacuous), shape/cpp/decay/alg/abi, determinism
88×8, cases, ext, torture 0 FAIL, cts, opt-parity 1552 PARITY / 0 DIVERGE,
csmith 254 PARITY / 0 DIVERGE / 0 TIMEOUT, yarpgen 300 PARITY / 0 DIVERGE /
0 TIMEOUT / 0 CTIMEOUT, musl. `cargo test` 171/0.

**Residual, Law-4 exhaustion — each classified fundamental (a) or convenience
truncation (b), none swept under a green gate:**

| # | residual | size | class | disposition |
|---|---|---|---|---|
| i | `[sp,#600]` / evicted-parameter carry unreachable | 243 stores, one function | (b) | lever named: regional split of the parameter at the terminator |
| ii | `some-preds` reloads untouched | 1,284 | (b) | needs a real profitability model past the cold-edge depth fence |
| iii | frame `str` rose while `ldr` fell | +148 `str` / −1,091 `ldr` vs baseline | (b) | net good (−943 total), but stores are the larger half of the gcc gap |
| iv | uncoalesced phi edge copies | 956 surviving params × up to `\|preds\|` copies, partially cleared by biased colouring | (b) | `THEORY.md` A7 — Boissinot value-based merging is the named upgrade |
| v | trivial-phi fixpoint round-count unproven | — | unclassified (proof gap, not a defect) | PARKED on 0 TIMEOUT/0 CTIMEOUT over 600 fuzzer programs; follow-up = bound the round count or worklist it |
| vi | phi-insertion cost fence | 513 frame loads for ~790 `mov`s | measured and REFUSED (Law 0) | not shipped — purity over a number |
| — | `web-none` (no register anywhere) | 193 values | (a) | fundamental — genuinely over-pressured, no split reaches them by definition |

Front accounting (§13o): the sqlite gap was 25,882 instructions at the
`761bbd7` baseline; only the spill-traffic front (9,270) was in scope for
this restructure. `mov`/coalescing (6,813), constant materialization (4,861)
and misc (~4,800) are enabled-not-done — this restructure opens their
headroom, it does not spend it. **Judged against the spec §2 floor (~1.10×,
not 1.0×): sqlite closed 182,956 → 181,609 = 1.1648× → 1.1562×**, with a
perfect elimination of the entire spill front bounding the best any lever in
this scope could reach at 173,686 = 1.106×.

**Pre-existing defect found and fixed, not introduced by this restructure**:
`src/regalloc/verify.rs` obligation (b) inherited its "already stored" set
from the immediate dominator — sound (a slot stored on every path to
`idom(b)` is stored on every path to `b`) but incomplete, since it can only
see a store that DOMINATES the reload. `evict_params` is exactly the shape
that separates the two: every incoming edge stores the evicted parameter's
value, so the slot is written on every path INTO the block and on none of the
blocks that dominate it. Replaced with the forward MUST dataflow
(`in[b] = ∩_preds out[p]`, `out[b] = in[b] ∪ stores(b)`, iterated to the
greatest fixed point) the obligation always meant — a strict superset of what
dominance could prove, never a subset. Two independent readers reproduced the
A/B: the old check fails identically with the regional split forced OFF, so
the false alarm pre-dates this session's work. `src/mir/verify.rs` untouched.

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
5. ⛔ **pointer-IV** (`pass/iv.rs`) — built, proven, and OFF. Gate discharged NEGATIVE on the fixed
   baseline (§13k): 0 win / 1 loss / 7 flat, worse on every axis. A64's free scaled index makes the
   premise false; re-opening needs a cycle-level cost model for post-index
5b. ✅ **isel addressing-mode fold** (§13j) — EXEC ≥30 ms 1.4610 → **1.3789**, 8 of 8 improve, 0
   regress, sqlite flat. Strictly better than row 5 was on every axis
6a. ✅ **IV widening** (`pass/iv.rs::widen`, §13l) — the last instruction between `mycopy`'s loop
   and gcc's. Five instructions each, one for one. EXEC-neutral, INSN 1.2419 → 1.2410. Subsumes
   LFTR for this shape: the narrow counter AND its test are gone
6b. ✅ **final-value** — CLOSED on measured absence of demand (§13m): 0 closable loops in sqlite
   and 0 in geo40, with the counting oracle validated against a known-positive first

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

### §13q — THE ROW-STRIDED POINTER IV, and the FIRST case where the cost model is provably blind (2026-08-26)

**WHAT WAS MEASURED, and why it is worth a section.** `tests/bench/matmul.c` sat at **1.638×**
gcc -O1 while `loops.c`, `fib.c` and `sieve.c` were at parity. Its inner loop was SEVEN instructions
against gcc's six — a one-instruction gap that could not explain 64%.

The localization was mechanical and did not touch the compiler. The k-loop was hand-edited into two
variants, assembled and linked from otherwise IDENTICAL zcc output, and all four binaries print
`414714994`:

| k-loop form | insns | ms, best of 5 | vs gcc -O1 |
|---|---|---|---|
| gcc -O1 | 6 | 69 | 1.000 |
| zcc, `madd x12,x11,x4,x1` then `ldr [x12,x9]` | 7 | 113 | **1.638** |
| **E1** — B walked by `add x14,x14,#1920`, counter kept | 7 | **69** | **1.000** |
| E2 — E1 plus post-index on A and a pointer-limit exit test | 6 | 69 | 1.000 |

**E1 and E2 are the same number.** gcc's other two tricks — the post-index writeback and dropping the
counter for a pointer compare — are worth nothing here. The entire gap is ONE multiply standing at
the head of a dependence chain that ends in a strided load.

**THIS IS THE COST MODEL'S BLIND SPOT, exactly as §10 predicts.** `cost = |MIR|` is exact by
construction — one `MInst` is one machine instruction — so it scored the `madd` form and E1
IDENTICALLY at seven, and always will. R4.7's j3 fact was the first instance (`add …,sxtw` at 2
cycles against 1, MEASURED M1); this is the second, and it is starker: same count, 64%. Recorded as
**MEASURED M9**.

**WHY THE PASS THAT EXISTS DID NOT FIRE — two separate defects, both Law-4 residuals.**

1. **The default-off verdict was over-broad.** `iv.rs` shipped OFF on MEASURED M2, whose A/B varied a
   UNIT-STRIDE address — `p[i]`, where step == the access size. That is the only case A64's scaled
   index reaches (`ldr Xt,[Xn,Xm,lsl #3]` scales by the access size and by nothing else, DDI 0487
   C6.2.130), so there the address really is free and a pointer really is pure cost. A ROW stride of
   1920 has no such mode: the address is rebuilt with a multiply. One verdict was covering two
   theorems. The gate moved down into `strengthen` — `ENABLED`/`ZCC_IV` now gates the unit-stride
   half alone, and M2's scope was narrowed in `MEASURED.md` to say so.

2. **`scev::AddRec` holds ONE symbolic base.** `B[k][j]` is `&B + k*1920 + j*8`: two loop-invariant
   symbolic terms around one recurrence, so `eval` refused the whole address and the load kept its
   multiply even with the pass forced on (`ZCC_IV=1` still emitted the `madd`). `iv::affine` now
   splits the top-level `add` and asks again — if one side carries the recurrence and the other is a
   pure invariant, the address is affine and its base is the SUM of the invariant terms, which is
   itself invariant and so computed once in the preheader. The commuting square is unchanged: the
   parameter holds exactly what the old address computation produced on iteration `n`.

**THE RESIDUAL PRINT SHIPPED FIRST**, as R4.13 requires. `ZCC_IVDBG=1` prints one line per declined
in-loop load with the reason. On matmul, after the fix: **17 `scev-refused`, 3 `unit-stride-gated`,
0 `no-symbolic-base`** — the count per reason is the prediction the next amendment is judged against.

**THE RULE THAT SHIPPED, and why it is narrower than "non-unit stride".** The first cut fired on any
step other than the access size. That was too broad and the A/B said so: sqlite **+1,245** static
instructions, geo40 unmoved. Law 3c names what actually costs cycles — a MULTIPLY in front of a load —
and a POWER-OF-TWO stride is not one: `fold::canon` has already turned `k*2^n` into `k<<n`, and isel
folds `add(base, shl(k,n))` into a single shifted-register `add`. Narrowed to strides that need a real
`mul`, the cost falls to **+951** and the win is untouched.

**THE FINAL A/B, all in one box session.**

| | baseline `8023b3c` | + `madd` literal | + this row |
|---|---|---|---|
| matmul | 1.638 | 1.638 | **1.000** |
| loops.c | 1.245 | **0.905** | 0.905 |
| geo40 INSN (35) | — | 1.0211 | **1.0211 — IDENTICAL** |
| geo40 EXEC (18) | — | 1.0433 | 1.0464 (noise; INSN identical ⟹ same code) |
| sqlite static insns | — | 172,393 | 173,344 (**+951**) |
| sqlite run TOTAL | 1.715 | 1.737 | 1.693 (noise) |

The taxonomy suite has **ZERO sites** for this row — INSN is identical to the digit with it on and off —
so geo40 is neither helped nor harmed and the EXEC wobble is the harness's noise floor, measured. The
row's whole effect is on matmul and on sqlite's 951 sites. Under Law 0 (`exec > size`) and Law 3c,
+951 static instructions buys a proven 1.638× → 1.000× where it fires: it ships.

**A PRE-EXISTING DEFECT, FOUND HERE AND NOT INTRODUCED HERE.** `realprog.sh` reports **REALPROG RED**
— `p04_point` and `p07_join` DIVERGE against gcc -O1. Both phases diverge identically on the baseline
compiler `8023b3c`, built and run in the same session, so today's two rows are innocent. It is a real
sqlite miscompile that no gate above catches (torture, cts, opt-parity, csmith300, yarpgen300 are all
green) and it is recorded here as OPEN.

**WHAT THIS OPENS.** §13's "THE MISSING DUAL" set an explicit condition for building a time-dual cost
model: *open the row only if R4.7's latency table alone closes j3 from 1.94× to ≈1.0×.* It did, to
the third decimal. This section is the second validation, on a case where instruction count moved by
ZERO. The premise is proven twice and the row is authorized by the plan's own rule — see **R4.18**.
It is also now **Law 3c** in `CLAUDE.md`.

---

### §13q ii — IV SUBSTITUTION: d2_nested_loops, and the category-(a) verdict that was wrong (2026-08-26)

**THE MEASUREMENT.** `for (k=0;k<n;k++) s += (i*j+k) & 31;` — zcc six instructions per iteration
against gcc's five, and **1.400 on the clock** (14 ms against 10; geo40 reported 1.556 in its own
session). Hand-validated before any pass was written, the §13q method: the k-loop was edited into
gcc's shape in zcc's own `.s`, assembled and linked, and both print `418008592` — **11 ms**, so the
one instruction was the whole gap.

```
zcc  add w7,w5,w4 ; and w7,w7,#31 ; add x6,x6,x7 ; add w4,w4,#1 ; cmp w4,w0 ; b.lt
gcc  and x2,x1,31 ; add x0,x0,x2  ; add w1,w1,1 ; cmp w1,w3    ; bne
```

gcc runs `i*j + k` AS the induction variable: it starts at `i*j`, steps by one, and the exit bound
becomes `n + i*j`, computed once in the preheader. The add that rebuilds the value on every
iteration is gone and the mask reads its input a cycle earlier.

**WHY THE PARAMETER IS 64 BITS, and it is not a style choice.** `SEMANTICS.md` §7 defines signed
overflow as WRAPPING — a deliberate refinement of ⊥ — so gcc's "signed overflow is undefined"
argument is NOT available here and the rewrite has to be exact under wrapping. In I32 it is not:
shifting `k <s bound` by `inv` flips at the sign boundary, and the corner is reachable — when
`inv + bound - 1 == INT_MAX` the shifted test exits on the FIRST evaluation instead of the last. In
I64 it is exact with no side condition, because `sext(inv)` and `sext(k)` are both 32-bit ranged and
their sum needs 33 bits. The four steps are in the pass's own header comment; the only external fact
is `no_wrap_signed(k)`, which `widen` already needed and `scev::find_nowrap` already proves.

**AND THE SAVING CAME STRAIGHT BACK, ONCE.** The first cut left `mov w7, w5` — the truncation of the
wide parameter — and the loop stayed at six. `fold::narrow_mask` is the rule that closes it:
`ext(trunc(x) & m) = x & m` for `0 ≤ m ≤ INT_MAX`, since the mask clears every bit either conversion
could have touched. Not special to this pass: any `(int)(long_expr) & MASK` promoted back to `long`
has the shape. With it, `and x7, x5, #31` — exactly what the model predicted when the same loop was
written in 64-bit C and compiled, BEFORE the pass existed.

**THE VERDICT THIS OVERTURNS, and it is the important part.** `THEORY.md` A7b recorded scalar
strength-reduction as CLOSED, Law-4 **category (a)**, on this premise:

> the rewrite is 1:1 static and an out-of-order core pipelines `mul` at ≈`add` cost, so it is null on
> every target (REARCH §13c)

That premise is **refuted by measurement**. It is true of a `mul` sitting in a basic block and false
of a `mul` at the head of a dependence chain, because what such a `mul` delays is everything
downstream of it. §13q's matmul moved **1.638× → 1.000× at identical instruction count**; this row
moved d2 **1.400 → 1.000**. Both were closed by one sentence of plausible micro-architectural
reasoning that no one had measured. `THEORY.md` now carries the correction, and this is the standing
example for R4.18: **a category-(a) closure taken on the size model is not a closure at all.**

**Gate:** cargo 176/0 (squares `an_invariant_plus_the_counter_becomes_the_counter` and
`a_masked_truncation_needs_no_widening`), provenance PASS, shape/cpp/decay/alg/abi PASS, cases OK,
ext PASS, torture 0 FAIL, determinism 88×8, opt-parity 1552/0, csmith300 254/0, yarpgen300 300/0,
musl PASS — **14 PASS / 1 RED**, the RED being `cts`, which reports `0 pass, 0 fail`: a suite that
was not found, on this run and on the two before it.

**THE TARGET-KNOWLEDGE FILE.** Everything in §13q and §13q ii that is about the MACHINE rather than
about a theorem now lives in **`src/arm64_elf.md`** — the shapes, what each costs, how to establish a
codegen claim, and the big-win ledger (the standing rule: any change taking a program from 1.3–1.5×
to parity or below gets a row). It is the successor to the pre-rearch `src/codegen/arm64_elf.md`,
which catalogued algorithms; this one catalogues the target.

---

## §14 Decision log (settled; reopen only with a stated reason)

| decision | choice | why |
|---|---|---|
| frontend | keep as input | failure is entirely below AST; parser is an independent proven artifact |
| SLP layer (R5.3) | a MIR pass, NOT a HIR pass with `Ty::V128` | the vector data path already exists one layer down — `Width::Q`, `MemOp::Q`, the FPR class, 16-byte slots, all carried since `long double` — so what was actually missing was arithmetic. HIR would have needed a new type in every exhaustive `match Ty` in the frontend half plus a lane semantics in `hir::interp`, for a type the frontend can never produce |
| scheduler position (R5.4) | post-allocation but PRE-frame-lowering | post-RA so no schedule can create a live range; pre-frame so it never sees a prologue, an epilogue, or an sp-writeback address. Learned the hard way: the first cut ran after `frame_fold` and the box returned corpus-wide SIGSEGV, because two memory READS are unordered and one of them was the epilogue's sp-restoring load |
| TBAA opt-out granularity (R5.2) | whole translation unit | `may_alias` and `optimize("-fno-strict-aliasing")` set one flag for the unit. The finer answer is a bit per `TypeId` beside `vol`; both gcc torture cases put the pun in `main`, where per-type buys nothing. Conservative direction: costs an optimization, never an answer |
| SSA representation | block parameters (HIR and MIR) | explicit edges, trivial destruction, one model |
| HIR types | closed `Ty` enum, signedness in opcodes | passes independent of TyTab; closed semantics |
| allocation | on SSA, Braun-Hack spill first, chordal greedy color | polynomial + optimal for the spill set; splitting free |
| SSA reconstruction after spilling | never built; R4.1 got the effect without it | carrying a copy only where every predecessor holds it IS dominance, so the copy's def dominates its uses and no φ/parameter is needed. §13n planned reconstruction; the measurement made it unnecessary |
| coalescing | biased coloring first; Boissinot merge only on measured residual | never breaks the pressure guarantee |
| the edge's parallel copy (2026-08-27) | its locations are registers **AND** spill slots | `evict_params` puts a slot on the edge, so read-before-write has to hold across the register/slot boundary; the register-only reading let a pointer rotation overwrite a slot another argument on the same edge still had to read, and the zcc-built sqlite CLI SIGSEGV'd on every two-table join. See THEORY A7 |
| `regalloc::verify` (2026-08-27) | runs on EVERY compile, post-allocation and pre-frame-lowering | it was called only from unit tests, so its obligations held on fixtures and were never asked of real input. Pre-frame-lowering is not a convenience: obligation (b) is stated in the `Spill`/`Reload` vocabulary and `ldst_pair` spends it, so the same check after `finish` reports a false `reload of unstored slot` |
| finding allocator defects | generated SHAPE families, not more corpus | the sqlite segfault survived 20,000 generated programs, torture 1694 and opt-parity 1552. No generator writes a pointer rotation under enough pressure to evict a parameter; 40 programs written to that shape found it in one run. A corpus corroborates, it does not discover |
| call-crossing values | modeled as `Clobber` constraints, no special logic | falls out of constraint-respecting greedy coloring |
| flags | k=1 register class | compare-elim = GVN; conflicts = liveness |
| scheduler | none | gcc -O1 has none; YAGNI |
| middle target-independent IR | deferred | one target |
| migration | big-bang on `mir-rearch`; `rc3` is the fallback | user directive; incremental rejected |
| scratch registers | x16, x17 (GPR), v31 (FPR) reserved | AAPCS64 IP0/IP1; parallel-copy cycle breaking |
| R0/R1 local storage | every C local stays in ONE frame slot (memory); promotion is R2.2 SROA+mem2reg | the parser reports `Var(off)`, not variable identity — two locals in disjoint scopes may share an offset, so promotion at build time would rest on an unproven disambiguation. Consequence: R0/R1 exercise the allocator on expression temporaries only, and the R1 allocator KPI is re-measured at R2.2 (noted in §12 R1) |

---

## §19 PARKED — the real-program measurement spine (opened 2026-08-25 by user directive; NOT started, does NOT change the R4 order)

**Why it is parked and not a row.** The 35-program taxonomy suite times only 18
programs, and it structurally CANNOT see three things: instruction-cache
pressure (every kernel fits in L1i, so zcc's +35% instruction count costs zero
cycles there), register pressure (§13n already records that no function in the
suite spills — so rows (g)/(i)/R4.8/R4.10, **58% of the measured size gap**,
cannot move any exec number this project has ever taken), and working sets past
L2. sqlite is measured for STATIC SIZE and never RUN. Adding more micro-kernels
fixes none of that; it only makes the geomean cover more of a sample that is
blind in the same places — and since the 15 currently-skipped kernels are the
EASY ones (a2 0.973, b3 0.925, f3 1.140), adding them would pull the headline
DOWN without the compiler being faster. That is the flattering-single-number
trap the 2026-08-24 directive already rules out.

**The reference shape** — `github.com/harshavmb/compare-claude-compiler`, a
published GCC-vs-a-new-compiler comparison, measures on more axes than this
project does. Its spine, recorded so ours can be framed against it:

| axis | what it measures |
|---|---|
| compile | wall time, user CPU, **peak RSS of the compiler**, binary size |
| runtime | total exec time, **per-query breakdown**, user CPU cycles, **peak RSS of the program** |
| code quality | size-bloat ratio, disassembly line counts, icache efficiency |
| workload A | Linux 6.9 — 2,844 translation units, link success/failure, system metrics sampled every 5 s |
| workload B | sqlite 3.46 amalgamation — 42 SQL operations over 10 phases (INSERT/JOIN/subquery/UPDATE/DELETE/GROUP BY…), 100k-row primary + 10k-row secondary table |
| correctness | 5 crash/edge tests — NULL handling, large BLOBs, recursive CTEs, Unicode, integer overflow |
| reporting | side-by-side tables with slowdown ratios; per-operation charts; CPU/memory-over-time graphs |

**What ours must add when it is taken up.** `~/.cache/zcc-suites/sqlite/shell.c`
is already in the corpus, so the sqlite CLI can be built by zcc and by gcc-O1
and run against a fixed SQL script — one measurement that covers icache and
spilling, which nothing in this project currently does. **Peak RSS belongs in
it** (user directive): both the compiler's and the program's, on both axes,
because a compiler that reaches parity on time by spending unbounded memory has
not reached parity. Reported paired and as a distribution, per the standing
measurement method — never as one number.

**Standing rule while parked: this changes nothing about the R4 order.** It is
a MEASUREMENT project, not a lever; it is taken up when a row's prediction needs
it or when R4 closes, and never in the middle of a row.

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
| ✔ multiply-accumulate | `madd/msub/mneg`, `smull/umull/smaddl/umaddl/smulh/umulh` | `add(mul)`, `sub(mul)`, `neg(mul)`, widened products | the `add`/`sext` — **residual taken 2026-08-26**: the row read the multiply's operands as VALUES and refused itself on an `Imm`, so `a*K + C` with literal `K` kept a separate `add`. A literal multiplier has to reach a register before `mul` can read it either way, so the register was already paid for; category (b), now closed (`AluFold::Mul3` carries `Operand`s). `tests/bench/loops.c` 24 → 22 insns/iteration, **1.245× → 0.905× gcc-O1** |
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
