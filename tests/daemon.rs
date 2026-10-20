#![cfg(unix)]

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const BIN: &str = env!("CARGO_BIN_EXE_nullidentd");

struct Daemon {
    child: Child,
    logs: Receiver<String>,
    addresses: Vec<SocketAddr>,
}

impl Daemon {
    fn start(listen: &str, count: usize, timeout: &str, fd_limit: Option<u32>) -> Self {
        let mut command = if let Some(limit) = fd_limit {
            let mut command = Command::new("sh");
            command.args(["-c", &format!("ulimit -n {limit}; exec \"$@\""), "nullidentd-test", BIN]);
            command
        } else {
            Command::new(BIN)
        };
        let mut child = command
            .args([
                "--listen",
                listen,
                "--timeout",
                timeout,
                "--ident",
                "nobody",
                "--verbose",
            ])
            .env("TOKIO_WORKER_THREADS", "2")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (send, logs) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let mut daemon = Self {
            child,
            logs,
            addresses: Vec::new(),
        };
        for _ in 0..count {
            let line = daemon.wait_log("Listening on ");
            let address = line.split_once("Listening on ").unwrap().1.trim().parse().unwrap();
            daemon.addresses.push(address);
        }
        daemon
    }

    fn wait_log(&self, text: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(7);
        loop {
            let line = self
                .logs
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if line.contains(text) {
                return line;
            }
        }
    }

