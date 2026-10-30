//! The C compiler, the assembler and the ISS: the C programs of tests/ on the
//! instruction-set simulator and (cc_linux_programs) as RV64 Linux executables.

/// a C test program (tests/) compiled and run on the ISS: its serial output
fn cc_run(name: &str) -> (Vec<u8>, String, u64) {
    let dir = format!("{}/tests", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(format!("{}/{}.c", dir, name)).unwrap();
    let o = crate::cc::compile(&format!("{}.c", name), &src, &Default::default()).unwrap_or_else(|e| panic!("{:?}", e));
    let mut s = crate::iss::Iss::new(&o.program.bytes, 8192);
    assert!(s.run(20_000_000).unwrap(), "{} did not halt", name);
    (o.program.bytes, String::from_utf8_lossy(&s.tx).to_string(), s.cycles)
}

/// each program prints what the host C compiler's build of it prints (the .out files,
/// from `cc -funsigned-char -fwrapv`; RISC-V's char is unsigned)
#[test]
fn cc_programs() {
    let dir = format!("{}/tests", env!("CARGO_MANIFEST_DIR"));
    let mut n = 0;
    for f in std::fs::read_dir(&dir).unwrap() {
        let p = f.unwrap().path();
        if p.extension().map(|e| e == "c") != Some(true) { continue; }
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let want = std::fs::read_to_string(p.with_extension("out")).unwrap();
        let (_, got, _) = cc_run(&name);
        assert_eq!(got.replace("\r\n", "\n"), want, "{}", name);
        n += 1;
    }
    assert!(n >= 3);
}

/// the same programs, and those in tests/linux (system calls: files, the heap), compiled
/// for Linux on RV64 (`Options::linux`) and run with spike's proxy kernel (pk: the same
/// system call numbers as Linux): the same output. Without spike / pk (SPIKE, PK) only
/// the executables are checked.
#[test]
fn cc_linux_programs() {
    let dir = format!("{}/tests", env!("CARGO_MANIFEST_DIR"));
    let spike = std::env::var("SPIKE").unwrap_or("spike".into());
    let pk = std::env::var("PK").unwrap_or("/opt/homebrew/opt/riscv-pk/riscv64-unknown-elf/bin/pk".into());
    let have = std::path::Path::new(&pk).exists() && std::process::Command::new(&spike).arg("--help").output().is_ok();
    let tmp = std::env::temp_dir().join(format!("veryl_cc_linux_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let mut n = 0;
    let files: Vec<std::path::PathBuf> = [dir.clone(), format!("{}/linux", dir)].iter().flat_map(|d| std::fs::read_dir(d).unwrap().map(|f| f.unwrap().path())).collect();
    for p in files {
        if p.extension().map(|e| e == "c") != Some(true) { continue; }
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&p).unwrap();
        let o = crate::cc::compile(&format!("{}.c", name), &src, &crate::cc::Options { linux: true, ..Default::default() }).unwrap_or_else(|e| panic!("{}: {:?}", name, e));
        let elf = o.elf.unwrap();
        assert_eq!(&elf[..4], b"\x7fELF");
        assert_eq!(u16::from_le_bytes([elf[18], elf[19]]), 243, "EM_RISCV");
        n += 1;
        if !have { continue; }
        let exe = tmp.join(&name);
        std::fs::write(&exe, &elf).unwrap();
        let r = std::process::Command::new(&spike).args(["--isa=rv64imac", &pk, exe.to_str().unwrap()]).current_dir(&tmp).output().unwrap();
        let got = String::from_utf8_lossy(&r.stdout).replace("bbl loader\r\n", "").replace("bbl loader\n", "");
        let want = std::fs::read_to_string(p.with_extension("out")).unwrap();
        assert_eq!(got, want, "{} (spike pk)", name);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(n >= 4);
    eprintln!("cc_linux_programs: {} programs{}", n, if have { " match on spike + pk" } else { " compiled (no spike / pk)" });
}

/// compile errors point at the line
#[test]
fn cc_errors() {
    let e = |src: &str| crate::cc::compile("t.c", src, &Default::default()).err().unwrap_or_default();
    let x = e("int main(void) {\n  int a;\n  b = 1;\n  return 0;\n}\n");
    assert!(x.iter().any(|d| d.line == 3 && d.msg.contains("'b' is not declared")), "{:?}", x);
    let x = e("int main(void) {\n  int *p;\n  p = 5;\n  foo(1);\n}\n");
    assert!(x.iter().any(|d| d.line == 3 && d.msg.contains("cast")), "{:?}", x);
    assert!(x.iter().any(|d| d.line == 4 && d.msg.contains("foo is not declared")), "{:?}", x);
    let x = e("int f(int a) { return a; }\nint main(void) { return f(1, 2) }\n");
    assert!(x.iter().any(|d| d.line == 2), "{:?}", x);
    let x = e("int main(void) { struct s x; }\n");
    assert!(!x.is_empty());
}

/// the assembler round trip: every RV32I instruction the compiler can emit, assembled and executed
#[test]
fn asm_and_iss() {
    let p = crate::asm::assemble(".text\n_start:\n li sp, 0x1000\n li a0, 0x12345678\n li a1, -5\n add a2, a0, a1\n la t0, val\n lw t1, 0(t0)\n sw a2, 4(t0)\n lbu t2, 3(t0)\n lb t3, 2(t0)\n call f\n j .\nf:\n addi a0, a0, 1\n ret\n .data\nval: .word 0x80ff0102, 0\n").unwrap();
    let mut s = crate::iss::Iss::new(&p.bytes, 8192);
    assert!(s.run(100).unwrap());
    let r = |n: &str| s.x[crate::asm::reg(n).unwrap() as usize];
    assert_eq!((r("a0"), r("a2"), r("t1"), r("t2"), r("t3")), (0x12345679, 0x12345673, 0x80ff0102, 0x80, 0xffffffff));
    assert!(crate::asm::assemble("addi a0, a0, 5000").is_err());
}
