// zcc — C89+ compiler, target: AArch64 ELF (Linux).
// THEORY II-6 — the driver's flag surface; THEORY II-4 — ELF output
// A cc-compatible CLI for drop-in use in a Makefile (CC=zcc):
//   zcc [-c | -S] [-o out] [other cc flags: swallowed silently] <in.c>
//   default: temp .s → `as` → temp .o → `ld` (directly, not via the cc driver)
//   producing the ELF executable `a.out`. -c stops at .o (`<stem>.o`), -S at .s
//   (`<stem>.s`). ld: entry via crt1.o (_start → __libc_start_main → main),
//   ctor/dtor via crti/crtn.
mod ast;
mod cfg;
mod compile;
mod emit;
mod ext;
mod hir;
mod isel;
mod lexer;
mod mem;
mod mir;
mod parser;
mod preprocess;
mod regalloc;
#[cfg(test)]
mod testutil;
use std::{env, fs, process::Command, process::ExitCode};

fn run(cmd: &str, args: &[&str]) -> bool {
    match Command::new(cmd).args(args).status() {
        Ok(st) => st.success(),
        Err(e) => {
            eprintln!("zcc: {}: {}", cmd, e);
            false
        }
    }
}

fn write_or_die(path: &str, data: &str) -> bool {
    fs::write(path, data)
        .map_err(|e| eprintln!("zcc: {}: {}", path, e))
        .is_ok()
}

fn main() -> ExitCode {
    // The parser recurses deeply on real TUs (sqlite testfixture's 88 TUs reach
    // the edge of the default 8MB stack — an intermittent segv because the env
    // size shifts the starting point). Allocate 256MB explicitly: a hard ceiling
    // instead of a chance margin.
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(drive)
        .unwrap()
        .join()
        .unwrap()
}

