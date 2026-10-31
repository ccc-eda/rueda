//! `reda emu [--top TASK] [--test NAME]... FILE.fx...`: the program's tests with its tasks (or
//! TASK) compiled by fluxa-emu instead of interpreted; the exit code as `reda sim`'s.

fn usage() -> ! {
    eprintln!("usage: reda emu [--top TASK] [--test NAME]... FILE.fx...");
    std::process::exit(2)
}

pub fn main(args: &[String]) {
    let (mut top, mut only, mut files) = (None::<String>, vec![], vec![]);
    let mut a = args.iter().cloned();
    while let Some(x) = a.next() {
        match x.as_str() {
            "--top" | "-s" => top = Some(a.next().unwrap_or_else(|| usage())),
            "--test" => only.push(a.next().unwrap_or_else(|| usage())),
            "-h" | "--help" => usage(),
            o if o.starts_with('-') => { eprintln!("reda emu: unknown option {}", o); usage() }
            _ => files.push(x),
        }
    }
    if files.is_empty() { usage(); }
    let mut srcs = vec![];
    for f in &files {
        let s = std::fs::read_to_string(f).unwrap_or_else(|e| { eprintln!("{}: {}", f, e); std::process::exit(2) });
        srcs.push((f.clone(), s));
    }
    let prog = fluxa::load_many(&srcs).unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(2) });
    let p = std::sync::Arc::new(crate::Provider::new(&prog, top.as_deref()));
    let opts = fluxa::Options { hw: Some(p.clone()), coop: true, ..Default::default() };
    let (mut pass, mut fail) = (0, 0);
    let mut out = String::new();
    for t in prog.tests.iter().filter(|t| only.is_empty() || only.contains(&t.name)) {
        let r = fluxa::run_test(&prog, t, &opts);
        out += &r.output;
        if r.passed {
            pass += 1;
            out += &format!("[PASS] {} ({:.3}s, emu)\n", r.name, r.secs);
        } else {
            fail += 1;
            out += &format!("[FAIL] {} ({:.3}s, emu)\n", r.name, r.secs);
            for l in r.message.lines() { out += &format!("       {}\n", l); }
        }
    }
    for (task, r) in p.report.lock().unwrap().iter() {
        match r {
            Ok(s) => println!("fluxa-emu: {}: {}", task, s),
            Err(e) => println!("fluxa-emu: {} stays interpreted: {}", task, e),
        }
    }
    print!("{}", out);
    println!("{} passed, {} failed", pass, fail);
    if fail > 0 { std::process::exit(1); }
}
