# dcw — Worktree Creator (Rust)

`dcw` (Dev-Captain Worktree) sebagai **binary Rust tunggal** — pengganti script
bash lama di `~/dcw`. Same concept, same isolasi per-branch.

Setiap *feature branch* proyek HMX (Django + Celery) mendapat lingkungan dev yang
**terisolasi penuh**:

1. **Worktree** sendiri di `<repo>/.worktrees/<branch>`
2. **Port** sendiri (base `5200`, worktree `5201+`)
3. **Database clone** sendiri (`hmxmain` → DB per-branch, `createdb -T` + fallback dump/restore)
4. **Namespace Redis** sendiri (`HMX_INSTANCE_NAMESPACE` per branch, ditulis ke `.env` worktree)
5. **Worker Celery** sendiri + **server Django** sendiri

State tersimpan di `~/.dev-captain/` (registry port, log, pidfile worker).

## Install (curl)

Cara termudah — **tanpa clone repo dan tanpa Rust toolchain**:

```sh
curl -fsSL https://git.hashmicro.com/wildanhashmicro/dcw/-/raw/main/install.sh | bash
```

Installer mengunduh binary prebuilt untuk OS/arch mesin Anda, memasangnya ke
`~/.local/bin/dcw`, lalu menjalankan `dcw init` non-interaktif (pakai default).
Untuk konfigurasi ulang repo/venv/port: `dcw init`.

Override saat install:

| Env | Keterangan |
|---|---|
| `DCW_UPDATE_URL` | URL lengkap binary (lewati deteksi OS/arch) |
| `DCW_UPDATE_BASE` | base URL tempat nama asset ditempel (default: GitLab release permalink) |
| `DCW_BIN_DIR` | direktori install (default: `~/.local/bin`) |

## Build dari source (developer)

```sh
export PATH="$HOME/.cargo/bin:$PATH"   # kalau cargo belum di PATH shell Anda
cd dcw
cargo build --release      # -> target/release/dcw

# pasang + konfigurasi interaktif (mirip dcw install.sh):
./target/release/dcw init --bin ~/.local/bin
```

`dcw init` menampilkan **konfigurasi saat ini**, lalu (kalau config sudah ada)
bertanya `Reconfigure? [y/N]` sebelum menjalankan wizard. Wizard menanyakan
**folder project HMX → venv Python → base port → max port**, lalu menulis
`~/.config/dcw/config`. Default-nya **pre-fill dari config dcw** yang sudah ada
(`~/.config/dcw/config`), jadi kalau dcw sudah pernah di-setup, tinggal tekan Enter.

`dcw setup` adalah alias `dcw init`.

Non-interaktif:

```sh
./target/release/dcw init --yes \
    --repo ~/work/hmx --venv ~/work/venv \
    --port-base 5300 --port-max 5320 --bin ~/.local/bin
```

`--bin DIR` menyalin binary ke `DIR/dcw` (default tidak menyalin — cukup ganti binary
build Anda sendiri). Menyetel ulang kapan saja: jalankan `dcw init` lagi.

## Rilis (prebuilt binary)

Instalasi `curl` dan `dcw update` sama-sama mengunduh asset `dcw-<os>-<arch>` dari
base URL rilis (default: permalink GitLab release). Agar keduanya bekerja, lampirkan
binary per-target ke release GitLab dengan nama persis:

| Asset | Target |
|---|---|
| `dcw-linux-x86_64` | Linux x86_64 |
| `dcw-linux-aarch64` | Linux ARM64 |
| `dcw-macos-x86_64` | macOS Intel |
| `dcw-macos-aarch64` | macOS Apple Silicon |

`<os>`/`<arch>` mengikuti `std::env::consts` Rust (`linux`, `macos`, `x86_64`,
`aarch64`) — bukan `darwin`/`amd64`/`arm64`. Build per-target bisa via CI atau
cross-compile; contoh satu target (host):

```sh
cargo build --release                    # target host (mis. linux/x86_64)
cp target/release/dcw dcw-linux-x86_64   # ganti nama ke konvensi di atas
```

## Usage

