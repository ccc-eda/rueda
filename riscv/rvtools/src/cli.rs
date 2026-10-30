//! `reda cc`: the C compiler and assembler on the command line.

fn usage() -> ! {
    eprintln!("reda cc: wrong arguments (reda help cc)");
    std::process::exit(2)
}

/// `reda cc PROG.c [-o PROG.bin] [-S PROG.s] [-L PROG.lst] [--run [N]] [--mem BYTES] [--linux]`:
/// compile C for RvSoc (or, --linux, an RV64 Linux executable); a .s / .S file is assembled
pub fn cc(args: &[String]) -> ! {
    let (mut src, mut out, mut asm_out, mut run, mut mem) = (None::<String>, None::<String>, None::<String>, None::<u64>, 8192u32);
    let mut lst: Option<String> = None;
    let mut linux = false;
    let mut a = args.iter();
    while let Some(x) = a.next() {
        match x.as_str() {
            "-o" => out = a.next().cloned(),
            "-S" => asm_out = a.next().cloned(),
            "-L" => lst = a.next().cloned(),
            "--run" => run = Some(a.clone().next().and_then(|n| n.parse().ok()).inspect(|_| { a.next(); }).unwrap_or(10_000_000)),
            "--linux" => linux = true,
            "--mem" => mem = a.next().and_then(|n| n.parse().ok()).unwrap_or_else(|| usage()),
            f if !f.starts_with('-') => src = Some(f.to_string()),
            _ => usage(),
        }
    }
    let Some(path) = src else { usage() };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| { eprintln!("{}: {}", path, e); std::process::exit(1) });
    let name = std::path::Path::new(&path).file_name().unwrap().to_string_lossy().to_string();
    // an assembly file: just the assembler
    if name.ends_with(".s") || name.ends_with(".S") {
        let p = crate::asm::assemble(&text).unwrap_or_else(|es| { for e in es { eprintln!("{}:{}: error: {}", name, e.line, e.msg); } std::process::exit(1) });
        if let Some(o) = &out { std::fs::write(o, &p.bytes).unwrap(); }
        if let Some(l) = &lst { std::fs::write(l, crate::asm::listing_text(&p)).unwrap(); }
        eprintln!("{}: {} bytes", name, p.bytes.len());
        std::process::exit(0);
    }
    let o = match crate::cc::compile(&name, &text, &crate::cc::Options { mem_size: mem, linux }) {
        Ok(o) => o,
        Err(es) => { for e in es { eprintln!("{}:{}: error: {}", e.file, e.line, e.msg); } std::process::exit(1) }
    };
    if let Some(elf) = &o.elf {
        if let Some(p) = &asm_out { std::fs::write(p, &o.asm).unwrap(); }
        if let Some(p) = &lst { std::fs::write(p, crate::asm::listing_text(&o.program)).unwrap(); }
        let p = out.unwrap_or_else(|| name.trim_end_matches(".c").to_string());
        std::fs::write(&p, elf).unwrap();
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)); }
        eprintln!("{}: {} ({} bytes, RV64 Linux)", name, p, elf.len());
        std::process::exit(0);
    }
    eprintln!("{}: {} bytes of code and data, {} with bss (RAM {} bytes)", name, o.program.bytes.len(), o.program.end, mem);
    if let Some(p) = &asm_out { std::fs::write(p, &o.asm).unwrap(); }
    if let Some(p) = &lst { std::fs::write(p, crate::asm::listing_text(&o.program)).unwrap(); }
    if let Some(p) = &out { std::fs::write(p, &o.program.bytes).unwrap(); }
    if let Some(n) = run {
        let mut s = crate::iss::Iss::new(&o.program.bytes, mem as usize);
        let halted = s.run(n).unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1) });
        print!("{}", String::from_utf8_lossy(&s.tx));
        eprintln!("{} after {} instructions ({} cycles); LED = 0x{:02x}", if halted { "halted" } else { "stopped" }, s.instret, s.cycles, s.led);
    }
    std::process::exit(0)
}
