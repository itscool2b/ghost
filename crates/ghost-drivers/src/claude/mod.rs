//! Native Claude Code driver and protocol mapping.

mod rules;
mod session;
mod tasks;
pub mod translate;
pub mod wire;

#[cfg(test)]
mod tests;

use crate::driver::{AgentDriver, DriverContext, DriverExit};
use futures::{FutureExt, future::BoxFuture};

#[derive(Debug, Default)]
pub struct ClaudeDriver;

impl ClaudeDriver {
    pub fn new() -> Self {
        Self
    }
}

impl AgentDriver for ClaudeDriver {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        session::run(ctx).boxed()
    }
}
