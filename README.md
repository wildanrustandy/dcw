# dcw — Worktree Creator (Rust)

Binary Rust tunggal pengganti script bash `dcw`. Setiap *feature branch* proyek HMX
(Django + Celery) mendapat lingkungan dev **terisolasi penuh**: worktree sendiri, port
sendiri, clone DB sendiri, namespace Redis sendiri, plus server Django & worker Celery
sendiri. State tersimpan di `~/.dev-captain/`.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/wildanrustandy/dcw/main/install.sh | bash
```

Memasang binary prebuilt ke `~/.local/bin/dcw` lalu menjalankan `dcw init` dengan
default. Set ulang repo/venv/port kapan saja: `dcw init` (interaktif) atau non-interaktif:

```sh
dcw init --yes --repo ~/work/hmx --venv ~/work/venv --port-base 5300 --port-max 5320 --bin ~/.local/bin
```

`dcw setup` = alias `dcw init`.

> **Windows**: jalankan di dalam **WSL2** (`wsl --install -d Ubuntu`) — pakai binary
> Linux yang sama, tidak perlu build khusus.

## Usage

```
dcw new <branch>        buat worktree (clone DB + file privat + namespace Redis) lalu jalankan
dcw start [branch]      jalankan server+worker untuk worktree yang SUDAH ADA
dcw stop [branch]       stop server+worker (worktree & DB tetap)
dcw restart [branch]    stop lalu start server+worker (worktree & DB tetap)
dcw jump                pilih worktree → cd ke folder hmx-nya
dcw list                daftar worktree + status (up/dead) + URL
dcw abort [branch]      hapus worktree + stop + DROP DB (konfirmasi)
dcw logs [branch]       tail log server atau worker
dcw celery <cmd>        kelola worker (start|restart|stop|status)
dcw update              unduh binary rilis terbaru, ganti binary terpasang
dcw init                lihat config / set up ulang
dcw uninstall           hapus dcw dari mesin
```

**`new` vs `start`**: `new` hanya untuk worktree yang belum ada; `start` hanya yang sudah
ada. Tanpa argumen, `start`/`stop`/`abort`/`logs`/`jump` membuka **menu panah** (butuh TTY;
di script/cron, sebut branch secara eksplisit).

`dcw jump` butuh fungsi shell `dcw()` yang ditulis oleh `dcw init --bin` (buka terminal
baru setelahnya). Helpers venv `venvmain`/`venvhere` juga dipasang oleh `dcw init`.

## Config

Precedence: **env per-call** > `~/.config/dcw/config` > default.

| Env | Default | Keterangan |
|---|---|---|
| `DEV_CAPTAIN_REPO` | `~/dev/hmx-002` | root repo HMX |
| `DEV_CAPTAIN_VENV` | `~/dev/.venv` | venv Python |
| `DEV_CAPTAIN_BASE` | `origin/HEAD` | base branch |
| `PORT_BASE` / `PORT_MAX` | `5200` / `5220` | range port |
| `DCW_DOMAIN` | auto | domain preview (kosong = `http://127.0.0.1:<port>`) |
| `DCW_CELERY` | `1` | `0` = tanpa worker |

## Build & rilis

```sh
cargo build --release   # -> target/release/dcw
```

Rilis Linux di-build otomatis oleh GitHub Actions (`.github/workflows/release.yml`)
saat push tag `v*`. Binary macOS di-cross-compile via `cargo-zigbuild` (atau build di
Mac), lalu `gh release upload <tag> dcw-macos-<arch>`.

## Uninstall

```sh
dcw uninstall
```

Menghapus binary, `~/.config/dcw`, dan blok rc-file (di-backup ke `<rc>.dcw-uninstall.bak`).
Worktree, DB clone, dan state `~/.dev-captain/` **tidak** disentuh.

## Catatan

- State: `~/.dev-captain/` (port registry, log, pidfile) + `<repo>/.worktrees/<branch>`.
- `dcw abort` hapus DB branch **permanen** (ada konfirmasi); DB `hmxmain` tak pernah disentuh.
- macOS/Linux only. Butuh `git`, `lsof`, `curl`/`wget`, dan (untuk `new`/`abort`)
  `psql`/`createdb`/`pg_dump`/`dropdb` di PATH.