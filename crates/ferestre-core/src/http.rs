//! The little HTTP this crate does.
//!
//! Anonymous GETs -- a catalog lookup, an image, and the signed runtime
//! release -- and nothing else. Everything that needs an account goes through
//! the client as a child process, which is where the tokens live and where they
//! stay.
//!
//! Blocking on purpose. Every caller already runs these off its main thread,
//! and an async runtime is a large dependency to carry for a few requests.

use anyhow::{anyhow, bail, Result};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::time::Duration;

/// Short, because everything fetched here is decoration. A window that hangs
/// for half a minute because a CDN is sulking is worse than one showing product
/// ids and no art.
const TIMEOUT: Duration = Duration::from_secs(15);
/// Art can be a megabyte and a slow line is not a failure.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// A ceiling, so a wrong URL that streams forever cannot fill a disk.
const MAX_DOWNLOAD: u64 = 16 * 1024 * 1024;
/// Runtime archives are large, while still bounded to protect the state disk.
const MAX_LARGE_DOWNLOAD: u64 = 4 * 1024 * 1024 * 1024;
/// A detached signature is a few hundred bytes.
const MAX_SIGNATURE: u64 = 16 * 1024;

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(concat!("ferestre/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// How long a large download waits on each phase before it calls a request
/// stalled, and how often it tries again.
///
/// There is deliberately no limit on the whole transfer. A 300 MB runtime on a
/// 10 Mbit/s line takes four minutes and that is not a failure; a single total
/// budget, which is what ureq's global timeout is, cut every such download off
/// at the same point and threw it away. Instead each request may read the body
/// for `attempt` and is then resumed from where it got to with a Range request.
/// A link that is slow but moving finishes; one that has stopped moving fails
/// after `retries` attempts in a row that brought nothing.
///
/// How ureq applies the body limit differs between releases: 3.2 lets reads go
/// on after it with a one-second idle timeout, 3.4 ends the read there. Either
/// way a request ends within about `attempt` once data stops, and a request
/// that ended early is resumed, so both behave.
#[derive(Clone, Copy, Debug)]
struct Patience {
    connect: Duration,
    response: Duration,
    attempt: Duration,
    /// For a server that ignores Range, where a request that ends early starts
    /// again from zero. Long enough for 300 MB on a slow line, but still a
    /// bound on a connection that has silently died.
    unresumable: Duration,
    retries: u32,
    backoff: Duration,
}

const LARGE: Patience = Patience {
    connect: Duration::from_secs(30),
    response: Duration::from_secs(60),
    attempt: Duration::from_secs(120),
    unresumable: Duration::from_secs(60 * 60),
    retries: 5,
    backoff: Duration::from_secs(2),
};

fn large_agent(patience: &Patience, body: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_resolve(Some(patience.connect))
        .timeout_connect(Some(patience.connect))
        .timeout_send_request(Some(patience.connect))
        .timeout_recv_response(Some(patience.response))
        .timeout_recv_body(Some(body))
        .user_agent(concat!("ferestre/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Set query parameters on a URL, replacing any that are already there.
///
/// Appending blindly is the obvious thing and it is wrong: an Xbox gamerpic URL
/// already carries `format`, and a second copy of it is answered with 400. The
/// failure is invisible -- an avatar that quietly never appears -- so this
/// replaces rather than appends.
///
/// Deliberately not a URL parser. It splits on the first `?`, walks `&`-joined
/// pairs and leaves every value exactly as it found it, because these URLs
/// carry opaque tokens with `.` and `_` in them that a re-encoding round trip
/// would corrupt.
pub fn with_params(url: &str, params: &[(&str, String)]) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, query),
        None => (url, ""),
    };
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (pair.to_string(), String::new()),
        })
        .collect();

    for (key, value) in params {
        match pairs.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value.clone(),
            None => pairs.push((key.to_string(), value.clone())),
        }
    }

    let query: Vec<String> = pairs
        .into_iter()
        .map(|(k, v)| if v.is_empty() { k } else { format!("{k}={v}") })
        .collect();
    if query.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", query.join("&"))
    }
}

