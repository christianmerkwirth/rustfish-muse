//! Rustfish binary: read UCI commands from stdin, write replies to stdout.

mod position;
mod search;
mod uci;

fn main() {
    let info = uci::EngineInfo {
        name: "Rustfish",
        version: env!("CARGO_PKG_VERSION"),
        author: "christianmerkwirth",
    };
    let stdin = std::io::stdin();
    run(&stdin, &mut std::io::stdout(), info);
}

fn run(stdin: &std::io::Stdin, stdout: &mut std::io::Stdout, info: uci::EngineInfo) {
    uci::run_loop(stdin.lock(), stdout.lock(), info);
}
