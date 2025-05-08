// bin/nullidentd

use std::net;

use clap::Parser;
use tokio::io::{self, AsyncWriteExt, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::{task, time};
use tokio_stream::StreamExt;
use tokio_util::codec::{FramedRead, LinesCodec};

use nullidentd::*;

const BUF_SZ: usize = 8 * 1024;

fn main() -> Result<(), io::Error> {
    let opts = OptsCommon::parse();
    opts.start_pgm(env!("CARGO_BIN_NAME"));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        let mut tasks = task::JoinSet::new();
        for listen_addr in opts.listen {
            tasks.spawn(run_server(listen_addr, opts.timeout, opts.ident.clone()));
        }

        while let Some(res) = tasks.join_next().await {
            debug!("Server task result: {:?}", res);
        }
    });
    runtime.shutdown_timeout(time::Duration::new(5, 0));
    info!("Exit.");
    Ok(())
}

async fn run_server(listen_addr: String, timeout: u64, ident: String) -> Result<(), io::Error> {
    let listener = TcpListener::bind(&listen_addr).await?;
    info!("Listening on {listen_addr}");
    let mut i: u64 = 0;
    let timeout = time::Duration::from_secs(timeout);
    loop {
        let (socket, c_addr) = listener.accept().await?;
        i += 1;
        let cn = i;

        let id = ident.clone();
        let listen = listen_addr.clone();
        tokio::spawn(async move {
            if let Err(e) =
                time::timeout(timeout, process_conn(id, &listen, socket, c_addr, cn)).await
            {
                error!("Connection ({listen})#{cn} timed out : {e}");
            }
        });
    }
}

async fn process_conn(
    ident: String,
    listen_addr: &str,
    mut socket: TcpStream,
    addr: net::SocketAddr,
    cn: u64,
) -> Result<(), io::Error> {
    info!("New connection ({listen_addr})#{cn} from {addr:?}");
    let (sock_r, sock_w) = socket.split();
    let mut reader = FramedRead::new(sock_r, LinesCodec::new_with_max_length(BUF_SZ));
    let mut writer = BufWriter::new(sock_w);

    while let Some(Ok(line)) = reader.next().await {
        let response = format!("{line} : USERID : UNIX : {ident}\r\n");
        writer.write_all(response.as_bytes()).await?;
        writer.flush().await?;
    }
    info!("Connection ({listen_addr})#{cn} closed.");
    Ok(())
}
// EOF
