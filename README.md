# dcw — Dev-Captain Worktree (macOS)

Manajemen **git worktree + port + server Django HMX** dalam satu command — di-porting dari
`dev-captain` / `dcw` HashMicro DevSpace agar bisa dipakai di Mac lokal.

Setiap feature branch mendapat:

- **Worktree** sendiri di `<repo>/.worktrees/<branch>`
- **Port** sendiri (base `5200`, worktree `5201+`)
- **Database clone** sendiri (`hmxmain` → DB per-branch, via `createdb -T`, fallback dump/restore)
- **File privat** tersalin otomatis: `.agents`, `.claude`, `.vscode`, `AGENTS.md`, `hmx/.env`, `hmx/hmx/db.json`
- `.env` dan `db.json` worktree otomatis diarahkan ke DB branch tersebut (data utama aman)

## Install

```sh
git clone <repo-ini> ~/dcw
~/dcw/install.sh
```

Installer **interaktif** — akan menanyakan:

1. Folder project HMX (default `~/dev/hmx-002`)
2. Folder venv Python (default `~/dev/.venv`)
3. Base port & max port (default `5200` & `5220` — base branch on `5200`, worktrees use `5201+`)

Jawaban tersimpan di `~/.config/dcw/config` dan dibaca `dcw` setiap kali jalan.
Bisa juga non-interaktif lewat flag:

```sh
~/dcw/install.sh --repo ~/work/hmx --venv ~/work/venv --port-base 5300 --port-max 5320 --yes
```

### Lokasi binary & PATH

`--bin DIR` menentukan folder instalasi. Defaultnya **auto**: folder pertama dari
`~/.local/bin`, `~/bin` yang **sudah ada di PATH shell login**; kalau dua-duanya tidak ada,
`~/.local/bin`.

Installer tidak mempercayai `$SHELL`. Dia **menanyakan shell login yang sebenarnya**
(lalu membunuhnya lagi dengan SIGKILL kalau rc-nya menggantung) untuk tahu PATH yang
benar-benar dipakai terminal baru, dan menulis PATH ke file rc yang shell itu baca:

| Shell login | File rc |
|---|---|
| zsh | `~/.zshenv` (dibaca semua zsh: login, interaktif, skrip) |
| bash | `~/.bashrc`, lalu escalate ke `~/.bash_profile` / `~/.bash_login` / `~/.profile` |
| fish | `~/.config/fish/config.fish` |

Setiap tulisan langsung **diverifikasi** dengan menanyakan ulang shell login. Ini penting di
host tanpa root: `/etc/passwd` masih menunjuk `/bin/bash` (karena `chsh` butuh root)
sementara `~/.bashrc` men-`exec` zsh user-local — di setup itu baris PATH di `~/.bashrc`
tidak pernah dieksekusi dan zsh tidak membaca `.bashrc` sama sekali. Login bash juga tidak
membaca `~/.bashrc`, jadi kalau verifikasi gagal installer naik ke kandidat berikutnya
alih-alih melaporkan sukses palsu.

Prioritas konfigurasi: **env variable per-call** > **config file** > *default bawaan*.
Installer juga mengecek dependency (`git`, `python3`, `lsof`, PostgreSQL client tools)
dan memvalidasi path repo/venv yang dimasukkan.

Perubahan `install.sh` sebaiknya diverifikasi dengan `./test-install.sh` (27 asersi:
deteksi shell login, penulisan PATH, idempotensi, anti-hang). Pakai
`SKIP_LIVE=1 ./test-install.sh` untuk melewati asersi instalasi di host ini.

### Prasyarat

1. **Repo HMX** ada di `~/dev/hmx-002` (default; bisa di-override)
2. **Venv Python** di `~/dev/.venv` — berisi dependency `requirements.txt` + `hmx` CLI
3. **PostgreSQL** lokal dengan DB utama `hmxmain` (client tools di PATH:
   `psql`, `createdb`, `dropdb`, `pg_dump` — `brew install postgresql@16 libpq`)
4. Kredensial DB dibaca otomatis dari `<repo>/hmx/.env` (`DATABASE_*`)

Kalau lokasi Anda beda dari yang dipilih saat install, cukup set env sebelum memanggil (override sekali jalan):

```sh
DEV_CAPTAIN_REPO="$HOME/dev/hmx-002" DEV_CAPTAIN_VENV="$HOME/dev/.venv" dcw start <branch>
```

atau edit `~/.config/dcw/config`.

## Usage

```sh
dcw start <branch>    # buat worktree + clone DB + salin file privat + jalankan server
dcw status            # daftar captain aktif (port, pid, hidup/mati)
dcw list              # daftar semua git worktree
dcw logs <branch>     # tail log server
dcw stop <branch>     # stop server (worktree + DB tetap)
dcw abort <branch>    # hapus worktree + stop server + DROP DB branch (konfirmasi)
```

Contoh:

```sh
dcw start wildantest
# ✓ captain started: wildantest
#   worktree: ~/dev/hmx-002/.worktrees/wildantest
#   port:     5201   preview: http://127.0.0.1:5201
#   db:       cloning hmxmain -> wildantest

dcw start Inventory   # base branch → langsung di repo utama, port 5200
```

Base branch dideteksi dari branch yang aktif di repo (fallback `origin/HEAD`).

## State & kebersihan

| Path | Isi |
|---|---|
| `~/.dev-captain/ports.json` | registry port ↔ branch ↔ pid |
| `~/.dev-captain/logs/` | log server per branch |
| `<repo>/.worktrees/<branch>` | worktree (di-gitignore repo) |
| Postgres | DB per-branch (nama = nama branch, lowercase) |

File privat di worktree otomatis ditambahkan ke git exclude worktree — tidak akan ter-commit.

## Catatan

- `dcw abort` menghapus DB branch secara permanen (ada konfirmasi).
- DB `hmxmain` (utama) tidak pernah disentuh oleh `start`/`abort` worktree.
- Dirancang untuk macOS (`lsof`, `mkdir` lock; tanpa `fuser`/`ss`/`flock`).
