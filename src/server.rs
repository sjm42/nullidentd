use std::{collections::BTreeSet, net::SocketAddr, sync::Arc, time::Duration};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::{
    io::{self, AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream, lookup_host},
    sync::watch,
    task::{JoinError, JoinSet},
    time::{self, Instant},
};

use crate::*;

const MAX_CONNECTIONS: usize = 64;
const MAX_REQUEST: usize = 1024;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Run all configured listeners until interrupted or a listener fails fatally.
pub async fn run(opts: OptsCommon) -> io::Result<()> {
    opts.validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut signals = ShutdownSignals::new()?;
    let listeners = tokio::select! {
        result = bind_listeners(&opts.listen) => result?,
        result = signals.recv() => return result,
    };
    let (shutdown, shutdown_rx) = watch::channel(false);
    let mut acceptors = JoinSet::new();
    let mut connections = JoinSet::new();
    let ident: Arc<str> = opts.ident.into();
    let timeout = Duration::from_secs(opts.timeout);

    for listener in listeners {
        info!("Listening on {}", listener.local_addr()?);
        acceptors.spawn(
            Acceptor {
                listener,
                last_error_log: None,
            }
            .accept(),
        );
    }

    let result = loop {
        tokio::select! {
            biased;
            result = signals.recv() => break result,
            Some(result) = connections.join_next(), if !connections.is_empty() => report_connection_task(result),
            result = acceptors.join_next() => {
                let (acceptor, socket, peer) = match result {
                    Some(Ok(Ok(accepted))) => accepted,
                    Some(Ok(Err(error))) => break Err(error),
                    Some(Err(error)) => break Err(io::Error::other(format!("listener task failed: {error}"))),
                    None => break Err(io::Error::other("no listeners remain")),
                };
                let local = match socket.local_addr() {
                    Ok(local) => local,
                    Err(error) => break Err(error),
                };
                acceptors.spawn(acceptor.accept());
                // Reap completed tasks first; never spawn tasks waiting for capacity.
                if connections.len() >= MAX_CONNECTIONS {
                    debug!(%peer, %local, "Connection limit reached; closing connection");
                    continue;
                }
                let ident = Arc::clone(&ident);
                let shutdown = shutdown_rx.clone();
                connections.spawn(async move {
                    debug!(%peer, %local, "New connection");
                    match time::timeout(timeout, process_conn(socket, &ident, shutdown)).await {
                        Ok(Ok(())) => debug!(%peer, %local, "Connection closed"),
                        Ok(Err(error)) => debug!(%peer, %local, %error, "Connection failed"),
                        Err(_) => debug!(%peer, %local, "Connection lifetime exceeded"),
                    }
                });
            }
        }
    };

    info!("Shutting down");
    shutdown.send_replace(true);
    acceptors.abort_all();
    while acceptors.join_next().await.is_some() {}

    let mut result = result;
    tokio::select! {
        _ = drain_connections(&mut connections) => {},
        _ = time::sleep(SHUTDOWN_GRACE) => {
            warn!("Shutdown grace period expired; aborting remaining connections");
        },
        signal = signals.recv() => {
            info!("Second shutdown signal; aborting remaining connections");
            if let Err(error) = signal {
                result = Err(error);
            }
        },
    }
    connections.abort_all();
    drain_connections(&mut connections).await;
    result
}

async fn bind_listeners(addresses: &[String]) -> io::Result<Vec<TcpListener>> {
    let mut listeners = Vec::new();
    let mut seen = BTreeSet::new();
    for configured in addresses {
        let resolved: BTreeSet<_> = lookup_host(configured.as_str())
            .await
            .map_err(|error| io::Error::new(error.kind(), format!("cannot resolve {configured}: {error}")))?
            .collect();
        if resolved.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                format!("no addresses for {configured}"),
            ));
        }
        for address in resolved {
            if address.port() != 0 && !seen.insert(address) {
                continue;
            }
            let listener = bind_listener(address).map_err(|error| {
                io::Error::new(error.kind(), format!("cannot bind {address} ({configured}): {error}"))
            })?;
            listeners.push(listener);
        }
    }
    if listeners.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "at least one listen address is required",
        ));
    }
    Ok(listeners)
}

fn bind_listener(address: SocketAddr) -> io::Result<TcpListener> {
    let socket = Socket::new(Domain::for_address(address), Type::STREAM, Some(Protocol::TCP))?;
    if address.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&address.into())?;
    socket.listen(128)?;
    TcpListener::from_std(socket.into())
}

struct Acceptor {
    listener: TcpListener,
    last_error_log: Option<Instant>,
}

impl Acceptor {
    async fn accept(mut self) -> io::Result<(Self, TcpStream, SocketAddr)> {
        let local = self.listener.local_addr()?;
        let mut backoff = Duration::from_millis(100);
        loop {
            match self.listener.accept().await {
                Ok((socket, peer)) => return Ok((self, socket, peer)),
                Err(error) if fatal_accept_error(&error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("listener {local} failed: {error}"),
                    ));
                }
                Err(error) => {
                    // Includes descriptor/memory exhaustion. Avoid a busy loop and a log flood.
                    if self
                        .last_error_log
                        .is_none_or(|last| last.elapsed() >= Duration::from_secs(30))
                    {
                        error!(%local, %error, "Accept failed; retrying with backoff");
                        self.last_error_log = Some(Instant::now());
                    }
                    time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(1));
                }
            }
        }
    }
}

