mod cli;
mod config;
mod feed;
mod tui;

fn main() {
    if let Err(e) = cli::run() {
        eprintln!("feedr: {e}");
        std::process::exit(1);
    }
}
