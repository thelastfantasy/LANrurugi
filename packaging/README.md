# LANrurugi — native package

This directory is a complete LANrurugi installation for one platform. Unpack it anywhere (for
example `/opt/lanrurugi` or `~/Applications/lanrurugi`); the program finds its web interface,
plugins, documentation, bundled libraries and models relative to its own location.

Full guide: <https://docs.lrrg.dev/guide/install-native/>

## 1. Install the prerequisites

| | Linux (Debian/Ubuntu) | Linux (Fedora) | Linux (Arch) | macOS (Homebrew) | FreeBSD |
|---|---|---|---|---|---|
| Redis ≥ 7.4 | `apt install redis-server` ¹ | `dnf install redis` ² | `pacman -S redis` | `brew install redis` | `pkg install redis` |
| Deno (plugins) | [deno.com](https://deno.com) install script | [deno.com](https://deno.com) install script | `pacman -S deno` | `brew install deno` | `pkg install deno` |
| DjVu books | `apt install djvulibre-bin` | `dnf install djvulibre` | `pacman -S djvulibre` | `brew install djvulibre` | `pkg install djvulibre` |
| CJK fonts | `apt install fonts-noto-cjk` | `dnf install google-noto-serif-cjk-fonts google-noto-sans-cjk-fonts` | `pacman -S noto-fonts-cjk` | built in | `pkg install noto-serif-sc noto-sans-sc noto-serif-jp noto-sans-jp noto-serif-kr noto-sans-kr` |
| Archive codecs | built in | built in | built in | `brew install xz lz4` | `pkg install lz4` |
| PDF books | bundled | bundled | bundled | bundled | `pkg install pdfium` |
| OCR / translation / recommendations | bundled | bundled | bundled | bundled | `pkg install onnxruntime` |

¹ Debian 12 ships Redis 7.0 — fine for a new library; data copied from the container image needs
7.4 (use the redis.io repository).
² Recent Fedora installs Valkey, a Redis-compatible server — fine for a new library; it cannot open
data files written by Redis 7.4, so use Redis itself to keep a container deployment's data.

## 2. Run it

```sh
redis-server --daemonize yes              # or your system's Redis service
./bin/lanrurugi-server serve \
  --library-path ~/manga --thumb-dir ~/.local/share/lanrurugi/thumb \
  --temp-dir ~/.local/share/lanrurugi/temp --log-dir ~/.local/share/lanrurugi/log
```

Open <http://localhost:3000>. The first-run wizard changes the default password, takes the AI API
key and can import a LANraragi backup. The start-up log lists any optional component it could not
find and which features that disables.

## 3. Translation models (optional)

```sh
./fetch-models.sh                  # OCR + text removal (several hundred MB)
./fetch-models.sh --with-bubbles   # also exact speech-bubble shapes (needs `uv`)
```

## 4. Run as a service

```sh
sudo ./install-service.sh /var/lib/lanrurugi   # Linux (systemd), FreeBSD (rc.d)
./install-service.sh ~/LANrurugi                # macOS (launchd, per user)
```

It creates the data directories and `lanrurugi.env` (library location, Redis URL, port) and installs
the service definition for this system. `service/redis.conf.example` configures a dedicated Redis
like the container's, which can reuse a container deployment's `database` directory.

## What works where

GPU-accelerated inference is Linux-only; macOS and FreeBSD translate on the CPU. Everything else
works on every platform once the prerequisites above are installed.
