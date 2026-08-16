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
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                output = args.get(i).map(|s| s.as_str());
            }
            "-S" => mode = "s",
            "-c" => mode = "o",
            "-arch" | "-isysroot" | "-framework" | "-include" => i += 1, // flag có đối số rời
            s if s.starts_with('-') => {} // -O -g -W… -std=… : nuốt để tương thích cc
            s => inputs.push(s),
        }
        i += 1;
    }
    let [path] = inputs[..] else {
        eprintln!("usage: zcc [-c | -S] [-o out] <in.c>");
        return ExitCode::FAILURE;
    };
    let asm = match preprocess::preprocess(path).and_then(|t| parser::parse(&t)) {
        Ok(ast) => codegen::emit(&ast),
        Err(e) => {
            eprintln!("zcc: {}: {}", path, e);
            return ExitCode::FAILURE;
        }
    };
    // Tên output mặc định theo đúng cc: -S → <stem>.s, -c → <stem>.o, link → a.out
    let file = path.rsplit('/').next().unwrap();
    let stem = file.strip_suffix(".c").unwrap_or(file);
    let default = format!("{}.{}", stem, if mode == "s" { "s" } else { "o" });
    let ok = match mode {
        "s" => write_or_die(output.unwrap_or(&default), &asm),
        "o" => {
            let out = output.unwrap_or(&default);
            let tmp_s = format!("{}.zcc.s", out);
            let ok = write_or_die(&tmp_s, &asm) && run("as", &[&tmp_s, "-o", out]);
            fs::remove_file(&tmp_s).ok();
            ok
        }
        _ => {
            // Nhánh toolchain của target arm64_darwin; target mới sẽ match ở đây
            let out = output.unwrap_or("a.out");
            let (tmp_s, tmp_o) = (format!("{}.zcc.s", out), format!("{}.zcc.o", out));
            let sdk = Command::new("xcrun")
                .args(["-sdk", "macosx", "--show-sdk-path"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            let ok = write_or_die(&tmp_s, &asm)
                && run("as", &[&tmp_s, "-o", &tmp_o])
                && run(
                    "ld",
                    &[&tmp_o, "-o", out, "-lSystem", "-syslibroot", &sdk, "-arch", "arm64"],
                );
            fs::remove_file(&tmp_s).ok();
            fs::remove_file(&tmp_o).ok();
            ok
        }
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
