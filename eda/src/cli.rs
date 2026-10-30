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