    fn signal(&self, signal: &str) {
        assert!(
            Command::new("kill")
                .args([signal, &self.child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
    }

    fn wait_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(7);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "daemon did not shut down");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn connect(address: SocketAddr) -> TcpStream {
    let socket = TcpStream::connect_timeout(&address, Duration::from_secs(1)).unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    socket.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
    socket
}

fn query(socket: &mut TcpStream, request: &[u8]) -> io::Result<String> {
    socket.write_all(request)?;
    let mut response = String::new();
    BufReader::new(socket).read_line(&mut response)?;
    Ok(response)
}

#[test]
fn serves_ident_and_invalid_port_replies() {
    let mut daemon = Daemon::start("127.0.0.1:0", 1, "5", None);
    let mut socket = connect(daemon.addresses[0]);
    assert_eq!(
        query(&mut socket, b" \t123 , 456 \t\r\n").unwrap(),
        "123,456 : USERID : UNIX :nobody\r\n"
    );
    assert_eq!(
        query(&mut socket, b"0,65536\r\n").unwrap(),
        "0,65536 : ERROR : INVALID-PORT\r\n"
    );
    assert_eq!(query(&mut socket, b"GET / HTTP/1.1\r\n").unwrap(), "");
    daemon.signal("-TERM");
    assert!(daemon.wait_exit().success());
}

#[test]
fn occupied_listener_fails_startup_without_reporting_readiness() {
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = format!("127.0.0.1:0,{}", occupied.local_addr().unwrap());
    let output = Command::new(BIN)
        .args(["--listen", &listen, "--verbose"])
        .env("TOKIO_WORKER_THREADS", "2")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cannot bind"), "{stderr}");
    assert!(!stderr.contains("Listening on"), "{stderr}");
}

#[test]
fn global_connection_limit_is_shared_and_releases_slots() {
    let mut daemon = Daemon::start("127.0.0.1:0,127.0.0.1:0", 2, "30", None);
    let mut clients = Vec::new();
    for i in 0..64 {
        let mut socket = connect(daemon.addresses[i % 2]);
        assert_eq!(
            query(&mut socket, b"1,2\r\n").unwrap(),
            "1,2 : USERID : UNIX :nobody\r\n"
        );
        clients.push(socket);
    }
    let mut extra = connect(daemon.addresses[1]);
    let mut byte = [0];
    match extra.read(&mut byte) {
        Ok(0) => {}
        Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
        result => panic!("connection above limit was not closed: {result:?}"),
    }
    clients.pop();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut socket = connect(daemon.addresses[0]);
        if query(&mut socket, b"1,2\r\n").is_ok_and(|reply| reply == "1,2 : USERID : UNIX :nobody\r\n") {
            break;
        }
        assert!(Instant::now() < deadline, "connection slot was not released");
        thread::sleep(Duration::from_millis(10));
    }
    daemon.signal("-TERM");
    assert!(daemon.wait_exit().success());
}

#[test]
fn timeout_limits_total_connection_lifetime_even_with_queries() {
    let mut daemon = Daemon::start("127.0.0.1:0", 1, "1", None);
    let mut socket = connect(daemon.addresses[0]);
    let deadline = Instant::now() + Duration::from_secs(3);
    assert_eq!(
        query(&mut socket, b"1,2\r\n").unwrap(),
        "1,2 : USERID : UNIX :nobody\r\n"
    );
    loop {
        thread::sleep(Duration::from_millis(100));
        match query(&mut socket, b"1,2\r\n") {
            Ok(reply) if !reply.is_empty() => {}
            _ => break,
        }
        assert!(Instant::now() < deadline, "queries extended connection lifetime");
    }
    daemon.signal("-TERM");
    assert!(daemon.wait_exit().success());
}

#[test]
fn ctrl_c_and_sigterm_close_idle_connections_cleanly() {
    for signal in ["-INT", "-TERM"] {
        let mut daemon = Daemon::start("127.0.0.1:0", 1, "30", None);
        let mut socket = connect(daemon.addresses[0]);
        assert_eq!(
            query(&mut socket, b"1,2\r\n").unwrap(),
            "1,2 : USERID : UNIX :nobody\r\n"
        );
        daemon.signal(signal);
        assert!(daemon.wait_exit().success());
        assert_eq!(socket.read(&mut [0]).unwrap(), 0);
    }
}

#[test]
fn descriptor_exhaustion_recovers_after_clients_disconnect() {
    let mut daemon = Daemon::start("127.0.0.1:0", 1, "30", Some(40));
    let clients: Vec<_> = (0..70).map(|_| connect(daemon.addresses[0])).collect();
    daemon.wait_log("Accept failed; retrying with backoff");
    assert!(daemon.child.try_wait().unwrap().is_none());
    drop(clients);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut socket = connect(daemon.addresses[0]);
        if query(&mut socket, b"1,2\r\n").is_ok_and(|reply| reply == "1,2 : USERID : UNIX :nobody\r\n") {
            break;
        }
        assert!(Instant::now() < deadline, "listener did not recover");
        thread::sleep(Duration::from_millis(50));
    }
    daemon.signal("-TERM");
    assert!(daemon.wait_exit().success());
}

#[test]
fn blocked_response_is_aborted_at_deadline_or_second_signal() {
    for force in [false, true] {
        let mut daemon = Daemon::start("127.0.0.1:0", 1, "30", None);
        // A tiny receive window plus pipelined requests blocks a response write.
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .unwrap();
        socket.set_recv_buffer_size(1024).unwrap();
        socket
            .connect_timeout(&daemon.addresses[0].into(), Duration::from_secs(1))
            .unwrap();
        let mut socket: TcpStream = socket.into();
        socket.set_write_timeout(Some(Duration::from_millis(200))).unwrap();
        match socket.write_all(&b"1,2\r\n".repeat(1_000_000)) {
            Ok(()) => {}
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            result => panic!("cannot pipeline requests: {result:?}"),
        }
        thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        daemon.signal("-INT");
        daemon.wait_log("Shutting down");
        if force {
            daemon.signal("-INT");
            daemon.wait_log("Second shutdown signal");
        } else {
            daemon.wait_log("Shutdown grace period expired");
        }
        assert!(daemon.wait_exit().success());
        if force {
            assert!(
                started.elapsed() < Duration::from_secs(4),
                "second signal did not skip grace period"
            );
        }
    }
}
