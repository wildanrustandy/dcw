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
dcw new <branch...>     buat worktree (clone DB + file privat + namespace Redis), jalankan, lalu cd ke folder hmx-nya
dcw start [worktree]    jalankan server+worker untuk worktree ATAU repo utama yang SUDAH ADA
dcw stop [worktree]     stop server+worker (worktree & DB tetap)
dcw restart [worktree]  stop lalu start server+worker (worktree & DB tetap)
dcw jump                pilih worktree → cd ke folder hmx-nya
dcw list                daftar worktree + status (up/dead) + URL
dcw abort [worktree]    hapus worktree + stop + DROP DB (konfirmasi)
dcw logs [worktree]     tail log server atau worker
dcw celery <cmd>        kelola worker (start|restart|stop|status)
dcw update              unduh binary rilis terbaru, ganti binary terpasang, tampilkan changelog
dcw changelog [versi]   tampilkan catatan rilis (semua, atau yang lebih baru dari <versi>)
dcw init                lihat config / set up ulang
dcw uninstall           hapus dcw dari mesin
```

**`new` vs `start`**: `new` hanya untuk worktree yang belum ada; `start` hanya yang sudah
ada. Tanpa argumen, `start`/`stop`/`restart`/`abort`/`logs`/`jump` membuka **menu panah**
(butuh TTY; di script/cron, sebut worktree secara eksplisit).

**Beberapa worktree sekaligus.** `dcw new` menerima lebih dari satu nama:

```sh
dcw new PRD_INV_1 PRD_INV_2 PRD_INV_3
dcw new PRD_INV_1,PRD_INV_2,PRD_INV_3
```

Maksimal **3** worktree dibangun serentak; sisanya menunggu di antrean. Tiap worktree
dapat satu baris progress sendiri. Batas 3 bukan soal sopan-santun: checkout dan
`createdb -T` itu I/O-bound (disk terukur ~94% sibuk), jadi paralel berlebih hanya
menambah sedikit kecepatan, sementara tiap worktree hidup memakan ~1.6 GB RAM dan ~1.4 GB
disk. Karena tidak ada satu direktori tujuan, `cd` otomatis hanya dilakukan bila namanya
satu.

**Identitas = nama worktree, bukan nama branch.** `dcw start`/`stop`/`restart`/`abort`/
`logs` menerima nama direktori worktree (mis. `PRD_INV_00296`), karena satu worktree bisa
di-`git switch` ke branch lain kapan saja. Nama branch tetap diterima sebagai alias dan
diresolusi ke worktree yang memilikinya, tetapi output selalu menyebut nama worktree.

**Repo utama juga bisa di-start.** Selain worktree, `dcw start` menerima repo utama
(yang di `dcw list` tampil sebagai `(repo)`) dengan empat cara: nama base branch
(mis. `dcw start Inventory`), label `(repo)`, path repo (`dcw start .` dari dalam repo,
atau path absolut). Server-nya selalu jalan di `$REPO` pada `PORT_BASE`, log ke
`logs/main.log`. `dcw stop`/`restart` menerima keempat bentuk itu juga, dan menu
`dcw start`/`stop` ikut menampilkan baris `(repo)` saat captain-nya mati (`dcw abort`
tetap menolak repo utama). Bila checkout utama sedang memegang branch lain, dcw
memberi peringatan bahwa yang dilayani adalah branch itu.

`dcw jump` dan `dcw new` butuh fungsi shell `dcw()` yang ditulis oleh `dcw init --bin`
(buka terminal baru setelahnya) — keduanya melakukan `cd` ke shell pemanggil: `jump`
(setelah memilih worktree) dan `new` (setelah selesai membuat worktree; path diteruskan
lewat `$DCW_CD_FILE`). `dcw new` yang gagal tidak melakukan `cd`. Helpers venv
`venvmain`/`venvhere` juga dipasang oleh `dcw init`.

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
| `DCW_CELERY_CONCURRENCY` | `2` | jumlah child worker Celery |
| `DCW_CELERY_QUEUES` | 9 queue | queue yang di-consume worker |
| `DCW_CELERY_LOGLEVEL` | `INFO` | level log worker |
| `DCW_USER` | `$USER` | nama user di URL preview |

## Build & rilis

```sh
cargo build --release   # -> target/release/dcw
```

Rilis Linux x86_64 dan macOS (arm64 + Intel) di-build otomatis oleh GitHub Actions
(`.github/workflows/release.yml`) saat push tag `v*`. `dcw update` mengunduh
`dcw-<os>-<arch>` dari release **latest** — jadi setiap rilis baru harus menyertakan
ketiga asset itu, kalau tidak `dcw update` di platform yang asset-nya hilang akan
404. Upload manual bila perlu: `gh release upload <tag> dcw-macos-<arch>`.

Catatan rilis ditulis di [`CHANGELOG.md`](CHANGELOG.md), **satu bagian per tag** dengan
judul `## <versi>` (tanpa awalan `v`). Berkas itu di-*embed* ke dalam binary saat build,
jadi:

- `dcw update` menampilkan changelog setelah binary diganti — dari versi yang sedang
  berjalan sampai versi yang baru dipasang, sehingga lompat beberapa versi tetap terlihat
  semuanya.
- Rilis **lama** tetap bisa menampilkan changelog rilis baru, karena catatannya dibaca dari
  binary yang baru diunduh, bukan dari binary yang sedang jalan. Justru karena itu
  changelog harus di-embed: tanpa itu, `dcw update` tidak akan pernah bisa menceritakan
  rilis yang baru saja ia pasang.

Menambah rilis baru: tambahkan bagian `## <versi>` di paling atas `CHANGELOG.md`, naikkan
`version` di `Cargo.toml` dengan angka yang sama, lalu push tag `v<versi>`.

Isi release di GitHub dibuat otomatis dan tidak memuat catatan — sumber kebenarannya adalah
`CHANGELOG.md` di dalam binary.

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