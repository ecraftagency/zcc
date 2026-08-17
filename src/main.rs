// zcc — C89 compiler, target: AArch64 Darwin (Apple Silicon macOS).
// CLI tương thích cc để drop-in vào Makefile (CC=zcc):
//   zcc [-c | -S] [-o out] [flag cc khác: nuốt im lặng] <in.c>
//   mặc định: .s tạm → `as` → .o tạm → `ld` (trực tiếp, không qua cc driver)
//   ra Mach-O executable `a.out`. -c dừng ở .o (`<stem>.o`), -S ở .s (`<stem>.s`).
//   ld cần -lSystem (libc; dynamic executable không cần crt0.o — entry qua
//   LC_MAIN) + -syslibroot từ xcrun; ld64 arm64 tự ad-hoc codesign.
mod ast;
mod codegen;
mod lexer;
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
    fs::write(path, data).map_err(|e| eprintln!("zcc: {}: {}", path, e)).is_ok()
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let (mut inputs, mut output, mut mode) = (Vec::new(), None, "ld");
    let (mut defs, mut incs, mut undefs) =
        (Vec::<String>::new(), Vec::<String>::new(), Vec::<String>::new());
    // -l/-L giữ nguyên thứ tự CLI, forward thẳng cho ld; -MMD/-MF sinh .d cho make
    let (mut libs, mut depgen, mut depfile) = (Vec::<String>::new(), false, None::<String>);
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
                println!("zcc — C89+ compiler, target arm64-apple-darwin (Mach-O)");
                if args.len() == 2 {
                    return ExitCode::SUCCESS; // configure dò `$CC -v`: phải exit 0
                }
            }
            "-MMD" | "-MD" => depgen = true, // .d chỉ chứa header thật (nhúng <..> bỏ)
            "-MP" => {}                      // deps của mình đều 1 dòng, phony khỏi cần
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
    let sdk = Command::new("xcrun")
        .args(["-sdk", "macosx", "--show-sdk-path"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    // M11: header không phải nhúng và không thấy ở -I → tra SDK thật (ưu tiên CUỐI,
    // sau mọi -I của user; header nhúng vẫn thắng cho 25 tên libc cơ bản)
    if !sdk.is_empty() {
        incs.push(format!("{}/usr/include", sdk));
    }
    // .c → (asm text, danh sách file thật đã đọc — cho -MMD); .o/.a đi thẳng linker
    let emit_asm = |path: &str| -> Option<(String, Vec<String>)> {
        match preprocess::preprocess(path, &defs, &undefs, &incs).and_then(|(t, locs, files)| {
            parser::parse(&t, &locs, &files).map(|ast| (ast, files))
        }) {
            Ok((ast, files)) => Some((codegen::emit(&ast), files)),
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
        file.strip_suffix(".c").unwrap_or(file).to_string()
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
            // Nhánh toolchain của target arm64_darwin; target mới sẽ match ở đây
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
            if ok {
                let mut ld: Vec<&str> = objs.iter().map(|s| s.as_str()).collect();
                ld.extend(libs.iter().map(|s| s.as_str())); // -l/-L theo thứ tự CLI
                ld.extend(["-o", out, "-lSystem", "-syslibroot", &sdk, "-arch", "arm64"]);
                ok = run("ld", &ld);
            }
            for t in &tmps {
                fs::remove_file(t).ok();
            }
            ok
        }
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
