use tokio::{io, runtime, time::Duration};

use nullidentd::*;

fn main() -> io::Result<()> {
    let opts = OptsCommon::parse();
    opts.start_pgm(env!("CARGO_BIN_NAME"));

    let runtime = runtime::Builder::new_multi_thread().enable_all().build()?;
    let result = runtime.block_on(run(opts));
    runtime.shutdown_timeout(Duration::from_secs(5));
    info!("Exit.");
    result
}
