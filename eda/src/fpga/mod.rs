//! FPGA flow: technology mapping (iCE40 SB_LUT4 / SB_CARRY / SB_DFF* / SB_RAM40_4K,
//! ECP5 LUT4 / CCU2C / TRELLIS_FF / DP16KD), the own iCE40 place & route and
//! bitstream (`pnr`), and the virtual iCEBreaker board (`board`).

pub mod board;
pub mod ecp5;
pub mod ice40;
pub mod pnr;