fn fatal_accept_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::InvalidInput | io::ErrorKind::NotConnected | io::ErrorKind::Unsupported
    )
}

fn report_connection_task(result: Result<(), JoinError>) {
    if let Err(error) = result
        && !error.is_cancelled()
    {
        error!(%error, "Connection task failed");
    }
}

async fn drain_connections(connections: &mut JoinSet<()>) {
    while let Some(result) = connections.join_next().await {
        report_connection_task(result);
    }
}

async fn process_conn<S>(socket: S, ident: &str, mut shutdown: watch::Receiver<bool>) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut reader = BufReader::with_capacity(MAX_REQUEST, socket);
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let request = tokio::select! {
            biased;
            _ = shutdown.changed() => return Ok(()),
            request = read_request(&mut reader) => request?,
        };
        let Some(request) = request else { return Ok(()) };
        let response = match parse_query(&request) {
            Query::Ports(server, client) => format!("{server},{client} : USERID : UNIX :{ident}\r\n"),
            Query::InvalidPorts(server, client) => format!("{server},{client} : ERROR : INVALID-PORT\r\n"),
            Query::Malformed => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "malformed ident query"));
            }
        };
        // Once a reply starts, finish it before observing graceful shutdown.
        reader.get_mut().write_all(response.as_bytes()).await?;
        reader.get_mut().flush().await?;
    }
}

async fn read_request<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut request = Vec::new();
    loop {
        let bytes = reader.fill_buf().await?;
        if bytes.is_empty() {
            return if request.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(io::ErrorKind::InvalidData, "incomplete ident query"))
            };
        }
        let newline = bytes.iter().position(|&byte| byte == b'\n');
        let length = newline.map_or(bytes.len(), |index| index + 1);
        if request.len() + length > MAX_REQUEST {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ident query exceeds 1024 bytes",
            ));
        }
        request.extend_from_slice(&bytes[..length]);
        reader.consume(length);
        if newline.is_some() {
            if !request.ends_with(b"\r\n") {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "ident query requires CRLF"));
            }
            request.truncate(request.len() - 2);
            return Ok(Some(request));
        }
    }
}

#[derive(Debug, PartialEq)]
enum Query<'a> {
    Ports(u16, u16),
    InvalidPorts(&'a str, &'a str),
    Malformed,
}

fn parse_query(request: &[u8]) -> Query<'_> {
    let Ok(request) = std::str::from_utf8(request) else {
        return Query::Malformed;
    };
    let Some((server, client)) = request.split_once(',') else {
        return Query::Malformed;
    };
    let server = server.trim_matches([' ', '\t']);
    let client = client.trim_matches([' ', '\t']);
    let port = |value: &str| {
        (value.len() <= 5 && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.parse::<u16>().ok().filter(|&port| port != 0))
            .flatten()
    };
    if let (Some(server), Some(client)) = (port(server), port(client)) {
        return Query::Ports(server, client);
    }
    // Echo only short numeric fields in INVALID-PORT replies; never reflect arbitrary probe text.
    let numeric = |value: &str| {
        !value.is_empty()
            && value.len() <= 32
            && value.bytes().any(|byte| byte.is_ascii_digit())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.'))
    };
    if numeric(server) && numeric(client) {
        Query::InvalidPorts(server, client)
    } else {
        Query::Malformed
    }
}

#[cfg(unix)]
struct ShutdownSignals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl ShutdownSignals {
    fn new() -> io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    async fn recv(&mut self) -> io::Result<()> {
        let signal = tokio::select! {
            signal = self.interrupt.recv() => signal,
            signal = self.terminate.recv() => signal,
        };
        signal.ok_or_else(|| io::Error::other("shutdown signal stream closed"))
    }
}

#[cfg(not(unix))]
struct ShutdownSignals;

#[cfg(not(unix))]
impl ShutdownSignals {
    fn new() -> io::Result<Self> {
        Ok(Self)
    }

    async fn recv(&mut self) -> io::Result<()> {
        tokio::signal::ctrl_c().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, duplex};

