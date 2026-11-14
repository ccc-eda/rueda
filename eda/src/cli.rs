//! Command-line pieces of the back end that need no HDL front end (`reda pdk`).

use std::path::PathBuf;

/// `reda pdk [DIR]`: read SKY130's Liberty and LEF files, print what the flow uses
pub fn pdk_check(dir: Option<&String>) -> ! {
    use crate::pdk;
    let d = dir.map(PathBuf::from).or_else(pdk::sky130_dir).unwrap_or_else(|| { eprintln!("no SKY130 directory (give one, or set VERYL_EDA_SKY130)"); std::process::exit(1) });
    let t0 = std::time::Instant::now();
    let t = pdk::sky130_from_dir(&d).unwrap_or_else(|e| { eprintln!("{}: {}", d.display(), e); std::process::exit(1) });
    let (lib, lef) = (t.liberty.as_ref().unwrap(), t.lef.as_ref().unwrap());
    println!("{}: Liberty {} ({} cells, {} V), LEF {} macros, {} layers, {} vias  ({:.2} s)", d.display(), lib.name, lib.cells.len(), lib.voltage, lef.macros.len(), lef.layers.len(), lef.vias.len(), t0.elapsed().as_secs_f64());
    println!("site {} {} x {} µm, {} tracks per row; routing:", t.site, t.site_w, t.row_h, t.tracks);
    for l in lef.routing() { println!("  {:5} {} pitch {} width {} spacing {} R {} Ω/sq", l.name, if l.horizontal { "H" } else { "V" }, if l.horizontal { l.pitch.1 } else { l.pitch.0 }, l.width, l.spacing, l.rpersq); }
    let mut names: Vec<&String> = t.cells.keys().collect();
    names.sort();
    println!("logical cell -> library cell (function checked against the Liberty file), width in sites, pin access");
    for n in names {
        let b = &t.cells[n];
        let acc: Vec<String> = b.access.iter().map(|a| format!("{}={}@{},{}", a.0, b.lib_pin(a.0), a.1, a.2)).collect();
        println!("  {:7} {:28} {:3} {:7.3} µm²  {}", n, b.cell, b.width, b.area, acc.join(" "));
    }
    std::process::exit(0)
}

/// `reda board [--parts] FILE.board`: the board's parts and nets, and its electrical rule check
pub fn board(args: &[String]) -> ! {
    if args.iter().any(|a| a == "--parts") {
        println!("parts:");
        for k in crate::pcb::parts::KINDS {
            println!("  {:8} {}", k.name, k.desc.1);
            println!("           pins: {}", k.pins.iter().map(|p| format!("{} {}", p.0, p.1)).collect::<Vec<_>>().join(", "));
        }
        println!("host connectors (plug PART HOST):");
        for h in crate::pcb::parts::HOSTS {
            println!("  {:18} FPGA pins {}", h.name, h.pins.iter().map(|p| p.map_or("-".to_string(), |p| p.to_string())).collect::<Vec<_>>().join(" "));
        }
        std::process::exit(0);
    }
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let [file] = files.as_slice() else {
        eprintln!("usage: reda board [--parts] FILE.board (reda help board)");
        std::process::exit(2)
    };
    let src = std::fs::read_to_string(file).unwrap_or_else(|e| { eprintln!("{}: {}", file, e); std::process::exit(2) });
    let b = crate::pcb::Board::parse(&src).unwrap_or_else(|e| { eprintln!("{}: {}", file, e); std::process::exit(1) });
    print!("{}", b.report());
    let d = crate::pcb::erc::check(&b);
    for x in &d { println!("{}", x); }
    let errors = d.iter().filter(|x| x.error).count();
    println!("ERC: {} error(s), {} warning(s)", errors, d.len() - errors);
    std::process::exit(if errors > 0 { 1 } else { 0 })
}
