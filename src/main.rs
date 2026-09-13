//! Rustfish binary: read UCI commands from stdin, write replies to stdout.

mod eval;
mod position;
mod search;
mod tt;
mod uci;

fn main() {
    let info = uci::EngineInfo {
        name: "Rustfish",
        version: env!("CARGO_PKG_VERSION"),
        author: "christianmerkwirth",
    };
    let stdin = std::io::stdin();
    run(&stdin, std::io::stdout(), info);
}

fn run(stdin: &std::io::Stdin, stdout: std::io::Stdout, info: uci::EngineInfo) {
    uci::run_loop(stdin.lock(), stdout, info);
}
