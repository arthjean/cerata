//! One Cerata process per user: a second launch hands its arguments and
//! working directory to the running instance over a local socket (a named
//! pipe on Windows) and exits.
//!
//! Replaces `tauri-plugin-single-instance` (Writer `lib.rs:461-493`), with
//! Writer's defect fixed: the second launch's working directory is sent and
//! relative paths resolve against it (Writer ignored it, line 495).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use interprocess::ConnectWaitMode;
use interprocess::local_socket::{
    ConnectOptions, GenericFilePath, Listener, ListenerOptions, Stream, prelude::*,
};
use serde::{Deserialize, Serialize};

/// Largest request accepted, far above any real argument list.
const MAX_REQUEST: u64 = 64 * 1024;
/// How long a second launch waits for the running instance.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const ACK: &[u8] = b"ok\n";

/// What a launch asks the running instance to open.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchRequest {
    pub cwd: PathBuf,
    /// The launch's arguments after the program name, in order.
    pub args: Vec<String>,
}

impl LaunchRequest {
    pub fn from_env(args: impl IntoIterator<Item = std::ffi::OsString>, cwd: PathBuf) -> Self {
        Self {
            cwd,
            args: args
                .into_iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        }
    }

    /// Each argument as a path, relative ones joined to the launch's
    /// working directory.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.args.iter().map(|arg| self.cwd.join(arg)).collect()
    }
}

/// Where the instances of one user meet: a socket file in the data
/// directory, or a named pipe whose name derives from it on Windows.
#[derive(Clone, Debug)]
pub struct Endpoint(PathBuf);

impl Endpoint {
    pub fn for_data_dir(data_dir: &Path) -> Self {
        if cfg!(windows) {
            // FNV-1a of the data directory: per user, stable, no separators.
            let hash = data_dir
                .to_string_lossy()
                .bytes()
                .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
                });
            Self(PathBuf::from(format!(r"\\.\pipe\cerata-{hash:016x}")))
        } else {
            Self(data_dir.join("cerata.sock"))
        }
    }
}

/// The outcome of [`claim`].
pub enum Claim {
    /// This process is the instance; it serves later launches.
    Primary(Listener),
    /// A running instance took the request; this process should exit.
    Forwarded,
}

/// Hands `request` to the running instance, or becomes the instance.
pub fn claim(endpoint: &Endpoint, request: &LaunchRequest) -> io::Result<Claim> {
    // Two attempts: a listener that appears between our failed connect and
    // our bind wins, and the second connect reaches it.
    for _ in 0..2 {
        if forward(endpoint, request).is_ok() {
            return Ok(Claim::Forwarded);
        }
        match listen(endpoint) {
            Ok(listener) => return Ok(Claim::Primary(listener)),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                if forward(endpoint, request).is_ok() {
                    return Ok(Claim::Forwarded);
                }
                remove_stale(endpoint);
            }
            Err(error) => return Err(error),
        }
    }
    listen(endpoint).map(Claim::Primary)
}

fn forward(endpoint: &Endpoint, request: &LaunchRequest) -> io::Result<()> {
    let name = endpoint.0.as_path().to_fs_name::<GenericFilePath>()?;
    let mut stream = ConnectOptions::new()
        .name(name)
        .wait_mode(ConnectWaitMode::Timeout(CONNECT_TIMEOUT))
        .connect_sync()?;
    let mut line = serde_json::to_vec(request).map_err(io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.flush()?;
    let mut ack = [0_u8; 3];
    stream.read_exact(&mut ack)?;
    if ack != ACK {
        return Err(io::Error::other(
            "unexpected reply from the running instance",
        ));
    }
    Ok(())
}

fn listen(endpoint: &Endpoint) -> io::Result<Listener> {
    let name = endpoint.0.as_path().to_fs_name::<GenericFilePath>()?;
    let listener = ListenerOptions::new().name(name).create_sync()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&endpoint.0, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(listener)
}

/// A socket file left by a crashed instance refuses connections; removing
/// it lets this launch become the instance. Named pipes vanish with their
/// process, so Windows has nothing to clean.
fn remove_stale(endpoint: &Endpoint) {
    if cfg!(unix) {
        let _ = std::fs::remove_file(&endpoint.0);
    }
}

/// Serves later launches on a background thread: each request reaches
/// `deliver`, in arrival order, before its sender is acknowledged.
pub fn serve(
    listener: Listener,
    deliver: impl Fn(LaunchRequest) + Send + 'static,
) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("cerata-instance".into())
        .spawn(move || {
            for connection in listener.incoming() {
                match connection.and_then(|stream| read_request(stream, &deliver)) {
                    Ok(()) => {}
                    Err(error) => log::warn!("launch request dropped: {error}"),
                }
            }
        })
}

fn read_request(stream: Stream, deliver: &impl Fn(LaunchRequest)) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    (&mut reader)
        .take(MAX_REQUEST)
        .read_until(b'\n', &mut line)?;
    if line.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request too long or truncated",
        ));
    }
    let request: LaunchRequest =
        serde_json::from_slice(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    deliver(request);
    let mut stream = reader.into_inner();
    stream.write_all(ACK)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn endpoint(dir: &Path) -> Endpoint {
        Endpoint::for_data_dir(dir)
    }

    #[test]
    fn a_second_launch_forwards_its_arguments_and_cwd_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint(dir.path());
        let first = LaunchRequest::default();
        let Claim::Primary(listener) = claim(&endpoint, &first).unwrap() else {
            panic!("the first launch must become the instance");
        };
        let (sender, received) = mpsc::channel();
        serve(listener, move |request| {
            let _ = sender.send(request);
        })
        .unwrap();

        let second = LaunchRequest {
            cwd: dir.path().join("work"),
            args: vec!["notes".into(), "b.md".into()],
        };
        assert!(matches!(
            claim(&endpoint, &second).unwrap(),
            Claim::Forwarded
        ));
        let third = LaunchRequest {
            cwd: dir.path().to_path_buf(),
            args: vec!["c.md".into()],
        };
        assert!(matches!(
            claim(&endpoint, &third).unwrap(),
            Claim::Forwarded
        ));

        let got = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(got, second);
        assert_eq!(
            got.paths(),
            [dir.path().join("work/notes"), dir.path().join("work/b.md")]
        );
        assert_eq!(
            received.recv_timeout(Duration::from_secs(5)).unwrap(),
            third
        );
    }

    #[test]
    fn absolute_arguments_ignore_the_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let absolute = dir.path().join("x.md");
        let request = LaunchRequest {
            cwd: PathBuf::from("elsewhere"),
            args: vec![absolute.display().to_string()],
        };
        assert_eq!(request.paths(), [absolute]);
    }

    #[test]
    fn a_malformed_request_is_dropped_without_stopping_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = endpoint(dir.path());
        let Claim::Primary(listener) = claim(&endpoint, &LaunchRequest::default()).unwrap() else {
            panic!("primary expected");
        };
        let (sender, received) = mpsc::channel();
        serve(listener, move |request| {
            let _ = sender.send(request);
        })
        .unwrap();
        let name = endpoint
            .0
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .unwrap();
        let mut raw = Stream::connect(name).unwrap();
        raw.write_all(b"{not json}\n").unwrap();
        drop(raw);
        let good = LaunchRequest {
            cwd: dir.path().to_path_buf(),
            args: vec!["a".into()],
        };
        assert!(matches!(claim(&endpoint, &good).unwrap(), Claim::Forwarded));
        assert_eq!(received.recv_timeout(Duration::from_secs(5)).unwrap(), good);
    }
}
