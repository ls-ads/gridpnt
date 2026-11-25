//! Handing SQL to a running gridpnt from another process.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use gridpnt::config::parse_ini;
use gridpnt::remote::{listen_in, send_in};

/// A private directory for one test, removed afterwards.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("gridpnt-test-remote-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Dir(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn sql_reaches_the_running_instance() {
    let dir = Dir::new("basic");
    let (delivered, inbox) = mpsc::channel();
    let _listener = listen_in(&dir.0, "work", move |sql| delivered.send(sql).unwrap()).unwrap();

    send_in(&dir.0, "SELECT 1;\n-- with a comment\n", None).unwrap();
    assert_eq!(
        inbox.recv_timeout(Duration::from_secs(2)).unwrap(),
        "SELECT 1;\n-- with a comment\n"
    );
}

#[test]
fn writes_are_refused_before_they_are_delivered() {
    let dir = Dir::new("refuse");
    let (delivered, inbox) = mpsc::channel();
    let _listener = listen_in(&dir.0, "work", move |sql| delivered.send(sql).unwrap()).unwrap();

    let error = send_in(&dir.0, "DROP TABLE t", None).unwrap_err();
    assert!(error.starts_with("refused (read-only)"), "{error}");
    assert!(inbox.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn the_wrong_secret_is_denied() {
    let dir = Dir::new("secret");
    let (delivered, inbox) = mpsc::channel();
    let _listener = listen_in(&dir.0, "work", move |sql| delivered.send(sql).unwrap()).unwrap();

    // Find the port the way another local user could: it is not secret.
    let file = std::fs::read_dir(&dir.0).unwrap().next().unwrap().unwrap().path();
    let announced = parse_ini(&std::fs::read_to_string(file).unwrap());
    let port: u16 = announced[0].get("port").unwrap().parse().unwrap();
    assert_eq!(announced[0].get("secret").unwrap().len(), 64);

    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(b"not-the-secret\nSELECT 1").unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();

    assert_eq!(reply, "denied");
    assert!(inbox.recv_timeout(Duration::from_millis(200)).is_err());
}

#[cfg(unix)]
#[test]
fn the_secret_file_is_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new("private");
    let _listener = listen_in(&dir.0, "work", |_| {}).unwrap();

    let file = std::fs::read_dir(&dir.0).unwrap().next().unwrap().unwrap().path();
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&dir.0).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn with_nothing_running_the_sender_is_told() {
    let dir = Dir::new("none");
    let error = send_in(&dir.0, "SELECT 1", None).unwrap_err();
    assert!(error.starts_with("no running gridpnt found"), "{error}");
}

#[test]
fn a_stopped_instance_is_forgotten() {
    let dir = Dir::new("stale");
    let listener = listen_in(&dir.0, "work", |_| {}).unwrap();
    drop(listener);
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[test]
fn a_name_that_matches_nothing_is_reported() {
    let dir = Dir::new("named");
    let _listener = listen_in(&dir.0, "work", |_| {}).unwrap();
    let error = send_in(&dir.0, "SELECT 1", Some("elsewhere")).unwrap_err();
    assert!(error.starts_with("no running gridpnt found"), "{error}");
    assert!(send_in(&dir.0, "SELECT 1", Some("work")).is_ok());
}