/// A square image URL at a given pixel size. Both Microsoft image hosts resize
/// server-side, which turns a 1.6 MB avatar into 1.7 KB.
pub fn sized_image(url: &str, px: u32) -> String {
    with_params(
        url,
        &[
            ("format", "png".to_string()),
            ("w", px.to_string()),
            ("h", px.to_string()),
        ],
    )
}

pub fn get_text(url: &str) -> Result<String> {
    agent(TIMEOUT)
        .get(url)
        .call()
        .map_err(|e| anyhow!("{url}: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| anyhow!("{url}: {e}"))
}

/// Fetch an image to `dest`, atomically. An existing image is left alone.
///
/// Written beside the target and renamed, because the alternative is that a
/// launcher killed mid-download leaves a truncated image that every later run
/// then loads and fails on -- a corruption that outlives the thing that caused
/// it and looks like a bug in the drawing code.
///
/// Both callers fetch pictures, so what arrives has to start like one, and a
/// cached file that does not is fetched again rather than trusted forever.
/// Each download gets its own temporary file: the window can ask for the same
/// icon from two workers at once, and a shared one let the second truncate the
/// first's file under it.
pub fn download(url: &str, dest: &Path) -> Result<()> {
    if dest.is_file() {
        if starts_like_an_image(dest) {
            return Ok(());
        }
        let _ = std::fs::remove_file(dest);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let reader = agent(DOWNLOAD_TIMEOUT)
        .get(url)
        .call()
        .map_err(|e| anyhow!("{url}: {e}"))?
        .into_body()
        .into_reader();
    let (temporary, mut file) = create_temporary(dest)?;
    let result = (|| -> Result<()> {
        copy_limited(reader, &mut file, MAX_DOWNLOAD, &|| false)?;
        file.sync_all()?;
        if !starts_like_an_image(&temporary) {
            bail!("the server did not return an image");
        }
        std::fs::rename(&temporary, dest)?;
        Ok(())
    })();
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(anyhow!("{url}: {e}"));
    }
    Ok(())
}

/// PNG, JPEG, GIF or WebP by their first bytes. The picture host answers
/// `format=png` with a JPEG, so the extension is no guide.
fn starts_like_an_image(path: &Path) -> bool {
    let mut head = [0u8; 12];
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut filled = 0;
    while filled < head.len() {
        match file.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return false,
        }
    }
    let head = &head[..filled];
    head.starts_with(b"\x89PNG\r\n\x1a\n")
        || head.starts_with(&[0xff, 0xd8, 0xff])
        || head.starts_with(b"GIF87a")
        || head.starts_with(b"GIF89a")
        || (head.len() == 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP")
}

/// Fetch a signed release artefact, such as a runtime archive, and keep it
/// only if its signature verifies against the release key built into this
/// launcher. See [`download_large_cancellable`].
pub fn download_large(url: &str, dest: &Path) -> Result<()> {
    download_large_cancellable(url, dest, || false)
}

/// Fetch a signed release artefact, such as a runtime archive, to `dest`.
///
/// The detached minisign signature is fetched from `<url>.minisig` first and
/// checked against [`crate::signing::RELEASE_PUBLIC_KEY`]. The artefact is
/// renamed into place only after it verifies, so nothing unverified is ever at
/// `dest`. A copy already at `dest` is verified again rather than trusted, and
/// is fetched afresh when it does not match: a bad cached archive used to fail
/// every later install the same way until someone found and deleted it.
///
/// The transfer has no total time limit. See [`Patience`] for how a slow link
/// is told apart from a dead one.
pub fn download_large_cancellable(
    url: &str,
    dest: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<()> {
    let signature_url = format!("{url}{}", crate::signing::SIGNATURE_SUFFIX);
    download_release(
        url,
        &signature_url,
        dest,
        &cancelled,
        &LARGE,
        crate::signing::RELEASE_PUBLIC_KEY,
    )
}

/// [`download_large_cancellable`] with the signature's URL and the public key
/// given: the signature the release lists beside the artefact (a runtime
/// release's `<asset>.minisig`), and normally
/// [`crate::signing::RELEASE_PUBLIC_KEY`]. Tests pass a throwaway key.
pub fn download_signed(
    url: &str,
    signature_url: &str,
    dest: &Path,
    cancelled: impl Fn() -> bool,
    public_key: &str,
) -> Result<()> {
    download_release(url, signature_url, dest, &cancelled, &LARGE, public_key)
}

fn download_release(
    url: &str,
    signature_url: &str,
    dest: &Path,
    cancelled: &dyn Fn() -> bool,
    patience: &Patience,
    public_key: &str,
) -> Result<()> {
    anyhow::ensure!(!cancelled(), "Download stopped");
    // Refuse before a 300 MB download, not after it.
    crate::signing::check_public_key(public_key)?;
    let name = published_name(url)?;
    let signature = fetch_signature(signature_url)?;
    crate::signing::check_signature(&signature, name)?;
    if dest.is_file() {
        if crate::signing::verify_file_with_key(public_key, &signature, name, dest).is_ok() {
            return Ok(());
        }
        std::fs::remove_file(dest)
            .map_err(|e| anyhow!("{}: cannot replace a damaged copy: {e}", dest.display()))?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::cache::sweep_stale_temporaries(dest, "part");
    let (temporary, mut file) = create_temporary(dest)?;
    let result = (|| -> Result<()> {
        fetch_resumable(url, &mut file, MAX_LARGE_DOWNLOAD, cancelled, patience)?;
        file.sync_all()?;
        anyhow::ensure!(!cancelled(), "Download stopped");
        crate::signing::verify_file_with_key(public_key, &signature, name, &temporary)?;
        std::fs::rename(&temporary, dest)?;
        if let Some(parent) = dest.parent() {
            crate::cache::sync_directory(parent);
        }
        Ok(())
    })();
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(anyhow!("{url}: {e}"));
    }
    Ok(())
}

/// The file name the release published, which is what the signature's
/// trusted comment names.
fn published_name(url: &str) -> Result<&str> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow!("{url}: no file name to check a signature against"))
}

fn fetch_signature(url: &str) -> Result<String> {
    let mut text = String::new();
    agent(TIMEOUT)
        .get(url)
        .call()
        .map_err(|e| anyhow!("{url}: cannot fetch the release signature: {e}"))?
        .into_body()
        .into_reader()
        .take(MAX_SIGNATURE + 1)
        .read_to_string(&mut text)
        .map_err(|e| anyhow!("{url}: {e}"))?;
    if text.len() as u64 > MAX_SIGNATURE {
        bail!("{url}: not a signature: it is larger than any signature is");
    }
    Ok(text)
}

fn create_temporary(dest: &Path) -> Result<(std::path::PathBuf, std::fs::File)> {
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = crate::cache::temporary_beside(dest, "part");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| anyhow!("{}: {e}", temporary.display()))?;
    Ok((temporary, file))
}

