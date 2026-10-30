//! RISC-V software tools: a C compiler (`cc`, programs for the RV32 SoC of
//! veryl/examples/src/rvsoc.veryl, or static RV64 Linux executables), an RV32I
//! assembler (`asm`) and an RV32IMA instruction-set simulator (`iss`). A compiled
//! program goes into the SoC's block RAMs (`eda::bram::program_lanes`), on the
//! virtual board or in a bitstream (`eda::fpga::pnr::patch_ram`).

pub mod asm;
pub mod cc;
pub mod iss;

#[cfg(test)]
mod tests;
