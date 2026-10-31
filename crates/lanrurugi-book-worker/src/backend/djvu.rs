//! DjVu through DjVuLibre's command-line tools (`djvused` for structure, `ddjvu` for pixels).
//! DjVuLibre is GPL; running its programs rather than linking the library keeps it a separate
//! work. `LANRURUGI_DJVU_BIN_DIR` points at the tools, otherwise `PATH` is searched.

#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use lanrurugi_book::protocol::{BookInfo, OutlineItem, PageSource};
use lanrurugi_book::BookFormat;

use super::{encode_webp, Failure, Result};

fn tool(name: &str) -> PathBuf {
    match std::env::var_os("LANRURUGI_DJVU_BIN_DIR") {
        Some(dir) => PathBuf::from(dir).join(name),
        None => PathBuf::from(name),
    }
}

/// Runs a DjVuLibre tool. On Linux it is killed with this worker if the worker is killed (a
/// timed-out worker must not leave a decoder running); elsewhere a tool outlives a killed worker
/// only until it finishes the one page it was asked for.
fn run(name: &str, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new(tool(name));
    command
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(target_os = "linux")]
    // SAFETY: prctl is async-signal-safe and touches no Rust state between fork and exec.
    unsafe {
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
    let output = command
        .output()
        .map_err(|e| Failure::internal(format!("cannot run {name} (DjVuLibre installed?): {e}")))?;
    if !output.status.success() {
        return Err(Failure::unreadable(format!(
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

/// Pixel size of every page.
fn page_sizes(path: &str) -> Result<Vec<(u32, u32)>> {
    let count: usize = String::from_utf8_lossy(&run("djvused", &["-e", "n", path])?)
        .trim()
        .parse()
        .map_err(|_| Failure::unreadable("djvused: no page count"))?;
    let script: String = (1..=count).map(|n| format!("select {n}; size; ")).collect();
    let out = String::from_utf8_lossy(&run("djvused", &["-e", &script, path])?).into_owned();
    let sizes: Vec<(u32, u32)> = out
        .lines()
        .filter_map(|line| {
            let field = |key: &str| {
                line.split_whitespace()
                    .find_map(|kv| kv.strip_prefix(key))
                    .and_then(|v| v.parse::<u32>().ok())
            };
            Some((field("width=")?, field("height=")?))
        })
        .collect();
    if sizes.len() != count || count == 0 {
        return Err(Failure::unreadable("djvused: page sizes unavailable"));
    }
    Ok(sizes)
}

fn scaled((w, h): (u32, u32), width: u32) -> (u32, u32) {
    let w = w.max(1);
    (width, ((h as u64 * width as u64) / w as u64).max(1) as u32)
}

pub fn inspect(path: &str, width: u32) -> Result<BookInfo> {
    let sizes: Vec<(u32, u32)> = page_sizes(path)?
        .into_iter()
        .map(|s| scaled(s, width))
        .collect();
    let outline = run("djvused", &["-u", "-e", "print-outline", path])
        .map(|out| outline_from_sexpr(&String::from_utf8_lossy(&out), sizes.len()))
        .unwrap_or_default();
    Ok(BookInfo {
        format: BookFormat::Djvu,
        pages: vec![PageSource::Render; sizes.len()],
        sizes,
        outline,
        title: None,
        author: None,
    })
}

pub fn page(path: &str, index: u32, width: u32) -> Result<(String, Vec<u8>)> {
    let size = run(
        "djvused",
        &["-e", &format!("select {}; size", index + 1), path],
    )?;
    let size = String::from_utf8_lossy(&size);
    let field = |key: &str| {
        size.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key))
            .and_then(|v| v.parse::<u32>().ok())
    };
    let (w, h) = field("width=")
        .zip(field("height="))
        .ok_or(Failure::out_of_range(index))?;
    let (tw, th) = scaled((w, h), width);
    let out = std::env::temp_dir().join(format!("lrr-djvu-{}-{index}.ppm", std::process::id()));
    let out_str = out.to_string_lossy().into_owned();
    let result = run(
        "ddjvu",
        &[
            "-format=ppm",
            &format!("-page={}", index + 1),
            &format!("-size={tw}x{th}"),
            path,
            &out_str,
        ],
    )
    .and_then(|_| std::fs::read(&out).map_err(Failure::from));
    let _ = std::fs::remove_file(&out);
    let picture = image::load_from_memory_with_format(&result?, image::ImageFormat::Pnm)
        .map_err(|e| Failure::unreadable(format!("ddjvu output: {e}")))?;
    Ok(("webp".into(), encode_webp(&picture.to_rgb8())?))
}

/// An s-expression node of `djvused print-outline` output.
#[derive(Debug, PartialEq)]
enum Node {
    List(Vec<Node>),
    Str(String),
    Atom(String),
}

fn parse_sexpr(input: &str) -> Option<Node> {
    fn node(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<Node> {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        match chars.next()? {
            '(' => {
                let mut items = Vec::new();
                loop {
                    while chars.peek().is_some_and(|c| c.is_whitespace()) {
                        chars.next();
                    }
                    match chars.peek()? {
                        ')' => {
                            chars.next();
                            return Some(Node::List(items));
                        }
                        _ => items.push(node(chars)?),
                    }
                }
            }
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => return Some(Node::Str(s)),
                        '\\' => match chars.next()? {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            other => s.push(other),
                        },
                        c => s.push(c),
                    }
                }
                None
            }
            c => {
                let mut atom = c.to_string();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == '(' || c == ')' {
                        break;
                    }
                    atom.push(c);
                    chars.next();
                }
                Some(Node::Atom(atom))
            }
        }
    }
    node(&mut input.chars().peekable())
}

fn outline_from_sexpr(text: &str, page_count: usize) -> Vec<OutlineItem> {
    fn walk(entries: &[Node], depth: u32, page_count: usize, out: &mut Vec<OutlineItem>) {
        for entry in entries {
            let Node::List(items) = entry else { continue };
            if let (Some(Node::Str(title)), Some(Node::Str(target))) = (items.first(), items.get(1))
            {
                let page = target
                    .strip_prefix('#')
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|&n| n >= 1 && n <= page_count);
                if let (Some(page), false) = (page, title.trim().is_empty()) {
                    out.push(OutlineItem {
                        title: title.trim().to_owned(),
                        page: page as u32 - 1,
                        depth,
                    });
                }
                if depth < 6 {
                    walk(&items[2..], depth + 1, page_count, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    if let Some(Node::List(items)) = parse_sexpr(text) {
        if items.first() == Some(&Node::Atom("bookmarks".into())) {
            walk(&items[1..], 0, page_count, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_sexpr() {
        let text = r##"(bookmarks
 ("第一章 \"春\"" "#1"
  ("Section" "#2") )
 ("Chapter 2" "#5")
 ("Dangling" "#99") )"##;
        let outline = outline_from_sexpr(text, 10);
        let items: Vec<_> = outline
            .iter()
            .map(|o| (o.title.as_str(), o.page, o.depth))
            .collect();
        assert_eq!(
            items,
            [
                ("第一章 \"春\"", 0, 0),
                ("Section", 1, 1),
                ("Chapter 2", 4, 0)
            ]
        );
    }
}
