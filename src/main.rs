mod cli;

fn main() {
    logger(env_logger::Env::default().default_filter_or("warn,postbode=info")).init();
    if let Err(e) = cli::run() {
        eprintln!(
            "error: {}",
            postbode::message::clean(&format!("{e:#}"), false)
        );
        std::process::exit(1);
    }
}

/// async-imap traces every command (LOGIN with the password) and response (bodies), so no RUST_LOG may enable it.
fn logger(env: env_logger::Env) -> env_logger::Builder {
    let mut builder = env_logger::Builder::from_env(env);
    for module in ["async_imap", "async_imap::imap_stream"] {
        builder.filter_module(module, log::LevelFilter::Debug);
    }
    builder
}

#[cfg(test)]
mod tests {
    use log::Log;

    #[test]
    fn rust_log_trace_never_enables_async_imap_traces() {
        for rust_log in ["trace", "async_imap=trace", "async_imap::imap_stream=trace"] {
            let logger =
                super::logger(env_logger::Env::new().filter_or("POSTBODE_TEST_UNSET", rust_log))
                    .build();
            let metadata = log::Metadata::builder()
                .level(log::Level::Trace)
                .target("async_imap::imap_stream")
                .build();
            assert!(!logger.enabled(&metadata), "{rust_log}");
        }
    }
}