/// How a copy ended early. Reading and writing fail for different reasons and
/// only a failed read is worth another request.
#[derive(Debug)]
enum CopyError {
    Stopped,
    TooLarge(u64),
    Read(std::io::Error),
    Write(std::io::Error),
}

impl std::fmt::Display for CopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyError::Stopped => write!(f, "Download stopped"),
            CopyError::TooLarge(limit) => write!(f, "larger than the {limit} bytes allowed"),
            CopyError::Read(e) => write!(f, "{e}"),
            CopyError::Write(e) => write!(f, "cannot write the download: {e}"),
        }
    }
}

impl std::error::Error for CopyError {}

/// Copy at most `limit` bytes and fail, rather than stop quietly, when there
/// is more: a body cut at the limit is not the file that was asked for.
fn copy_limited(
    reader: impl Read,
    file: &mut std::fs::File,
    limit: u64,
    cancelled: &dyn Fn() -> bool,
) -> std::result::Result<u64, CopyError> {
    let mut reader = reader.take(limit.saturating_add(1));
    let mut buffer = vec![0; 64 * 1024];
    let mut copied = 0u64;
    loop {
        if cancelled() {
            return Err(CopyError::Stopped);
        }
        let count = match reader.read(&mut buffer) {
            Ok(count) => count,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(CopyError::Read(e)),
        };
        if count == 0 {
            return Ok(copied);
        }
        copied += count as u64;
        if copied > limit {
            return Err(CopyError::TooLarge(limit));
        }
        file.write_all(&buffer[..count]).map_err(CopyError::Write)?;
    }
}

