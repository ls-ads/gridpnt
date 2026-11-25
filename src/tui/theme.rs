//! Colours. Catppuccin Mocha is the built-in default. (LOOK-001)
//!
//! No background colour is ever painted except for highlights, so a
//! transparent terminal stays transparent. (LOOK-003)

use ratatui::style::Color;

pub const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
pub const SUBTEXT: Color = Color::Rgb(0xa6, 0xad, 0xc8);
pub const OVERLAY: Color = Color::Rgb(0x6c, 0x70, 0x86);
pub const SURFACE2: Color = Color::Rgb(0x58, 0x5b, 0x70);
pub const SURFACE0: Color = Color::Rgb(0x31, 0x32, 0x44);
pub const CRUST: Color = Color::Rgb(0x11, 0x11, 0x1b);

pub const MAUVE: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
pub const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);
pub const PEACH: Color = Color::Rgb(0xfa, 0xb3, 0x87);
pub const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
pub const TEAL: Color = Color::Rgb(0x94, 0xe2, 0xd5);
pub const BLUE: Color = Color::Rgb(0x89, 0xb4, 0xfa);
