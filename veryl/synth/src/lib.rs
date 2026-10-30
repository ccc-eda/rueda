//! Veryl synthesis front end: elaborates Veryl (parsed by veryl-parser) into
//! eda's gate netlist ([`elab::synthesize`]); the rest of the flow (FPGA, ASIC)
//! is the language-independent `eda` crate. Also the post-synthesis simulation
//! (`gatesim`: the testbenches against a netlist) and the command line.

#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod elab;
pub mod expr;
pub mod gatesim;
pub mod value;

#[cfg(test)]
mod tests;
