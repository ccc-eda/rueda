//! The back end of the command line, the same for every language: a synthesized gate netlist
//! goes to the writers (yosys JSON, Verilog), the FPGA flows (iCE40: own or nextpnr place & route,
//! bitstream; ECP5: nextpnr-ecp5 + ecppack) or the ASIC flow (vlib or SKY130, DEF, GDS, SPICE).
//! A front end (`reda synth` for Veryl or Fluxa) parses its own options (`--top`, the files…),
//! leaves the rest to [`Opts::parse`], synthesizes, and calls [`run`].

use crate::fpga::{ecp5, ice40, pnr};
use crate::{asic, bram, netlist, output};
use std::path::PathBuf;

/// the back end's options (the flags of `reda synth / fpga / asic`)
pub struct Opts {
    pub json: Option<String>,
    pub verilog: Option<String>,
    pub ice40: bool,
    pub ecp5: bool,
    pub pcf: Option<String>,
    pub asc: Option<String>,
    pub bin: Option<String>,
    pub lpf: Option<String>,
    pub config: Option<String>,
    pub bit: Option<String>,
    pub device: Option<String>,
    pub package: Option<String>,
    pub pnr_tool: String,
    pub seed: u64,
    pub verbose: bool,
    pub timing_driven: bool,
    pub program: Option<String>,
    pub asic: bool,
    pub pdk: String,
    pub def: Option<String>,
    pub map_only: bool,
    pub gds: Option<String>,
    pub spice: Option<String>,
    pub sdc: Option<crate::sdc::Constraints>,
}

impl Opts {
    /// take the back end's flags out of `args`; the rest (the front end's options and the files) is
    /// returned in order. An error is a wrong value of one of these flags.
    pub fn parse(args: &[String]) -> Result<(Opts, Vec<String>), String> {
        let mut o = Opts {
            json: None, verilog: None, ice40: false, ecp5: false, pcf: None, asc: None, bin: None, lpf: None, config: None, bit: None,
            device: None, package: None, pnr_tool: "nextpnr".into(), seed: 1, verbose: false, timing_driven: true, program: None,
            asic: false, pdk: "vlib".into(), def: None, map_only: false, gds: None, spice: None, sdc: None,
        };
        let mut rest = vec![];
        let mut a = args.iter().cloned();
        let need = |v: Option<String>, f: &str| v.ok_or_else(|| format!("{} needs a value", f));
        while let Some(x) = a.next() {
            match x.as_str() {
                "-o" | "--json" => o.json = Some(need(a.next(), &x)?),
                "--verilog" => o.verilog = Some(need(a.next(), &x)?),
                "--ice40" => o.ice40 = true,
                "--ecp5" => o.ecp5 = true,
                "--lpf" => o.lpf = Some(need(a.next(), &x)?),
                "--config" => o.config = Some(need(a.next(), &x)?),
                "--bit" => o.bit = Some(need(a.next(), &x)?),
                "--asic" => o.asic = true,
                "--pdk" => o.pdk = need(a.next(), &x)?,
                "--def" => o.def = Some(need(a.next(), &x)?),
                "--map-only" => o.map_only = true,
                "--gds" => o.gds = Some(need(a.next(), &x)?),
                "--spice" => o.spice = Some(need(a.next(), &x)?),
                "--sdc" => {
                    let f = need(a.next(), &x)?;
                    let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {}", f, e))?;
                    o.sdc = Some(crate::sdc::parse(&text).map_err(|e| format!("{}: {}", f, e))?);
                }
                "--clock" => {
                    let mhz: f64 = need(a.next(), &x)?.parse().ok().filter(|m: &f64| *m > 0.0).ok_or("--clock needs a frequency in MHz")?;
                    o.sdc = Some(crate::sdc::Constraints { clocks: vec![crate::sdc::Clock { name: "clk".into(), period: 1000.0 / mhz, port: None }], ..Default::default() });
                }
                "--no-timing-driven" => o.timing_driven = false,
                "--pcf" => o.pcf = Some(need(a.next(), &x)?),
                "--asc" => o.asc = Some(need(a.next(), &x)?),
                "--bin" => o.bin = Some(need(a.next(), &x)?),
                "--program" => o.program = Some(need(a.next(), &x)?),
                "--device" => o.device = Some(need(a.next(), &x)?),
                "--package" => o.package = Some(need(a.next(), &x)?),
                "--pnr" => o.pnr_tool = need(a.next(), &x)?,
                "--seed" => o.seed = need(a.next(), &x)?.parse().map_err(|_| "--seed needs a number")?,
                "-v" | "--verbose" => o.verbose = true,
                _ => rest.push(x),
            }
        }
        Ok((o, rest))
    }

