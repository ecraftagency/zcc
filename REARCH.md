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
| R2.3 inline (+purity), licm (unconditional), iv/pointer-iv/LFTR | 🔨 inline + licm banked; **iv/strength-reduction/LFTR still ⬜**. licm: EXEC geo40 1.9415 → 1.8374 (−5.4%) for +0.011 INSN and +0.75% sqlite — banked because §13a's directive makes EXEC the target and size the byproduct. inline: the bound is DERIVED, not tuned — a body no larger than the call sequence it replaces (`params + 2`: one instruction to place each argument, the `bl`, one to take the result) cannot grow the program — plus gcc-O1's own `-finline-functions-called-once`. Net EXEC 1.8374 → 1.7468, INSN 1.5357 → 1.5148, sqlite 315,665 → 317,285. A called-once callee must also be DELETED once its last call site is gone, or the rule is a pure size loss: sqlite grew 25% before that existed |
| R2.4 if_convert, rotate/final-value/pure-call hoist (+ sink, added) | 🔨 if_convert and sink banked (`pass/ifconv.rs`): a side-effect-free diamond becomes `select`, speculating at most two pure trap-free instructions per arm. Refuses a store, a load, a division whose divisor is not a non-zero literal, and — for now — a FLOAT diamond, since `fcsel` has no MIR form yet. `pass/sink.rs` is licm's dual and was added here rather than planned: §13b ranked register pressure as the largest remaining item, and sinking is the cheapest thing that shortens a live range. **rotate / final-value / invariant-pure-call hoist still ⬜** |
| R2 gate + measurement | opt-parity (passes off vs on) 0 DIVERGE; csmith/yarpgen 0 DIVERGE. KPI: INSN geo ≤ 1.58 (rc3), sqlite ≤ 1.5×. **Merge-to-main eligibility starts here** | ✅ **both KPIs met and passed**: opt-parity 1552/0, csmith300 254/0, yarpgen300 300/0, torture 1471/0. See the R3 measurement row for the numbers — R2 and R3 were measured together because the isel and MIR rows landed in the same session |

### R3 — machine passes (§8) + isel munch table complete (§6)
| task | status |
|---|---|
| R3.1 munch patterns: addressing modes, cmp-branch fusion, csel forms, madd/msub, bfx, extend folding, mul-by-const | ✅ `isel::munch` — one pre-pass deciding which producers each consumer absorbs, because the producer is emitted first and the consumer's choice has to be known before its turn comes. Two licences, not interchangeable: an ADDRESS folds when EVERY use of it is a memory operand (folding into some while still computing it for others only duplicates work); an ALU operand folds on a SINGLE use (the shift or extension happens inside the consumer). Rows: `[base, #off]`, `[slot, #off]`, `[base, idx, ext #shift]`, `add/sub … , sxtw`, `op … , lsl #k`, `madd`/`msub`, `cmp`+`b.cc`, `cbz`/`cbnz`, `cmp`+`csel`. `ubfx`/`sbfx`, `cbz`/`cbnz`, `tbz`/`tbnz` on the sign bit. `mul(x, 2^k) → shl` is an HIR canonicalization (`fold::canon`) because only the shift form folds into an address. A producer that has itself absorbed something may NOT be absorbed again — the value it swallowed would then be defined nowhere |
| R3.2 cmp_elim, auto_inc, ext_lattice, ldst_pair | 🔨 `ext_lattice` (`mir/pass/ext.rs`), `ldst_pair` (`mir/pass/ldstp.rs`) and `cmp_elim` (`mir/pass/cmpelim.rs`) banked — `uxtb` 3,918 → 344, `uxth` 1,357 → 142, 7,104 `ldp` + 3,224 `stp` where there were none, `cmp` 13,312 → 9,487. cmp_elim fuses only where the CONDITION CODE survives: `cmp d, #0` sets C=1 and V=0 by definition, `adds` sets them from the addition, so only the codes reading N and Z alone carry over — and `lt`/`ge` are rewritten to `mi`/`pl`. **auto_inc still ⬜** |
| R3.3 switch jump tables, block layout, shrink-wrap | 🔨 jump tables banked: a switch with ≥4 cases occupying ≥half its span becomes `sub`/`cmp`/`b.hi` + `adrp`/`ldrsw`/`br` over a `.rodata` table of signed 32-bit offsets (position-independent, no run-time relocation). Block layout was already R0's; BRANCH RELAXATION was added to it, since `tbz` reaches ±32 KB and `b.cc`/`cbz` ±1 MB against `b`'s ±128 MB and the assembler cannot fix it — a far conditional gets a trampoline, placed AFTER the fall-through inversion so the inversion cannot undo it. **shrink-wrap still ⬜** |
| R3.4 **Law-1 sync — DO THIS NEXT.** `THEORY.md` ⊕ the specs IS the source of zcc and `src/*.rs` its compiled object; the docs currently describe a compiler that no longer exists. Re-derive them for everything R2/R3 shipped: **A6** (isel is no longer "the base case only, no munching" — `isel::munch` is the table, and there is no `isel/pattern.rs`) · **A7** (the spiller is Braun-Hack now, WITH its two recorded deviations: no SSA reconstruction, and a spilled parameter leaves the IR — both are theory, not implementation notes) · **A7b** (twelve passes are shipped, not `[PLANNED]`) · the MIR ladder (`ext_lattice`, `ldst_pair`, `cmp_elim` shipped) · rematerialization (shipped). **Side-II constants missing entirely**: `ldp`/`stp`'s scaled signed-7 offset · the branch reaches (`tbz` ±32 KB, `b.cc`/`cbz` ±1 MB, `b` ±128 MB) · `ubfx`/`sbfx` · the jump-table density rule · and above all **DDI 0487 B1.2.1, that every 32-bit write zeroes bits 63:32** — three of §15c's defects are that one line, and it appears in no table. `SEMANTICS.md` owes ⟦`Pair`⟧ and ⟦`Bfx`⟧ and the ⟦·⟧ obligation of each new pass | ⬜ |
| R3 measurement | `corpus25.sh` excess histogram per mnemonic; each class classified fundamental vs convenience (Law-4). Band: sqlite ≤ 1.3×, geo40 INSN/EXEC ≤ 1.2 | 🔨 sqlite **241,055 = 1.527×** (R1 origin 2.997×, rc3 1.768×), geo40 INSN **1.2982** (origin 2.5168, rc3 1.5835), geo40 EXEC **1.5857** (origin 4.4077). Band (≤1.3× / ≤1.2) not yet met — see §13b for the excess histogram and what each class is |

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
