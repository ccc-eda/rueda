//! The Veryl synthesis command line: `reda synth / fpga / asic / gatesim`.

use crate::elab;
use eda::fpga::{ecp5, ice40, pnr};
use eda::{asic, bram, netlist, output};
use std::sync::OnceLock;

/// set by `reda` to its subcommand: argument errors point to `reda help <subcommand>`
pub static CMD: OnceLock<&'static str> = OnceLock::new();

use std::path::PathBuf;
use veryl_parser::Parser;

fn usage() -> ! {
    let cmd = CMD.get().copied().unwrap_or("synth");
    eprintln!("reda {}: wrong arguments (reda help {})", cmd, cmd);
    std::process::exit(2)
}

/// the constraint report: per clock its period and endpoints, WNS / TNS, the failing endpoints
pub fn slack_line(s: &eda::asic::sta::Slack) -> String {
    let clocks: Vec<String> = s.clocks.iter().map(|(n, p, e)| format!("{} {:.3} ns ({:.1} MHz, {} endpoints)", n, p, 1000.0 / p, e)).collect();
    format!("timing: constraints {}: WNS {:+.3} ns, TNS {:.3} ns, {} of {} endpoints failing{}{}",
        if clocks.is_empty() { "(IO only)".to_string() } else { clocks.join(", ") }, s.wns, s.tns, s.failing, s.checked,
        if s.failing == 0 { ", met" } else { "" }, if s.failing > 0 { format!(" (worst: {})", s.worst) } else { String::new() })
}

