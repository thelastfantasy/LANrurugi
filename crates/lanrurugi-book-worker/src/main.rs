//! E-book worker — a subprocess spawned by `lanrurugi_book`'s client, never by a user. It answers
//! requests on stdin/stdout (a socketpair set up by the client; framing in
//! `lanrurugi_book::protocol`), one at a time, and exits after [`IDLE_EXIT`] without a request.
//! A panic inside a request is caught and reported, and everything the worker had open is
//! dropped.

mod backend;
mod flow;
mod markup;

use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
use std::panic::{self, AssertUnwindSafe};
use std::time::Duration;

use lanrurugi_book::protocol::{self, ErrorKind, Request, Response};

use backend::Worker;

const IDLE_EXIT: Duration = Duration::from_secs(300);

fn handle(worker: &mut Worker, request: Request) -> (Response, Option<Vec<u8>>) {
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| match request {
        Request::Inspect { path, format } => worker
            .inspect(&path, format, lanrurugi_book::DEFAULT_RENDER_WIDTH)
            .map(|info| (Response::Info { info }, None)),
        Request::Page {
            path,
            format,
            index,
            source,
            target_width,
        } => worker
            .page(&path, format, index, source, target_width)
            .map(|(extension, bytes)| (Response::Page { extension }, Some(bytes))),
    }));
    match outcome {
        Ok(Ok(ok)) => ok,
        Ok(Err(f)) => (
            Response::Error {
                kind: f.kind,
                message: f.message,
            },
            None,
        ),
        Err(panic) => {
            worker.reset();
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_else(|| "panic".into());
            (
                Response::Error {
                    kind: ErrorKind::Internal,
                    message,
                },
                None,
            )
        }
    }
}

fn main() {
    // stdin and stdout are the same socket (see the client); fd 0 is used for both directions.
    // SAFETY: fd 0 is owned by this process for its whole lifetime and nothing else uses it.
    let mut stream = unsafe { UnixStream::from_raw_fd(0) };
    if stream.set_read_timeout(Some(IDLE_EXIT)).is_err() {
        std::process::exit(2);
    }
    // Panics are reported to the client; the default hook would only add noise to the server log.
    panic::set_hook(Box::new(|_| {}));

    let mut worker = Worker::default();
    loop {
        let request: Request = match protocol::read_json(&mut stream) {
            Ok(r) => r,
            // Idle timeout, the client closing the socket, or garbage: all mean stop.
            Err(_) => return,
        };
        let (response, payload) = handle(&mut worker, request);
        if protocol::write_json(&mut stream, &response).is_err() {
            return;
        }
        if let Some(bytes) = payload {
            if protocol::write_frame(&mut stream, &bytes).is_err() {
                return;
            }
        }
    }
}
