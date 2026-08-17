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
    let (mut defs, mut incs) = (Vec::<String>::new(), Vec::<String>::new());
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
            "-D" | "-I" => {
                let k = args[i].clone();
                i += 1;
                if let Some(v) = args.get(i) {
                    if k == "-D" { defs.push(v.clone()) } else { incs.push(v.clone()) }
                }
            }
            s if s.starts_with("-D") => defs.push(s[2..].to_string()),
            s if s.starts_with("-I") => incs.push(s[2..].to_string()),
            "-arch" | "-isysroot" | "-framework" | "-include" => i += 1, // flag có đối số rời
            s if s.starts_with('-') => {} // -O -g -W… -std=… : nuốt để tương thích cc
            s => inputs.push(s),
        }
        i += 1;
    }
    if inputs.is_empty() {
        eprintln!("usage: zcc [-c | -S] [-o out] <in.c>...");
        return ExitCode::FAILURE;
    }
    if output.is_some() && mode != "ld" && inputs.len() > 1 {
        eprintln!("zcc: -o không dùng được với nhiều input khi có -c/-S");
        return ExitCode::FAILURE;
    }
    // .c → asm text; input khác (.o/.a) đi thẳng xuống linker
    let emit_asm = |path: &str| -> Option<String> {
        match preprocess::preprocess(path, &defs, &incs)
            .and_then(|(t, locs, files)| parser::parse(&t, &locs, &files))
        {
            Ok(ast) => Some(codegen::emit(&ast)),
            Err(e) => {
                eprintln!("zcc: {}: {}", path, e);
                None
            }
        }
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
                match preprocess::preprocess(path, &defs, &incs) {
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
                let Some(asm) = emit_asm(path) else {
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
                    Some(asm) => write_or_die(&tmp_s, &asm) && run("as", &[&tmp_s, "-o", &tmp_o]),
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
                let sdk = Command::new("xcrun")
                    .args(["-sdk", "macosx", "--show-sdk-path"])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default();
                let mut ld: Vec<&str> = objs.iter().map(|s| s.as_str()).collect();
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
