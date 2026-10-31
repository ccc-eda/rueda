//! `reda emu [--top TASK] [--test NAME]... FILE.fx...`: the program's tests with its tasks (or
//! TASK) compiled by fluxa-emu instead of interpreted; the exit code as `reda sim`'s.
//! `reda emu -o OUT.wasm [--top TASK] [--param NAME=VALUE]... FILE.fx...`: TASK as a WebAssembly
//! module (and OUT.json for the harness, js/emu.mjs).

fn usage() -> ! {
    eprintln!("usage: reda emu [--top TASK] [--test NAME]... FILE.fx...\n       reda emu -o OUT.wasm [--top TASK] [--param NAME=VALUE]... FILE.fx...");
    std::process::exit(2)
}

pub fn main(args: &[String]) {
    let (mut top, mut only, mut files, mut out, mut params) = (None::<String>, vec![], vec![], None::<String>, vec![]);
    let mut a = args.iter().cloned();
    while let Some(x) = a.next() {
        match x.as_str() {
            "--top" | "-s" => top = Some(a.next().unwrap_or_else(|| usage())),
            "-o" => out = Some(a.next().unwrap_or_else(|| usage())),
            "--param" | "-P" => {
                let kv = a.next().unwrap_or_else(|| usage());
                let (k, v) = kv.split_once('=').unwrap_or_else(|| usage());
                params.push((k.to_string(), v.to_string()));
            }
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
    if let Some(out) = out {
        let mut ps = std::collections::HashMap::new();
        for (k, v) in params {
            let x = match v.parse::<i128>() { Ok(n) => fluxa::value::Value::Int(n), Err(_) => fluxa::eval_str(&prog, &v).unwrap_or_else(|e| { eprintln!("--param {}: {}", k, e); std::process::exit(1) }) };
            ps.insert(k, x);
        }
        let top = top.or_else(|| fluxa_synth::default_top(&prog)).unwrap_or_else(|| { eprintln!("no task to compile"); std::process::exit(1) });
        let t0 = std::time::Instant::now();
        let m = crate::compile(&prog, &top, &ps).unwrap_or_else(|e| { eprintln!("fluxa-emu: {}", e); std::process::exit(1) });
        let w = crate::wgen::compile(&m);
        std::fs::write(&out, &w.wasm).unwrap_or_else(|e| { eprintln!("{}: {}", out, e); std::process::exit(1) });
        let json = out.strip_suffix(".wasm").unwrap_or(&out).to_string() + ".json";
        std::fs::write(&json, &w.json).unwrap();
        eprintln!("{}: {}\nwrote {} ({} KB) and {} in {:.1}s", top, m.stats(), out, w.wasm.len() / 1024, json, t0.elapsed().as_secs_f64());
        return;
    }
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
