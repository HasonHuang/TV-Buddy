//! 可用性校验与测速筛选。
//!
//! 设计见 `docs/design/13-availability-and-speedtest.md`。

pub mod probe;
pub mod rules;

pub mod schedule;
pub mod state;