    /// set the technology and the constraints (before synthesis: they are read while mapping)
    pub fn apply(&self) -> Result<(), String> {
        let tech = crate::pdk::by_name(&self.pdk).map_err(|e| format!("--pdk: {}", e))?;
        crate::pdk::set_tech(Some(tech));
        crate::sdc::set(self.sdc.clone());
        Ok(())
    }
}

/// the constraint report: per clock its period and endpoints, WNS / TNS, the failing endpoints
pub fn slack_line(s: &asic::sta::Slack) -> String {
    let clocks: Vec<String> = s.clocks.iter().map(|(n, p, e)| format!("{} {:.3} ns ({:.1} MHz, {} endpoints)", n, p, 1000.0 / p, e)).collect();
    format!("timing: constraints {}: WNS {:+.3} ns, TNS {:.3} ns, {} of {} endpoints failing{}{}",
        if clocks.is_empty() { "(IO only)".to_string() } else { clocks.join(", ") }, s.wns, s.tns, s.failing, s.checked,
        if s.failing == 0 { ", met" } else { "" }, if s.failing > 0 { format!(" (worst: {})", s.worst) } else { String::new() })
}

/// the synthesized netlist `nl` of `top` through the flow `o` asks for: statistics, reports and
/// output files; exits on errors (it is the command line's back half)
pub fn run(mut nl: netlist::Netlist, warnings: Vec<String>, top: &str, o: &Opts) {
    let tech = crate::pdk::tech();
    let target = o.sdc.as_ref().and_then(|c| c.clocks.iter().find(|k| k.port.is_some()).or(c.clocks.first()).map(|k| k.period));
    let (mut ice40, ecp5, asic_flow, map_only, verbose, seed, timing_driven) = (o.ice40, o.ecp5, o.asic, o.map_only, o.verbose, o.seed, o.timing_driven);
    let (json, verilog, pcf, asc, bin, lpf, config, bit) = (o.json.clone(), o.verilog.clone(), o.pcf.clone(), o.asc.clone(), o.bin.clone(), o.lpf.clone(), o.config.clone(), o.bit.clone());
    let (program, pnr_tool, gds_out, spice_out, def_out) = (o.program.clone(), o.pnr_tool.clone(), o.gds.clone(), o.spice.clone(), o.def.clone());
    let (device, package) = (o.device.clone(), o.package.clone());
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
            nl = crate::pdk::to_library(&m);
            if verilog.is_some() { asic_models = true; }
        } else {
            let out = asic::flow(&nl, seed).unwrap_or_else(|e| { eprintln!("ASIC flow error: {}", e); std::process::exit(1) });
            nl = crate::pdk::to_library(&out.mapped);
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
        let g = asic::gds::write(&l, &o.mapped.name, crate::pdk::SKY130_GDS).unwrap_or_else(|e| { eprintln!("GDS: {}", e); std::process::exit(1) });
        std::fs::write(p, &g).unwrap_or_else(|e| { eprintln!("{}: {}", p, e); std::process::exit(1) });
        println!("wrote {} ({} bytes: {} cell instances, {} shapes, {} labels)", p, g.len(), l.insts.len(), l.shapes.len(), l.labels.len());
    }
    if let (Some(p), Some(o)) = (&spice_out, &asic_out) {
        if tech.is_vlib() { eprintln!("--spice needs a PDK (--pdk sky130)"); std::process::exit(2); }
        // the PDK's cell subcircuits, where the full SKY130 files are
        let cdl = crate::pdk::sky130_dir().map(|d| d.join("sky130hd.cdl")).filter(|p| p.exists());
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
        if asic_out.is_some() || asic_models { v += &crate::pdk::verilog_models(); }
        std::fs::write(&p, v).unwrap_or_else(|e| { eprintln!("{}: {}", p, e); std::process::exit(1) });
        println!("wrote {}", p);
    }
}
