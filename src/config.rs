// config.rs

use crate::*;

#[derive(Debug, Parser)]
pub struct OptsCommon {
    #[arg(short, long)]
    pub verbose: bool,
    #[arg(short, long)]
    pub debug: bool,
    #[arg(short, long)]
    pub trace: bool,

    #[arg(short, long, default_value = "localhost:1113", value_delimiter = ',', num_args = 1..)]
    pub listen: Vec<String>,
    /// Maximum connection lifetime in seconds (1..=3600).
    #[arg(long, default_value = "5", value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub timeout: u64,
    /// Fixed username (1..=512 printable ASCII bytes).
    #[arg(long, default_value = "user", value_parser = parse_ident)]
    pub ident: String,
}

impl Default for OptsCommon {
    fn default() -> Self {
        Self::parse_from(["nullidentd"])
    }
}

fn parse_ident(value: &str) -> Result<String, String> {
    if value.is_empty() || value.len() > 512 || !value.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
        return Err("ident must contain 1..=512 printable ASCII bytes".into());
    }
    Ok(value.to_owned())
}

impl OptsCommon {
    pub(crate) fn validate(&self) -> Result<(), String> {
        parse_ident(&self.ident)?;
        if !(1..=3600).contains(&self.timeout) {
            return Err("timeout must be between 1 and 3600 seconds".into());
        }
        Ok(())
    }

    pub fn get_loglevel(&self) -> Level {
        if self.trace {
            Level::TRACE
        } else if self.debug {
            Level::DEBUG
        } else if self.verbose {
            Level::INFO
        } else {
            Level::ERROR
        }
    }

    pub fn start_pgm(&self, name: &str) {
        tracing_subscriber::fmt()
            .with_max_level(self.get_loglevel())
            .with_target(false)
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .init();

        info!("Starting up {name} v{}...", env!("CARGO_PKG_VERSION"));
        debug!("Git branch: {}", env!("GIT_BRANCH"));
        debug!("Git commit: {}", env!("GIT_COMMIT"));
        debug!("Source timestamp: {}", env!("SOURCE_TIMESTAMP"));
        debug!("Compiler version: {}", env!("RUSTC_VERSION"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_ident() {
        for ident in ["", "a\rb", "a\nb", "a\0b", "a\tb", "a\u{7f}", "å"] {
            assert!(parse_ident(ident).is_err(), "{ident:?}");
        }
        assert!(parse_ident(&"a".repeat(513)).is_err());
        assert_eq!(parse_ident("nobody").unwrap(), "nobody");
        assert!(parse_ident(&"a".repeat(512)).is_ok());
    }

    #[test]
    fn validates_timeout() {
        for timeout in ["0", "3601", "18446744073709551615"] {
            assert!(OptsCommon::try_parse_from(["nullidentd", "--timeout", timeout]).is_err());
        }
        for timeout in ["1", "3600"] {
            assert!(OptsCommon::try_parse_from(["nullidentd", "--timeout", timeout]).is_ok());
        }
    }

    #[test]
    fn defaults_and_log_levels() {
        let mut opts = OptsCommon::default();
        assert_eq!(opts.listen, ["localhost:1113"]);
        assert_eq!(opts.timeout, 5);
        assert_eq!(opts.ident, "user");
        assert_eq!(opts.get_loglevel(), Level::ERROR);
        opts.verbose = true;
        assert_eq!(opts.get_loglevel(), Level::INFO);
        opts.debug = true;
        assert_eq!(opts.get_loglevel(), Level::DEBUG);
        opts.trace = true;
        assert_eq!(opts.get_loglevel(), Level::TRACE);
    }
}
// EOF