```
dcw new <branch>        BUAT worktree baru (clone DB + salin file privat + namespace
                        Redis) lalu nyalakan server + worker. Butuh nama branch;
                        error kalau worktree-nya sudah ada (pakai `dcw start`)
dcw start [branch]      jalankan server + worker untuk worktree yang SUDAH ADA
                        (base branch → $REPO di PORT_BASE). tanpa <branch> → menu
                        panah worktree yang sedang mati. Tidak pernah buat worktree
dcw stop [branch]       stop server + worker (worktree & DB tetap). tanpa → menu panah
dcw jump                menu panah pilih worktree (up/dead) → shell cd ke folder itu
dcw list                daftar semua worktree (nama worktree, bukan branch) + status (up/dead) + URL, up dulu
dcw abort [branch]      hapus worktree + stop + DROP DB (konfirmasi). tanpa → menu
dcw logs [branch] [celery]  tail log server atau worker. tanpa → menu panah (multi-select)
dcw celery start|restart|stop|status [branch]   kelola worker (default: base branch)
dcw update              unduh binary release terbaru, lalu ganti binary terpasang
dcw init                setup/inisialisasi ulang: lihat config + tanya reconfigure
dcw uninstall           hapus dcw dari mesin (binary + config + blok rc)
```

### `dcw update` — self-update

`dcw update` mengunduh binary prebuilt rilis terbaru untuk OS/arch mesin ini lalu
**menukarnya secara atomik** dengan binary yang sedang berjalan — tanpa git checkout
dan tanpa Rust toolchain. Butuh `curl` (atau fallback `wget`) di mesin target.

URL di-resolve dengan urutan prioritas:

1. `DCW_UPDATE_URL`  — URL lengkap binary (hosting override)
2. `DCW_UPDATE_BASE` — base URL tempat nama asset ditempel
3. bawaan            — GitLab release permalink proyek ini

Nama asset adalah `dcw-<os>-<arch>` (mis. `dcw-linux-x86_64`) — sama persis dengan
yang dihasilkan `install.sh` dari `uname` dan `std::env::consts` di Rust.

`start`/`stop`/`abort`/`logs`/`jump` tanpa argumen membuka **menu navigasi panah**
(`Space` menandai beberapa baris, `Enter` konfirmasi, `q` batal) seperti dcw. Menu butuh
TTY; di script/cron, sebut nama branch secara eksplisit.

**`new` vs `start`**: `new` hanya untuk worktree yang belum ada — kalau sudah ada ia
berhenti dan menyuruh pakai `dcw start`. `start` hanya menjalankan yang sudah ada —
kalau worktree-nya tidak ada ia berhenti dan menyuruh pakai `dcw new`.

`stop` dan `abort` memakai kandidat yang sama: semua worktree milik dcw
(`<repo>/.worktrees/*`), diurutkan `up` dulu lalu by port, dengan kolom
`BRANCH | PORT | STATUS`.

### `dcw jump` — pindah ke worktree

`dcw jump` membuka menu (worktree `up` maupun `dead`, plus base branch sebagai baris
`base`), lalu **shell Anda `cd` ke folder `hmx` worktree itu**:

```sh
~/repo $ dcw jump
  BRANCH                             PORT    STATUS
> Inventory                          -       base
  posko                              4203    up
  PRD_INV_00296                      -       down

  up/down move   Enter select   q cancel
~/repo/.worktrees/posko/hmx $        # ← sudah pindah
```

Menu `jump` (dan `start`) bersifat **single-select** — tidak ada checkbox; baris di bawah
kursor adalah pilihannya, `Enter` menjalankannya.

Ini **tidak bisa** dikerjakan binary sendiri: proses anak tidak dapat mengubah working
directory shell pemanggilnya. Karena itu `dcw init --bin DIR` juga menulis **fungsi
shell `dcw()`** ke file rc login shell Anda (`~/.zshenv` untuk zsh, `~/.bashrc` +
login file untuk bash) — fungsi itu menangkap stdout `dcw jump` (yang berisi **hanya
path**; menunya digambar ke `/dev/tty`) lalu `cd` ke `<path>/hmx`, fallback ke `<path>`.
Subcommand lain diteruskan apa adanya; `command dcw ...` melewati fungsi; `DCW_BIN`
mengarahkan fungsi ke binary lain.

Jalankan `dcw init --bin ~/.local/bin` sekali (lalu buka terminal baru) supaya fungsi ini
terpasang.

### venv helpers: `venvmain` dan `venvhere`

Sama seperti dcw (`venvhmx`/`venvhere`), dengan satu nama berbeda —
**`venvmain`** (dulu `venvhmx`):

| Fungsi | Efek |
|---|---|
| `venvmain` | activate venv project **dan** `cd` ke repo utama |
| `venvhere` | activate venv **tanpa** pindah direktori — tetap di tempat Anda |

