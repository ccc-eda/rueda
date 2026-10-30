//! The language-independent EDA back end. Every HDL front end (veryl/synth,
//! later fluxa) produces a [`netlist::Netlist`] of yosys internal gate cells;
//! from there on nothing depends on the source language:
//!
//! - `netlist`, `bram`, `output`, `sdc`, `power`, `netsim`: the gate netlist, block
//!   RAM inference, yosys JSON / Verilog writers, timing constraints, power and a
//!   gate-level simulator
//! - `fpga`: iCE40 and ECP5 technology mapping, place & route, bitstreams and
//!   the virtual iCEBreaker board
//! - `asic`, `pdk`: the standard-cell flow (vlib or SkyWater SKY130) to a GDS

pub mod asic;
pub mod bram;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod fpga;
pub mod netlist;
pub mod netsim;
pub mod output;
pub mod pdk;
pub mod power;
pub mod sdc;