    #[tokio::test]
    async fn validates_programmatically_supplied_options() {
        let opts = OptsCommon {
            ident: "injected\r\nreply".into(),
            ..OptsCommon::default()
        };
        assert_eq!(run(opts).await.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        let opts = OptsCommon {
            timeout: u64::MAX,
            ..OptsCommon::default()
        };
        assert_eq!(run(opts).await.unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn binds_all_addresses_resolved_for_a_hostname() {
        let expected: BTreeSet<_> = lookup_host("localhost:0")
            .await
            .unwrap()
            .map(|address| address.ip())
            .collect();
        let listeners = bind_listeners(&["localhost:0".into()]).await.unwrap();
        let bound: BTreeSet<_> = listeners
            .iter()
            .map(|listener| listener.local_addr().unwrap().ip())
            .collect();
        assert_eq!(bound, expected);
    }

    #[test]
    fn parses_ports_and_whitespace() {
        for (query, ports) in [("1,65535", (1, 65535)), (" \t00123 \t, 00456\t ", (123, 456))] {
            assert_eq!(parse_query(query.as_bytes()), Query::Ports(ports.0, ports.1));
        }
    }

    #[test]
    fn rejects_invalid_ports_and_probe_text() {
        for query in ["0,1", "65536,1", "-1,1", "+1,1", "1.5,1", "000001,1"] {
            assert!(
                matches!(parse_query(query.as_bytes()), Query::InvalidPorts(..)),
                "{query}"
            );
        }
        for query in [
            "",
            "GET / HTTP/1.1",
            "abc,123",
            "1,2,3",
            "1,2 : ERROR : HIDDEN-USER",
            "1,2\0",
            "1,2\r",
            "１,2",
            "1 2,3",
        ] {
            assert_eq!(parse_query(query.as_bytes()), Query::Malformed, "{query:?}");
        }
        assert_eq!(parse_query(b"\xff,1"), Query::Malformed);
    }

    #[tokio::test]
    async fn framing_requires_crlf_and_enforces_boundary() {
        let valid = [b" ".repeat(MAX_REQUEST - 5), b"1,2\r\n".to_vec()].concat();
        assert_eq!(valid.len(), MAX_REQUEST);
        let mut reader = BufReader::with_capacity(7, valid.as_slice());
        assert_eq!(read_request(&mut reader).await.unwrap().unwrap().len(), MAX_REQUEST - 2);
        assert!(read_request(&mut reader).await.unwrap().is_none());
        for bytes in [
            b"1,2\n".to_vec(),
            b"1,2".to_vec(),
            b"1,2\r".to_vec(),
            vec![b'x'; MAX_REQUEST + 1],
        ] {
            let mut reader = BufReader::with_capacity(7, bytes.as_slice());
            assert_eq!(
                read_request(&mut reader).await.unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[tokio::test]
    async fn handles_fragmented_and_pipelined_requests() {
        let (mut client, server) = duplex(2048);
        let (_shutdown, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(async move { process_conn(server, "nobody", shutdown_rx).await });
        client.write_all(b"1,").await.unwrap();
        tokio::task::yield_now().await;
        client.write_all(b"2\r").await.unwrap();
        tokio::task::yield_now().await;
        client.write_all(b"\n3,4\r\n0,5\r\n").await.unwrap();
        client.shutdown().await.unwrap();
        let mut replies = String::new();
        client.read_to_string(&mut replies).await.unwrap();
        assert_eq!(
            replies,
            "1,2 : USERID : UNIX :nobody\r\n3,4 : USERID : UNIX :nobody\r\n0,5 : ERROR : INVALID-PORT\r\n"
        );
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn malformed_input_is_not_echoed() {
        let (mut client, server) = duplex(2048);
        let (_shutdown, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(async move { process_conn(server, "user", shutdown_rx).await });
        client.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.is_empty());
        assert_eq!(task.await.unwrap().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn shutdown_closes_idle_connections() {
        let (_client, server) = duplex(64);
        let (shutdown, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(async move { process_conn(server, "user", shutdown_rx).await });
        tokio::task::yield_now().await;
        shutdown.send_replace(true);
        time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn shutdown_finishes_started_response_without_reading_next_query() {
        let (mut client, server) = duplex(16);
        let (shutdown, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(async move { process_conn(server, "nobody", shutdown_rx).await });
        client.write_all(b"1,2\r\n3,4\r\n").await.unwrap();
        // Reading one byte proves the server has started its backpressured write.
        let mut first = [0];
        client.read_exact(&mut first).await.unwrap();
        shutdown.send_replace(true);
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).await.unwrap();
        let response = [first.as_slice(), &rest].concat();
        assert_eq!(response, b"1,2 : USERID : UNIX :nobody\r\n");
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn connection_write_errors_are_propagated() {
        let (mut client, server) = duplex(64);
        let (_shutdown, shutdown_rx) = watch::channel(false);
        client.write_all(b"1,2\r\n").await.unwrap();
        drop(client);
        assert!(process_conn(server, "user", shutdown_rx).await.is_err());
    }

    #[tokio::test]
    async fn ipv4_and_ipv6_wildcards_can_share_a_port() {
        let address = "[::]:0".parse().unwrap();
        let ipv6 = match bind_listener(address) {
            Ok(listener) => listener,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::AddrNotAvailable | io::ErrorKind::Unsupported
                ) =>
            {
                return;
            }
            Err(error) => panic!("IPv6 bind failed: {error}"),
        };
        let port = ipv6.local_addr().unwrap().port();
        let ipv4 = bind_listener(SocketAddr::from(([0, 0, 0, 0], port))).unwrap();
        assert_eq!(ipv4.local_addr().unwrap().port(), port);
    }
}