`venvhere` dipasangkan dengan `dcw jump`: setelah `dcw jump` Anda berada di `hmx` di
dalam worktree, lalu `venvhere` mengaktifkan venv tanpa memindahkan Anda lagi:

```sh
~/repo $ dcw jump          # pilih worktree dari menu
~/repo/.worktrees/posko/hmx $ venvhere
venvhere: aktif -> Python 3.14.4 @ .../posko/hmx
```

Urutan pencarian venv `venvhere`: argumen (`venvhere /path/venv`) → `.venv` di
direktori sekarang → `.venv` di root git/worktree → venv dari config dcw
(`DEV_CAPTAIN_VENV`). `venvmain` membaca `~/.config/dcw/config` untuk repo + venv, jadi
keduanya mengikuti config — tidak ada path hardcode.

`dcw init` menulis helper-nya ke `~/.config/dcw/shell/venv.sh` (file terpisah, jadi
re-run `dcw init` memperbarui fungsinya tanpa mengubah rc file lagi) dan menambahkan
baris `source` ke rc file yang sama dengan blok `dcw()`. Buka terminal baru
(setelah `dcw init`) untuk mengaktifkannya.

## Config & env

Precedence: **env per-call** > `~/.config/dcw/config` > default bawaan

| Env | Default | Keterangan |
|---|---|---|
| `DEV_CAPTAIN_REPO` | `~/dev/hmx-002` | root repo HMX |
| `DEV_CAPTAIN_VENV` | `~/dev/.venv` | path venv Python |
| `DEV_CAPTAIN_BASE` | `origin/HEAD` | base branch |
| `PORT_BASE` / `PORT_MAX` | `5200` / `5220` | range port |
| `DCW_DOMAIN` | auto (DevSpace nginx) | domain preview URL; kosong = `http://127.0.0.1:<port>` |
| `DCW_USER` | `$USER` | bagian user pada host preview |
| `DCW_CELERY` | `1` | `0` = jangan jalankan worker |
| `DCW_CELERY_QUEUES` | `interactive,import,export,scheduled,notifications,csv,data_import,demo_pool,celery` | daftar queue |
| `DCW_CELERY_CONCURRENCY` | `2` | `--concurrency` worker |
| `DCW_CELERY_LOGLEVEL` | `INFO` | loglevel worker |
| `DCW_UPDATE_URL` | GitLab release permalink | URL lengkap binary untuk `dcw update` |
| `DCW_UPDATE_BASE` | GitLab release permalink | base URL tempat `dcw-<os>-<arch>` ditempel |

## State

| Path | Isi |
|---|---|
| `~/.dev-captain/ports.json` | registry port ↔ branch ↔ pid |
| `~/.dev-captain/logs/` | log server + `<branch>-celery.log` |
| `~/.dev-captain/celery/<branch>.pid` | pid + nama branch worker |
| `<repo>/.worktrees/<branch>` | worktree (di-gitignore repo) |

## Catatan

- `dcw abort` menghapus DB branch secara **permanen** (ada konfirmasi).
- DB `hmxmain` tidak pernah disentuh oleh `new`/`abort` worktree.
- Deteksi base branch dari `origin/HEAD`; fallback branch saat ini, lalu `main`.
- **Depend on macOS/Linux**: `lsof` untuk cek/port, dan `/proc/<pid>/stat` untuk
  deteksi zombie (seperti dcw).

## Catatan implementasi

- Tidak ada `dcw path` — direktori worktree dicetak oleh `dcw jump` (interaktif).
- venv helpers: `venvmain` (activate venv + cd ke repo) dan `venvhere`
  (activate venv di tempat); keduanya dipasang `dcw init` (lihat bagian venv helpers).
- Binary spawn server/worker memakai `setsid` + ignore-SIGHUP **in-process**
  (`pre_exec`), tanpa binary `setsid`/`nohup` eksternal.
- Artefak build `target/` di-gitignore; distribusi cukup binary `dcw` tunggal.

## Uninstall

```sh
dcw uninstall
```

Menghapus binary (`dcw`/`wtc` + marker `.*-source` beserta sisa file `.incoming`),
`~/.config/dcw` (config + `shell/venv.sh`), dan blok yang ditulis installer di rc
file (`~/.zshenv` / `~/.bashrc` / …) — setiap rc file yang diubah **di-backup**
ke `<rc>.dcw-uninstall.bak`. Worktree di `<repo>/.worktrees/`, DB hasil clone, dan
state `~/.dev-captain/` **tidak** disentuh (sama seperti uninstall dcw). Buka
terminal baru setelahnya agar fungsi shell lama hilang.