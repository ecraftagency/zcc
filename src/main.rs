// zcc — C89+ compiler, target: AArch64 ELF (Linux).
// CLI tương thích cc để drop-in vào Makefile (CC=zcc):
//   zcc [-c | -S] [-o out] [flag cc khác: nuốt im lặng] <in.c>
//   mặc định: .s tạm → `as` → .o tạm → `ld` (trực tiếp, không qua cc driver)
//   ra ELF executable `a.out`. -c dừng ở .o (`<stem>.o`), -S ở .s (`<stem>.s`).
//   ld: entry qua crt1.o (_start → __libc_start_main → main), ctor/dtor crti/crtn.
mod ast;
mod codegen;
mod ext;
mod ir;
mod lexer;
mod opt;
mod parser;
mod preprocess;
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
    // Đệ quy parser sâu theo TU thật (sqlite testfixture 88 TU đụng mép stack
    // 8MB mặc định — segv lúc có lúc không vì cỡ env xê dịch điểm xuất phát,
    // 2026-08-18). Cấp tường minh 256MB: trần cứng thay vì biên may rủi.
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
    // -l/-L giữ nguyên thứ tự CLI, forward thẳng cho ld; -MMD/-MF sinh .d cho make
    let (mut libs, mut depgen, mut depfile) = (Vec::<String>::new(), false, None::<String>);
    let mut shared = false; // -shared → ld -dylib (xxhash của redis build dylib)
    let mut pic = false; // -fPIC → backend ELF đi GOT cho global non-static
    let (mut nostdinc, mut bundle) = (false, false);
    let mut export_dyn = false; // -rdynamic → ld --export-dynamic (backtrace_symbols resolve tên hàm)
    // IR→ops→asm là đường DUY NHẤT (đã phủ trọn suite/csmith/musl). --ir giữ như
    // no-op cho back-compat script/wrapper; AST-walk emit() đã xoá (seal-ir-10k).
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
                    return ExitCode::SUCCESS; // configure dò `$CC -v`: phải exit 0
                }
            }
            "-shared" | "-dynamiclib" => shared = true,
            // ELF .so: global non-static = preemptible, PHẢI qua GOT (ld từ chối
            // adrp trực tiếp khi -shared); redis module build .xo bằng -fPIC
            "-fPIC" | "-fpic" => pic = true,
            "-bundle" => bundle = true, // dlopen module → .so (redis test modules)
            "-rdynamic" | "-export-dynamic" | "--export-dynamic" => export_dyn = true,
            // flag Darwin có đối số rời (không dùng trên ELF) — nuốt CẢ CẶP kẻo
            // đối số bị coi là input file ("-undefined dynamic_lookup", "--target …")
            "-undefined" | "--target" | "-target" => i += 1,
            "--ir" => {} // no-op: IR là đường mặc định & duy nhất
            "-nostdinc" => nostdinc = true, // musl: chỉ dùng -I, bỏ header nhúng + SDK
            "-MMD" | "-MD" => depgen = true, // .d chỉ chứa header thật (nhúng <..> bỏ)
            "-MP" => {}                     // deps của mình đều 1 dòng, phony khỏi cần
            "-MF" => {
                i += 1;
                depfile = args.get(i).cloned();
            }
            s if s.starts_with("-D") => defs.push(s[2..].to_string()),
            s if s.starts_with("-I") => incs.push(s[2..].to_string()),
            s if s.starts_with("-U") => undefs.push(s[2..].to_string()),
            s if s.starts_with("-l") || s.starts_with("-L") => libs.push(s.to_string()),
            // flag có đối số rời: nuốt cả cặp
            "-arch" | "-isysroot" | "-framework" | "-include" | "-x" | "-MT" | "-MQ"
            | "-Xlinker" => i += 1,
            s if s.starts_with('-') => {} // -O -g -W… -std=… : nuốt để tương thích cc
            s => inputs.push(s),
        }
        i += 1;
    }
    if inputs.is_empty() {
        eprintln!("usage: zcc [-c | -S] [-o out] <in.c | in.o>... [-lfoo -Ldir]");
        return ExitCode::FAILURE;
    }
    if output.is_some() && mode != "ld" && inputs.len() > 1 {
        eprintln!("zcc: -o không dùng được với nhiều input khi có -c/-S");
        return ExitCode::FAILURE;
    }
    // ELF: ZCC_SYSROOT=<musl install> — header + crt + libc lấy TRỌN từ musl
    // (libc-test M17 đòi: test phải compile/link против musl do zcc build,
    // không phải glibc của box); không set thì glibc của box như cũ.
    let sysroot = std::env::var("ZCC_SYSROOT").ok();
    // -nostdinc (chuẩn cc): sentinel \x01 đầu incs tắt bảng nhúng.
    if nostdinc {
        incs.insert(0, "\u{1}nostdinc".to_string());
    } else if let Some(s) = &sysroot {
        // musl headers trọn bộ — tắt bảng nhúng (header nhúng mang giá trị libc box)
        incs.insert(0, "\u{1}nostdinc".to_string());
        incs.push(format!("{s}/include"));
    } else {
        // sentinel: bảng nhúng chỉ phục vụ header compiler-owned, libc = glibc box
        incs.insert(0, "\u{1}elf".to_string());
        incs.push("/usr/include/aarch64-linux-gnu".to_string());
        incs.push("/usr/include".to_string());
    }
    // .c → (asm text, danh sách file thật đã đọc — cho -MMD); .o/.a đi thẳng linker
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
                let asm = codegen::emit_ir(&ast);
                Some((asm, files))
            }
            Err(e) => {
                eprintln!("zcc: {}: {}", path, e);
                None
            }
        }
    };
    // make đọc "<obj>: <src> <headers>"; header nhúng (<stdio.h>…) không phải file
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
            // dump token đã preprocess (debug); mỗi token cách 1 space, xuống dòng theo line gốc
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
                // luật driver drop-in: build system thật (musl) đưa file .s
                // thẳng cho $CC -c — chuyển nguyên văn cho as, không qua C;
                // .S (viết hoa) = asm cần C preprocessor (musl memset.S #define reg)
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
            // Nhánh toolchain ELF (as → ld trực tiếp); target mới thêm nhánh ở đây
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
            // glibc: atexit nằm trong libc_nonshared.a tham chiếu __dso_handle
            // (hidden) — gcc cấp qua crtbegin.o; zcc link as→ld trực tiếp nên tự
            // phát stub (self-address: hợp lệ cho cả exe lẫn .so, chỉ là tag
            // identity cho __cxa_atexit)
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
                // ELF executable: entry qua crt1.o (_start → __libc_start_main → main),
                // ctor/dtor qua crti/crtn — khác Darwin (LC_MAIN, không cần crt).
                let crt = match &sysroot {
                    Some(s) => format!("{s}/lib"),
                    None => "/usr/lib/aarch64-linux-gnu".to_string(),
                };
                let (crt1, crti, crtn) = (
                    format!("{crt}/crt1.o"),
                    format!("{crt}/crti.o"),
                    format!("{crt}/crtn.o"),
                );
                // long double ELF: __extenddftf2/__trunctfdf2 sống trong libgcc.a
                // (soft-fp, freestanding — không kéo dep libc); glob version dir
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
                ld.extend(libs.iter().map(|s| s.as_str())); // -l/-L theo thứ tự CLI
                // PT_GNU_EH_FRAME (.eh_frame_hdr) để runtime unwinder tra được
                // CFI — glibc backtrace()/_Unwind_Backtrace cần segment này
                ld.push("--eh-frame-hdr");
                if export_dyn {
                    ld.push("--export-dynamic"); // -rdynamic: dynsym đủ để backtrace_symbols tra tên
                }
                if shared || bundle {
                    ld.push("-shared");
                } else {
                    ld.push(&crtn);
                    if sysroot.is_none() {
                        // musl sysroot chỉ có libc.a → static, không cần ld.so
                        ld.extend(["-dynamic-linker", "/lib/ld-linux-aarch64.so.1"]);
                    }
                }
                // musl install có libm.a rỗng nên -lm vô hại; glibc tách riêng
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
