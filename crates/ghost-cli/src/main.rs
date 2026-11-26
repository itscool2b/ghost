//! The ghost-cli executable. Everything lives in the library (`ghost_cli`), where tests reach it.

fn main() -> std::process::ExitCode {
    ghost_cli::main_with(std::env::args_os().skip(1))
}
