//! Plugin HTTP is relayed through the host (`crate::http`), not performed inside the Deno
//! subprocess. These tests drive the real dispatcher and a real local origin server, so what they
//! assert is the behaviour plugins actually get: fresh data (revalidation, never a blind cache
//! replay), one upstream request per piece of work, and the declared `net` hosts enforced.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use lanrurugi_plugin::pool::PluginPool;

/// `gzip("decompressed-by-the-host")`, precomputed so the test needs no encoder dependency.
const GZIPPED: [u8; 44] = [
    0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0x4b, 0x49, 0x4d, 0xce, 0xcf, 0x2d,
    0x28, 0x4a, 0x2d, 0x2e, 0x4e, 0x4d, 0xd1, 0x4d, 0xaa, 0xd4, 0x2d, 0xc9, 0x48, 0xd5, 0xcd, 0xc8,
    0x2f, 0x2e, 0x01, 0x00, 0x7d, 0x1a, 0x03, 0xbf, 0x18, 0x00, 0x00, 0x00,
];

/// A stub origin: `ETag`-bearing responses (so the relay can revalidate), a per-path request count,
/// and every request head recorded for assertions.
struct StubOrigin {
    addr: SocketAddr,
    state: Arc<OriginState>,
    shutdown: Arc<AtomicBool>,
}

#[derive(Default)]
struct OriginState {
    hits: Mutex<HashMap<String, usize>>,
    heads: Mutex<Vec<String>>,
}

impl StubOrigin {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let addr = listener.local_addr().expect("local addr");
        let state = Arc::new(OriginState::default());
        let shutdown = Arc::new(AtomicBool::new(false));
        let state_for_thread = state.clone();
        let shutdown_for_thread = shutdown.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if shutdown_for_thread.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let state = state_for_thread.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        if line == "\r\n" {
                            break;
                        }
                        head.push_str(&line);
                    }
                    let path = head
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    let count = {
                        let mut hits = state.hits.lock().expect("hits lock");
                        let entry = hits.entry(path.clone()).or_insert(0);
                        *entry += 1;
                        *entry
                    };
                    let lower = head.to_ascii_lowercase();
                    let revalidated = lower.contains("if-none-match");
                    state.heads.lock().expect("heads lock").push(head);

                    let accepts_gzip = lower.contains("accept-encoding:") && lower.contains("gzip");
                    let response = if path == "/gzipped" && accepts_gzip {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            GZIPPED.len()
                        )
                        .into_bytes()
                        .into_iter()
                        .chain(GZIPPED.iter().copied())
                        .collect::<Vec<u8>>()
                    } else if path == "/gzipped" {
                        // The host must have advertised gzip; answering with the raw body is how this
                        // test tells the difference.
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nplain"
                            .to_vec()
                    } else if path == "/cookies" {
                        b"HTTP/1.1 200 OK\r\nSet-Cookie: a=1; Path=/\r\nSet-Cookie: b=2; Path=/\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                            .to_vec()
                    } else if revalidated {
                        ("HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n")
                            .as_bytes()
                            .to_vec()
                    } else {
                        let body = format!("{path}#{count}");
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .into_bytes()
                    };
                    let response: Vec<u8> = response;
                    let _ = stream.write_all(&response);
                    let _ = stream.flush();
                    let mut sink = Vec::new();
                    let _ = reader.read_to_end(&mut sink);
                });
            }
        });
        Self {
            addr,
            state,
            shutdown,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn hits(&self, path: &str) -> usize {
        *self
            .state
            .hits
            .lock()
            .expect("hits lock")
            .get(path)
            .unwrap_or(&0)
    }

    fn heads_for(&self, path: &str) -> Vec<String> {
        self.state
            .heads
            .lock()
            .expect("heads lock")
            .iter()
            .filter(|head| {
                head.lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    == Some(path)
            })
            .cloned()
            .collect()
    }
}

impl Drop for StubOrigin {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(self.addr);
    }
}

fn deno_or_skip() -> Option<()> {
    let found = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("deno"))
            .find(|p| p.is_file())
    });
    if found.is_none() {
        eprintln!("skipping: deno not found on PATH");
    }
    found.map(|_| ())
}

