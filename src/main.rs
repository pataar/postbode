mod cli;

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,postbode=info"),
    )
    .init();
    if let Err(e) = cli::run() {
        eprintln!("error: {}", cli::clean(&format!("{e:#}"), false));
        std::process::exit(1);
    }
}
