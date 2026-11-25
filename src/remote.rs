//! Lets another program on this machine hand SQL to a running gridpnt:
//! `gridpnt run` in an editor keymap sends the statement under the cursor
//! to the gridpnt pane beside it. (EDIT-002)
//!
//! How it works: a running gridpnt listens on a port of this machine only
//! (127.0.0.1) and writes the port and a random secret to a file only the
//! user can read. `gridpnt run` reads that file, connects, and sends the
//! secret followed by the SQL. Another user on the same machine can reach
//! the port but cannot read the secret, so cannot use it.
//!
//! What arrives is treated exactly like SQL typed at the prompt: it goes
//! through the guard, and nothing is sent back but "sent" or a refusal.
//! Results stay on the gridpnt screen.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::config;
use crate::guard;

/// Largest request accepted, comfortably above any sensible statement.
const MAX_REQUEST_BYTES: u64 = 4 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// A running listener. Dropping it removes its file, so `gridpnt run`
/// stops finding this instance.
pub struct Listener {
    file: PathBuf,
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

/// Where instance files are kept. Private to the user.
fn instance_dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty());
    match runtime {
        Some(dir) if cfg!(unix) => Some(PathBuf::from(dir).join("gridpnt")),
        _ => config::cache_dir().map(|dir| dir.join("run")),
    }
}

/// Start listening and announce this instance in the default place.
pub fn listen(label: &str, deliver: impl Fn(String) + Send + 'static) -> Result<Listener, String> {
    let dir = instance_dir().ok_or("no place to announce this instance")?;
    listen_in(&dir, label, deliver)
}

/// Start listening, announcing the instance in `dir`. `deliver` is called
/// with each statement that arrives and passes the guard.
pub fn listen_in(dir: &Path, label: &str, deliver: impl Fn(String) + Send + 'static) -> Result<Listener, String> {
    let socket = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    let port = socket.local_addr().map_err(|e| e.to_string())?.port();

    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret).map_err(|e| format!("no randomness available: {e}"))?;
    let secret: String = secret.iter().map(|byte| format!("{byte:02x}")).collect();

    let file = dir.join(format!("{}.ini", std::process::id()));
    let announcement = format!("[instance]\nlabel = {label}\nport = {port}\nsecret = {secret}\n");
    write_private(dir, &file, &announcement).map_err(|e| format!("cannot write {}: {e}", file.display()))?;

    thread::spawn(move || {
        for stream in socket.incoming().flatten() {
            // One bad connection must not stop the listener.
            let _ = answer(stream, &secret, &deliver);
        }
    });
    Ok(Listener { file })
}

/// Create `file` so that only the current user can read it.
fn write_private(dir: &Path, file: &Path, content: &str) -> std::io::Result<()> {
    let mut make_dir = std::fs::DirBuilder::new();
    make_dir.recursive(true);
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        make_dir.mode(0o700);
        open.mode(0o600);
    }
    // On Windows the directory is inside the user's own profile, which
    // other users cannot read by default.
    make_dir.create(dir)?;
    let _ = std::fs::remove_file(file);
    open.open(file)?.write_all(content.as_bytes())
}

/// Handle one connection: check the secret, check the SQL, pass it on.
fn answer(mut stream: TcpStream, secret: &str, deliver: &impl Fn(String)) -> std::io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    let mut request = String::new();
    (&stream).take(MAX_REQUEST_BYTES).read_to_string(&mut request)?;
    let (offered, sql) = request.split_once('\n').unwrap_or((&request, ""));

    let reply = if !same(offered.as_bytes(), secret.as_bytes()) {
        "denied".to_string()
    } else {
        // Refuse here as well as in the interface, so the sender is told
        // straight away instead of having to look at the other pane.
        let verdict = guard::classify(sql);
        if verdict.allowed_read_only() {
            deliver(sql.to_string());
            "sent".to_string()
        } else {
            format!("refused (read-only): {}", verdict.reason)
        }
    };
    stream.write_all(reply.as_bytes())
}

/// Compare without stopping at the first difference, so the time taken
/// says nothing about how much of a guessed secret was right.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// An instance found on disk and confirmed to be answering.
struct Instance {
    label: String,
    secret: String,
    stream: TcpStream,
}

/// Send `sql` to a running gridpnt, from the default place.
pub fn send(sql: &str, to: Option<&str>) -> Result<(), String> {
    let dir = instance_dir().ok_or("no running gridpnt found")?;
    send_in(&dir, sql, to)
}

/// Send `sql` to the running gridpnt announced in `dir`. With several
/// running, `to` picks one by connection name.
pub fn send_in(dir: &Path, sql: &str, to: Option<&str>) -> Result<(), String> {
    let mut found: Vec<Instance> = instances(dir)
        .into_iter()
        .filter(|instance| to.is_none_or(|wanted| instance.label == wanted))
        .collect();

    let mut instance = match found.len() {
        0 => return Err("no running gridpnt found; start one in another pane first".into()),
        1 => found.remove(0),
        _ => {
            let labels: Vec<&str> = found.iter().map(|i| i.label.as_str()).collect();
            return Err(format!(
                "several gridpnt are running ({}); pick one with --to NAME",
                labels.join(", ")
            ));
        }
    };

    let failed = |e: std::io::Error| format!("could not talk to gridpnt: {e}");
    instance
        .stream
        .write_all(format!("{}\n{sql}", instance.secret).as_bytes())
        .map_err(failed)?;
    instance.stream.shutdown(Shutdown::Write).map_err(failed)?;
    let mut reply = String::new();
    instance.stream.read_to_string(&mut reply).map_err(failed)?;

    if reply == "sent" { Ok(()) } else { Err(reply) }
}

/// Every announced instance that still answers. Files left behind by a
/// gridpnt that is gone are removed along the way.
fn instances(dir: &Path) -> Vec<Instance> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut live = Vec::new();
    for file in entries.flatten().map(|entry| entry.path()) {
        if file.extension().is_none_or(|ext| ext != "ini") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let sections = config::parse_ini(&text);
        let field = |key: &str| sections.first().and_then(|s| s.get(key)).unwrap_or("").to_string();
        let port: u16 = field("port").parse().unwrap_or(0);

        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        match TcpStream::connect_timeout(&address, Duration::from_millis(500)) {
            Ok(stream) => {
                let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                live.push(Instance {
                    label: field("label"),
                    secret: field("secret"),
                    stream,
                });
            }
            Err(_) => {
                let _ = std::fs::remove_file(&file);
            }
        }
    }
    live
}
