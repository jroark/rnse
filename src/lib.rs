//! Emulator for the Audi RNS-E navigation unit (Aisin AW, SH-4).

pub mod board;
pub mod bootrom;
pub mod dev;
pub mod fw;
pub mod iolog;
pub mod machine;
pub mod png;
pub mod sh4;
pub mod ui;
#[cfg(target_arch = "wasm32")]
pub mod web;
