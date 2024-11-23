// bin/nullidentd

use std::net;

use clap::Parser;
use tokio::io::{self, AsyncWriteExt, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::time;
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, LinesCodec};

use nullidentd::*;

const BUF_SZ: usize = 8 * 1024;

fn main() -> Result<(), io::Error> {
    let opts = OptsCommon::parse();
    opts.start_pgm(env!("CARGO_BIN_NAME"));

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move { run_server(&opts).await })
}

async fn run_server(opts: &OptsCommon) -> Result<(), io::Error> {
    let addr = &opts.listen;
    let listener = TcpListener::bind(addr).await?;
    info!("Listening on {addr}");
    let mut i: u64 = 0;
    let timeout = time::Duration::from_secs(opts.timeout);
    loop {
        let (socket, c_addr) = listener.accept().await?;
        i += 1;
        let cn = i;
        let ident = opts.ident.clone();

        tokio::spawn(async move {
            let res = time::timeout(timeout, process_conn(ident, socket, c_addr, cn)).await;
            if let Err(e) = res {
                error!("Connection #{cn} timed out : {e}");
            }
        });
    }
}

async fn process_conn(
    ident: String,
    mut socket: TcpStream,
    addr: net::SocketAddr,
    cn: u64,
) -> Result<(), io::Error> {
    info!("New connection #{cn} from {addr:?}");

    let (sock_r, sock_w) = socket.split();
    let mut reader = FramedRead::new(sock_r, LinesCodec::new_with_max_length(BUF_SZ));
    let mut writer = BufWriter::new(sock_w);

    while let Some(Ok(line)) = reader.next().await {
        debug!("Read line: {line:?}");
        let response = format!("{line} : USERID : UNIX : {ident}\r\n");
        writer.write_all(response.as_bytes()).await?;
        writer.flush().await?;
    }
    info!("Connection #{cn} closed.");
    Ok(())
}
// EOF