/// Why one request of a large download ended early.
enum Interrupted {
    /// Worth another request: a timeout, a dropped connection, a 5xx.
    Transient(anyhow::Error),
    /// Asking again will get the same answer: a 404, a full disk, a stop.
    Fatal(anyhow::Error),
}

/// Download into `file`, resuming with Range requests when a request ends
/// early. Returns once the whole body is in `file`.
fn fetch_resumable(
    url: &str,
    file: &mut std::fs::File,
    limit: u64,
    cancelled: &dyn Fn() -> bool,
    patience: &Patience,
) -> Result<()> {
    // A server that ignores Range sends the whole body again on every request,
    // so a short per-request budget would never finish on a slow link. Once
    // that is seen, a request gets the long one.
    let mut ranges_work = true;
    let mut fruitless = 0u32;
    let mut delay = patience.backoff;
    loop {
        anyhow::ensure!(!cancelled(), "Download stopped");
        let have = file.stream_position()?;
        let before = have;
        let budget = if ranges_work {
            patience.attempt
        } else {
            patience.unresumable
        };
        match fetch_from(
            url,
            file,
            have,
            limit,
            cancelled,
            patience,
            budget,
            &mut ranges_work,
        ) {
            Ok(()) => return Ok(()),
            Err(Interrupted::Fatal(e)) => return Err(e),
            Err(Interrupted::Transient(e)) => {
                if file.stream_position()? > before {
                    fruitless = 0;
                    delay = patience.backoff;
                } else {
                    fruitless += 1;
                    if fruitless >= patience.retries {
                        return Err(e.context(format!(
                            "gave up after {fruitless} attempts in a row that brought no data"
                        )));
                    }
                    wait(delay, cancelled)?;
                    delay = (delay * 2).min(Duration::from_secs(60));
                }
            }
        }
    }
}

