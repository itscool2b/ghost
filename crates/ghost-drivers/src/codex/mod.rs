//! Native Codex app-server over an owned stdio process.

mod options;
mod requests;
mod session;
mod translate;
pub mod wire;

#[cfg(test)]
mod tests;

use crate::driver::{AgentDriver, DriverContext, DriverExit};
use futures::{FutureExt, future::BoxFuture};

#[derive(Debug, Default)]
pub struct CodexDriver;

impl CodexDriver {
    pub fn new() -> Self {
        Self
    }
}

impl AgentDriver for CodexDriver {
    fn run(self: Box<Self>, ctx: DriverContext) -> BoxFuture<'static, DriverExit> {
        session::run(ctx).boxed()
    }
}