fn drive() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let (mut inputs, mut output, mut mode) = (Vec::new(), None, "ld");
    let (mut defs, mut incs, mut undefs) = (
        Vec::<String>::new(),
        Vec::<String>::new(),
        Vec::<String>::new(),
    );
    // -l/-L keep their CLI order and are forwarded straight to ld; -MMD/-MF
    // generate a .d file for make
    let (mut libs, mut depgen, mut depfile) = (Vec::<String>::new(), false, None::<String>);
    let mut shared = false; // -shared → ld -dylib (redis's xxhash builds a dylib)
    let mut pic = false; // -fPIC → the ELF backend goes through the GOT for non-static globals
    let (mut nostdinc, mut bundle) = (false, false);
    let mut export_dyn = false; // -rdynamic → ld --export-dynamic (backtrace_symbols resolves function names)
    // IR→ops→asm is the ONLY path (it fully covers suite/csmith/musl); the
    // AST-walk emit() has been removed. There is no backend-selection flag — no
    // real compiler warrants one.
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                output = args.get(i).map(|s| s.as_str());
            }
            "-S" => mode = "s",
            "-E" => mode = "e",
            "-c" => mode = "o",
            "-D" | "-I" | "-U" | "-L" => {
                let k = args[i].clone();
                i += 1;
                if let Some(v) = args.get(i) {
                    match k.as_str() {
                        "-D" => defs.push(v.clone()),
                        "-I" => incs.push(v.clone()),
                        "-U" => undefs.push(v.clone()),
                        _ => libs.push(format!("-L{}", v)),
                    }
                }
            }
            "-v" | "--version" => {
                println!("zcc — C89+ compiler, target aarch64-linux-gnu (ELF)");
                if args.len() == 2 {
                    return ExitCode::SUCCESS; // configure probes `$CC -v`: must exit 0
                }
            }
            "-shared" | "-dynamiclib" => shared = true,
            // ELF .so: a non-static global is preemptible and MUST go through the
            // GOT (ld rejects a direct adrp under -shared); redis modules build .xo
            // with -fPIC
            "-fPIC" | "-fpic" => pic = true,
            "-bundle" => bundle = true, // a dlopen module → .so (redis test modules)
            "-rdynamic" | "-export-dynamic" | "--export-dynamic" => export_dyn = true,
            // Darwin flags with a separate argument (unused on ELF) — swallow the
            // WHOLE PAIR, otherwise the argument is treated as an input file
            // ("-undefined dynamic_lookup", "--target …")
            "-undefined" | "--target" | "-target" => i += 1,
            "-nostdinc" => nostdinc = true, // musl: use -I only, drop embedded headers + SDK
            "-MMD" | "-MD" => depgen = true, // the .d contains real headers only (embedded <..> dropped)
            "-MP" => {}                     // our deps are all one line, no phony targets needed
            "-MF" => {
                i += 1;
                depfile = args.get(i).cloned();
            }
            s if s.starts_with("-D") => defs.push(s[2..].to_string()),
            s if s.starts_with("-I") => incs.push(s[2..].to_string()),
            s if s.starts_with("-U") => undefs.push(s[2..].to_string()),
            s if s.starts_with("-l") || s.starts_with("-L") => libs.push(s.to_string()),
            // flags with a separate argument: swallow the whole pair
            "-arch" | "-isysroot" | "-framework" | "-include" | "-x" | "-MT" | "-MQ"
            | "-Xlinker" => i += 1,
            // drop-in gcc: honor the optimization level. `-O0` turns our optimizer
            // OFF (a debug build must get unoptimized code); `-O`, `-O1`, `-O2`,
            // `-O3`, `-Os`, `-Ofast`, `-Og` all map to our single optimizer tier —
            // the charter's stopping point is gcc-O1 parity, so there is nothing
            // above -O1 to give, and gcc never rejects a level, so neither do we.
            // The last -O on the line wins, exactly as gcc resolves it.
            // SAFETY: argument parsing runs on the main thread before any pass
            // thread is spawned, so this process-global write races nothing.
            "-O0" => unsafe { std::env::set_var("ZCC_O0", "1") },
            s if s.starts_with("-O") => unsafe { std::env::remove_var("ZCC_O0") },
            s if s.starts_with('-') => {} // -g -W… -std=… : swallowed for cc compatibility
            s => inputs.push(s),
        }
        i += 1;
    }
    if inputs.is_empty() {
        eprintln!("usage: zcc [-c | -S] [-o out] <in.c | in.o>... [-lfoo -Ldir]");
        return ExitCode::FAILURE;
    }
    if output.is_some() && mode != "ld" && inputs.len() > 1 {
        eprintln!("zcc: cannot use -o with multiple inputs when -c/-S is given");
        return ExitCode::FAILURE;
    }
    // ELF: ZCC_SYSROOT=<musl install> — headers + crt + libc are taken ENTIRELY
    // from musl (libc-test M17 requires it: tests must compile/link against the
    // musl that zcc built, not the box's glibc); if unset, the box's glibc as
    // before.
    let sysroot = std::env::var("ZCC_SYSROOT").ok();
    // -nostdinc (standard cc): a \x01 sentinel at the head of incs disables the
    // embedded table.
    if nostdinc {
        incs.insert(0, "\u{1}nostdinc".to_string());
    } else if let Some(s) = &sysroot {
        // full musl headers — disable the embedded table (the embedded headers
        // carry the box libc's values)
        incs.insert(0, "\u{1}nostdinc".to_string());
        incs.push(format!("{s}/include"));
    } else {
        // sentinel: the embedded table serves only compiler-owned headers, libc =
        // the box's glibc
        incs.insert(0, "\u{1}elf".to_string());
        incs.push("/usr/include/aarch64-linux-gnu".to_string());
        incs.push("/usr/include".to_string());
    }
    // .c → (asm text, list of real files read — for -MMD); .o/.a go straight to the linker
    let emit_asm = |path: &str| -> Option<(String, Vec<String>)> {
        match preprocess::preprocess(path, &defs, &undefs, &incs).and_then(
            |(t, locs, files)| {
                parser::parse(&t, &locs, &files).map(|mut ast| {
                    ast.pic = pic;
                    (ast, files)
                })
            },
        ) {
            Ok((ast, files)) => {
                let asm = compile::compile(&ast);
                Some((asm, files))
            }
            Err(e) => {
                eprintln!("zcc: {}: {}", path, e);
                None
            }
        }
    };
    // make reads "<obj>: <src> <headers>"; an embedded header (<stdio.h>…) is not a file
    let write_deps = |obj: &str, files: &[String]| -> bool {
        let d = depfile
            .clone()
            .unwrap_or_else(|| format!("{}.d", obj.strip_suffix(".o").unwrap_or(obj)));
        let mut deps: Vec<&str> = Vec::new();
        for f in files {
            if !f.starts_with('<') && !deps.contains(&f.as_str()) {
                deps.push(f);
            }
        }
        write_or_die(&d, &format!("{}: {}\n", obj, deps.join(" ")))
    };
    let stem_of = |path: &str| {
        let file = path.rsplit('/').next().unwrap();
        let stem = file
            .strip_suffix(".c")
            .or_else(|| file.strip_suffix(".s"))
            .or_else(|| file.strip_suffix(".S"));
        stem.unwrap_or(file).to_string()
    };
    let ok = match mode {
        "e" => {
            // dump the preprocessed tokens (debug); one space between tokens, newline follows the original line
            let mut ok = true;
            for path in &inputs {
                match preprocess::preprocess(path, &defs, &undefs, &incs) {
                    Ok((toks, locs, _)) => {
                        let mut cur = (u32::MAX, u32::MAX);
                        let mut s = String::new();
                        for (t, &l) in toks.iter().zip(&locs) {
                            if l != cur {
                                s.push('\n');
                                cur = l;
                            }
                            s.push_str(&preprocess::spell(t));
                            s.push(' ');
                        }
                        s.push('\n');
                        print!("{}", s);
                    }
                    Err(e) => {
                        eprintln!("zcc: {}: {}", path, e);
                        ok = false;
                    }
                }
            }
            ok
        }
        "s" | "o" => {
            let mut ok = true;
            for path in &inputs {
                // drop-in driver rule: a real build system (musl) hands a .s file
                // straight to $CC -c — pass it verbatim to as, not through C;
                // .S (uppercase) = asm that needs the C preprocessor (musl
                // memset.S #defines regs)
                if path.ends_with(".s") || path.ends_with(".S") {
                    let stem = stem_of(path);
                    let default = format!("{stem}.o");
                    let out = output.unwrap_or(&default);
                    ok = if path.ends_with(".S") {
                        let tmp_s = format!("{}.zcc.s", out);
                        match preprocess::preprocess_asm(path, &defs, &undefs, &incs) {
                            Ok(asm) => {
                                let r =
                                    write_or_die(&tmp_s, &asm) && run("as", &[&tmp_s, "-o", out]);
                                fs::remove_file(&tmp_s).ok();
                                r
                            }
                            Err(e) => {
                                eprintln!("zcc: {}: {}", path, e);
                                false
                            }
                        }
                    } else {
                        run("as", &[path, "-o", out])
                    };
                    if !ok {
                        break;
                    }
                    continue;
                }
                let Some((asm, files)) = emit_asm(path) else {
                    ok = false;
                    break;
                };
                let default = format!("{}.{}", stem_of(path), mode);
                let out = output.unwrap_or(&default);
                if mode == "s" {
                    ok = write_or_die(out, &asm);
                } else {
                    let tmp_s = format!("{}.zcc.s", out);
                    ok = write_or_die(&tmp_s, &asm) && run("as", &[&tmp_s, "-o", out]);
                    fs::remove_file(&tmp_s).ok();
                    if ok && depgen {
                        ok = write_deps(out, &files);
                    }
                }
                if !ok {
                    break;
                }
            }
            ok
        }
        _ => {
            // The ELF toolchain branch (as → ld directly); a new target adds its branch here
            let out = output.unwrap_or("a.out");
            let (mut objs, mut tmps, mut ok) = (Vec::new(), Vec::new(), true);
            for (k, path) in inputs.iter().enumerate() {
                if !path.ends_with(".c") {
                    objs.push(path.to_string());
                    continue;
                }
                let (tmp_s, tmp_o) = (format!("{out}.zcc{k}.s"), format!("{out}.zcc{k}.o"));
                ok = match emit_asm(path) {
                    Some((asm, _)) => {
                        write_or_die(&tmp_s, &asm) && run("as", &[&tmp_s, "-o", &tmp_o])
                    }
                    None => false,
                };
                fs::remove_file(&tmp_s).ok();
                tmps.push(tmp_o.clone());
                objs.push(tmp_o);
                if !ok {
                    break;
                }
            }
            // glibc: atexit lives in libc_nonshared.a and references __dso_handle
            // (hidden) — gcc supplies it via crtbegin.o; zcc links as→ld directly,
            // so it emits the stub itself (a self-address: valid for both an exe
            // and a .so, it is only an identity tag for __cxa_atexit)
            if ok && sysroot.is_none() {
                let (ds, do_) = (format!("{out}.zccdso.s"), format!("{out}.zccdso.o"));
                ok = write_or_die(
                    &ds,
                    ".hidden __dso_handle\n.globl __dso_handle\n.data\n.p2align 3\n__dso_handle:\n.xword __dso_handle\n",
                ) && run("as", &[&ds, "-o", &do_]);
                fs::remove_file(&ds).ok();
                tmps.push(do_.clone());
                objs.push(do_);
            }
            if ok {
                let mut ld: Vec<&str> = Vec::new();
                // ELF executable: entry via crt1.o (_start → __libc_start_main → main),
                // ctor/dtor via crti/crtn — unlike Darwin (LC_MAIN, no crt needed).
                let crt = match &sysroot {
                    Some(s) => format!("{s}/lib"),
                    None => "/usr/lib/aarch64-linux-gnu".to_string(),
                };
                let (crt1, crti, crtn) = (
                    format!("{crt}/crt1.o"),
                    format!("{crt}/crti.o"),
                    format!("{crt}/crtn.o"),
                );
                // long double ELF: __extenddftf2/__trunctfdf2 live in libgcc.a
                // (soft-fp, freestanding — pulls in no libc dep); glob the version dir
                let gccl = fs::read_dir("/usr/lib/gcc/aarch64-linux-gnu")
                    .ok()
                    .and_then(|d| {
                        d.filter_map(|e| e.ok().map(|e| e.path()))
                            .find(|p| p.join("libgcc.a").exists())
                    })
                    .map(|p| p.to_string_lossy().into_owned());
                if !shared && !bundle {
                    ld.extend([crt1.as_str(), crti.as_str()]);
                }
                ld.extend(objs.iter().map(|s| s.as_str()));
                ld.extend(libs.iter().map(|s| s.as_str())); // -l/-L in CLI order
                // PT_GNU_EH_FRAME (.eh_frame_hdr) so the runtime unwinder can look
                // up CFI — glibc backtrace()/_Unwind_Backtrace needs this segment
                ld.push("--eh-frame-hdr");
                if export_dyn {
                    ld.push("--export-dynamic"); // -rdynamic: enough dynsym for backtrace_symbols to resolve names
                }
                if shared || bundle {
                    ld.push("-shared");
                } else {
                    ld.push(&crtn);
                    if sysroot.is_none() {
                        // a musl sysroot has only libc.a → static, no ld.so needed
                        ld.extend(["-dynamic-linker", "/lib/ld-linux-aarch64.so.1"]);
                    }
                }
                // a musl install has an empty libm.a so -lm is harmless; glibc keeps it separate
                ld.extend(["-o", out, "-lc", "-lm", "-L", &crt]);
                if let Some(g) = &gccl {
                    ld.extend(["-L", g, "-lgcc"]);
                }
                ok = run("ld", &ld);
            }
            for t in &tmps {
                fs::remove_file(t).ok();
            }
            ok
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