/// run Veryl synthesis with command-line arguments (without the program name); exits on errors
pub fn main(all: Vec<String>) {
    let (mut top, mut json, mut verilog, mut list) = (None::<String>, None::<String>, None::<String>, false);
    let (mut ice40, mut pcf, mut asc, mut bin) = (false, None::<String>, None::<String>, None::<String>);
    let (mut device, mut package) = (None::<String>, None::<String>);
    let (mut ecp5, mut lpf, mut config, mut bit) = (false, None::<String>, None::<String>, None::<String>);
    let (mut pnr_tool, mut seed, mut verbose) = ("nextpnr".to_string(), 1u64, false);
    let mut asic_flow = false;
    let mut pdk_name = "vlib".to_string();
    let mut def_out: Option<String> = None;
    let mut map_only = false;
    let mut gds_out: Option<String> = None;
    let mut spice_out: Option<String> = None;
    let mut sdc: Option<eda::sdc::Constraints> = None;
    let mut gatesim: Option<String> = None;
    let mut timing_driven = true;
    let mut program: Option<String> = None;
    let mut files = vec![];
    let mut a = all.iter().cloned();
    while let Some(x) = a.next() {
        match x.as_str() {
            "--top" | "-s" => top = a.next(),
            "-o" | "--json" => json = a.next(),
            "--verilog" => verilog = a.next(),
            "--list" => list = true,
            "--ice40" => ice40 = true,
            "--ecp5" => ecp5 = true,
            "--lpf" => lpf = a.next(),
            "--config" => config = a.next(),
            "--bit" => bit = a.next(),
            "--asic" => asic_flow = true,
            "--pdk" => pdk_name = a.next().unwrap_or_else(|| usage()),
            "--def" => def_out = a.next(),
            "--map-only" => map_only = true,
            "--gds" => gds_out = a.next(),
            "--spice" => spice_out = a.next(),
            "--sdc" => {
                let f = a.next().unwrap_or_else(|| usage());
                let text = std::fs::read_to_string(&f).unwrap_or_else(|e| { eprintln!("{}: {}", f, e); std::process::exit(1) });
                sdc = Some(eda::sdc::parse(&text).unwrap_or_else(|e| { eprintln!("{}: {}", f, e); std::process::exit(2) }));
            }
            "--clock" => {
                let mhz: f64 = a.next().and_then(|s| s.parse().ok()).filter(|m: &f64| *m > 0.0).unwrap_or_else(|| usage());
                sdc = Some(eda::sdc::Constraints { clocks: vec![eda::sdc::Clock { name: "clk".into(), period: 1000.0 / mhz, port: None }], ..Default::default() });
            }
            "--gatesim" => gatesim = Some(a.next().unwrap_or_else(|| usage())),
            "--no-timing-driven" => timing_driven = false,
            "--pcf" => pcf = a.next(),
            "--asc" => asc = a.next(),
            "--bin" => bin = a.next(),
            "--program" => program = a.next(),
            "--device" => device = Some(a.next().unwrap_or_else(|| usage())),
            "--package" => package = Some(a.next().unwrap_or_else(|| usage())),
            "--pnr" => pnr_tool = a.next().unwrap_or_else(|| usage()),
            "--seed" => seed = a.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()),
            "-v" | "--verbose" => verbose = true,
            "-h" | "--help" => usage(),
            o if o.starts_with('-') && o.len() > 1 => { eprintln!("unknown option {}", o); usage() }
            _ => files.push(PathBuf::from(x)),
        }
    }
    if files.is_empty() { usage(); }
    let tech = eda::pdk::by_name(&pdk_name).unwrap_or_else(|e| { eprintln!("--pdk: {}", e); std::process::exit(2) });
    eda::pdk::set_tech(Some(tech.clone()));
    // the target clock (the first constrained one) for nextpnr and the FPGA timing report
    let target = sdc.as_ref().and_then(|c| c.clocks.iter().find(|k| k.port.is_some()).or(c.clocks.first()).map(|k| k.period));
    eda::sdc::set(sdc);

    let mut parsers = vec![];
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap_or_else(|e| { eprintln!("{}: {}", f.display(), e); std::process::exit(1) });
        match Parser::parse(&src, f) {
            Ok(p) => parsers.push(p),
            Err(e) => { eprintln!("{}: parse error: {:?}", f.display(), e); std::process::exit(1) }
        }
    }
    let mut lib = elab::Lib::new();
    for p in &parsers { lib.add(&p.veryl); }
    if list {
        for (n, t) in &lib.order { println!("{}{}", n, if *t { "  (#[test])" } else { "" }); }
        return;
    }
    let top = top.or_else(|| lib.order.iter().rev().find(|(_, t)| !t).map(|x| x.0.clone()))
        .unwrap_or_else(|| { eprintln!("no synthesizable (non-#[test]) module found"); std::process::exit(1) });

    if let Some(kind) = &gatesim {
        let srcs: Vec<(String, String)> = files.iter().map(|f| (f.display().to_string(), std::fs::read_to_string(f).unwrap())).collect();
        let prov = crate::gatesim::Provider::new(&lib, &top, kind);
        let f = |m: &str, ov: &std::collections::HashMap<String, veryl_sim::value::Val>| prov.get(m, ov);
        let r = veryl_sim::run_gates(&srcs, None, 1_000_000, false, true, Some(&f));
        for (ps, n) in prov.built.borrow().iter() {
            println!("gate-level simulation: {}{} as {} cells ({})", top, if ps.is_empty() { String::new() } else { format!(" #({})", ps) }, n, kind);
        }
        match r {
            Ok(r) => std::process::exit(if r.tops.iter().any(|t| t.fails > 0) { 1 } else { 0 }),
            Err(e) => { eprintln!("{}", e); std::process::exit(1) }
        }
    }
    let (mut nl, warnings) = match elab::synthesize(&lib, &top) {
        Ok(r) => r,
        Err(e) => { eprintln!("synthesis error in '{}': {}", top, e); std::process::exit(1) }
    };
    if ecp5 && ice40 { eprintln!("--ecp5 and --ice40 exclude each other"); std::process::exit(2); }
    if !ecp5 && (asc.is_some() || bin.is_some()) { ice40 = true; }
    if ecp5 && (asc.is_some() || bin.is_some() || pcf.is_some()) { eprintln!("ECP5: the outputs are --config / --bit, the pins --lpf"); std::process::exit(2); }
    let device = device.unwrap_or_else(|| if ecp5 { "85k" } else { "up5k" }.to_string());
    let package = package.unwrap_or_else(|| if ecp5 { "CABGA381" } else { "sg48" }.to_string());
    let mut asic_out = None;
    let mut asic_models = false;
    if asic_flow {
        if ice40 || ecp5 { eprintln!("--asic and an FPGA exclude each other"); std::process::exit(2); }
        if !tech.is_vlib() { println!("asic: technology {} ({} µm rows, {} µm sites)", tech.name, tech.row_h, tech.site_w); }
        if map_only {
            // technology mapping only (no place & route): for the netlist
            let m = asic::map::map(&nl).unwrap_or_else(|e| { eprintln!("ASIC mapping error: {}", e); std::process::exit(1) });
            nl = eda::pdk::to_library(&m);
            if verilog.is_some() { asic_models = true; }
        } else {
            let out = asic::flow(&nl, seed).unwrap_or_else(|e| { eprintln!("ASIC flow error: {}", e); std::process::exit(1) });
            nl = eda::pdk::to_library(&out.mapped);
            asic_out = Some(out);
        }
    }
    if ice40 {
        nl = ice40::map(&nl).unwrap_or_else(|e| { eprintln!("iCE40 mapping error: {}", e); std::process::exit(1) });
        if let Some(p) = &program {
            let bytes = std::fs::read(p).unwrap_or_else(|e| { eprintln!("{}: {}", p, e); std::process::exit(1) });
            let lanes = bram::program_lanes(&bram::mem_names(&nl), &bytes).unwrap_or_else(|e| { eprintln!("--program: {}", e); std::process::exit(1) });
            for (m, words) in lanes { bram::set_mem_init(&mut nl, &m, &words); }
            println!("program: {} bytes in the block RAMs", bytes.len());
        }
    } else if ecp5 {
        nl = ecp5::map(&nl).unwrap_or_else(|e| { eprintln!("ECP5 mapping error: {}", e); std::process::exit(1) });
        if program.is_some() { eprintln!("--program: ECP5 block RAMs come in a later version"); std::process::exit(2); }
    } else if program.is_some() {
        eprintln!("--program needs --ice40"); std::process::exit(2);
    }
    for w in warnings { eprintln!("warning: {}", w); }

    println!("=== {} ===", top);
    let st = nl.stats();
    let total: usize = st.iter().map(|x| x.1).sum();
    for p in &nl.ports { println!("  port {:6} {:3} {}", if p.dir == netlist::Dir::In { "input" } else { "output" }, p.bits.len(), p.name); }
    println!("  cells: {}", total);
    for (t, n) in st { println!("    {:14} {}", t, n); }
    if let Some(o) = &asic_out {
        let (pl, rt, tm) = (&o.placement, &o.routing, &o.timing);
        println!("asic: area {:.1} µm² (cells), die {:.1} × {:.1} µm, {} rows, utilization {:.0}%, fullest 16 µm bin {:.0}% (target {:.0}%)", o.area, pl.w, pl.h, pl.rows, pl.util * 100.0, pl.max_density * 100.0, pl.target_density * 100.0);
        println!("asic: wirelength {:.0} µm, {} vias, overflow {} after {} routing iterations", rt.wirelength, rt.vias, rt.overflow, rt.iters.len());
        let pw = &o.power;
        println!("power: {:.3} mW at {:.1} MHz = dynamic {:.3} + leakage {:.4}; per cycle {:.2} pJ (nets {:.2}, clock {:.2}, internal {:.2}, SRAM {:.2}); IR drop worst {:.2} mV ({} random cycles)",
            pw.total, pw.freq_mhz, pw.dynamic, pw.leakage, pw.e_nets + pw.e_clock + pw.e_internal + pw.e_sram, pw.e_nets, pw.e_clock, pw.e_internal, pw.e_sram, pw.ir_worst, pw.cycles);
        let pd = &o.pdn;
        println!("asic: power grid {} rails, {} straps, {} via stacks; {} filler cells", pd.rails, pd.straps.len(), pd.vias.len(), pd.fillers.len());
        let d = &o.detail;
        if !tech.is_vlib() {
            match asic::layout::build(o) {
                Ok(l) => {
                    let r = asic::drc::check(&l, &o.mapped);
                    println!("asic: DRC {} shapes: {} shorts, {} spacing, {} min-area, {} opens; {} taps ({} sites too far)", r.shapes, r.shorts, r.spacing, r.area, r.opens, o.pdn.taps.len(), o.pdn.tap_viol);
                    if verbose { for e in &r.details { println!("  {}", e); } }
                    let v = asic::lvs::check(&l, o);
                    println!("asic: LVS {} nets, {} terminals, {} devices: {} opens, {} shorts, {} device errors{} ({} pieces, {} floating)",
                        v.nets, v.terminals, v.devices, v.opens, v.shorts, v.device_errors, if v.errors() == 0 { ", layout matches the netlist" } else { "" }, v.pieces, v.floating);
                    if verbose || v.errors() > 0 { for e in &v.details { println!("  {}", e); } }
                    let a = asic::antenna::check(&l, &o.mapped);
                    let mut per: Vec<(String, usize)> = vec![];
                    for v in &a.violations { match per.iter_mut().find(|p| p.0 == v.layer) { Some(p) => p.1 += 1, None => per.push((v.layer.clone(), 1)) } }
                    let per = if per.is_empty() { String::new() } else { format!(" ({})", per.iter().map(|p| format!("{} {}", p.0, p.1)).collect::<Vec<_>>().join(", ")) };
                    println!("asic: antenna {} pieces with gates checked: {} violations{} (worst {:.0}% of its limit); {} diodes inserted", a.checked, a.violations.len(), per, a.worst * 100.0, o.diodes);
                    if verbose || !a.violations.is_empty() {
                        for v in a.violations.iter().take(12) { println!("  {} on {}: gate {}: ratio {:.0} > {:.0}", v.net, v.layer, v.pin, v.ratio, v.limit); }
                    }
                }
                Err(e) => println!("asic: layout: {}", e),
            }
        }
        let per: Vec<String> = (0..4).map(|l| format!("{} {:.0}", tech.layers[l], d.per_layer[l])).collect();
        println!("asic: detailed routing {:.0} µm ({}), {} vias, {} conflicts, {} opens after {} iterations",
            d.wirelength, per.join(" / "), d.vias, d.conflicts, d.opens, d.iters.len());
        println!("asic: clock tree {} buffers ({}), insertion {:.3} ns, skew {:.3} ns; hold: worst slack {:.3} ns, {} delay buffers",
            o.cts.buffers, o.cts.trees.iter().map(|t| format!("{} sinks / {} levels", t.1, t.3)).collect::<Vec<_>>().join(", "), tm.insertion, tm.skew, tm.hold_slack, o.hold_bufs);
        if let Some(e) = &o.extraction { println!("asic: extraction {} nets from the detailed routing: {:.3} pF of wire, the longest wire delay {:.1} ps", e.nets, e.cap.values().sum::<f64>(), e.worst * 1000.0); }
        println!("timing: critical path {:.3} ns, Fmax {:.1} MHz ({} endpoints; ideal clock {:.1} MHz, wirelength-only placement {:.1} MHz)", tm.period, tm.fmax, tm.endpoints, o.fmax_ideal, o.fmax_wl);
        if let Some(s) = &tm.slack { println!("{}", slack_line(s)); }
        if let (Some(b), Some(s)) = (o.repair.before, &tm.slack) {
            if o.repair.upsized + o.repair.buffers > 0 || b < 0.0 { println!("timing: repair {} cells made stronger, {} buffers: WNS {:+.3} -> {:+.3} ns", o.repair.upsized, o.repair.buffers, b, s.wns); }
        }
        if verbose { for s in &tm.path { println!("  {:8.3} {:+7.3}  {}", s.at, s.delay, s.what); } }
    }

    if let Some(p) = &json {
        std::fs::write(&p, output::to_json(&nl)).unwrap_or_else(|e| { eprintln!("{}: {}", p, e); std::process::exit(1) });
        println!("wrote {}", p);
    }
    if asc.is_some() || bin.is_some() {
        let dir = std::env::temp_dir().join(format!("rueda-synth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mapped = json.clone().map(PathBuf::from).unwrap_or_else(|| dir.join("mapped.json"));
        std::fs::write(&mapped, output::to_json(&nl)).unwrap();
        let asc_path = asc.clone().map(PathBuf::from).unwrap_or_else(|| dir.join("out.asc"));
        if pnr_tool == "veryl" {
            if device != "up5k" { eprintln!("--pnr veryl supports only --device up5k"); std::process::exit(1); }
            let pcf_map = match &pcf {
                Some(p) => pnr::parse_pcf(&std::fs::read_to_string(p).unwrap_or_else(|e| { eprintln!("{}: {}", p, e); std::process::exit(1) })),
                None => Default::default(),
            };
            let t0 = std::time::Instant::now();
            let opts = pnr::Opts { chipdb: None, package: package.clone(), pcf: pcf_map, seed, verbose, timing_driven };
            let out = pnr::place_and_route(&nl, &opts).unwrap_or_else(|e| { eprintln!("place & route error: {}", e); std::process::exit(1) });
            println!("  veryl pnr: {} LCs, {} routing pips, {:.1}s", out.lcs, out.wires, t0.elapsed().as_secs_f64());
            let pw = &out.power;
            println!("  power: {:.3} mW at {:.1} MHz = dynamic {:.3} + static {:.3}; per cycle {:.1} pJ (nets {:.1}, clock {:.1}, RAM {:.1})",
                pw.total, pw.freq_mhz, pw.dynamic, pw.static_mw, pw.e_nets + pw.e_clock + pw.e_ram, pw.e_nets, pw.e_clock, pw.e_ram);
            println!("  timing: critical path {:.2} ns, Fmax {:.2} MHz{}", out.timing.period, out.timing.fmax,
                out.fmax_wl.map(|f| format!(" (wirelength-only pass {:.2} MHz)", f)).unwrap_or_default());
            if let Some(p) = target { println!("  timing: target {:.3} ns ({:.2} MHz): slack {:+.3} ns, {}", p, 1000.0 / p, p - out.timing.period, if out.timing.period <= p { "met" } else { "VIOLATED" }); }
            if verbose { for st in &out.timing.path { println!("    {:7.3} {:6.3}  {}", st.at, st.delay, st.what); } }
            std::fs::write(&asc_path, &out.asc).unwrap();
            std::fs::write(asc_path.with_extension("pcf"), &out.pcf).unwrap();
            println!("wrote {} (+ .pcf)", asc_path.display());
            if let Some(b) = &bin {
                // own port of icepack (UP5K)
                let bits = pnr::bitstream::asc_to_bin(&out.asc).unwrap_or_else(|e| { eprintln!("bitstream: {}", e); std::process::exit(1) });
                std::fs::write(b, bits).unwrap();
                println!("wrote {}", b);
            }
        } else {
        let mut cmd = std::process::Command::new("nextpnr-ice40");
        cmd.arg(format!("--{}", device)).args(["--package", &package, "--json"]).arg(&mapped).arg("--asc").arg(&asc_path);
        match &pcf { Some(p) => { cmd.args(["--pcf", p]); } None => { cmd.arg("--pcf-allow-unconstrained"); } }
        if let Some(p) = target { cmd.args(["--freq", &format!("{:.3}", 1000.0 / p)]); }
        println!("running {:?}", cmd);
        let out = cmd.output().unwrap_or_else(|e| { eprintln!("cannot run nextpnr-ice40: {}", e); std::process::exit(1) });
        let log = String::from_utf8_lossy(&out.stderr);
        for l in log.lines().filter(|l| l.contains("ICESTORM_LC:") || l.contains("Max frequency") || l.contains("ERROR")) {
            println!("  {}", l.trim_start_matches("Info:").trim());
        }
        if !out.status.success() { eprintln!("nextpnr-ice40 failed"); std::process::exit(1); }
        println!("wrote {}", asc_path.display());
        }
        if let (Some(b), false) = (&bin, pnr_tool == "veryl") {
            let st = std::process::Command::new("icepack").arg(&asc_path).arg(b).status()
                .unwrap_or_else(|e| { eprintln!("cannot run icepack: {}", e); std::process::exit(1) });
            if !st.success() { eprintln!("icepack failed"); std::process::exit(1); }
            println!("wrote {}", b);
        }
    }
    if ecp5 && (config.is_some() || bit.is_some()) {
        let dir = std::env::temp_dir().join(format!("rueda-synth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mapped = json.clone().map(PathBuf::from).unwrap_or_else(|| dir.join("mapped.json"));
        std::fs::write(&mapped, output::to_json(&nl)).unwrap();
        let cfg = config.clone().map(PathBuf::from).unwrap_or_else(|| dir.join("out.config"));
        let mut cmd = std::process::Command::new("nextpnr-ecp5");
        cmd.arg(format!("--{}", device)).args(["--package", &package, "--seed", &seed.to_string(), "--json"]).arg(&mapped).arg("--textcfg").arg(&cfg);
        match &lpf { Some(p) => { cmd.args(["--lpf", p]); } None => { cmd.arg("--lpf-allow-unconstrained"); } }
        if let Some(p) = target { cmd.args(["--freq", &format!("{:.3}", 1000.0 / p)]); }
        println!("running {:?}", cmd);
        let out = cmd.output().unwrap_or_else(|e| { eprintln!("cannot run nextpnr-ecp5: {}", e); std::process::exit(1) });
        let log = String::from_utf8_lossy(&out.stderr);
        // utilisation of the final placement, the clocks' Fmax, errors
        let util: Vec<&str> = log.lines().filter(|l| l.contains("Device utilisation")).collect();
        let tail = log.rsplit("Device utilisation").next().unwrap_or("");
        if !util.is_empty() {
            for l in tail.lines().skip(1).take_while(|l| l.contains('/')).filter(|l| !l.contains(" 0/")) { println!("  {}", l.trim_start_matches("Info:").trim()); }
        }
        // nextpnr reports Fmax after placement and after routing: the last report of each clock
        let mut fmax: Vec<(String, String)> = vec![];
        for l in log.lines().filter(|l| l.contains("Max frequency for clock")) {
            let l = l.trim_start_matches("Info:").trim().to_string();
            let clk = l.split(':').next().unwrap_or("").to_string();
            match fmax.iter_mut().find(|x| x.0 == clk) { Some(x) => x.1 = l, None => fmax.push((clk, l)) }
        }
        for (_, l) in &fmax { println!("  {}", l); }
        for l in log.lines().filter(|l| l.contains("ERROR")) { println!("  {}", l.trim_start_matches("Info:").trim()); }
        if verbose { for l in log.lines().filter(|l| l.contains("Critical path") || l.contains("ns logic") || l.contains("ns routing")) { println!("  {}", l.trim_start_matches("Info:").trim()); } }
        if !out.status.success() { eprintln!("nextpnr-ecp5 failed"); std::process::exit(1); }
        if config.is_some() { println!("wrote {}", cfg.display()); }
        if let Some(b) = &bit {
            let st = std::process::Command::new("ecppack").arg(&cfg).arg("--bit").arg(b).status()
                .unwrap_or_else(|e| { eprintln!("cannot run ecppack: {}", e); std::process::exit(1) });
            if !st.success() { eprintln!("ecppack failed"); std::process::exit(1); }
            println!("wrote {}", b);
        }
    }
    if let (Some(p), Some(o)) = (&gds_out, &asic_out) {
        if tech.is_vlib() { eprintln!("--gds needs a PDK with cell layouts (--pdk sky130)"); std::process::exit(2); }
        let l = asic::layout::build(o).unwrap_or_else(|e| { eprintln!("layout: {}", e); std::process::exit(1) });
        let g = asic::gds::write(&l, &o.mapped.name, eda::pdk::SKY130_GDS).unwrap_or_else(|e| { eprintln!("GDS: {}", e); std::process::exit(1) });
        std::fs::write(p, &g).unwrap_or_else(|e| { eprintln!("{}: {}", p, e); std::process::exit(1) });
        println!("wrote {} ({} bytes: {} cell instances, {} shapes, {} labels)", p, g.len(), l.insts.len(), l.shapes.len(), l.labels.len());
    }
    if let (Some(p), Some(o)) = (&spice_out, &asic_out) {
        if tech.is_vlib() { eprintln!("--spice needs a PDK (--pdk sky130)"); std::process::exit(2); }
        // the PDK's cell subcircuits, where the full SKY130 files are
        let cdl = eda::pdk::sky130_dir().map(|d| d.join("sky130hd.cdl")).filter(|p| p.exists());
        let s = asic::spice::write(o, cdl.as_ref().and_then(|p| p.to_str())).unwrap_or_else(|e| { eprintln!("SPICE: {}", e); std::process::exit(1) });
        std::fs::write(p, &s).unwrap_or_else(|e| { eprintln!("{}: {}", p, e); std::process::exit(1) });
        println!("wrote {} ({} cells{})", p, o.mapped.cells.len(), if cdl.is_some() { "" } else { "; no sky130hd.cdl found: add the PDK's cell subcircuits yourself (tools/sky130.sh)" });
    }
    if let (Some(p), Some(o)) = (&def_out, &asic_out) {
        std::fs::write(p, asic::def(o)).unwrap_or_else(|e| { eprintln!("{}: {}", p, e); std::process::exit(1) });
        println!("wrote {}", p);
    }
    if let Some(p) = verilog {
        let mut v = output::to_verilog(&nl);
        if asic_out.is_some() || asic_models { v += &eda::pdk::verilog_models(); }
        std::fs::write(&p, v).unwrap_or_else(|e| { eprintln!("{}: {}", p, e); std::process::exit(1) });
        println!("wrote {}", p);
    }
}
