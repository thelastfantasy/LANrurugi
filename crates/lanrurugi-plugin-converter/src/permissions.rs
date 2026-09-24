//! Heuristic detection of what Deno permissions a converted plugin will likely need
//! (constitution Principle IV — the host must never grant more than a plugin declares). This
//! can only ever be a hint: it greps the Perl source for module imports/builtins that imply
//! network or filesystem access, so the human finishing the conversion knows to look, rather
//! than silently defaulting every converted plugin to the widest-possible grant "to be safe".

const NETWORK_MARKERS: &[&str] = &[
    "Mojo::UserAgent",
    "LWP::UserAgent",
    "HTTP::Tiny",
    "Net::",
    "->get(",
    "->post(",
];

const FILESYSTEM_WRITE_MARKERS: &[&str] = &["unlink", "open( my $fh, '>'", "open(my $fh, '>'"];

pub fn guess_network_usage(source: &str) -> bool {
    NETWORK_MARKERS.iter().any(|m| source.contains(m))
}

pub fn guess_filesystem_write(source: &str) -> bool {
    FILESYSTEM_WRITE_MARKERS.iter().any(|m| source.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_mojo_useragent() {
        assert!(guess_network_usage("use Mojo::UserAgent;\n"));
    }

    #[test]
    fn detects_lwp() {
        assert!(guess_network_usage("use LWP::UserAgent;\n"));
    }

    #[test]
    fn pure_local_plugin_has_no_network_markers() {
        assert!(!guess_network_usage(
            "use LANraragi::Utils::Archive qw(is_file_in_archive extract_file_from_archive);\n"
        ));
    }

    #[test]
    fn detects_unlink_as_a_write() {
        assert!(guess_filesystem_write("unlink $filepath;\n"));
    }
}