fn wait(delay: Duration, cancelled: &dyn Fn() -> bool) -> Result<()> {
    let step = Duration::from_millis(100);
    let mut waited = Duration::ZERO;
    while waited < delay {
        anyhow::ensure!(!cancelled(), "Download stopped");
        std::thread::sleep(step.min(delay - waited));
        waited += step;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn fetch_from(
    url: &str,
    file: &mut std::fs::File,
    have: u64,
    limit: u64,
    cancelled: &dyn Fn() -> bool,
    patience: &Patience,
    budget: Duration,
    ranges_work: &mut bool,
) -> std::result::Result<(), Interrupted> {
    let mut request = large_agent(patience, budget)
        .get(url)
        // Range offsets count the bytes on the wire. A compressed response
        // would make them count something else.
        .header("Accept-Encoding", "identity");
    if have > 0 {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let response = match request.call() {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(416)) if have > 0 => {
            // Nothing past what we have: the file was already complete when
            // the last request was cut off.
            return Ok(());
        }
        Err(ureq::Error::StatusCode(code)) if code == 408 || code == 429 || code >= 500 => {
            return Err(Interrupted::Transient(anyhow!("HTTP {code}")))
        }
        Err(ureq::Error::StatusCode(code)) => {
            return Err(Interrupted::Fatal(anyhow!("HTTP {code}")))
        }
        Err(e) => return Err(Interrupted::Transient(anyhow!("{e}"))),
    };
    let fatal = |e: std::io::Error| Interrupted::Fatal(e.into());
    if have > 0 {
        let resumed = response.status().as_u16() == 206
            && response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(content_range_start)
                == Some(have);
        if !resumed {
            // The whole body again, from the start.
            *ranges_work = false;
            file.set_len(0).map_err(fatal)?;
            file.rewind().map_err(fatal)?;
        }
    }
    let start = file.stream_position().map_err(fatal)?;
    let reader = response.into_body().into_reader();
    match copy_limited(reader, file, limit.saturating_sub(start), cancelled) {
        Ok(_) => Ok(()),
        Err(e @ CopyError::Read(_)) => Err(Interrupted::Transient(e.into())),
        // A full disk, a stop, or a body past the limit: asking the server
        // again will not change any of them.
        Err(e) => Err(Interrupted::Fatal(e.into())),
    }
}

/// The first byte position of `bytes <start>-<end>/<total>`.
fn content_range_start(value: &str) -> Option<u64> {
    value
        .trim()
        .strip_prefix("bytes ")?
        .split_once('-')?
        .0
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::signing::testing::TestKey;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// A release artefact with its signature at `<url>.minisig`.
    fn signed_release(
        url: &str,
        dest: &Path,
        cancelled: &dyn Fn() -> bool,
        patience: &Patience,
        public_key: &str,
    ) -> Result<()> {
        let signature_url = format!("{url}{}", crate::signing::SIGNATURE_SUFFIX);
        download_release(url, &signature_url, dest, cancelled, patience, public_key)
    }

    /// What the test server answers for one path.
    #[derive(Clone)]
    struct Served {
        status: u16,
        body: Vec<u8>,
        ranges: bool,
        chunk: usize,
        pause: Duration,
        /// Close the connection after this many bytes unless the request
        /// asked for a range: a dropped connection part way through.
        cut: Option<usize>,
    }

    impl Served {
        fn file(body: &[u8]) -> Self {
            Served {
                status: 200,
                body: body.to_vec(),
                ranges: true,
                chunk: body.len().max(1),
                pause: Duration::ZERO,
                cut: None,
            }
        }

        fn slowly(mut self, chunk: usize, pause: Duration) -> Self {
            self.chunk = chunk;
            self.pause = pause;
            self
        }
    }

    /// A tiny HTTP/1.1 server: one thread per connection, `Connection: close`,
    /// Range support when asked for. Returns the base URL and a log of
    /// "<path> <range>" for every request.
    fn serve(routes: Vec<(&str, Served)>) -> (String, Arc<Mutex<Vec<String>>>) {
        use std::io::{BufRead, BufReader};
        let routes: Arc<HashMap<String, Served>> = Arc::new(
            routes
                .into_iter()
                .map(|(path, served)| (path.to_string(), served))
                .collect(),
        );
        let log = Arc::new(Mutex::new(Vec::new()));
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.local_addr().unwrap());
        let shared_log = log.clone();
        std::thread::spawn(move || {
            for stream in server.incoming() {
                let Ok(mut stream) = stream else { return };
                let routes = routes.clone();
                let log = shared_log.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                    let mut range = None;
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                            break;
                        }
                        if let Some((name, value)) = header.split_once(':') {
                            if name.eq_ignore_ascii_case("range") {
                                range = value
                                    .trim()
                                    .strip_prefix("bytes=")
                                    .and_then(|v| v.strip_suffix('-'))
                                    .and_then(|v| v.parse::<usize>().ok());
                            }
                        }
                    }
                    log.lock().unwrap().push(format!("{path} {range:?}"));
                    let Some(served) = routes.get(&path).cloned() else {
                        let _ = stream.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                        return;
                    };
                    if served.status != 200 {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 {} Nope\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            served.status
                        );
                        return;
                    }
                    let total = served.body.len();
                    let start = match range {
                        Some(start) if served.ranges && start < total => start,
                        _ => 0,
                    };
                    let head = if start > 0 {
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{total}\r\nConnection: close\r\n\r\n",
                            total - start,
                            total - 1
                        )
                    } else {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n"
                        )
                    };
                    if stream.write_all(head.as_bytes()).is_err() {
                        return;
                    }
                    let end = match served.cut {
                        Some(cut) if range.is_none() => cut,
                        _ => total,
                    };
                    for piece in served.body[start..end].chunks(served.chunk) {
                        std::thread::sleep(served.pause);
                        if stream.write_all(piece).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (base, log)
    }

    const QUICK: Patience = Patience {
        connect: Duration::from_secs(5),
        response: Duration::from_secs(5),
        attempt: Duration::from_millis(300),
        unresumable: Duration::from_secs(30),
        retries: 3,
        backoff: Duration::from_millis(10),
    };

    const NAME: &str = "ferestre-runtime-11.0.1.tar.xz";

    fn runtime_bytes() -> Vec<u8> {
        (0..96 * 1024u32).map(|i| (i % 251) as u8).collect()
    }

    fn signed(key: &TestKey, served: Served) -> Vec<(String, Served)> {
        let signature = key.sign_as(NAME, &served.body);
        vec![
            (format!("/{NAME}"), served),
            (
                format!("/{NAME}.minisig"),
                Served::file(signature.as_bytes()),
            ),
        ]
    }

    fn routes(list: &[(String, Served)]) -> Vec<(&str, Served)> {
        list.iter().map(|(p, s)| (p.as_str(), s.clone())).collect()
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect()
    }

    /// The bug: a single 60 s budget for the whole transfer cut off every
    /// runtime download on a link slower than about 40 Mbit/s, and the next
    /// attempt started from zero. Here the body takes several times longer
    /// than one request is allowed, and still arrives whole.
    #[test]
    fn a_slow_download_that_outlasts_one_request_still_finishes() {
        let key = TestKey::new();
        let data = runtime_bytes();
        let list = signed(
            &key,
            Served::file(&data).slowly(4096, Duration::from_millis(40)),
        );
        let (base, _) = serve(routes(&list));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join(NAME);
        let started = std::time::Instant::now();
        signed_release(
            &format!("{base}/{NAME}"),
            &dest,
            &|| false,
            &QUICK,
            &key.public(),
        )
        .unwrap();
        assert!(
            started.elapsed() > QUICK.attempt * 2,
            "the transfer outlasted any single request"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(leftovers(dir.path()), vec![NAME.to_string()]);
    }

    /// A connection that drops part way used to throw the partial file away
    /// and start the next attempt from zero.
    #[test]
    fn a_dropped_connection_resumes_where_it_stopped() {
        let key = TestKey::new();
        let data = runtime_bytes();
        let mut served = Served::file(&data);
        served.cut = Some(40_000);
        let list = signed(&key, served);
        let (base, log) = serve(routes(&list));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join(NAME);
        signed_release(
            &format!("{base}/{NAME}"),
            &dest,
            &|| false,
            &QUICK,
            &key.public(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let log = log.lock().unwrap();
        assert!(
            log.contains(&format!("/{NAME} Some(40000)")),
            "resumed from the byte it had reached: {log:?}"
        );
    }

    /// Without Range support every request starts again from zero, so a
    /// per-request limit would never let a slow transfer finish.
    #[test]
    fn a_server_that_ignores_ranges_is_read_to_the_end() {
        let key = TestKey::new();
        let data = runtime_bytes();
        let mut served = Served::file(&data).slowly(4096, Duration::from_millis(30));
        served.ranges = false;
        let list = signed(&key, served);
        let (base, _) = serve(routes(&list));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join(NAME);
        signed_release(
            &format!("{base}/{NAME}"),
            &dest,
            &|| false,
            &QUICK,
            &key.public(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    /// A link that has stopped moving is told apart from a slow one: attempts
    /// that bring nothing are counted, and the download gives up.
    #[test]
    fn a_stalled_server_is_given_up_on() {
        let key = TestKey::new();
        let data = runtime_bytes();
        let list = signed(
            &key,
            Served::file(&data).slowly(1, Duration::from_secs(3600)),
        );
        let (base, log) = serve(routes(&list));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join(NAME);
        let error = signed_release(
            &format!("{base}/{NAME}"),
            &dest,
            &|| false,
            &QUICK,
            &key.public(),
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("brought no data"),
            "{error:#}"
        );
        let requests = log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with(&format!("/{NAME} ")))
            .count();
        assert_eq!(requests, QUICK.retries as usize);
        assert!(leftovers(dir.path()).is_empty());
    }

    /// A 404 will be a 404 next time too.
    #[test]
    fn a_missing_artefact_is_not_asked_for_again() {
        let key = TestKey::new();
        let mut list = signed(&key, Served::file(b"x"));
        list[0].1.status = 404;
        let (base, log) = serve(routes(&list));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join(NAME);
        let error = signed_release(
            &format!("{base}/{NAME}"),
            &dest,
            &|| false,
            &QUICK,
            &key.public(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("404"), "{error}");
        let requests = log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with(&format!("/{NAME} ")))
            .count();
        assert_eq!(requests, 1);
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn an_artefact_that_does_not_verify_is_never_kept() {
        let key = TestKey::new();
        let data = runtime_bytes();
        let mut list = signed(&key, Served::file(&data));
        list[0].1.body[1000] ^= 1;
        let (base, _) = serve(routes(&list));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join(NAME);
        let error = signed_release(
            &format!("{base}/{NAME}"),
            &dest,
            &|| false,
            &QUICK,
            &key.public(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not match"), "{error}");
        assert!(
            leftovers(dir.path()).is_empty(),
            "nothing unverified stays on disk"
        );

        // No signature published at all is a refusal too, before any download.
        let (base, log) = serve(vec![(&format!("/{NAME}")[..], Served::file(&data))]);
        assert!(signed_release(
            &format!("{base}/{NAME}"),
            &dest,
            &|| false,
            &QUICK,
            &key.public()
        )
        .is_err());
        assert_eq!(*log.lock().unwrap(), vec![format!("/{NAME}.minisig None")]);
        assert!(leftovers(dir.path()).is_empty());
    }

    /// A cached archive used to be trusted forever, so a bad one failed every
    /// later install the same way until someone found and deleted it.
    #[test]
    fn a_damaged_cached_archive_is_fetched_again_and_a_good_one_is_not() {
        let key = TestKey::new();
        let data = runtime_bytes();
        let list = signed(&key, Served::file(&data));
        let (base, log) = serve(routes(&list));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join(NAME);
        std::fs::write(&dest, b"half an archive").unwrap();
        let url = format!("{base}/{NAME}");
        signed_release(&url, &dest, &|| false, &QUICK, &key.public()).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        signed_release(&url, &dest, &|| false, &QUICK, &key.public()).unwrap();
        let fetched = log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with(&format!("/{NAME} ")))
            .count();
        assert_eq!(fetched, 1, "a verified copy is reused");
    }

    #[test]
    fn stopping_a_runtime_download_removes_the_partial_archive() {
        let key = TestKey::new();
        let data = runtime_bytes();
        let list = signed(
            &key,
            Served::file(&data).slowly(1024, Duration::from_millis(5)),
        );
        let (base, _) = serve(routes(&list));
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join(NAME);
        let checks = std::cell::Cell::new(0);
        let result = signed_release(
            &format!("{base}/{NAME}"),
            &dest,
            &|| {
                checks.set(checks.get() + 1);
                checks.get() > 4
            },
            &QUICK,
            &key.public(),
        );
        assert!(result.unwrap_err().to_string().contains("stopped"));
        assert!(leftovers(dir.path()).is_empty());
    }

    /// The built-in key is checked before anything is downloaded, so a build
    /// without one fails at once rather than after 300 MB.
    #[test]
    fn the_release_key_is_checked_before_any_request() {
        let dir = tempfile::tempdir().unwrap();
        let result =
            download_large_cancellable("http://127.0.0.1:9/never", &dir.path().join("x"), || false);
        if !crate::signing::has_release_key() {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("no release signing key"), "{error}");
        }
        assert!(leftovers(dir.path()).is_empty());
    }

    /// Reaching the limit used to end the copy as if the body were complete,
    /// and the cut file was kept.
    #[test]
    fn a_body_past_the_limit_is_an_error_not_a_shorter_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = std::fs::File::create(dir.path().join("out")).unwrap();
        let exact = copy_limited(&[7u8; 100][..], &mut file, 100, &|| false).unwrap();
        assert_eq!(exact, 100);
        let over = copy_limited(&[7u8; 101][..], &mut file, 100, &|| false);
        assert!(matches!(over, Err(CopyError::TooLarge(100))), "{over:?}");
    }

    #[test]
    fn an_image_that_is_not_one_is_neither_kept_nor_trusted_from_the_cache() {
        const PNG: &[u8] = include_bytes!("../tests/fixtures/steam-art.png");
        let (base, log) = serve(vec![
            ("/picture", Served::file(PNG)),
            ("/error-page", Served::file(b"<html>rate limited</html>")),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("icon.img");
        assert!(download(&format!("{base}/error-page"), &dest).is_err());
        assert!(leftovers(dir.path()).is_empty());

        // A damaged cached copy -- the zero-filled start a truncating writer
        // left behind -- is fetched again instead of being kept for good.
        std::fs::write(&dest, [0u8; 64]).unwrap();
        download(&format!("{base}/picture"), &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), PNG);
        download(&format!("{base}/picture"), &dest).unwrap();
        assert_eq!(
            log.lock().unwrap().len(),
            2,
            "a good cached image is not fetched again"
        );
    }

    /// Two workers fetching the same icon each write their own file, so
    /// neither can truncate the other's under it.
    #[test]
    fn concurrent_downloads_of_one_file_do_not_share_a_temporary() {
        const PNG: &[u8] = include_bytes!("../tests/fixtures/steam-art.png");
        let (base, _) = serve(vec![(
            "/picture",
            Served::file(PNG).slowly(64, Duration::from_millis(2)),
        )]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("icon.img");
        let url = format!("{base}/picture");
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| download(&url, &dest)))
                .collect();
            for worker in workers {
                worker.join().unwrap().unwrap();
            }
        });
        assert_eq!(std::fs::read(&dest).unwrap(), PNG);
        assert_eq!(leftovers(dir.path()), vec!["icon.img".to_string()]);
        let a = crate::cache::temporary_beside(&dest, "part");
        let b = crate::cache::temporary_beside(&dest, "part");
        assert_ne!(a, b);
    }

    #[test]
    fn the_published_name_is_the_last_path_segment() {
        assert_eq!(
            published_name(
                "https://github.com/o/r/releases/download/runtime-1/ferestre-runtime-1.tar.xz"
            )
            .unwrap(),
            "ferestre-runtime-1.tar.xz"
        );
        assert_eq!(
            published_name("https://h/a/b.tar.xz?x=1#y").unwrap(),
            "b.tar.xz"
        );
        assert!(published_name("https://h/a/").is_err());
        assert_eq!(content_range_start("bytes 100-199/200"), Some(100));
        assert_eq!(content_range_start("bytes */200"), None);
    }

    #[test]
    fn parameters_replace_rather_than_pile_up() {
        // The real shape of a gamerpic URL: it already carries `format`, and a
        // second copy is answered with 400.
        let url = "https://images-eds-ssl.xboxlive.com/image?url=aB.c_d&format=png";
        assert_eq!(
            sized_image(url, 48),
            "https://images-eds-ssl.xboxlive.com/image?url=aB.c_d&format=png&w=48&h=48"
        );
        assert_eq!(
            sized_image(&sized_image(url, 48), 64),
            "https://images-eds-ssl.xboxlive.com/image?url=aB.c_d&format=png&w=64&h=64",
            "sizing twice is sizing once"
        );
    }

    #[test]
    fn a_url_with_no_query_gains_one() {
        assert_eq!(
            sized_image("https://store-images.example/image/apps.1", 128),
            "https://store-images.example/image/apps.1?format=png&w=128&h=128"
        );
    }

    /// These URLs carry opaque tokens with `.` and `_` in them, and a
    /// re-encoding round trip would corrupt them.
    #[test]
    fn values_are_left_exactly_as_found() {
        let url = "https://h/i?url=wH.K4gGf7_cq%2Fx&q=90";
        assert_eq!(
            with_params(url, &[("w", "48".into())]),
            "https://h/i?url=wH.K4gGf7_cq%2Fx&q=90&w=48"
        );
    }

    #[test]
    fn a_valueless_parameter_survives() {
        assert_eq!(
            with_params("https://h/i?flag&a=1", &[("b", "2".into())]),
            "https://h/i?flag&a=1&b=2"
        );
    }
}
