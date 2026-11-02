# MistCache

## Introduction
MistCache is a fast and memory-efficient caching server for your Steam library, written in Rust. Under the hood, MistCache takes advantage of Cloudflare's wonderful Pingora library for proxying.

The first time a game is downloaded, it's proxied to your MistCache server. The next time anyone downloads the same game, they get it at LAN speed. This is far faster than having to download it straight from the internet again.

## How it works

1. A built-in DNS server answers `lancache.steamcontent.com` with your machine's IP, so Steam clients send their downloads there. Every other DNS lookup is forwarded to an upstream resolver.
2. The HTTP cache checks whether the requested file is already on disk.
   - **Hit:** the file is streamed straight from disk.
   - **Miss:** the file is fetched from Steam, sent to the client, and saved to disk as it downloads.
3. If you set a size limit, the least recently used files are deleted once the cache grows past it.

Only successful `GET` responses are cached. Downloads that are interrupted are never saved, so the cache won't contain partial files.

## Building

You need to have the [Rust toolchain](https://rustup.rs) installed on your system.

```sh
git clone https://github.com/DownThePark/MistCache
cd MistCache
cargo build --release
```

Run it once to generate `config.toml`. Afterwards, stop the program, edit the configuration file, and run the program again.

```sh
sudo ./target/release/mistcache
```

Then, set the DNS server on your gaming PCs (or your router's DHCP settings) to the IP of the machine running MistCache.

## Configuration

Settings live in `config.toml`, which is created on first run. Restart the program after modifying it.

| Setting | Description |
| --- | --- |
| `cache_dir` | Folder where cached files are stored. Created if missing. |
| `max_cache_size_gb` | Size limit in GB before old files are deleted. `0` means unlimited. |
| `upstream_dns` | Resolver that all other DNS lookups are forwarded to. |
| `http_bind` | IP the cache listens on. With DNS enabled, this must be the machine's LAN IP (for example `192.168.1.10`), not `0.0.0.0`. |
| `dns_enabled` | `true` runs the built-in DNS server. `false` disables it, and you'll need to point clients to lancache.steamcontent.com to your MistCache IP another way. 
| `dns_bind` | IP the DNS server listens on. `0.0.0.0` accepts queries on every interface. |

## Comparison to LANCache
MistCache is served as a single binary file, and uses far less memory than LANCache (which uses multiple services tied together).

However, MistCache is only intended to work with Steam, whereas LANCache works with various game launchers and services.

## Notes
- The machine running MistCache must not use itself as its DNS server for Steam lookups. Otherwise cache misses would loop back to the cache.
- Log verbosity can be reduced with the `RUST_LOG` environment variable. For example: `RUST_LOG=warn`.
- Disclaimer: AI was largely used to work on this project.
