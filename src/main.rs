mod cli;
mod config;
mod feed;

fn main() {
    if let Err(e) = cli::run() {
        eprintln!("feedr: {e}");
        std::process::exit(1);
    }
}
