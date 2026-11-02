# LANCache-Rust

A small Steam download cache for your LAN, written in Rust. The first time a game file is downloaded, it's saved to disk. Every other PC on your network then gets it at LAN speed instead of pulling it from the internet again.

## How it works

1. A built-in DNS server answers `lancache.steamcontent.com` with this machine's IP, so Steam clients send their downloads here. Every other DNS lookup is forwarded to an upstream resolver.
2. The HTTP cache (built on [Pingora](https://github.com/cloudflare/pingora)) checks whether the requested file is already on disk.
   - **Hit:** the file is streamed straight from disk.
   - **Miss:** the file is fetched from Steam, sent to the client, and saved to disk as it downloads.
3. If you set a size limit, the least recently used files are deleted once the cache grows past it.

Only successful `GET` responses are cached. Downloads that are interrupted are never saved, so the cache won't contain partial files.

## Building

You need the [Rust toolchain](https://rustup.rs)

```sh
git clone https://github.com/DownThePark/LANCache-Rust
cd LANCache-Rust
cargo build --release
```

Run it once to generate `config.toml`. Afterwards, stop the program to edit configuration file, then run the program again.

```sh
sudo ./target/release/lancache
```

Then set the DNS server on your gaming PCs (or your router's DHCP settings) to the IP of the machine running LANCache.

## Configuration

Settings live in `config.toml`, which is created on first run. Restart the program after modifying it.

| Setting | Description |
| --- | --- |
| `cache_dir` | Folder where cached files are stored. Created if missing. |
| `max_cache_size_gb` | Size limit in GB before old files are deleted. `0` means unlimited. |
| `upstream_dns` | Resolver that all other DNS lookups are forwarded to. |
| `http_bind` | IP the cache listens on. With DNS enabled, this must be the machine's LAN IP (for example `192.168.1.10`), not `0.0.0.0`. |
| `dns_enabled` | `true` runs the built-in DNS server. `false` disables it, and you'll need to point clients to lancache.steamcontent.com to your LANCache IP another way. 
| `dns_bind` | IP the DNS server listens on. `0.0.0.0` accepts queries on every interface. |

## Notes
- The HTTP and DNS ports are fixed at 80/tcp and 53/udp.
- The machine running LANCache must not use itself as its DNS server for Steam lookups. Otherwise cache misses would loop back to the cache.
- Log verbosity can be changed with the `RUST_LOG` environment variable (for example `RUST_LOG=debug`).
