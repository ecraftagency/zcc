//! AArch64 peephole / cost-square passes: pure assembly-text transforms.
//! Each is a Law-3 cost-theorem (⟦text⟧ preserved, len reduced); the parent's
//! emit_ir spine sequences them. All operate on `&str` -> `String`, no Cg access.
use std::fmt::Write;

// ─── the SOLE assembly register-operand decoder ──────────────────────────────
// Side-II (AAPCS64 §4.1 register naming): a GP operand token is `x<N>` (64-bit) or
// `w<N>` (32-bit) over the SAME physical register N. Every peephole reads operand
// registers through exactly these three fns — one spec-transcription of the token
// grammar, so operand decoding is deterministic and auditable in a single place
// (the formal-verification routine points here). None for sp / #imm / [mem] / :reloc:.
// Leading/trailing spaces are tolerated (trim is idempotent) so callers may pass a raw
// split() field or an already-trimmed token interchangeably.
fn xreg(t: &str) -> Option<u32> {
    t.trim().strip_prefix('x')?.parse().ok()
}
fn wreg(t: &str) -> Option<u32> {
    t.trim().strip_prefix('w')?.parse().ok()
}
fn gpreg(t: &str) -> Option<u32> {
    let t = t.trim();
    t.strip_prefix('x').or_else(|| t.strip_prefix('w'))?.parse().ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// BACKEND PEEPHOLE (Phase C) — machine-level redundant register-move elimination.
//
// WHY (MEASURED, not assumed): the emitter is an x0-accumulator machine ("every scalar
// lives in x0", top-of-file). The Stage-5b allocator gives each IR temp a HOME register,
// but the emitter still routes every op through x0/x1 and copies to/from the home — so a
// value is stored to its home (`mov xH, x0`) and immediately reloaded (`mov x0, xH`). On
// matmul this makes 197 of 398 instructions reg-reg `mov`s (gcc-O0: 0). This pass removes
// the provably-redundant ones — the single biggest measured lever toward QBE-class codegen.
//
// SEMANTICS PRESERVED — the safety argument (machine-level translation validation):
//   Track, within a STRAIGHT-LINE region, a value-equivalence over 64-bit GP registers: a
//   `mov xD, xS` makes D≡S (they hold the identical 64-bit value). The ONLY rewrite is:
//   DROP a `mov xD, xS` when the model already proves D≡S — the copy is then a verified
//   no-op, so removing it cannot change any later observation. The model stays SOUND because
//   every value-changing event breaks the relevant equivalence:
//     • a recognized DEF (first-operand-writing instruction) gives its destination a FRESH
//       value id — so no stale equivalence to it survives;
//     • an unrecognized mnemonic, any branch/call/label (a basic-block boundary) FLUSHES the
//       whole model — we never reason across control flow or an instruction we don't model.
//   32-bit (`w`) writes and float ops that define a GP reg still invalidate that register's
//   slot; equivalences are FORMED only by full-width `mov x,x`, so a partial-width write can
//   never be mistaken for a 64-bit copy. Live-out is safe: a redundant `mov x0, xH` at a
//   region end is dropped only when x0 ALREADY holds xH's value, so the return/epilogue sees
//   the same x0. Correctness is re-validated end-to-end by opt-parity (0 DIVERGE) + torture.
// ─────────────────────────────────────────────────────────────────────────────

/// Parse `mov xD, xS` (both 64-bit GP) → (D, S); None for `mov x,#imm` / `mov w,w` / shifts.
pub(super) fn parse_mov_xx(t: &str) -> Option<(u32, u32)> {
    let rest = t.strip_prefix("mov ")?;
    let mut it = rest.split(',');
    let d = xreg(it.next()?)?;
    let s = xreg(it.next()?)?;
    if it.next().is_some() {
        return None; // a third operand (shift) ⟹ not a plain reg-reg move
    }
    Some((d, s))
}

/// The slot of the first register operand (x or w share a physical slot), for DEF tracking.
pub(super) fn first_reg_slot(operands: &str) -> Option<u32> {
    let tok = operands.split(',').next()?.trim();
    gpreg(tok)
}

/// The register slots an instruction READS and WRITES, plus whether it ends a straight-line
/// region (branch/call/ret/unknown/writeback-addressing ⟹ we stop reasoning). Only x/w GP
/// registers are tracked; sp/fp/float operands are ignored (they never form a `mov x,x` we
/// rewrite, and over-counting a read only KEEPS more moves — the safe direction).
pub(super) fn reg_uses(t: &str) -> (Vec<u32>, Vec<u32>, bool) {
    // Writeback / pre-post-index addressing mutates the base register implicitly — rather
    // than model it, treat the line as a region boundary (conservative = keep everything).
    if t.contains('!') || t.contains("],") {
        return (vec![], vec![], true);
    }
    let mn = t.split(|c: char| c.is_whitespace()).next().unwrap_or("");
    let operands = t[mn.len()..].trim_start();
    // A GP-register slot in one operand TOKEN, or None if the token is a float/vector reg
    // (q/d/s/v/h/b), an immediate, a label, or a condition. Brackets (memory `[x0]`) stripped.
    let slot = |tok: &str| -> Option<u32> {
        let tok = tok.trim().trim_start_matches('[').trim_end_matches(']');
        gpreg(tok)
    };
    // Operand tokens, POSITIONALLY (comma-split). The destination of a def-first instruction
    // is token[0]; a memory operand like `[x0, x1]` splits into two tokens, both address READS.
    let toks: Vec<&str> = operands.split(',').collect();
    let gp_in = |range: &[&str]| -> Vec<u32> { range.iter().filter_map(|tk| slot(tk)).collect() };
    const BOUNDARY: &[&str] =
        &["b", "bl", "blr", "br", "ret", "cbz", "cbnz", "tbz", "tbnz"];
    const NO_DEF: &[&str] =
        &["str", "strb", "strh", "stp", "cmp", "cmn", "tst", "fcmp", "ccmp"];
    const DEF_FIRST: &[&str] = &[
        "mov", "movz", "movn", "add", "sub", "mul", "msub", "madd", "neg", "mvn", "and",
        "orr", "eor", "bic", "lsl", "lsr", "asr", "sdiv", "udiv", "sxtw", "sxth", "sxtb",
        "uxtw", "uxth", "uxtb", "cset", "csel", "csinc", "cinc", "adrp", "ldr", "ldrb",
        "ldrh", "ldrsw", "ldrsb", "ldrsh", "fmov", "scvtf", "ucvtf", "fcvt", "fadd", "fsub",
        "fmul", "fdiv", "fneg", "fcvtzs", "fcvtzu", "sxtl", "ubfx", "ubfiz", "sbfx", "sbfiz",
    ];
    if mn.starts_with("b.") || BOUNDARY.contains(&mn) {
        (vec![], vec![], true)
    } else if NO_DEF.contains(&mn) {
        (gp_in(&toks), vec![], false) // stores/compares: every GP operand is a READ
    } else if mn == "ldp" {
        // token[0], token[1] are destinations; the rest are address READS.
        let n = toks.len().min(2);
        (gp_in(&toks[n..]), gp_in(&toks[..n]), false)
    } else if mn == "movk" {
        (gp_in(&toks), gp_in(&toks[..toks.len().min(1)]), false) // merge: reads its own dst too
    } else if DEF_FIRST.contains(&mn) {
        // token[0] is the destination POSITION. If it is a GP reg → the WRITE; if it is a
        // float/vector reg (q0/d0/s0/…) → NO GP write, and every GP operand is a READ (the
        // bug this fixes: `ldr q0, [x0]` / `fmov d0, x0` must NOT treat x0 as the destination).
        match toks.split_first() {
            Some((first, rest)) => match slot(first) {
                Some(d) => (gp_in(rest), vec![d], false),
                None => (gp_in(rest), vec![], false),
            },
            None => (vec![], vec![], false),
        }
    } else {
        (vec![], vec![], true) // unknown ⟹ boundary (never mis-model)
    }
}

/// Machine-level move cleanup over one function body (see the block comment).
/// LEVER 1 — BITFIELD FUSION (ARM64 delicacy #6: ubfm-family). A two-instruction shift/mask
/// bitfield idiom is one AArch64 bitfield insn. Translation-validation tier (pure ISA identity,
/// like the move peephole) — fused ONLY when the two are ADJACENT, the second reads+writes the
/// first's dest (the in-place form zcc emits), same register width; the intermediate value dies
/// at the second insn so nothing observes it. N = 64 (`x`) or 32 (`w`).
///   `lsl rD,rS,#a ; lsr rD,rD,#b`  ≡ (rS<<a)>>b  →  b≥a: `ubfx rD,rS,#(b-a),#(N-b)`
///                                                    b<a: `ubfiz rD,rS,#(a-b),#(N-a)`
///   `lsr rD,rS,#k ; and rD,rD,#m`  ≡ (rS>>k)&m   →  m=2^w−1 ∧ k+w≤N: `ubfx rD,rS,#k,#w`
/// (two shifts compose to one unsigned field extract for ANY a,b; the mask arm needs a
/// contiguous low-bit mask.) Both replacements are 1 insn for 2 — size and dep-chain both shrink.
pub(super) fn fuse_bitfield(body: &str) -> String {
    // parse "mnem rD, rS, #imm" (imm decimal or 0x-hex) → (mnem, 'x'/'w', d, s, imm); the two
    // register operands must share the width prefix, else not a form we rewrite.
    fn p3(t: &str) -> Option<(&str, char, u32, u32, i64)> {
        let t = t.trim();
        let mn = t.split(|c: char| c.is_whitespace()).next()?;
        if mn != "lsl" && mn != "lsr" && mn != "and" {
            return None;
        }
        let mut it = t[mn.len()..].trim_start().split(',');
        let (dt, st, it3) = (it.next()?.trim(), it.next()?.trim(), it.next()?.trim());
        if it.next().is_some() {
            return None; // a 4th operand (shifted reg) ⟹ not the plain form
        }
        let pref = dt.chars().next()?;
        if (pref != 'x' && pref != 'w') || st.chars().next()? != pref {
            return None;
        }
        let d = dt[1..].parse::<u32>().ok()?;
        let s = st[1..].parse::<u32>().ok()?;
        let imm = it3.strip_prefix('#')?;
        let imm = match imm.strip_prefix("0x") {
            Some(h) => i64::from_str_radix(h, 16).ok()?,
            None => imm.parse::<i64>().ok()?,
        };
        Some((mn, pref, d, s, imm))
    }
    let lines: Vec<&str> = body.lines().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < lines.len() {
        if i + 1 < lines.len()
            && let (Some((m1, p1, d1, s1, a)), Some((m2, p2, d2, s2, b))) =
                (p3(lines[i]), p3(lines[i + 1]))
            && p1 == p2
            && d2 == d1
            && s2 == d1
        // second insn is in-place on the first's dest
        {
            let n = if p1 == 'x' { 64i64 } else { 32 };
            let ind = &lines[i][..lines[i].len() - lines[i].trim_start().len()];
            let fused = match (m1, m2) {
                ("lsl", "lsr") if a < n && b < n => Some(if b >= a {
                    format!("{ind}ubfx {p1}{d1}, {p1}{s1}, #{}, #{}", b - a, n - b)
                } else {
                    format!("{ind}ubfiz {p1}{d1}, {p1}{s1}, #{}, #{}", a - b, n - a)
                }),
                ("lsr", "and") => {
                    let m = b as u64;
                    let w = m.count_ones() as i64;
                    ((m & (m.wrapping_add(1))) == 0 && m != 0 && a + w <= n)
                        .then(|| format!("{ind}ubfx {p1}{d1}, {p1}{s1}, #{a}, #{w}"))
                }
                _ => None,
            };
            if let Some(fl) = fused {
                out.push_str(&fl);
                out.push('\n');
                i += 2;
                continue;
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// LEVER 2 — REDUNDANT SIGN-EXTEND elimination. The value contract re-canonicalizes every int32
/// to sign-extended-64 (`sxtw xD, wD`) at each materialization; when the value in xD is ALREADY
/// sign-canonical (bit63==bit31), that `sxtw` is a pure no-op. Track a per-block set of registers
/// known sign-canonical, produced by the 64-bit sign-extending ops (`sxtw`/`ldrsw`/`sxth`/`sxtb`/
/// `ldrsb`/`ldrsh` with an X destination — bits 32..63 filled from the sign) and `cset` (0/1).
/// Drop `sxtw xD, wD` iff D is in the set. Cleared at every block boundary (label / branch / call /
/// unknown) — sound by construction (only PROVEN-canonical regs enter the set). Same translation-
/// validation tier as the move peephole. [sqlite: 343 ldrsw→sxtw + 36 double-sxtw + tail.]
/// W-form producers are deliberately excluded (a `w`-dst leaves bits 32..63 zero, not sign-filled),
/// as are mov/bitwise propagation (their canonicality is width-subtle — safety over the extra ~17).
pub(super) fn drop_redundant_sxtw(body: &str) -> String {
    use std::collections::HashSet;
    let mut canon: HashSet<u32> = HashSet::new();
    let mut out = String::with_capacity(body.len());
    // `sxtw xD, wS` → (D, S), else None.
    let parse_sxtw = |t: &str| -> Option<(u32, u32)> {
        let r = t.strip_prefix("sxtw ")?;
        let mut it = r.split(',');
        let d = xreg(it.next()?)?;
        let s = wreg(it.next()?)?;
        it.next().is_none().then_some((d, s))
    };
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            out.push('\n');
            continue;
        }
        if t.ends_with(':') {
            canon.clear(); // basic-block boundary
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.starts_with('.') {
            out.push_str(line); // directive — touches no register
            out.push('\n');
            continue;
        }
        if let Some((d, s)) = parse_sxtw(t) {
            if d == s && canon.contains(&s) {
                continue; // value already sign-canonical → the sxtw is a no-op → DROP
            }
            canon.insert(d); // sxtw makes its X dest canonical (kept or not)
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let (_, writes, boundary) = reg_uses(t);
        if boundary {
            canon.clear();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mn = t.split(|c: char| c.is_whitespace()).next().unwrap_or("");
        let operands = t[mn.len()..].trim_start();
        // 64-bit sign-extending producer (X dst) ⟹ result canonical; cset ⟹ 0/1, canonical.
        let prod = (matches!(mn, "sxth" | "sxtb" | "ldrsw" | "ldrsb" | "ldrsh")
            && operands.starts_with('x'))
            || mn == "cset";
        for &w in &writes {
            if prod {
                canon.insert(w);
            } else {
                canon.remove(&w); // any other def clobbers the known-canonical status
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// LEVER 8 (direct) — REDUNDANT ZERO-EXTEND elimination (the zero-extend sibling of LEVER 2). A
/// byte/half load already zero-extends its destination: `ldrb wD` clears bits 8..63, `ldrh wD`
/// clears bits 16..63. A subsequent in-place `uxtb wD,wD` / `uxth wD,wD` that only re-clears
/// already-zero high bits is a pure no-op (it comes from the C integer promotion of an `unsigned
/// char`/`unsigned short`, emitted without noticing the load did the extension). Track a per-block
/// map reg → the bit index at/above which the register is KNOWN zero (8 for a byte producer, 16 for
/// a half producer). Drop `uxtb xD` iff D's known-zero floor ≤ 8; drop `uxth xD` iff ≤ 16 (a
/// byte-extended value is also half-extended, so `uxth` after `ldrb` is a no-op too; but `uxtb`
/// after `ldrh` is REAL — the half load leaves bits 8..15). Producers = `ldrb`/`ldrh` (X or W dst)
/// and the uxt themselves; sign-extending loads (`ldrsb`/`ldrsh`) and word loads (`ldr wD`) are
/// deliberately NOT producers (they do not clear bits 8..31). Cleared at every block boundary and on
/// any other write of D. Same translation-validation tier as LEVER 2 (pure ISA zero-extend identity
/// on a register with a proven-zero high field). [sqlite: 2,501 uxtb-after-ldrb + 1,047 uxth-after-ldrh.]
pub(super) fn drop_redundant_uxt(body: &str) -> String {
    use std::collections::HashMap;
    let mut zfloor: HashMap<u32, u32> = HashMap::new(); // reg → bits at/above this index are zero
    let mut out = String::with_capacity(body.len());
    // "uxtb|uxth wD, wD" (in-place) → (width_bits, D); width 8 for uxtb, 16 for uxth.
    let parse_uxt = |t: &str| -> Option<(u32, u32)> {
        let (w, rest) = if let Some(r) = t.strip_prefix("uxtb ") {
            (8u32, r)
        } else if let Some(r) = t.strip_prefix("uxth ") {
            (16u32, r)
        } else {
            return None;
        };
        let mut it = rest.split(',');
        let d = wreg(it.next()?)?;
        let s = wreg(it.next()?)?;
        (it.next().is_none() && d == s).then_some((w, d))
    };
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            out.push('\n');
            continue;
        }
        if t.ends_with(':') {
            zfloor.clear(); // basic-block boundary
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.starts_with('.') {
            out.push_str(line); // directive — touches no register
            out.push('\n');
            continue;
        }
        if let Some((w, d)) = parse_uxt(t) {
            if zfloor.get(&d).is_some_and(|&f| f <= w) {
                continue; // bits ≥ w already zero ⟹ the uxt is a no-op ⟹ DROP
            }
            zfloor.insert(d, w); // the uxt itself establishes the floor (kept or not)
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let (_, writes, boundary) = reg_uses(t);
        if boundary {
            zfloor.clear();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mn = t.split(|c: char| c.is_whitespace()).next().unwrap_or("");
        // byte/half loads zero-extend their dst; every other def clobbers the known-zero floor.
        let load_w = match mn {
            "ldrb" => Some(8u32),
            "ldrh" => Some(16u32),
            _ => None,
        };
        for &wr in &writes {
            match load_w {
                Some(w) => {
                    zfloor.insert(wr, w);
                }
                None => {
                    zfloor.remove(&wr);
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// LEVER 7 — W-FORM SIGN-EXTEND elimination (the DEMAND-side dual of `drop_redundant_sxtw`).
/// Value contract (db9cb93): an int32 lives in the LOW 32 bits of its 64-bit home; bits 32..63 are
/// DON'T-CARE. An in-place re-canonicalization `sxtw xD, wD` is therefore DEAD unless a later
/// instruction OBSERVES those high bits — i.e. reads D in **x-form** (a 64-bit operand or an
/// address base/index). Scanning forward from the sxtw within its region:
///   - x-form read of D reached first  → high bits observed        → KEEP
///   - D fully redefined first (w- or x-dest) → old value incl. sign bits dead → DROP
///   - w-form read of D                → this use ignores bits 32..63 → keep scanning
///   - region boundary first (label / branch / call / writeback / unknown) → live-out
///                                       unknown, an x-form read may exist downstream → KEEP
/// Translation-validation tier (pure ISA identity, like the move peephole): the rewrite preserves
/// ⟦·⟧ because every reader that could observe a difference — an x-form read — forces KEEP; only
/// extensions whose high bits are provably never observed before redefinition are dropped. This is
/// the exact dual of LEVER 2's supply-side canonical-set: there `sxtw` is dropped when the value is
/// ALREADY canonical; here when the canonicality is never DEMANDED.
pub(super) fn drop_wform_sxtw(body: &str) -> String {
    // `sxtw xD, wD` (same reg, the in-place re-canon form) → Some(D); the widening `sxtw xD, wS`
    // (D≠S) is a genuine int→long move and is never touched.
    fn parse_inplace(t: &str) -> Option<u32> {
        let r = t.strip_prefix("sxtw ")?;
        let mut it = r.split(',');
        let d = xreg(it.next()?)?;
        let s = wreg(it.next()?)?;
        (it.next().is_none() && d == s).then_some(d)
    }
    // Does any operand token of `t` name GP register `d` at width `pref` ('x'/'w')? Brackets are
    // stripped so an address `[x5, w6, sxtw]` is matched component-wise across the comma split.
    let token_present = |t: &str, pref: char, d: u32| -> bool {
        let mn = t.split(|c: char| c.is_whitespace()).next().unwrap_or("");
        let operands = t[mn.len()..].trim_start();
        let want = format!("{pref}{d}");
        operands.split(',').any(|tok| {
            tok.trim().trim_start_matches('[').trim_end_matches(']') == want
        })
    };
    let lines: Vec<&str> = body.lines().collect();
    let mut drop = vec![false; lines.len()];
    for (i, li) in lines.iter().enumerate() {
        let Some(d) = parse_inplace(li.trim()) else { continue };
        for lj in &lines[i + 1..] {
            let t = lj.trim();
            if t.is_empty() {
                continue;
            }
            if t.ends_with(':') {
                break; // label = region boundary (merge point) → live-out unknown → KEEP
            }
            if t.starts_with('.') {
                continue; // directive — no register effect
            }
            let (reads, writes, boundary) = reg_uses(t);
            if boundary {
                break; // branch/call/ret/writeback/unknown → conservative KEEP
            }
            let read_x = reads.contains(&d) && token_present(t, 'x', d);
            if read_x {
                break; // high bits observed → the extension is demanded → KEEP
            }
            if writes.contains(&d) {
                drop[i] = true; // D redefined before any x-form read → sxtw is DEAD → DROP
                break;
            }
            // reads D only in w-form, or does not touch D → keep scanning
        }
    }
    let mut out = String::with_capacity(body.len());
    for (i, line) in lines.iter().enumerate() {
        if drop[i] {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// COPY PROPAGATION first (funnel every read to its value's producer, so the x0-scratch
/// copies the emitter inserts become dead), THEN redundant round-trips, THEN dead stores,
/// THEN bitfield fusion (LEVER 1) + redundant sign-extend elim (LEVER 2) + the demand-side w-form
/// sign-extend elim (LEVER 7) on the settled stream.
pub(super) fn peephole_moves(body: &str, exit_live: u64) -> String {
    drop_wform_sxtw(&drop_dead_moves(
        &drop_redundant_uxt(&drop_redundant_sxtw(&fuse_bitfield(&drop_redundant_moves(
            &propagate_copies(body),
        )))),
        exit_live,
    ))
}

// The target `.L` label of a local branch, or None if the line is not one. Branches to a
// local label: `b .L`, `b.<cc> .L`, `cbz/cbnz r, .L`, `tbz/tbnz r, #n, .L` — the label is
// always the final operand. `bl`/`br`/`adr` are deliberately excluded (call / indirect /
// address-of, handled by the caller's safety bail).
pub(super) fn branch_target(t: &str) -> Option<&str> {
    let is_br = t.starts_with("b ")
        || t.starts_with("b.")
        || t.starts_with("cbz ")
        || t.starts_with("cbnz ")
        || t.starts_with("tbz ")
        || t.starts_with("tbnz ");
    if !is_br {
        return None;
    }
    let last = t.rsplit(|c: char| c == ',' || c.is_whitespace()).next()?;
    last.starts_with(".L").then_some(last)
}

/// Machine-level JUMP-THREADING (control-flow identity — the same translation-validation
/// tier as drop_redundant_moves/loads: a pure output rewrite over provably-equal control
/// flow). A block that is nothing but `b .Lt` is a pure forwarder; every branch to its
/// label is retargeted to `.Lt` (chains collapse to a fixpoint), and a forwarder block that
/// is then unreachable — no branch targets it AND control cannot fall into it (the preceding
/// instruction is an unconditional `b`/`ret`) — is deleted. Runs AFTER peephole_moves, so a
/// forwarder whose only content was a φ-destruction copy that coalesced to a dropped
/// self-move is now visible as an empty `label: b` block (the case the IR-level pass cannot
/// see, because coalescing is a backend fact). SAFETY: bails on any body that forms a label
/// ADDRESS (computed goto / jump table — `br xN`, `adr/adrp … .L`, `.quad/.word .L`): there a
/// label is reachable through data this text rewrite does not model.
pub(super) fn thread_asm_branches(body: &str) -> String {
    use std::collections::{HashMap, HashSet};
    let lines: Vec<&str> = body.lines().collect();
    // Pass 0: forwarder map + safety scan. forwarder[L] = T for a block `L:` whose first
    // instruction is `b .T`. Any `.L` reached other than as a branch target ⟹ bail.
    let mut forwarder: HashMap<&str, &str> = HashMap::new();
    for (i, raw) in lines.iter().enumerate() {
        let t = raw.trim();
        if t.starts_with("br ") || t.starts_with("adr ") || t.starts_with("adrp ") && t.contains(".L") {
            return body.to_string();
        }
        if (t.starts_with(".quad") || t.starts_with(".word") || t.starts_with(".xword")) && t.contains(".L") {
            return body.to_string();
        }
        if let Some(lbl) = t.strip_suffix(':').filter(|s| s.starts_with(".L")) {
            // first non-blank line after the label
            let nxt = lines[i + 1..].iter().map(|l| l.trim()).find(|l| !l.is_empty());
            if let Some(nt) = nxt
                && let Some(rest) = nt.strip_prefix("b ")
                && rest.trim().starts_with(".L")
                && rest.trim() != lbl
            // exclude a genuine empty self-loop `for(;;);` (`L: b L`) — it is NOT a
            // forwarder; retargeting/deleting it would destroy the infinite loop.
            {
                forwarder.insert(lbl, rest.trim());
            }
        }
    }
    if forwarder.is_empty() {
        return body.to_string();
    }
    // Resolve each forwarder to its chain's final target (cycle-guarded: a genuine empty
    // self-loop `for(;;);` resolves to itself and is left intact).
    let resolve = |start: &str| -> String {
        let mut cur = start;
        let mut seen: HashSet<&str> = HashSet::new();
        while let Some(&next) = forwarder.get(cur) {
            if next == cur || !seen.insert(cur) {
                break;
            }
            cur = next;
        }
        cur.to_string()
    };
    // Pass 1: retarget every branch whose target is a forwarder to that chain's final label.
    let mut retargeted: Vec<String> = Vec::with_capacity(lines.len());
    for raw in &lines {
        let t = raw.trim();
        if let Some(tgt) = branch_target(t)
            && forwarder.contains_key(tgt)
        {
            let fin = resolve(tgt);
            if fin != tgt {
                // replace only the trailing target token (labels are unique), keep leading tab.
                let lead = &raw[..raw.len() - raw.trim_start().len()];
                retargeted.push(format!("{lead}{}", t.strip_suffix(tgt).unwrap().to_string() + &fin));
                continue;
            }
        }
        retargeted.push((*raw).to_string());
    }
    // Which labels are STILL a branch target after retargeting. A forwarder that stays
    // referenced — e.g. a member of a multi-block cycle (an infinite loop) where resolve()
    // returns a cycle member, so its incoming branches were left in place — must NOT be
    // deleted: dropping its `b` would fall a live predecessor into the wrong next block.
    let mut referenced: HashSet<String> = HashSet::new();
    for raw in &retargeted {
        if let Some(tg) = branch_target(raw.trim()) {
            referenced.insert(tg.to_string());
        }
    }
    // Pass 2: delete a forwarder block (`L:` + its `b`) only when it is BOTH unreferenced by
    // any surviving branch AND fall-through-unreachable (the previous instruction is an
    // unconditional `b`/`ret`) — i.e. genuinely dead. Either condition alone is unsound.
    let mut out = String::with_capacity(body.len());
    let mut prev_unconditional = false; // last real instruction was `b …` / `ret`
    let mut i = 0;
    while i < retargeted.len() {
        let raw = &retargeted[i];
        let t = raw.trim();
        if let Some(lbl) = t.strip_suffix(':').filter(|s| s.starts_with(".L"))
            && forwarder.contains_key(lbl)
            && !referenced.contains(lbl)
            && prev_unconditional
        {
            // dead forwarder: skip the label and its single `b` (the next non-blank line).
            let mut j = i + 1;
            while j < retargeted.len() && retargeted[j].trim().is_empty() {
                j += 1;
            }
            i = j + 1; // drop label..=the `b`
            continue; // prev_unconditional stays true (we removed a `b`, still after one)
        }
        out.push_str(raw);
        out.push('\n');
        if let Some(lbl) = t.strip_suffix(':').filter(|s| s.starts_with(".L")) {
            // A REFERENCED label is a branch target: control can arrive here and fall through
            // into the next block, so that block is reachable regardless of the last insn.
            // An unreferenced label is transparent (a fall-through can only reach it from the
            // preceding instruction) and leaves prev_unconditional unchanged.
            if referenced.contains(lbl) {
                prev_unconditional = false;
            }
        } else if !t.is_empty() && !t.starts_with('.') {
            prev_unconditional = t.starts_with("b ") || t == "ret" || t.starts_with("ret ");
        }
        i += 1;
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// MACHINE-LEVEL COPY PROPAGATION (Tier-A, pressure-FREE). [OPT.md §4 diagnostic:
// the matmul inner k-loop carried 10/39 reg-reg `mov`s — the emitter's x0 funnel
// (`mov x0, xSRC; mov xDST, x0`; `<compute> x0; mov xDST, x0`). These are pure copies:
// removing one LOWERS register pressure, so this is a WIN independent of the pressure
// guard (§2). The funnel also inflated measured SSA pressure, which is what blocked LICM
// from hoisting the invariant `adrp` (the §4 anomaly, cause (c)).]
//
// TRANSFORM. Within a straight-line region, maintain `home[r]` = the register that
// canonically holds r's current value (the ROOT of its copy chain), formed ONLY by a
// full-width `mov x,x`. REWRITE every READ operand `r` to `home[r]`. This funnels each read
// back to the value's producer, so the intermediate scratch copies are read by nothing and
// die (removed by `drop_dead_moves`). No line is deleted here — only read registers renamed
// among provably-equal registers.
//
// SOUNDNESS (same model as drop_redundant_moves — machine translation validation):
//   `home[r]=c` is established only when r and c provably hold the identical 64-bit value
//   (a `mov x,x`), so substituting a read r→c cannot change any value. The model stays sound
//   because every value-changing event severs the stale link:
//     • a real DEF of register D (any first-operand write that is NOT a copy) makes D its own
//       root AND severs every x with home[x]==D — those x still hold the OLD value at x, so
//       their root becomes x, never the redefined D;
//     • a `mov xD,xS` first severs D (its value is being replaced), then sets home[D]=root(S);
//     • any label / branch / call / unknown mnemonic / writeback-addressing FLUSHES the model
//       (we never reason across a boundary).
//   A `w` (32-bit) read substitutes to the `w` form of the root: full-64 equality implies
//   low-32 equality, so it is safe. Only x/w GP registers are tracked; sp/fp/vector operands
//   never match the substitution scan. Re-validated end-to-end by opt-parity (0 DIVERGE) +
//   torture — exactly the net that guards the existing peephole.
// ─────────────────────────────────────────────────────────────────────────────

/// Substitute the single GP register in one operand token by its `home` root (letter and
/// surrounding syntax — brackets, offset — preserved). Immediates, symbols, conditions, FP
/// registers, and `sp` never match (no `x`/`w` + digits), so they pass through untouched.
pub(super) fn sub_reg_token(tok: &str, home: &std::collections::HashMap<u32, u32>) -> String {
    // A relocation / symbol operand (`:lo12:x00`, `:got:`) can hold a C global whose name
    // looks exactly like a register (`x00`) — NEVER a register, so never substitute it. The
    // ':' marks it unambiguously; adrp's bare-symbol operand is skipped at the call site.
    if tok.contains(':') {
        return tok.to_string();
    }
    let b = tok.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        // a register starts with x/w NOT preceded by an alphanumeric (so the 'x' inside a
        // symbol like ".Lx3" or "lo12" is never mistaken for a register).
        if (c == b'x' || c == b'w') && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            // A real GP register is `x0`..`x30` — 1–2 digits, value ≤ 30, NO leading zero.
            // A symbol like `x00` (leading zero) or `x40` (>30) fails this and is left alone.
            let digits = &tok[i + 1..j];
            let canonical = j > i + 1
                && (j == b.len() || !b[j].is_ascii_alphanumeric())
                && (digits.len() == 1 || !digits.starts_with('0'));
            if canonical {
                if let Ok(n) = digits.parse::<u32>() {
                    if n <= 30 {
                        let r = *home.get(&n).unwrap_or(&n);
                        if r != n {
                            return format!("{}{}{}{}", &tok[..i], c as char, r, &tok[j..]);
                        }
                        return tok.to_string();
                    }
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    tok.to_string()
}

/// Copy-propagate read operands to their value's producer within each straight-line region.
pub(super) fn propagate_copies(body: &str) -> String {
    use std::collections::HashMap;
    let mut out = String::with_capacity(body.len());
    let mut home: HashMap<u32, u32> = HashMap::new(); // reg → canonical root holding its value
    // Sever register D: it is being (re)defined, so nothing may read the OLD value through it.
    let sever = |home: &mut HashMap<u32, u32>, d: u32| {
        home.retain(|_, v| *v != d); // copies of the old D value: root becomes themselves
        home.remove(&d); // D is now its own root (holds the fresh value)
    };
    const NO_DEF: &[&str] =
        &["str", "strb", "strh", "stp", "cmp", "cmn", "tst", "fcmp", "ccmp"];
    const DEF_FIRST: &[&str] = &[
        "mov", "movz", "movn", "add", "sub", "mul", "msub", "madd", "neg", "mvn", "and",
        "orr", "eor", "bic", "lsl", "lsr", "asr", "sdiv", "udiv", "sxtw", "sxth", "sxtb",
        "uxtw", "uxth", "uxtb", "cset", "csel", "csinc", "cinc", "adrp", "ldr", "ldrb",
        "ldrh", "ldrsw", "ldrsb", "ldrsh", "fmov", "scvtf", "ucvtf", "fcvt", "fadd", "fsub",
        "fmul", "fdiv", "fneg", "fcvtzs", "fcvtzu", "sxtl",
    ];
    // Boundary mnemonics that still READ a register before the region ends (fold the read,
    // then flush). Plain b/bl/ret carry no GP read we propagate.
    const READ_THEN_FLUSH: &[&str] = &["cbz", "cbnz", "tbz", "tbnz", "br", "blr"];
    let gp = |tok: &str| -> Option<u32> {
        let t = tok.trim().trim_start_matches('[').trim_end_matches(']');
        gpreg(t)
    };
    for line in body.lines() {
        let t = line.trim();
        // Label FIRST — an emitted label `.Lir_x:` both starts with '.' and ends with ':';
        // it is a basic-block boundary and MUST flush before the directive fast-path.
        if t.ends_with(':') {
            home.clear(); // label = basic-block boundary
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.is_empty() || t.starts_with('.') {
            out.push_str(line); // blank / directive — no register effect
            out.push('\n');
            continue;
        }
        // Writeback / pre-post-index mutates the base implicitly ⟹ boundary (never model it).
        if t.contains('!') || t.contains("],") {
            home.clear();
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mn = t.split(|c: char| c.is_whitespace()).next().unwrap_or("");
        let operands = t[mn.len()..].trim_start();
        let toks: Vec<&str> = operands.split(',').collect();
        // Rewrite the chosen READ tokens; leave the destination position and update `home`.
        let emit = |toks: &[String]| -> String { format!("\t{} {}", mn, toks.join(",")) };
        let sub_all = |toks: &[&str], home: &HashMap<u32, u32>| -> Vec<String> {
            toks.iter().map(|tk| sub_reg_token(tk, home)).collect()
        };
        if mn.starts_with("b.") || matches!(mn, "b" | "bl" | "ret") {
            out.push_str(line);
            out.push('\n');
            home.clear();
            continue;
        }
        if READ_THEN_FLUSH.contains(&mn) {
            let nt = sub_all(&toks, &home); // the register operands are reads
            out.push_str(&emit(&nt));
            out.push('\n');
            home.clear();
            continue;
        }
        if NO_DEF.contains(&mn) {
            let nt = sub_all(&toks, &home); // stores/compares: every operand is a read
            out.push_str(&emit(&nt));
            out.push('\n');
            continue;
        }
        if mn == "ldp" {
            // token[0], token[1] are destinations (leave); the rest are address reads.
            let n = toks.len().min(2);
            let mut nt: Vec<String> = toks[..n].iter().map(|s| s.to_string()).collect();
            nt.extend(sub_all(&toks[n..], &home));
            out.push_str(&emit(&nt));
            out.push('\n');
            for tk in &toks[..n] {
                if let Some(d) = gp(tk) {
                    sever(&mut home, d);
                }
            }
            continue;
        }
        if mn == "movk" {
            // token[0] is a read+write merge (leave it — substituting the accumulator dst is
            // wrong); the rest are reads. The partial write severs the dst.
            let mut nt = vec![toks[0].to_string()];
            nt.extend(sub_all(&toks[1..], &home));
            out.push_str(&emit(&nt));
            out.push('\n');
            if let Some(d) = gp(toks[0]) {
                sever(&mut home, d);
            }
            continue;
        }
        if mn == "adrp" {
            // `adrp xD, SYM` — the second operand is a BARE symbol (a global named `x5` would
            // masquerade as a register); never substitute it. Just record the fresh dst.
            out.push_str(line);
            out.push('\n');
            if let Some(d) = toks.first().and_then(|s| gp(s)) {
                sever(&mut home, d);
            }
            continue;
        }
        if DEF_FIRST.contains(&mn) {
            let dst = toks.first().and_then(|s| gp(s));
            let mut nt = vec![toks[0].to_string()]; // destination stays
            nt.extend(sub_all(&toks[1..], &home));
            out.push_str(&emit(&nt));
            out.push('\n');
            // Update the model. A FULL-WIDTH `mov x,x` is a 64-bit COPY: D takes S's root.
            // Any other GP write — including a narrow `mov w,w`, which zero-extends the low
            // 32 bits and so produces a DIFFERENT 64-bit value (the bswap-1 truncation bug) —
            // gives D a fresh value and records NO equivalence. `parse_mov_xx` accepts only
            // `mov x,x`, exactly the copies drop_redundant_moves already trusts. A float/vector
            // dst (gp() == None) touches no GP reg.
            if let Some(d) = dst {
                // Resolve S's root BEFORE severing D — sever may drop entries that point at D
                // (e.g. `mov x0,x24` when x24 was itself a copy of x0), and we must read the
                // root as it stood before this instruction. rs==d ⟹ the copy is redundant
                // (D already holds its own value) ⟹ record nothing, keeping D its own root.
                let rs = parse_mov_xx(t).map(|(_, s)| *home.get(&s).unwrap_or(&s));
                sever(&mut home, d);
                if let Some(rs) = rs {
                    if rs != d {
                        home.insert(d, rs);
                    }
                }
            }
            continue;
        }
        // Unknown mnemonic ⟹ boundary (never mis-model).
        out.push_str(line);
        out.push('\n');
        home.clear();
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// B4 — LOAD/STORE PAIR FORMATION (`ldp`/`stp`). [Side-I structural theorem —
// OPT.md §5 (B4) / §6 Tier-5 #23.]
//
// THEOREM. Two ADJACENT same-class accesses to `[base,#o]` and `[base,#o+sz]` (sz the
// access width) have the SAME memory effect as one pair op `ldp/stp rA,rB,[base,#o]`
// — the paired form transfers exactly the two words at the two addresses. Merging two
// consecutive lines introduces no reordering (nothing executes between them) and the
// disjoint word addresses make the store order immaterial. Emitted-`.s`-level
// (machine translation-validation via opt-parity + torture), NOT IR `equiv` — it is a
// pure output rewrite the backend model already trusts (like the move peephole).
//
// IMPROVEMENT (static, no race): the memory-op count HALVES on every run of ≥2
// adjacent same-base accesses — the callee-save slab (every non-leaf function),
// struct copies, HFA/param spills.
//
// SOUNDNESS FENCES (each a constrained-unpredictable/aliasing hazard avoided):
//   • same load/store direction, same register class (x/w/d/s), same base symbol;
//   • the second offset is EXACTLY first + sz, and `o` is a legal scaled imm7
//     (multiple of sz, o/sz ∈ [-64,63]);
//   • the base register is not one of the two transferred GP/W registers (its value
//     must survive to address the pair — a `ldr xBase,[xBase,..]` mustn't be paired);
//   • `ldp` forbids the two destinations being identical.
// Only plain `ldr`/`str` (full-width, non-extending) parse; `ldrb`/`ldrsw`/`q`-regs
// are skipped (different scaling / no pairing form).
// ─────────────────────────────────────────────────────────────────────────────

/// Parse `str|ldr {x|w|d|s}N, [<base>[, #<off>]]` → (is_load, class byte, reg#, base, off).
pub(super) fn parse_ldst(line: &str) -> Option<(bool, u8, u32, String, i64)> {
    let t = line.trim();
    let (is_load, rest) = if let Some(r) = t.strip_prefix("ldr ") {
        (true, r)
    } else if let Some(r) = t.strip_prefix("str ") {
        (false, r)
    } else {
        return None;
    };
    let (reg_s, mem) = rest.split_once(", [")?;
    let mem = mem.strip_suffix(']')?;
    let cls = reg_s.as_bytes().first().copied()?;
    if !matches!(cls, b'x' | b'w' | b'd' | b's') {
        return None;
    }
    let reg: u32 = reg_s.get(1..)?.parse().ok()?;
    let (base, off) = match mem.split_once(", #") {
        Some((b, o)) => (b.to_string(), o.parse::<i64>().ok()?),
        None => (mem.to_string(), 0),
    };
    Some((is_load, cls, reg, base, off))
}

/// Fuse consecutive adjacent accesses into `ldp`/`stp`. Runs AFTER the move peephole
/// (which may delete lines between two accesses, exposing the adjacency).
pub(super) fn pair_ldst(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < lines.len() {
        if i + 1 < lines.len() {
            if let (Some((la, ca, ra, ba, oa)), Some((lb, cb, rb, bb, ob))) =
                (parse_ldst(lines[i]), parse_ldst(lines[i + 1]))
            {
                let sz: i64 = if ca == b'x' || ca == b'd' { 8 } else { 4 };
                let scaled = oa % sz == 0 && (oa / sz) >= -64 && (oa / sz) <= 63;
                // an x/w transfer register aliases the 64-bit x base; d/s live in the
                // separate FP file and never clash.
                let base_clash = matches!(ca, b'x' | b'w')
                    && (ba == format!("x{ra}") || ba == format!("x{rb}"));
                if la == lb
                    && ca == cb
                    && ba == bb
                    && ob == oa + sz
                    && scaled
                    && !base_clash
                    && !(la && ra == rb) // ldp destinations must differ
                {
                    let mn = if la { "ldp" } else { "stp" };
                    let c = ca as char;
                    let addr = if oa == 0 { format!("[{ba}]") } else { format!("[{ba}, #{oa}]") };
                    _ = writeln!(out, "\t{mn} {c}{ra}, {c}{rb}, {addr}");
                    i += 2;
                    continue;
                }
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// POST-INDEX ADDRESSING (loop-IV walk — Tier-1 #5). A bare-base access `mem Rt, [xP]` followed
/// later in the same straight-line region by `add xP, xP, #k` (0<k≤255, the post-index simm9
/// range, a conservative subset of the true simm9 −256..255 — the negative/`sub` half is a Law-4
/// coverage residual, not a bug) — with xP neither read nor written on any line between — folds
/// into `mem Rt, [xP], #k` and deletes the add. THEOREM: post-index means "access [xP], THEN
/// xP += k"; with no read or write of xP in the gap, hoisting the increment up to the access
/// changes no observation — every xP consumer at/after the original add still reads xP+k, and none
/// exists before it. An intervening bare `[xP]` access itself READS xP, so it aborts the scan:
/// only the access immediately preceding the increment (in xP-liveness) is fused. Excludes ANY
/// access whose Rt aliases the base xP (`mem xP,[xP],#k`): ARMv8-A makes base-writeback with the
/// transfer reg == base reg (base ≠ 31) CONSTRAINED UNPREDICTABLE for loads AND stores alike (a
/// store there may write an UNKNOWN value, not the pre-increment one), so it is never folded in
/// either direction. A region boundary — label, branch,
/// call, ret, writeback/`],` line, or unknown mnemonic (reg_uses.boundary) — ends the scan.
/// Machine translation-validation (opt-parity); one fewer insn per fused loop step (size + a hot
/// per-iteration cycle). Runs after peephole_moves exposes the clean increment.
pub(super) fn post_index(body: &str) -> String {
    // parse `<ldr*|str*> <w|x>Rt, [xP]` (bare base, no offset) → (is_load, rt, base).
    fn parse_bare(t: &str) -> Option<(bool, u32, u32)> {
        let mn = t.split(|c: char| c.is_whitespace()).next()?;
        let is_load = match mn {
            "ldr" | "ldrb" | "ldrh" | "ldrsw" | "ldrsb" | "ldrsh" => true,
            "str" | "strb" | "strh" => false,
            _ => return None,
        };
        let rest = t[mn.len()..].trim_start();
        let (reg_s, mem) = rest.split_once(", [")?;
        let base = mem.strip_suffix(']')?; // bare only — a `, #off]` or `], #k` keeps the ']'/','
        if base.contains([',', '!', ' ']) {
            return None;
        }
        let rt = gpreg(reg_s)?;
        let base = xreg(base)?;
        Some((is_load, rt, base))
    }
    // parse `add xP, xP, #k` → (dst, src, k).
    fn parse_add_imm(t: &str) -> Option<(u32, u32, i64)> {
        let rest = t.strip_prefix("add ")?;
        let mut it = rest.split(", ");
        let d = xreg(it.next()?)?;
        let s = xreg(it.next()?)?;
        let k = it.next()?.strip_prefix('#')?.parse().ok()?;
        if it.next().is_some() {
            return None;
        }
        Some((d, s, k))
    }
    let lines: Vec<&str> = body.lines().collect();
    let mut post: Vec<Option<i64>> = vec![None; lines.len()]; // access line → post-inc k
    let mut drop = vec![false; lines.len()]; // add line to delete
    for (i, li) in lines.iter().enumerate() {
        let Some((_is_load, rt, base)) = parse_bare(li.trim()) else { continue };
        if rt == base {
            // ARMv8-A: base-register writeback with the transfer reg == base reg (and base ≠ 31)
            // is CONSTRAINED UNPREDICTABLE for BOTH loads AND stores (a store may write an UNKNOWN
            // value, not the pre-increment one) — never fold `mem xP,[xP],#k` regardless of dir.
            continue;
        }
        for (off, lj) in lines[i + 1..].iter().enumerate() {
            let t = lj.trim();
            // Label FIRST — a `.Lir_*:` label both starts with '.' and ends with ':', and is a
            // region boundary (a merge point may be reached from other predecessors); the
            // directive-skip below must not swallow it, or the scan crosses a block boundary and
            // deletes a SHARED increment (ssad-run: the else-branch loses its pointer advance).
            if t.ends_with(':') {
                break;
            }
            if t.is_empty() || t.starts_with('.') {
                continue; // blank / directive (.cfi_*, .p2align, …) — no register effect
            }
            if let Some((d, s, k)) = parse_add_imm(t) {
                if d == base && s == base && k > 0 && k <= 255 {
                    post[i] = Some(k);
                    drop[i + 1 + off] = true;
                    break;
                }
            }
            let (reads, writes, boundary) = reg_uses(t);
            if boundary || reads.contains(&base) || writes.contains(&base) {
                break; // xP touched (or an opaque line) before the increment
            }
        }
    }
    let mut out = String::with_capacity(body.len());
    for (i, li) in lines.iter().enumerate() {
        if drop[i] {
            continue;
        }
        if let Some(k) = post[i] {
            // rewrite `mem Rt, [xP]` → `mem Rt, [xP], #k` (the ']' stays; the offset follows it)
            _ = writeln!(out, "{}, #{k}", li.trim_end());
        } else {
            out.push_str(li);
            out.push('\n');
        }
    }
    out
}

/// CBZ/CBNZ FUSION (Tier-1 #6 — compare-and-branch against zero). An adjacent `cmp Rn, #0` /
/// `b.eq|b.ne LABEL` pair collapses to `cbz|cbnz Rn, LABEL`, deleting the cmp. THEOREM: `cbz Rn,L`
/// branches iff Rn==0 (exactly `cmp Rn,#0; b.eq L`); `cbnz` iff Rn≠0 (`b.ne`). Rn's width (w/x) is
/// preserved; the branch range (imm19, ±1 MB) is identical to `b.cc`, so no target ever falls out
/// of reach. SOUNDNESS obligation — the cmp's NZCV flags must be dead on the fall-through past the
/// branch (cbz sets no flags): scan forward from the branch; a flag-WRITER or a control boundary
/// (label / b / bl / ret / cbz…) first ⟹ flags dead ⟹ SAFE; a flag-READER first (a second `b.cc`,
/// cset, csel, adc, ccmp…) ⟹ the cmp is still needed ⟹ DECLINE. The scan inspects ONLY the
/// fall-through successor, NOT the taken-branch target — sound under a standing zcc invariant:
/// **NZCV is never live-IN to a basic block.** zcc's SSA lowering emits every flag producer
/// (cmp/subs/…) and its consumer (b.cc/cset/csel) within one block, producer-before-consumer, so
/// no block reads NZCV as a live-in; arriving at `label` via a flag-clearing `cbz` therefore
/// observes nothing the original `b.eq` would have preserved. (A general assembler WITHOUT this
/// invariant could break on `cmp;b.eq .L; …flag-writer…; .L: cset` — not emittable by zcc.)
/// Machine translation-validation
/// (opt-parity); one fewer insn per branch (size + a hot compare-branch cycle). This is the
/// bare-truth-branch case the IR cbr-fusion misses (it fires only when the tested value is itself
/// a relational compare; here Rn is a plain integer — null-checks, `if(x)`, `while(n)`).
pub(super) fn cbz_fuse(body: &str) -> String {
    fn mnem(t: &str) -> &str {
        t.split(|c: char| c.is_whitespace() || c == '.').next().unwrap_or("")
    }
    // NZCV consumers (must run BEFORE the writer test — ccmp both reads and writes).
    fn flag_reads(t: &str) -> bool {
        if t.starts_with("b.") {
            return true; // a conditional branch reads NZCV
        }
        matches!(mnem(t),
            "cset" | "csetm" | "csel" | "csinc" | "csinv" | "csneg" | "cinc" | "cinv"
            | "cneg" | "adc" | "adcs" | "sbc" | "sbcs" | "ccmp" | "ccmn")
    }
    fn flag_writes(t: &str) -> bool {
        matches!(mnem(t),
            "cmp" | "cmn" | "tst" | "ccmp" | "ccmn" | "adds" | "subs" | "ands" | "bics"
            | "adcs" | "sbcs" | "negs" | "fcmp" | "fcmpe")
    }
    // control leaves this straight-line region (flags become don't-care past here).
    fn boundary(t: &str) -> bool {
        t.ends_with(':')
            || matches!(mnem(t), "b" | "br" | "bl" | "blr" | "ret" | "cbz" | "cbnz" | "tbz" | "tbnz")
    }
    let lines: Vec<&str> = body.lines().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        // cmp Rn, #0  (Rn a w/x register, immediate exactly 0)
        if let Some(reg) = t.strip_prefix("cmp ").and_then(|r| r.strip_suffix(", #0")) {
            if (reg.starts_with('w') || reg.starts_with('x')) && !reg.contains([',', ' ', '[']) {
                if let Some(br) = lines.get(i + 1).map(|l| l.trim()) {
                    let cbop = if let Some(l) = br.strip_prefix("b.eq ") {
                        Some(("cbz", l))
                    } else if let Some(l) = br.strip_prefix("b.ne ") {
                        Some(("cbnz", l))
                    } else {
                        None
                    };
                    if let Some((op, label)) = cbop {
                        // flags dead on the fall-through past the branch?
                        let mut safe = true;
                        for lj in &lines[i + 2..] {
                            let u = lj.trim();
                            if u.is_empty() || u.starts_with('.') && !u.ends_with(':') {
                                continue;
                            }
                            if flag_reads(u) {
                                safe = false;
                                break;
                            }
                            if flag_writes(u) || boundary(u) {
                                break; // flags overwritten / region left ⟹ dead ⟹ safe
                            }
                        }
                        if safe {
                            _ = writeln!(out, "\t{op} {reg}, {label}");
                            i += 2; // consumed cmp + branch
                            continue;
                        }
                    }
                }
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// GLOBAL DEAD-MOVE ELIMINATION (CFG live-out backward dataflow). [Phase 1.4 — supersedes the
/// region-local scan.] A `mov xD,xS` (and a dead in-place `sxtw xD,wD`) is deleted when xD is not
/// live-out at that point, computed over the WHOLE function CFG instead of reset to FULL at every
/// label/branch. The out-of-SSA φ-destruction copies the coalescer leaves in every loop header —
/// `mov xD,xS` overwritten in the body before any read, but ACROSS a block boundary — become
/// visibly dead only with cross-block liveness; the old local scan could never see it.
///
/// Soundness (translation-validation): a line is removed ONLY when its destination register is
/// dead on every CFG path (bit 0 in the fix-pointed live-out) — nothing observes the value, so
/// ⟦body⟧ = ⟦body∖line⟧. Every uncertainty WIDENS liveness, never narrows it:
///   • a call/unknown line ⟹ its whole block is OPAQUE (live-in = FULL): nothing dropped there;
///   • an unresolved branch target (label outside this body) ⟹ that block's live-out = FULL;
///   • a trailing block with no `ret` and no successor ⟹ live-out = FULL;
///   • `ret` seeds live-out with exactly the caller-visible return regs (`ret_gp` = 0 for void/
///     float/HFA, 1 for a scalar in x0, 2 for a 128-bit x0:x1) — callee-saved/fp/lr liveness
///     rides in on the epilogue's own reads/writes (its restore-`ldp` kills them from above).
/// Pre/post-index writeback (`[xN],#k` / `[xN,#k]!`) is MODELLED (base read+written, Rt read for
/// a store / written for a load) instead of forcing a boundary, so the post-index loads in hot
/// loops no longer poison the analysis (their block stays analyzable).
pub(super) fn drop_dead_moves(body: &str, exit_live: u64) -> String {
    use std::collections::HashMap;
    // Tracked register space, one u64 bit each: GP x0..x30 ↔ bits 0..30; FP/SIMD d0..d31 (also
    // s/h/b/q/v aliases — same physical reg, same slot) ↔ bits 32..63. sp/x31/xzr untracked.
    const GPN: u32 = 31;
    let gp_full: u64 = (1u64 << GPN) - 1;
    let fp_full: u64 = ((1u64 << 32) - 1) << 32;
    let full: u64 = gp_full | fp_full;
    let exit_live = exit_live & full;
    let lines: Vec<&str> = body.lines().collect();

    // FP/SIMD register id (d/s/h/b/q/v N → 32+N), or None for a GP reg / immediate / memory /
    // label token. `parse::<u32>` on the tail guards against symbol tokens (`sp`, `sym`, `:got:`).
    let fp_id = |tok: &str| -> Option<u32> {
        let tok = tok.trim().trim_start_matches('[').trim_end_matches('!').trim_end_matches(']');
        let tail = ['d', 's', 'h', 'b', 'q', 'v'].iter().find_map(|&c| tok.strip_prefix(c))?;
        tail.split('.').next()?.parse::<u32>().ok().filter(|&r| r < 32).map(|r| 32 + r)
    };
    // reads/writes as u64 bitmasks, or None ⟹ opaque (control transfer / unmodelable). The GP
    // side is `reg_uses` (which returns None-slots for FP operands); the FP side is supplemented
    // here by the same def-first / store / two-dest classification. Writeback addressing (which
    // reg_uses rejects as a boundary) is modelled in the second arm, GP base + GP-or-FP Rt.
    let live_rw = |t: &str| -> Option<(u64, u64)> {
        let gp_mask = |v: Vec<u32>| -> u64 { v.into_iter().filter(|&x| x < GPN).fold(0, |m, x| m | (1 << x)) };
        let mn = t.split_whitespace().next().unwrap_or("");
        if !(t.contains("],") || t.contains("]!")) {
            let (r, w, boundary) = reg_uses(t);
            if boundary {
                return None;
            }
            let (mut reads, mut writes) = (gp_mask(r), gp_mask(w));
            let no_def = mn.starts_with("st") || mn == "fcmp" || mn == "fcmpe" || mn == "fccmp";
            let two_dest = mn == "ldp";
            for (idx, tk) in t[mn.len()..].trim_start().split(',').enumerate() {
                if let Some(fid) = fp_id(tk) {
                    let is_dest = !no_def && if two_dest { idx < 2 } else { idx == 0 };
                    if is_dest {
                        writes |= 1 << fid;
                    } else {
                        reads |= 1 << fid;
                    }
                }
            }
            return Some((reads, writes));
        }
        let toks: Vec<&str> = t[mn.len()..].trim_start().split(',').collect();
        let gp_slot = |tok: &str| -> Option<u32> {
            let tok = tok.trim().trim_start_matches('[').trim_end_matches('!').trim_end_matches(']');
            gpreg(tok).filter(|&r| r < GPN)
        };
        let base = toks.iter().find(|x| x.contains('[')).and_then(|x| gp_slot(x));
        let is_store = mn.starts_with("st");
        let (mut reads, mut writes) = (0u64, 0u64);
        if let Some(b) = base {
            reads |= 1 << b; // base += imm : read then written by the writeback
            writes |= 1 << b;
        }
        for tk in &toks {
            if tk.contains('[') {
                continue;
            }
            if let Some(r) = gp_slot(tk).or_else(|| fp_id(tk)) {
                if is_store {
                    reads |= 1 << r;
                } else {
                    writes |= 1 << r;
                }
            }
        }
        Some((reads, writes))
    };

    // A pure single-destination def safe to delete when its dest is dead: a reg-reg `mov`, any
    // `fmov` (reg copy / d↔x bridge / #imm materialize), or an `sxtw` widen (D==S or D≠S). Returns
    // (dest-id, reads-mask). Loads/arith/etc are NOT here — only side-effect-free single-writes.
    let pure_def = |t: &str, r: u64, w: u64| -> Option<(u32, u64)> {
        if let Some((d, s)) = parse_mov_xx(t) {
            if d < GPN && s < GPN {
                return Some((d, 1 << s));
            }
        }
        let mn = t.split_whitespace().next().unwrap_or("");
        if (mn == "fmov" || mn == "sxtw") && w.count_ones() == 1 {
            return Some((w.trailing_zeros(), r));
        }
        None
    };

    // Per-line kind. Labels/targets borrow `body` (via `lines`).
    enum K<'a> {
        Skip,               // directive / blank — no liveness effect
        Label(&'a str),     // block header (name without ':')
        Jump(&'a str),      // b <target>
        Cond(u64, &'a str), // b.cc / cbz / tbz : (reads-mask, target); + fallthrough
        Exit(u64),          // ret / br : (reads-mask); no in-body successor
        Opaque,             // bl / blr / unmodelable ⟹ block opaque
        Pure(u32, u64),     // mov/fmov/sxtw : (dest-id, reads-mask) — drop candidate
        Op(u64, u64),       // everything else : (reads, writes)
    }
    let first_reg = |t: &str, mn: &str| -> u64 {
        let f = t[mn.len()..].trim_start().split(',').next().unwrap_or("").trim();
        gpreg(f)
            .filter(|&r| r < GPN).map(|r| 1u64 << r).unwrap_or(0)
    };
    fn last_tok(s: &str) -> &str {
        s.rsplit([',', ' ', '\t']).next().unwrap_or("").trim()
    }
    let kinds: Vec<K> = lines
        .iter()
        .map(|line| {
            let t = line.trim();
            if t.is_empty() || (t.starts_with('.') && !t.ends_with(':')) {
                return K::Skip;
            }
            if t.ends_with(':') {
                return K::Label(&t[..t.len() - 1]);
            }
            let mn = t.split_whitespace().next().unwrap_or("");
            if mn == "b" {
                return K::Jump(last_tok(t));
            }
            if mn.starts_with("b.") {
                return K::Cond(0, last_tok(t));
            }
            if mn == "cbz" || mn == "cbnz" || mn == "tbz" || mn == "tbnz" {
                return K::Cond(first_reg(t, mn), last_tok(t));
            }
            if mn == "ret" {
                return K::Exit(0);
            }
            if mn == "br" {
                return K::Exit(first_reg(t, mn));
            }
            if mn == "bl" || mn == "blr" {
                return K::Opaque;
            }
            match live_rw(t) {
                None => K::Opaque,
                Some((r, w)) => match pure_def(t, r, w) {
                    Some((d, reads)) => K::Pure(d, reads),
                    None => K::Op(r, w),
                },
            }
        })
        .collect();

    // Build blocks: a new block begins at the first meaningful line, at every label, and after
    // every terminator (Jump/Cond/Exit). `bl` does NOT split a block — it only marks it opaque.
    let mut blocks: Vec<Vec<usize>> = Vec::new();
    let mut prev_term = true; // force the first meaningful line to open a block
    for (i, k) in kinds.iter().enumerate() {
        if matches!(k, K::Skip) {
            continue;
        }
        if prev_term || matches!(k, K::Label(_)) || blocks.is_empty() {
            blocks.push(Vec::new());
        }
        let b = blocks.len() - 1;
        blocks[b].push(i);
        prev_term = matches!(k, K::Jump(_) | K::Cond(..) | K::Exit(_));
    }
    let nb = blocks.len();
    let mut label_map: HashMap<&str, usize> = HashMap::new();
    for (b, mem) in blocks.iter().enumerate() {
        if let Some(&f) = mem.first() {
            if let K::Label(name) = kinds[f] {
                label_map.insert(name, b);
            }
        }
    }
    // Static successor/exit/full/opaque info per block.
    struct BInfo {
        succ: Vec<usize>,
        exit: bool,
        full: bool, // live-out unconditionally FULL (unresolved target / trailing fall-through)
        opaque: bool,
    }
    let info: Vec<BInfo> = blocks
        .iter()
        .enumerate()
        .map(|(b, mem)| {
            let opaque = mem.iter().any(|&i| matches!(kinds[i], K::Opaque));
            let fallthrough = |b: usize| -> Option<usize> { (b + 1 < nb).then_some(b + 1) };
            let (mut succ, mut exit, mut full) = (Vec::new(), false, false);
            match kinds[*mem.last().unwrap()] {
                K::Exit(_) => exit = true,
                K::Jump(tg) => match label_map.get(tg) {
                    Some(&s) => succ.push(s),
                    None => full = true,
                },
                K::Cond(_, tg) => {
                    match label_map.get(tg) {
                        Some(&s) => succ.push(s),
                        None => full = true,
                    }
                    match fallthrough(b) {
                        Some(s) => succ.push(s),
                        None => full = true,
                    }
                }
                _ => match fallthrough(b) {
                    Some(s) => succ.push(s),
                    None => full = true, // trailing block, no terminator ⟹ conservative
                },
            }
            BInfo { succ, exit, full, opaque }
        })
        .collect();

    // Backward transfer of one line over the live-after set `cur`.
    let step = |i: usize, cur: u64| -> u64 {
        match kinds[i] {
            K::Cond(r, _) | K::Exit(r) => cur | r,
            K::Pure(d, reads) => (cur & !(1 << d)) | reads, // single-dest: writes d, reads `reads`
            K::Op(r, w) => (cur & !w) | r,
            _ => cur, // Skip / Label / Jump / Opaque : no reg effect (opaque block handled apart)
        }
    };
    let live_out = |b: usize, live_in: &[u64]| -> u64 {
        let mut lo = if info[b].full { full } else { 0 };
        if info[b].exit {
            lo |= exit_live;
        }
        for &s in &info[b].succ {
            lo |= live_in[s];
        }
        lo
    };

    // Fixpoint for live-in per block.
    let mut live_in = vec![0u64; nb];
    loop {
        let mut changed = false;
        for b in (0..nb).rev() {
            let li = if info[b].opaque {
                full
            } else {
                blocks[b].iter().rev().fold(live_out(b, &live_in), |cur, &i| step(i, cur))
            };
            if live_in[b] != li {
                live_in[b] = li;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Drop pass: re-scan each non-opaque block backward from its final live-out, deleting any
    // pure single-dest def (mov / fmov / sxtw) whose destination is dead at that point.
    let mut drop = vec![false; lines.len()];
    for b in 0..nb {
        if info[b].opaque {
            continue;
        }
        let mut cur = live_out(b, &live_in);
        for &i in blocks[b].iter().rev() {
            match kinds[i] {
                K::Pure(d, _) if (cur & (1 << d)) == 0 => drop[i] = true,
                _ => cur = step(i, cur),
            }
        }
    }
    let mut out = String::with_capacity(body.len());
    for (i, line) in lines.iter().enumerate() {
        if !drop[i] {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Redundant round-trip elimination via per-region value-equivalence (see the block comment).
pub(super) fn drop_redundant_moves(body: &str) -> String {
    use std::collections::HashMap;
    let mut out = String::with_capacity(body.len());
    let mut eq: HashMap<u32, u64> = HashMap::new(); // register slot → value id
    let mut next: u64 = 0;
    // Recognized destination-writing mnemonics (dst = first register operand). Everything
    // NOT here and NOT a store/compare/branch flushes the model (conservative = safe).
    const DEF_FIRST: &[&str] = &[
        "mov", "movk", "movz", "movn", "add", "sub", "mul", "msub", "madd", "neg", "mvn",
        "and", "orr", "eor", "bic", "lsl", "lsr", "asr", "sdiv", "udiv", "sxtw", "sxth",
        "sxtb", "uxtw", "uxth", "uxtb", "cset", "csel", "csinc", "cinc", "adrp", "ldr",
        "ldrb", "ldrh", "ldrsw", "ldrsb", "ldrsh", "fmov", "scvtf", "ucvtf", "fcvt", "fadd",
        "fsub", "fmul", "fdiv", "fneg", "fcvtzs", "fcvtzu", "sxtl",
    ];
    const NO_DEF: &[&str] =
        &["str", "strb", "strh", "stp", "cmp", "cmn", "tst", "fcmp", "ccmp"];
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            out.push('\n');
            continue;
        }
        if t.ends_with(':') {
            eq.clear(); // label = basic-block boundary
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.starts_with('.') {
            out.push_str(line); // directive — touches no register
            out.push('\n');
            continue;
        }
        let mn = t.split(|c: char| c.is_whitespace()).next().unwrap_or("");
        let operands = t[mn.len()..].trim_start();
        // The one rewrite: drop a mov xD,xS proven redundant; else record D≡S.
        if let Some((d, s)) = parse_mov_xx(t) {
            if d == s {
                continue; // `mov xN,xN` = orr xN,xzr,xN — an unconditional no-op (any value, no flags)
            }
            match (eq.get(&d), eq.get(&s)) {
                (Some(a), Some(b)) if a == b => continue, // D already ≡ S → DROP
                _ => {
                    let sid = *eq.entry(s).or_insert_with(|| {
                        next += 1;
                        next
                    });
                    eq.insert(d, sid);
                    out.push_str(line);
                    out.push('\n');
                    continue;
                }
            }
        }
        if mn == "ldp" {
            // two destinations = the first two register operands.
            let mut regs = operands.split(',');
            for _ in 0..2 {
                if let Some(r) = regs.next().and_then(|tok| {
                    let tok = tok.trim();
                    gpreg(tok)
                }) {
                    next += 1;
                    eq.insert(r, next);
                }
            }
        } else if NO_DEF.contains(&mn) {
            // no register destination — model unchanged.
        } else if DEF_FIRST.contains(&mn) {
            if let Some(r) = first_reg_slot(operands) {
                next += 1;
                eq.insert(r, next); // destination takes a fresh value ⟹ breaks stale ≡
            }
        } else {
            eq.clear(); // unrecognized (incl. b/bl/br/ret/cbz/…) ⟹ flush = safe
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// REDUNDANT-LOAD-AFTER-STORE elimination (store→load identity). [MEASURED lever:
// sqlite3.c carries 166,019 adjacent `str xN,[sp,#m]; ldr xN,[sp,#m]` pairs = 52% of
// ALL 319k loads — the value-contract materializes each temp to/from its frame slot per
// use (O0 style), and the register allocator's spill code round-trips through the slot;
// neither is visible to the IR-level load-elim (B2, §3), so they survive to the stream.
// This is the machine-level case of a veteran-compiler pass: GCC `postreload-cse`/
// `peephole2`, LLVM MachineCSE + store→load forwarding, QBE load.c.]
//
// THEOREM (store→load identity). State Σ=⟨ρ registers, μ memory⟩:
//   ⟦str xN,[m]⟧ : μ' = μ[addr(m) ↦ ρ(xN)], ρ unchanged
//   ⟦ldr xN,[m]⟧ : ρ' = ρ[xN ↦ μ(addr(m))], μ unchanged
// When the two are ADJACENT (nothing executes between ⟹ ρ(xN), the base register of m,
// and μ(addr(m)) are unperturbed), after `str` we have μ(addr(m)) = ρ(xN); then `ldr`
// assigns ρ(xN) := μ(addr(m)) = ρ(xN) — the IDENTITY on ρ. So ⟦str;ldr⟧ = ⟦str⟧ and
// deleting the `ldr` preserves ⟦·⟧. ∎  Full 64-bit `x` form only: a `w`-form reload
// zero-extends into the high 32 bits, an OBSERVABLE change unless those bits are already
// dead — that proof is not local, so `w` pairs are left untouched (there are none here).
//
// TWO HYPOTHESES OF THE THEOREM, both discharged by construction:
//   (1) NON-VOLATILE m — a volatile access must not be elided (C11 6.7.3/7: both the store
//       and the load are required observable side effects). The base is restricted to `[sp,`
//       (frame slots): sp is never a user pointer, so a frame slot is a compiler-generated
//       stack temp — never volatile, never aliased. (Measured: all 166,019 pairs are `[sp,`.)
//   (2) ADJACENCY with no control entry — a LABEL between the pair is an entry point at which
//       execution may reach the `ldr` WITHOUT having run the `str`, so μ(addr(m)) ≠ ρ(xN)
//       there. A label FLUSHES the pending store. Blank/directive lines carry no execution
//       and no entry, so the pair survives across them; any other instruction flushes (safe).
// SOUND like the move passes: the ONLY rewrite is deleting a proven-identity load; every
// value/memory-changing event drops the pending store. Re-validated by opt-parity (0 DIVERGE).

/// Parse a 64-bit `ldr`/`str xN, [sp, #k]` frame-slot access → (is_load, N, mem-text).
/// None for any other mnemonic, a `w`-form, a non-`[sp,` base, or writeback/index addressing.
pub(super) fn parse_frame_ldst(t: &str) -> Option<(bool, u32, &str)> {
    let (mn, rest) = t.split_once(char::is_whitespace)?;
    let is_load = match mn {
        "ldr" => true,
        "str" => false,
        _ => return None,
    };
    let rest = rest.trim_start();
    // writeback (`[sp,#k]!`) / post-index (`[sp],#k`) mutate sp — not a pure load/store.
    if rest.contains('!') || rest.contains("],") {
        return None;
    }
    let (reg, mem) = rest.split_once(',')?;
    let n = xreg(reg)?; // x-form (64-bit) only
    let mem = mem.trim();
    mem.starts_with("[sp,").then_some((is_load, n, mem))
}

/// FP VALUE RESIDENCY [Phase 4.2] — redundant `fmov` elimination via 64-bit value-equivalence
/// over BOTH the GP (x) and FP (d) register files. Lacking an FP register class, the emitter
/// round-trips every double d→GP→d and reloads it each use; a `fmov Dst,Src` (or `mov x,x`) whose
/// Dst already holds Src's exact 64-bit pattern (same value-id) is a proven no-op and dropped.
/// Only 64-bit copies form equivalences — `fmov d,x` / `fmov x,d` / `fmov d,d` / `mov x,x` — so
/// value-ids never mix widths. ANY other write to a register (including a w-form or s/q/v-reg
/// write, matched broadly for invalidation) mints a fresh id, breaking a stale equivalence; an
/// unrecognized / branch / label line flushes the model (basic-block scope). Same rewrite-
/// soundness as drop_redundant_moves (a value-id equality means identical bits), extended to d.
pub(super) fn fmov_residency(body: &str) -> String {
    use std::collections::HashMap;
    // 64-bit copy operand: x n → n (0..30), d n → 32+n. NOT w/s/q (width mismatch) → None.
    fn rid_copy(tok: &str) -> Option<u32> {
        let t = tok.trim();
        if let Some(n) = t.strip_prefix('x') {
            return n.parse::<u32>().ok().filter(|&r| r <= 30);
        }
        t.strip_prefix('d').and_then(|n| n.parse::<u32>().ok()).filter(|&r| r <= 31).map(|r| 32 + r)
    }
    // ANY-width destination register (for invalidation): x/w → GP slot; d/s/h/b/q/v → the shared
    // v-reg id 32+n. Brackets/immediates/labels → None.
    fn rid_def(tok: &str) -> Option<u32> {
        let t = tok.trim().trim_start_matches('[').trim_end_matches(']');
        if let Some(n) = t.strip_prefix('x').or_else(|| t.strip_prefix('w')) {
            return n.parse::<u32>().ok().filter(|&r| r <= 30);
        }
        t.strip_prefix(['d', 's', 'h', 'b', 'q', 'v'])
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|&r| r <= 31)
            .map(|r| 32 + r)
    }
    const NO_DEF: &[&str] = &["str", "strb", "strh", "stp", "cmp", "cmn", "tst", "fcmp", "ccmp"];
    const DEF_FIRST: &[&str] = &[
        "mov", "movk", "movz", "movn", "add", "sub", "mul", "msub", "madd", "neg", "mvn", "and",
        "orr", "eor", "bic", "lsl", "lsr", "asr", "sdiv", "udiv", "sxtw", "sxth", "sxtb", "uxtw",
        "uxth", "uxtb", "cset", "csel", "csinc", "cinc", "adrp", "ldr", "ldrb", "ldrh", "ldrsw",
        "ldrsb", "ldrsh", "fmov", "scvtf", "ucvtf", "fcvt", "fadd", "fsub", "fmul", "fdiv", "fneg",
        "fcvtzs", "fcvtzu", "sxtl", "ubfx", "ubfiz", "sbfx", "sbfiz", "fabs", "fsqrt", "fmadd",
        "fmsub", "fnmul", "fcsel", "frinta", "frintm", "frintp", "frintz", "dup",
    ];
    let mut eq: HashMap<u32, u64> = HashMap::new();
    let mut next = 0u64;
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            out.push('\n');
            continue;
        }
        if t.ends_with(':') {
            eq.clear(); // basic-block boundary
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.starts_with('.') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mn = t.split_whitespace().next().unwrap_or("");
        let ops = t[mn.len()..].trim_start();
        // Writeback addressing (`[xB], #k` post-index or `[xB, #k]!` pre-index) MODIFIES the base
        // register xB (xB += k) with NO first-operand destination — the NO_DEF/DEF_FIRST logic below
        // would leave eq[xB] stale. Invalidate it here, else a later `mov r,xB`/`mov xB,r` is wrongly
        // judged redundant. (post_index folds `str Rt,[xB]; add xB,xB,#k` into this form AFTER
        // drop_redundant_moves ran, so residency is the first value-model to meet the writeback.)
        if t.contains("]!") || t.contains("], #") {
            if let Some(lb) = t.rfind('[') {
                let base_tok = t[lb + 1..].split([',', ']']).next().unwrap_or("");
                if let Some(r) = rid_def(base_tok) {
                    next += 1;
                    eq.insert(r, next);
                }
            }
        }
        // A tracked 64-bit copy: `fmov`/`mov` with two register operands, both x-or-d (rid_copy).
        if mn == "fmov" || mn == "mov" {
            let mut it = ops.split(',');
            if let (Some(dt), Some(st), None) = (it.next(), it.next(), it.next()) {
                if let (Some(d), Some(s)) = (rid_copy(dt), rid_copy(st)) {
                    match (eq.get(&d), eq.get(&s)) {
                        (Some(a), Some(b)) if a == b => continue, // Dst already ≡ Src ⟹ DROP
                        _ => {
                            let sid = *eq.entry(s).or_insert_with(|| {
                                next += 1;
                                next
                            });
                            eq.insert(d, sid);
                            out.push_str(line);
                            out.push('\n');
                            continue;
                        }
                    }
                }
            }
            // else (immediate / s-reg / w-form) ⟹ fall through to def-invalidation below
        }
        if mn == "ldp" {
            for tok in ops.split(',').take(2) {
                if let Some(r) = rid_def(tok) {
                    next += 1;
                    eq.insert(r, next);
                }
            }
        } else if NO_DEF.contains(&mn) {
            // no destination — model unchanged
        } else if DEF_FIRST.contains(&mn) {
            if let Some(r) = ops.split(',').next().and_then(rid_def) {
                next += 1;
                eq.insert(r, next); // fresh value ⟹ breaks stale ≡
            }
        } else {
            eq.clear(); // unrecognized (branch/call/ret/…) ⟹ flush = safe
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// FP CONSTANT MATERIALIZATION [Phase 4.1]. zcc lowers an f64 constant as its 64-bit bit pattern in
/// a GPR (a `mov`/`movk` chain) then bridges it with `fmov dM, xN`. When the double is one of the
/// 256 values the AArch64 scalar FP immediate encodes (VFPExpandImm: sign · 3-bit exp · 4-bit
/// mantissa; the low 48 fraction bits are always zero), gcc emits a single `fmov dM, #imm`. An imm8
/// double therefore materializes as EXACTLY the shape
///     mov xN, #0 ; movk xN, #H, lsl #48 ; fmov dM, xN     →     fmov dM, #<val>
/// (only the top 16-bit lane is nonzero, so the low three `movk` lanes never appear). SOUND iff (a)
/// the reconstructed value H<<48 is imm8-encodable and (b) xN is dead after the fmov — a fresh
/// scratch, redefined before any read (a forward liveness scan like post_index's; a boundary or a
/// read of xN before a write ⟹ DECLINE). Machine translation-validation (pure ISA-immediate
/// identity). 3 insns → 1.
pub(super) fn fold_fp_imm(body: &str) -> String {
    // f64 bit pattern → Some(value) iff encodable as an AArch64 scalar FP `#imm8`, else None.
    // Decode obligation (ARM DDI0487 VFPExpandImm, N=64): frac<47:0>=0; exp<10:0> must equal
    // NOT(b) : b(×8) : xx — i.e. bits<9:2> all == b and bit<10> == !b (the two free bits are xx).
    fn imm8(bits: u64) -> Option<f64> {
        if bits & 0x0000_ffff_ffff_ffff != 0 {
            return None;
        }
        let exp = (bits >> 52) & 0x7ff;
        let b = (exp >> 9) & 1;
        if (exp >> 2) & 0xff != if b == 1 { 0xff } else { 0 } {
            return None;
        }
        if (exp >> 10) & 1 != b ^ 1 {
            return None;
        }
        Some(f64::from_bits(bits))
    }
    // `mov xN, #0` → N.
    fn p_mov0(t: &str) -> Option<u32> {
        let (n, rest) = t.strip_prefix("mov x")?.split_once(", #0")?;
        rest.is_empty().then_some(())?;
        n.parse().ok()
    }
    // `movk xN, #H, lsl #48` → (N, H).
    fn p_movk48(t: &str) -> Option<(u32, u64)> {
        let rest = t.strip_prefix("movk x")?;
        let mut it = rest.split(", ");
        let n: u32 = it.next()?.parse().ok()?;
        let h: u64 = it.next()?.strip_prefix('#')?.parse().ok()?;
        (it.next()? == "lsl #48" && it.next().is_none()).then_some((n, h))
    }
    // `fmov dM, xN` → (M, N).
    fn p_fmov(t: &str) -> Option<(u32, u32)> {
        let (m, n) = t.strip_prefix("fmov d")?.split_once(", x")?;
        Some((m.parse().ok()?, n.parse().ok()?))
    }
    let lines: Vec<&str> = body.lines().collect();
    let mut drop = vec![false; lines.len()];
    let mut repl: Vec<Option<String>> = vec![None; lines.len()];
    for i in 0..lines.len().saturating_sub(1) {
        // The materialization is an adjacent `mov xN,#0 ; movk xN,#H,lsl #48` (imm() emits the two
        // consecutively; no pass reorders inside it). The consuming `fmov dM,xN` is NOT necessarily
        // adjacent — scheduling interposes an FP bridge — so scan forward for it.
        let (Some(n0), Some((n1, h))) = (p_mov0(lines[i].trim()), p_movk48(lines[i + 1].trim()))
        else {
            continue;
        };
        if n0 != n1 {
            continue;
        }
        let Some(v) = imm8(h << 48) else { continue }; // not an imm8 double — leave the chain
        // Forward scan: xN's ONLY readers until it is redefined must be `fmov dM,xN` bridges (each
        // rewritable to the immediate). Any other read, or a boundary/label, ⟹ xN escapes the idiom
        // ⟹ decline (leave the materialization). A write of xN ends the live range (dead ⟹ fold).
        let mut consumers: Vec<(usize, u32)> = Vec::new();
        let mut ok = true;
        for (off, lj) in lines[i + 2..].iter().enumerate() {
            let t = lj.trim();
            if t.is_empty() || (t.starts_with('.') && !t.ends_with(':')) {
                continue;
            }
            if t.ends_with(':') {
                ok = false;
                break;
            }
            let (reads, writes, boundary) = reg_uses(t);
            if boundary {
                // reg_uses bails on writeback (`[xB],#k` / `]!`) and control-flow lines. Be precise
                // about xN. A line that never names xN as a register token neither reads nor writes
                // it. `ret` ends the function (a caller-saved scratch xN is dead); a call clobbers
                // caller-saved regs (x0..x18) so xN is dead past it iff caller-saved; any in-block
                // branch (b/b.cc/br) reaches another block where xN might be live ⟹ decline.
                let touches = |t: &str, n: u32| {
                    t.split(|c: char| !c.is_ascii_alphanumeric()).any(|tok| gpreg(tok) == Some(n))
                };
                let mn = t.split_whitespace().next().unwrap_or("");
                if touches(t, n1) {
                    ok = false;
                } else if mn == "ret" {
                    // fall through: function end, xN dead — break with `ok` intact (fold)
                } else if (mn == "bl" || mn == "blr") && n1 <= 18 {
                    // call clobbers caller-saved xN — dead past here
                } else if mn == "b" || mn == "br" || mn.starts_with("b.") || mn == "bl" || mn == "blr" {
                    ok = false; // inter-block edge / callee-saved across call ⟹ cannot prove dead
                } else {
                    continue; // writeback mem op not naming xN — xN untouched, keep scanning
                }
                break;
            }
            if reads.contains(&n1) {
                match p_fmov(t) {
                    Some((m, nn)) if nn == n1 => consumers.push((i + 2 + off, m)),
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if writes.contains(&n1) {
                break; // xN redefined ⟹ live range closed, materialization now dead
            }
        }
        if !ok || consumers.is_empty() {
            continue;
        }
        let s = format!("{v}");
        let s = if s.contains(['.', 'e', 'E']) { s } else { format!("{s}.0") };
        drop[i] = true;
        drop[i + 1] = true;
        for (j, m) in consumers {
            repl[j] = Some(format!("\tfmov d{m}, #{s}"));
        }
    }
    let mut out = String::with_capacity(body.len());
    for (i, line) in lines.iter().enumerate() {
        if drop[i] {
            continue;
        }
        match &repl[i] {
            Some(r) => {
                out.push_str(r);
                out.push('\n');
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// FP BRIDGE COLLAPSE [Phase 4.3]. Absent an FP register class, zcc funnels an FP value from one
/// d-reg to another through a GP scratch: `fmov xN, dS ; fmov dD, xN` (dS → xN → dD). Since dS is
/// unchanged between the two adjacent copies, the second may read dS directly — `fmov dD, dS` — and
/// the GP hop `fmov xN, dS` becomes dead (reaped by the following drop_dead_moves when xN is unused
/// elsewhere). Pure 64-bit register-copy identity (the same bits reach dD either way); the GP hop
/// is only kept when xN has another consumer, in which case the rewrite is still value-preserving.
pub(super) fn collapse_fp_bridge(body: &str) -> String {
    fn p_xd(t: &str) -> Option<(u32, u32)> {
        let (n, s) = t.strip_prefix("fmov x")?.split_once(", d")?;
        Some((n.parse().ok()?, s.parse().ok()?))
    }
    fn p_dx(t: &str) -> Option<(u32, u32)> {
        let (d, m) = t.strip_prefix("fmov d")?.split_once(", x")?;
        Some((d.parse().ok()?, m.parse().ok()?))
    }
    let lines: Vec<&str> = body.lines().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < lines.len() {
        if i + 1 < lines.len() {
            if let (Some((n, s)), Some((d, m))) =
                (p_xd(lines[i].trim()), p_dx(lines[i + 1].trim()))
            {
                if m == n {
                    out.push_str(lines[i]); // keep the GP hop; drop_dead_moves reaps it if xN dies
                    out.push('\n');
                    _ = writeln!(out, "\tfmov d{d}, d{s}"); // dD reads dS directly
                    i += 2;
                    continue;
                }
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// SHIFTED-REGISTER ARITHMETIC FUSION [Phase 3.1]. ARMv8 add/sub take a shifted second source in
/// ONE instruction: `add Xd, Xn, Xm, lsl #s`. A strength-reduced scaled index emits `lsl xT,xM,#s
/// ; add xD,xA,xT` (base + index·2^s). When the add OVERWRITES the shift's destination (xD==xT)
/// and its first source is independent (xA≠xT), the shift folds into the add and the `lsl` is
/// deleted. Soundness: xM is unchanged between the (adjacent) shift and add, so the fused
/// `add xD,xA,xM,lsl #s` reads the same value the `lsl` did; xT's shifted value had exactly one
/// consumer (this add) and xD==xT redefines it, so no later reader sees the removed intermediate.
/// Identical for `sub`. Register width (x/w) must match across all three.
pub(super) fn fuse_shifted_arith(body: &str) -> String {
    // "lsl {r}D, {r}M, #s" with D,M same width prefix → (r, D, M, s)
    fn plsl(t: &str) -> Option<(char, u32, u32, u32)> {
        let rest = t.trim().strip_prefix("lsl ")?;
        let mut it = rest.split(',');
        let (d, m, s) = (it.next()?.trim(), it.next()?.trim(), it.next()?.trim());
        if it.next().is_some() {
            return None; // a 4th operand ⟹ already shifted — not our form
        }
        let (r, rm) = (d.chars().next()?, m.chars().next()?);
        let sh: u32 = s.strip_prefix('#')?.parse().ok()?;
        (r == rm).then_some((r, d[1..].parse().ok()?, m[1..].parse().ok()?, sh))
    }
    // "add|sub {r}D, {r}A, {r}T" — three PLAIN regs, no shift/imm/memory → (mn, r, D, A, T)
    fn padd(t: &str) -> Option<(&str, char, u32, u32, u32)> {
        let t = t.trim();
        let (mn, rest) = t
            .strip_prefix("add ")
            .map(|r| ("add", r))
            .or_else(|| t.strip_prefix("sub ").map(|r| ("sub", r)))?;
        let mut it = rest.split(',');
        let (d, a, s) = (it.next()?.trim(), it.next()?.trim(), it.next()?.trim());
        if it.next().is_some() || s.contains('#') || s.contains('[') || s.contains(' ') {
            return None;
        }
        let r = d.chars().next()?;
        (a.chars().next()? == r && s.chars().next()? == r)
            .then_some((mn, r, d[1..].parse().ok()?, a[1..].parse().ok()?, s[1..].parse().ok()?))
    }
    let lines: Vec<&str> = body.lines().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < lines.len() {
        if let (Some((r1, dl, m, s)), Some((mn, r2, dd, a, tt))) =
            (plsl(lines[i]), lines.get(i + 1).and_then(|l| padd(l)))
        {
            if r1 == r2 && dd == dl && tt == dl && a != dl {
                _ = writeln!(out, "\t{mn} {r1}{dd}, {r1}{a}, {r1}{m}, lsl #{s}");
                i += 2;
                continue;
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// EXTENDED-REGISTER ARITHMETIC FUSION [Phase 3.1, sxtw arm]. ARMv8 add/sub take a sign-extended
/// 32-bit second source with a scale in ONE instruction: `add Xd, Xn, Wm, sxtw #k` (k ∈ 0..=4).
/// A scaled index off a signed `int` emits `sxtw xT,wS ; add xD,xB,xT[,lsl #k]` (widen the index,
/// then base + index·2^k). When the add OVERWRITES the sxtw dest (xD==xT) and the base is
/// independent (xB≠xT), both fold: the sxtw and the shift vanish into the add's extend field.
/// Soundness: xT = sext(wS), so xB + (xT≪k) = xB + (sext(wS)≪k) — exactly the extended-add; wS is
/// unchanged across the adjacent pair, and xT (single-consumer, redefined by xD==xT) leaves no
/// later reader. Only fires for k ≤ 4 (the extend-shift encoding range). Identical for `sub`.
pub(super) fn fuse_sxtw_extend(body: &str) -> String {
    // "sxtw xT, wS" → (T, S)
    fn psxtw(t: &str) -> Option<(u32, u32)> {
        let mut it = t.trim().strip_prefix("sxtw ")?.split(',');
        let d = xreg(it.next()?)?;
        let s = wreg(it.next()?)?;
        it.next().is_none().then_some((d, s))
    }
    // "add|sub xD, xB, xT[, lsl #k]" (x-form) → (mn, D, B, T, k); k defaults 0, capped ≤4.
    fn paddx(t: &str) -> Option<(&str, u32, u32, u32, u32)> {
        let t = t.trim();
        let (mn, rest) = t
            .strip_prefix("add ")
            .map(|r| ("add", r))
            .or_else(|| t.strip_prefix("sub ").map(|r| ("sub", r)))?;
        let mut it = rest.split(',');
        let d = xreg(it.next()?)?;
        let b = xreg(it.next()?)?;
        let tt = xreg(it.next()?)?;
        let k: u32 = match it.next() {
            None => 0,
            Some(sh) => sh.trim().strip_prefix("lsl #")?.parse().ok()?,
        };
        (it.next().is_none() && k <= 4).then_some((mn, d, b, tt, k))
    }
    let lines: Vec<&str> = body.lines().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < lines.len() {
        if let (Some((tt, s)), Some((mn, d, b, t2, k))) =
            (psxtw(lines[i]), lines.get(i + 1).and_then(|l| paddx(l)))
        {
            if d == tt && t2 == tt && b != tt {
                let sh = if k == 0 { String::new() } else { format!(" #{k}") };
                _ = writeln!(out, "\t{mn} x{d}, x{b}, w{s}, sxtw{sh}");
                i += 2;
                continue;
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// FRAME-ADJUST FUSION [Phase 1.2]. The prologue subtracts the fixed frame (`sub sp,sp,#fframe`)
/// and the IR body then subtracts its temp-spill slab (`sub sp,sp,#ir_tspill`) — two phases of
/// frame sizing emit two adjustments. When they land ADJACENT (nothing in between depends on the
/// intermediate sp — the case whenever emit_params spilled nothing, i.e. every promoted-param
/// leaf), they fuse to one `sub sp,sp,#(a+b)`. Pure sp-arithmetic identity: sp is lowered by the
/// same total and no instruction reads the intermediate value, so the x29-based CFA and every
/// baked `[sp,#k]` slot offset are unchanged. Fires only when a+b ≤ 4095 (imm12 single-sub range);
/// a larger total keeps two subs (the second `sub sp,sp,#b` case emits its own encoding). Strict
/// adjacency is the soundness fence — a spilled-param function (subs not adjacent) is left as-is
/// (a peephole truncation; the universal single-sub frame layout is deferred to a frame-layout
/// pass). Volatile-independent (touches only sp arithmetic).
pub(super) fn fuse_sp_adjust(body: &str) -> String {
    let parse = |t: &str| -> Option<u32> { t.trim().strip_prefix("sub sp, sp, #")?.parse().ok() };
    let lines: Vec<&str> = body.lines().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < lines.len() {
        if let (Some(a), Some(b)) = (parse(lines[i]), lines.get(i + 1).and_then(|l| parse(l))) {
            if a + b <= 4095 {
                _ = writeln!(out, "\tsub sp, sp, #{}", a + b);
                i += 2;
                continue;
            }
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// Delete every `ldr xN,[sp,#m]` immediately preceded by `str xN,[sp,#m]` (store→load
/// identity, see the block comment). Airtight and value-independent; the single largest
/// measured reduction in the load stream.
pub(super) fn drop_redundant_loads(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut pending_store: Option<(u32, String)> = None; // (reg, mem) of the last store
    for line in body.lines() {
        let t = line.trim();
        if t.ends_with(':') {
            // label (incl. local `.L…:`) = control-flow entry ⟹ store→load identity breaks.
            // Checked BEFORE the `.`-directive case, since local labels start with a dot.
            pending_store = None;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if t.is_empty() || t.starts_with('.') {
            out.push_str(line); // blank/directive: no execution, no control entry — keep pair
            out.push('\n');
            continue;
        }
        match parse_frame_ldst(t) {
            Some((true, reg, mem)) => {
                if pending_store.as_ref().is_some_and(|(sr, sm)| *sr == reg && sm == mem) {
                    pending_store = None; // the redundant reload — DROP it (not emitted)
                    continue;
                }
                pending_store = None; // a load that redefines xN: no store now pends
            }
            Some((false, reg, mem)) => pending_store = Some((reg, mem.to_string())),
            None => pending_store = None, // any other instruction may touch mem/regs ⟹ flush
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}