/// Writes a throwaway plugin whose `execMetadata` exercises the relay, and runs it through the real
/// dispatcher against `origin`.
async fn run_relay_probe(origin: &StubOrigin) -> Option<serde_json::Value> {
    deno_or_skip()?;
    let dispatcher = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dispatcher/dispatcher.ts");
    let dir = std::env::temp_dir().join("lrr-relay-probe");
    std::fs::create_dir_all(&dir).ok()?;
    // `plugin-sdk.ts` must sit beside the dispatcher (see `Worker::spawn`); it is already committed
    // there, so nothing needs writing for the normal case.
    let source = r#"
export function pluginInfo() {
  return {
    namespace: "relay-probe",
    type: "metadata" as const,
    parameters: [],
    declared_permissions: { net: ["127.0.0.1"], read: false, write: false },
    name: "relay-probe",
    author: "test",
    description: "test",
    version: "1",
  };
}

export async function execMetadata(hostArgs: Record<string, unknown>) {
  const base = String(hostArgs.base);
  const out: Record<string, unknown> = {};
  out.sequential = [
    await (await fetch(`${base}/revalidate`)).text(),
    await (await fetch(`${base}/revalidate`)).text(),
  ];
  const both = await Promise.all([
    fetch(`${base}/coalesce`).then((r) => r.text()),
    fetch(`${base}/coalesce`).then((r) => r.text()),
  ]);
  out.coalesced = both;
  out.gzipped = await (await fetch(`${base}/gzipped`)).text();
  const cookieRes = await fetch(`${base}/cookies`);
  out.setCookies = (cookieRes.headers as unknown as { getSetCookie(): string[] }).getSetCookie();
  try {
    await fetch("http://denied.invalid/x");
    out.denied = "allowed";
  } catch (e) {
    out.denied = String(e);
  }
  return out;
}
"#;
    std::fs::write(dir.join("relay-probe.ts"), source).ok()?;
    let pool = PluginPool::new("deno", dispatcher, dir);
    let result = pool
        .execute(
            "relay-probe",
            "exec_metadata",
            serde_json::json!({ "base": origin.base_url() }),
        )
        .await
        .expect("the plugin call itself must succeed");
    Some(result)
}

#[tokio::test]
async fn plugin_http_is_relayed_revalidated_coalesced_and_permission_checked() {
    let origin = StubOrigin::start();
    let Some(result) = run_relay_probe(&origin).await else {
        return;
    };

    // Revalidation: the second sequential fetch of the same URL still returns a body to the plugin
    // (the stored one), and the origin was asked a second time — conditionally — answering 304.
    let sequential = result["sequential"]
        .as_array()
        .expect("sequential results")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        sequential,
        vec!["/revalidate#1".to_string(), "/revalidate#1".to_string()],
        "a 304 must hand the plugin the stored body, not an empty one"
    );
    assert_eq!(origin.hits("/revalidate"), 2, "the origin was re-asked");
    let heads = origin.heads_for("/revalidate");
    assert!(
        heads[1]
            .to_ascii_lowercase()
            .contains("if-none-match: \"v1\""),
        "the second request must be conditional, got:\n{}",
        heads[1]
    );

    // Coalescing: two concurrent identical requests are one upstream request, and both callers get
    // the same answer.
    let coalesced = result["coalesced"]
        .as_array()
        .expect("coalesced results")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        coalesced,
        vec!["/coalesce#1".to_string(), "/coalesce#1".to_string()]
    );
    assert_eq!(
        origin.hits("/coalesce"),
        1,
        "concurrent identical requests must collapse onto one upstream request"
    );

    // Compression: the host negotiates it and hands the plugin decoded text — the plugin corpus
    // never sees an encoding, and every relayed request moves fewer bytes.
    assert_eq!(
        result["gzipped"].as_str().unwrap_or_default(),
        "decompressed-by-the-host",
        "a gzip response must reach the plugin decoded"
    );
    let gzip_heads = origin.heads_for("/gzipped");
    assert!(
        gzip_heads[0]
            .to_ascii_lowercase()
            .contains("accept-encoding:")
            && gzip_heads[0].to_ascii_lowercase().contains("gzip"),
        "the host must advertise gzip, got:\n{}",
        gzip_heads[0]
    );

    // Repeated response headers survive the relay: a login rotates several cookies at once, and the
    // dispatcher's cookie jar reads each of them individually.
    let mut set_cookies = result["setCookies"]
        .as_array()
        .expect("set-cookie array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    set_cookies.sort();
    assert_eq!(
        set_cookies,
        vec!["a=1; Path=/".to_string(), "b=2; Path=/".to_string()]
    );

    // Permissions: a host the plugin did not declare is refused by the host, not by a Deno flag.
    let denied = result["denied"].as_str().unwrap_or_default();
    assert!(
        denied.contains("not permitted"),
        "an undeclared host must be refused, got: {denied}"
    );
}
