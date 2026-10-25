//! fdpick UI 로직. Win32 와 무관한 순수 상태 머신이다.

pub mod controller;
pub mod pathutil;
pub mod view;

#[cfg(test)]
mod controller_tests;
